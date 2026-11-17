//! `POST /api/demo/chat`: one anonymous question about the demo course, answered as a stream.
//!
//! This is the only route without sign-in, so it is narrow on purpose:
//! - It reads the demo course (found through the reserved table item) and nothing else.
//! - It writes only under the reserved `DEMO#` partitions: the limit counters, the daily salt, and
//!   the demo conversations (24 hours, at most 10 turns). No user item, no course item, no usage
//!   item, no question record, no S3 object. Demo conversations have no user and no GSI key, so no
//!   user route can list or read them.
//! - The body has `message` and an optional `conversationId`. The server keeps the history, so no
//!   client-sent history is read. The server fixes the model, the output limit, and the prompt.
//! - The visitor is an HMAC of the client address with a daily salt. Neither is logged or stored.
//! - Every turn reserves its worst-case cost against the daily budget before it calls a model.
//!
//! Everything that can fail with a normal JSON error happens before the stream starts.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequest, Request, State};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use openbook_core::catalog::ModelEntry;
use openbook_core::demo::{
    self, BudgetReservation, DemoConversations, DemoCourse, LimitHit, VisitorReservation,
    parse_viewer_address, worst_case_tokens,
};
use openbook_core::domain::{CourseRecord, Document, DocumentStatus, MessageRole};
use openbook_core::model::{ChatModel, ChatRequest, ModelEvent, TokenUsage, estimate_tokens};
use openbook_core::prompt::{
    HISTORY_WINDOW, NOT_COVERED_NOTICE, REWRITE_SYSTEM_PROMPT, SourceMode, build_system_prompt_for,
    normalize_turns, rewrite_user_text,
};
use openbook_core::store::Store;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

use super::chat::{ChatTimeouts, EVENT_BUFFER, Failure, emit, error_event, next_event};
use crate::error::{ApiError, MODEL_UNAVAILABLE_MESSAGE};
use crate::extract::ApiJson;
use crate::retrieval::{
    FINAL_SOURCES, REWRITE_HISTORY, REWRITE_MAX_TOKENS, Retrieval, SNIPPET_CHARS, run_turn,
};
use crate::sse::{ChatEvent, sse_response};
use crate::{AppState, validate};

/// CloudFront adds this header to every request that it sends to the origin, and it replaces a value
/// that the viewer sends. The origin request policy must forward it (`infra/lib/openbook-stack.ts`).
pub const VIEWER_ADDRESS_HEADER: &str = "cloudfront-viewer-address";

/// The server adds this to the course prompt. It names the setting, so the model keeps answers short.
const DEMO_PROMPT_NOTE: &str =
    "\n\nThis is a public demo of Openbook. Keep the answer short: about 250 words at most.";

/// The extra characters around each source in the prompt (the tag and its attributes).
const SOURCE_WRAPPER_CHARS: usize = 200;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DemoBody {
    message: String,
    conversation_id: Option<String>,
}

/// The address of the visitor. In Lambda it is the `CloudFront-Viewer-Address` header: the socket
/// peer there is the Lambda runtime, and a header from anywhere else could be forged, so the header
/// is not read outside Lambda. Outside Lambda it is the socket address. `None` means unknown, and the
/// caller must fail closed.
pub fn client_ip(
    in_lambda: bool,
    viewer_header: Option<&str>,
    socket: Option<SocketAddr>,
) -> Option<IpAddr> {
    if in_lambda {
        viewer_header.and_then(parse_viewer_address)
    } else {
        socket.map(|s| s.ip())
    }
}

/// The most that one turn can cost, in millionths of a US dollar: the answer (the whole prompt with
/// the most sources that retrieval can return, and the full output limit) and, for a follow-up, the
/// rewrite. `system_chars` is the length of the course prompt with the longest notice.
pub fn worst_case_cost(
    chat: &ModelEntry,
    fast: &ModelEntry,
    system_chars: usize,
    history: &[(MessageRole, String)],
    message: &str,
) -> Option<u64> {
    let window = history.len().saturating_sub(HISTORY_WINDOW);
    let history_chars: usize = history[window..]
        .iter()
        .map(|(_, text)| text.chars().count())
        .sum();
    let sources_chars = FINAL_SOURCES * (SNIPPET_CHARS + SOURCE_WRAPPER_CHARS);
    let answer_input =
        worst_case_tokens(system_chars + sources_chars + history_chars + message.chars().count());
    let answer =
        chat.cost_micro_usd(answer_input, u64::try_from(chat.max_tokens()).unwrap_or(0))?;
    let rewrite = if history.is_empty() {
        0
    } else {
        let start = history.len().saturating_sub(REWRITE_HISTORY);
        let chars = REWRITE_SYSTEM_PROMPT.chars().count()
            + rewrite_user_text(&history[start..], message)
                .chars()
                .count();
        fast.cost_micro_usd(
            worst_case_tokens(chars),
            u64::try_from(REWRITE_MAX_TOKENS).unwrap_or(0),
        )?
    };
    Some(answer.saturating_add(rewrite))
}

/// One structured line per demo turn. It never has the address, the hash, or the question.
fn log_turn(
    outcome: &str,
    model: &str,
    usage: TokenUsage,
    cost_micro_usd: u64,
    started: Instant,
    limit: Option<LimitHit>,
) {
    tracing::info!(
        event = "demo_turn",
        outcome,
        model,
        input_tokens = usage.input_tokens,
        output_tokens = usage.output_tokens,
        cost_micro_usd,
        duration_ms = started.elapsed().as_millis() as u64,
        limit = limit.map_or("none", LimitHit::as_str),
        "demo turn"
    );
}

/// Everything the background task needs.
struct DemoRun {
    store: Store,
    provider: Arc<dyn ChatModel>,
    rewriter: Option<Arc<dyn ChatModel>>,
    chat_entry: ModelEntry,
    fast_entry: ModelEntry,
    course: CourseRecord,
    demo: DemoCourse,
    documents: Vec<Document>,
    retrieval: Arc<Retrieval>,
    message: String,
    conversation_id: String,
    expires_at: i64,
    /// The conversation before this question.
    earlier: Vec<(MessageRole, String)>,
    visitor: VisitorReservation,
    budget: BudgetReservation,
    timeouts: ChatTimeouts,
    started: Instant,
}

pub async fn chat(State(state): State<AppState>, request: Request) -> Result<Response, ApiError> {
    // The kill switch comes first: with the demo off, this route reads no body and asks no store.
    if !state.config.demo.enabled {
        return Err(ApiError::DemoOff);
    }
    let started = Instant::now();
    let address = client_ip(
        state.config.in_lambda,
        request
            .headers()
            .get(VIEWER_ADDRESS_HEADER)
            .and_then(|v| v.to_str().ok()),
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0),
    );
    let ApiJson(body) = ApiJson::<DemoBody>::from_request(request, &state).await?;
    let message = validate::demo_message(&body.message)?;

    let catalog = state.models.catalog();
    let chat_id = catalog.default_chat_id().to_owned();
    let nothing = TokenUsage::default();
    let Some(address) = address else {
        // No address, no visitor, no limit. Fail closed.
        tracing::error!("the demo request has no usable client address");
        log_turn("no_client_address", &chat_id, nothing, 0, started, None);
        return Err(ApiError::DemoUnavailable);
    };

    let unavailable = |reason: &str| {
        log_turn(reason, &chat_id, nothing, 0, started, None);
        ApiError::DemoUnavailable
    };
    let Some(demo_course) = state.store.get_demo_course().await? else {
        return Err(unavailable("not_provisioned"));
    };
    let Some(course) = state
        .store
        .get_course(&demo_course.course_id)
        .await?
        .filter(|c| !c.deleting)
    else {
        return Err(unavailable("course_missing"));
    };
    let documents = state.store.list_documents(&course.id).await?;
    if !documents.iter().any(|d| d.status == DocumentStatus::Ready) {
        return Err(unavailable("no_ready_documents"));
    }
    let (Some(chat_entry), Some(fast_entry)) = (
        catalog.get(&chat_id).cloned(),
        catalog.get(catalog.fast_id()).cloned(),
    ) else {
        return Err(ApiError::internal("the demo models are not in the catalog"));
    };
    let Some(provider) = state.models.provider(&chat_id) else {
        return Err(ApiError::internal(format!(
            "no provider for model {chat_id}"
        )));
    };
    let rewriter = state.models.provider(catalog.fast_id());

    let now = Utc::now();
    let visitor_id = state.demo_visitors.id(&state.store, now, address).await?;
    let visitor =
        match demo::reserve_visitor(&state.store, &visitor_id, now, &state.config.demo).await? {
            Ok(reservation) => reservation,
            Err(hit) => {
                log_turn("limit", &chat_id, nothing, 0, started, Some(hit));
                return Err(ApiError::DemoRateLimited {
                    retry_after_seconds: hit.retry_after_seconds(now),
                });
            }
        };

    // From here on, a failure must give the visitor's question back.
    let prepared = async {
        let resumed = demo::resume(&state.store, body.conversation_id.as_deref(), now).await?;
        let system_chars = build_system_prompt_for(
            &course.name,
            &course.description,
            course.tutoring_style,
            SourceMode::NotUsed,
        )
        .chars()
        .count()
            + NOT_COVERED_NOTICE.chars().count()
            + DEMO_PROMPT_NOTE.chars().count();
        let cost = worst_case_cost(
            &chat_entry,
            &fast_entry,
            system_chars,
            &resumed.history,
            &message,
        )
        .ok_or_else(|| ApiError::internal("the demo models have no prices"))?;
        let budget = demo::reserve_budget(
            &state.store,
            now,
            cost,
            state.config.demo.budget_micro_usd_per_day,
        )
        .await?;
        Ok::<_, ApiError>((resumed, budget))
    }
    .await;
    let (resumed, budget) = match prepared {
        Ok((resumed, Some(budget))) => (resumed, budget),
        Ok((_, None)) => {
            // The day's budget has no room for this turn.
            visitor.release(&state.store).await;
            log_turn("budget_full", &chat_id, nothing, 0, started, None);
            return Err(ApiError::DemoBusy);
        }
        Err(error) => {
            visitor.release(&state.store).await;
            return Err(error);
        }
    };

    let run = DemoRun {
        store: state.store.clone(),
        provider,
        rewriter,
        chat_entry,
        fast_entry,
        course,
        demo: demo_course,
        documents,
        retrieval: state.retrieval.clone(),
        message,
        conversation_id: resumed.id,
        expires_at: resumed.expires_at,
        earlier: resumed.history,
        visitor,
        budget,
        timeouts: state.chat_timeouts,
        started,
    };
    let (tx, rx) = mpsc::channel(EVENT_BUFFER);
    tokio::spawn(run_demo(run, tx).instrument(tracing::Span::current()));
    Ok(sse_response(ReceiverStream::new(rx)).into_response())
}

async fn run_demo(run: DemoRun, tx: mpsc::Sender<ChatEvent>) {
    let DemoRun {
        store,
        provider,
        rewriter,
        chat_entry,
        fast_entry,
        course,
        demo,
        documents,
        retrieval,
        message,
        conversation_id,
        expires_at,
        earlier,
        visitor,
        budget,
        timeouts,
        started,
    } = run;
    let model_id = chat_entry.id.clone();
    let deadline = started + timeouts.overall;
    emit(
        &tx,
        ChatEvent::DemoMeta {
            conversation_id: conversation_id.clone(),
            model: model_id.clone(),
            attribution: demo.attribution.clone(),
        },
    )
    .await;

    // The same retrieval as the course chat: a follow-up is rewritten into one query by the fast model.
    let turn = run_turn(
        &retrieval,
        rewriter.as_deref(),
        &course,
        &documents,
        &earlier,
        &message,
    )
    .await;
    // Retrieval tokens: the rewrite (fast-model prices) and the query embedding (counted at the same
    // price, which is far above the real one, so the bound holds).
    let retrieval_cost = fast_entry
        .cost_micro_usd(turn.usage.input_tokens, turn.usage.output_tokens)
        .unwrap_or(0);
    if !turn.sources.is_empty() {
        emit(
            &tx,
            ChatEvent::Sources {
                sources: turn.sources.clone(),
            },
        )
        .await;
    }
    let mut history = earlier;
    history.push((MessageRole::User, message.clone()));
    let mut system = build_system_prompt_for(
        &course.name,
        &course.description,
        course.tutoring_style,
        turn.mode(),
    );
    system.push_str(DEMO_PROMPT_NOTE);
    let request = ChatRequest {
        system,
        turns: normalize_turns(&history),
        // The catalog limit of the model, as in the course chat.
        max_tokens: Some(chat_entry.max_tokens()),
        temperature: None,
    };
    let input_chars = request.system.chars().count()
        + request
            .turns
            .iter()
            .map(|t| t.text.chars().count())
            .sum::<usize>();

    let first_wait = timeouts.first_delta.min(timeouts.overall);
    let mut stream = match tokio::time::timeout(first_wait, provider.stream(request)).await {
        Ok(Ok(stream)) => stream,
        outcome => {
            if let Ok(Err(error)) = &outcome {
                tracing::error!(%error, model = %model_id, "demo model call failed to start");
            }
            // Nothing reached the visitor: the question does not count, and only retrieval cost money.
            budget.settle(&store, retrieval_cost).await;
            visitor.release(&store).await;
            log_turn(
                "model_failed_to_start",
                &model_id,
                TokenUsage::default(),
                retrieval_cost,
                started,
                None,
            );
            emit(
                &tx,
                error_event("model_unavailable", MODEL_UNAVAILABLE_MESSAGE),
            )
            .await;
            return;
        }
    };

    let mut text = String::new();
    let mut finish: Option<(TokenUsage, String)> = None;
    let mut failure: Option<Failure> = None;
    loop {
        let wait = if text.is_empty() {
            timeouts.first_delta
        } else {
            timeouts.gap
        };
        match next_event(&mut stream, wait, deadline).await {
            Ok(Some(Ok(ModelEvent::Delta(delta)))) => {
                text.push_str(&delta);
                emit(&tx, ChatEvent::Delta { text: delta }).await;
            }
            Ok(Some(Ok(ModelEvent::Finish { usage, stop_reason }))) => {
                finish = Some((usage, stop_reason));
            }
            Ok(Some(Err(error))) => {
                tracing::error!(%error, model = %model_id, "demo model stream failed");
                failure = Some(Failure::Error);
                break;
            }
            Ok(None) => break,
            Err(timed_out) => {
                tracing::error!(model = %model_id, "demo model stream timed out");
                failure = Some(timed_out);
                break;
            }
        }
    }

    match (finish, failure) {
        (Some((usage, stop_reason)), None) => {
            let cost = chat_entry
                .cost_micro_usd(usage.input_tokens, usage.output_tokens)
                .unwrap_or(0)
                .saturating_add(retrieval_cost);
            budget.settle(&store, cost).await;
            // Keep the turn before `done`: in Lambda the invocation can end when the stream closes.
            match DemoConversations::append(&store, &conversation_id, &message, &text, expires_at)
                .await
            {
                Ok(true) => {}
                Ok(false) => tracing::warn!("the demo conversation is full; the turn was not kept"),
                Err(error) => tracing::error!(%error, "could not save the demo turn"),
            }
            log_turn("done", &model_id, usage, cost, started, None);
            emit(&tx, ChatEvent::Done { usage, stop_reason }).await;
        }
        (_, failure) => {
            let outcome = match failure {
                Some(Failure::TimedOut) => "timed_out",
                _ => "model_failed",
            };
            let usage = if text.is_empty() {
                // Nothing reached the visitor, so the question does not count.
                visitor.release(&store).await;
                TokenUsage::default()
            } else {
                // The model produced text and cost money: charge the estimate.
                TokenUsage {
                    input_tokens: estimate_tokens(input_chars),
                    output_tokens: estimate_tokens(text.chars().count()),
                }
            };
            let cost = chat_entry
                .cost_micro_usd(usage.input_tokens, usage.output_tokens)
                .unwrap_or(0)
                .saturating_add(retrieval_cost);
            budget.settle(&store, cost).await;
            log_turn(outcome, &model_id, usage, cost, started, None);
            emit(
                &tx,
                error_event("model_unavailable", MODEL_UNAVAILABLE_MESSAGE),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket(text: &str) -> Option<SocketAddr> {
        Some(text.parse().unwrap())
    }

    #[test]
    fn in_lambda_the_viewer_header_is_the_visitor() {
        let ip = client_ip(true, Some("203.0.113.7:46532"), socket("10.0.0.1:1")).unwrap();
        assert_eq!(ip.to_string(), "203.0.113.7");
    }

    #[test]
    fn in_lambda_a_missing_or_bad_header_fails_closed() {
        // The socket is never a fallback in Lambda: it is not the visitor.
        assert_eq!(client_ip(true, None, socket("10.0.0.1:1")), None);
        assert_eq!(client_ip(true, Some("garbage"), socket("10.0.0.1:1")), None);
        assert_eq!(client_ip(true, Some(""), None), None);
    }

    #[test]
    fn outside_lambda_the_socket_is_the_visitor_and_the_header_is_ignored() {
        let ip = client_ip(false, Some("203.0.113.7:46532"), socket("127.0.0.1:50000")).unwrap();
        assert_eq!(ip.to_string(), "127.0.0.1");
        assert_eq!(client_ip(false, Some("203.0.113.7:1"), None), None);
    }

    fn entry(input: f64, output: f64, max: i32) -> ModelEntry {
        serde_json::from_value(serde_json::json!({
            "id": "m", "label": "M", "vendor": "v", "provider": "fake", "modelId": "m",
            "residency": "au", "maxOutputTokens": max,
            "inputUsdPerMillion": input, "outputUsdPerMillion": output
        }))
        .unwrap()
    }

    #[test]
    fn the_worst_case_covers_the_prompt_the_sources_and_the_full_output_limit() {
        let sonnet = entry(3.0, 15.0, 2000);
        let haiku = entry(1.0, 5.0, 2000);
        let first = worst_case_cost(&sonnet, &haiku, 3000, &[], "What is a function?").unwrap();
        let input_tokens = (3000 + 8 * 2600 + 19_u64).div_ceil(3);
        assert_eq!(first, 3 * input_tokens + 15 * 2000);
        assert!(first < 100_000, "about 10 US cents at most: {first}");
        // A follow-up also reserves the rewrite and the longer prompt.
        let history = vec![
            (MessageRole::User, "x".repeat(300)),
            (MessageRole::Assistant, "y".repeat(3000)),
        ];
        let second = worst_case_cost(&sonnet, &haiku, 3000, &history, "And then?").unwrap();
        assert!(second > first);
        // Without prices there is no estimate, and the demo refuses to run.
        let free: ModelEntry = serde_json::from_value(serde_json::json!({
            "id": "m", "label": "M", "vendor": "v", "provider": "fake", "modelId": "m", "residency": "au"
        }))
        .unwrap();
        assert_eq!(worst_case_cost(&free, &haiku, 3000, &[], "q"), None);
    }
}
