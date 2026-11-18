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
    self, BudgetHit, BudgetReservation, DemoConversations, DemoCourse, LimitHit,
    VisitorReservation, parse_viewer_address, source_token_bound, student_message_bound,
    token_bound,
};
use openbook_core::domain::{
    CourseRecord, Document, DocumentStatus, MessageRole, TutoringStyle, effective_style,
};
use openbook_core::model::{ChatModel, ChatRequest, ModelEvent, TokenUsage};
use openbook_core::prompt::{
    REWRITE_SYSTEM_PROMPT, SourceContext, SourceMode, TAG_NONCE_CHARS, build_system_prompt_for,
    new_tag_nonce, rewrite_user_text, turn_note, turns_for,
};
use openbook_core::store::Store;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

use super::chat::{ChatTimeouts, EVENT_BUFFER, Failure, emit, error_event, next_event};
use crate::error::{ApiError, INTERNAL_MESSAGE, MODEL_UNAVAILABLE_MESSAGE};
use crate::extract::ApiJson;
use crate::retrieval::{
    FINAL_SOURCES, MAX_QUERY_CHARS, REWRITE_HISTORY, REWRITE_MAX_TOKENS, Retrieval, SNIPPET_CHARS,
    run_turn,
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

const SOURCE_ESCAPE_FACTOR: usize = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DemoBody {
    message: String,
    conversation_id: Option<String>,
    style: Option<TutoringStyle>,
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

pub fn system_prompt_tokens(course: &CourseRecord, style: TutoringStyle) -> u64 {
    let one_source = [SourceContext {
        n: u32::MAX,
        document_name: "d".repeat(validate::MAX_FILENAME_CHARS),
        page: Some(u32::MAX),
        text: String::new(),
    }];
    let modes = [
        SourceMode::NotUsed,
        SourceMode::NotCovered,
        SourceMode::Found(&one_source),
    ];
    modes
        .into_iter()
        .map(|mode| {
            token_bound(&build_system_prompt_for(
                &course.name,
                &course.description,
                style,
                mode,
                &"0".repeat(TAG_NONCE_CHARS),
            )) + turn_note(mode).map_or(0, |note| token_bound(note) + token_bound("\n\n"))
        })
        .max()
        .unwrap_or(0)
        + token_bound(DEMO_PROMPT_NOTE)
}

pub fn answer_input_tokens(system_tokens: u64, history_tokens: u64, message: &str) -> u64 {
    let sources_tokens = source_token_bound(
        FINAL_SOURCES * (SNIPPET_CHARS * SOURCE_ESCAPE_FACTOR + SOURCE_WRAPPER_CHARS),
    );
    system_tokens + sources_tokens + history_tokens + student_message_bound(message)
}

/// The most that one turn can cost, in millionths of a US dollar. It is an upper bound:
/// - the answer: the course prompt (`system_tokens`), the most sources that retrieval can return,
///   the history (`history_tokens`: the real output tokens of the stored answers and a safe bound for
///   the questions), the question, and the full demo output limit (`max_output`);
/// - for a follow-up, the rewrite by the fast model;
/// - the embedding of the query, at the price of the fast model (far above the real price).
pub fn worst_case_cost(
    chat: &ModelEntry,
    fast: &ModelEntry,
    system_tokens: u64,
    history: &[(MessageRole, String)],
    history_tokens: u64,
    message: &str,
    max_output: u64,
) -> Option<u64> {
    let answer_input = answer_input_tokens(system_tokens, history_tokens, message);
    let answer = chat.cost_micro_usd(answer_input, max_output)?;
    let rewrite = if history.is_empty() {
        0
    } else {
        let start = history.len().saturating_sub(REWRITE_HISTORY);
        let input = token_bound(REWRITE_SYSTEM_PROMPT)
            + token_bound(&rewrite_user_text(&history[start..], message));
        fast.cost_micro_usd(input, u64::try_from(REWRITE_MAX_TOKENS).unwrap_or(0))?
    };
    // One token for each character of the longest query.
    let embedding = fast.cost_micro_usd(MAX_QUERY_CHARS as u64, 0)?;
    Some(answer.saturating_add(rewrite).saturating_add(embedding))
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
    max_output: u32,
    style: TutoringStyle,
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
    // The output limit of the demo: from the context, and never above the limit of the model.
    let max_output = state
        .config
        .demo
        .max_output_tokens
        .min(u32::try_from(chat_entry.max_tokens()).unwrap_or(0));

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

    let style = effective_style(&course, body.style, None);
    // From here on, a failure must give the visitor's question back.
    let prepared = async {
        let resumed = demo::resume(&state.store, body.conversation_id.as_deref(), now).await?;
        let system_tokens = system_prompt_tokens(&course, style);
        let cost = worst_case_cost(
            &chat_entry,
            &fast_entry,
            system_tokens,
            &resumed.history,
            resumed.history_tokens,
            &message,
            u64::from(max_output),
        )
        .ok_or_else(|| ApiError::internal("the demo models have no prices"))?;
        // A transaction conflict that does not go away after the tries is a busy demo, not an error.
        let budget =
            match demo::reserve_budget(&state.store, &visitor_id, now, cost, &state.config.demo)
                .await
            {
                Err(openbook_core::store::StoreError::Conflict) => Ok(Err(BudgetHit::Global)),
                other => other,
            }?;
        Ok::<_, ApiError>((resumed, budget))
    }
    .await;
    let (resumed, budget) = match prepared {
        Ok((resumed, Ok(budget))) => (resumed, budget),
        Ok((_, Err(hit))) => {
            // The day's budget, or the visitor's cap, has no room for this turn.
            visitor.release(&state.store).await;
            return Err(match hit {
                BudgetHit::Global => {
                    log_turn("budget_full", &chat_id, nothing, 0, started, None);
                    ApiError::DemoBusy
                }
                BudgetHit::Visitor => {
                    log_turn(
                        "limit",
                        &chat_id,
                        nothing,
                        0,
                        started,
                        Some(LimitHit::VisitorBudget),
                    );
                    ApiError::DemoRateLimited {
                        retry_after_seconds: LimitHit::VisitorBudget.retry_after_seconds(now),
                    }
                }
            });
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
        max_output,
        style,
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
        max_output,
        style,
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
            style,
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
    let nonce = match new_tag_nonce() {
        Ok(nonce) => nonce,
        Err(error) => {
            tracing::error!(%error, "no tag nonce");
            budget.settle(&store, retrieval_cost).await;
            visitor.release(&store).await;
            log_turn(
                "internal_error",
                &model_id,
                TokenUsage::default(),
                retrieval_cost,
                started,
                None,
            );
            emit(&tx, error_event("internal", INTERNAL_MESSAGE)).await;
            return;
        }
    };
    let mut system = build_system_prompt_for(
        &course.name,
        &course.description,
        style,
        turn.mode(),
        &nonce,
    );
    system.push_str(DEMO_PROMPT_NOTE);
    let request = ChatRequest {
        system,
        turns: turns_for(&history, turn.mode(), &nonce),
        // The demo limit, which is at most the catalog limit of the model.
        max_tokens: Some(i32::try_from(max_output).unwrap_or(i32::MAX)),
        temperature: None,
    };
    let first_wait = timeouts.first_delta.min(timeouts.overall);
    let mut stream = match tokio::time::timeout(first_wait, provider.stream(request)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            // A clear error response: the model did not start, so only retrieval cost money.
            tracing::error!(%error, model = %model_id, "demo model call failed to start");
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
        Err(_) => {
            // A timeout is not a clear answer: the call may have run. Keep the whole reservation.
            tracing::error!(model = %model_id, "demo model call timed out before it started");
            let cost = budget.reserved_micro_usd();
            budget.keep(&store).await;
            visitor.release(&store).await;
            log_turn(
                "model_start_timed_out",
                &model_id,
                TokenUsage::default(),
                cost,
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
            if usage == TokenUsage::default() {
                // The model reported no usage. Never refund on a guess: keep the whole reservation.
                tracing::warn!(model = %model_id, "the demo model reported no usage");
                budget.keep(&store).await;
            } else {
                budget.settle(&store, cost).await;
            }
            // Keep the turn before `done`: in Lambda the invocation can end when the stream closes.
            match DemoConversations::append(
                &store,
                &conversation_id,
                &message,
                &text,
                usage.output_tokens,
                expires_at,
            )
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
            if text.is_empty() {
                // Nothing reached the visitor, so the question does not count.
                visitor.release(&store).await;
            }
            // A stream that failed may have used any number of tokens. The usage is not known, so
            // the money is not given back on an estimate: the turn keeps its whole reservation.
            let cost = budget.reserved_micro_usd();
            budget.keep(&store).await;
            log_turn(
                outcome,
                &model_id,
                TokenUsage::default(),
                cost,
                started,
                None,
            );
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

    fn course(allow_style_choice: bool) -> CourseRecord {
        CourseRecord {
            id: "demo".into(),
            name: "Demo".into(),
            description: String::new(),
            tutoring_style: TutoringStyle::Direct,
            allow_style_choice,
            model: "m".into(),
            owner_id: "o".into(),
            invite_code: String::new(),
            bm25_version: None,
            bm25_key: None,
            deleting: false,
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    #[test]
    fn the_reservation_follows_the_prompt_of_the_effective_style() {
        let open = course(true);
        let direct = system_prompt_tokens(&open, effective_style(&open, None, None));
        let hints = system_prompt_tokens(
            &open,
            effective_style(&open, Some(TutoringStyle::Hints), None),
        );
        assert_eq!(direct, system_prompt_tokens(&open, TutoringStyle::Direct));
        assert_eq!(hints, system_prompt_tokens(&open, TutoringStyle::Hints));
        assert_ne!(direct, hints, "the styles have prompts of different length");
        let locked = course(false);
        let asked = effective_style(&locked, Some(TutoringStyle::Hints), None);
        assert_eq!(
            system_prompt_tokens(&locked, asked),
            direct,
            "a locked course reserves for its own style"
        );
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
    fn the_worst_case_covers_the_prompt_the_sources_the_history_and_the_full_output_limit() {
        let sonnet = entry(3.3, 16.5, 2000);
        let haiku = entry(1.1, 5.5, 2000);
        let first =
            worst_case_cost(&sonnet, &haiku, 800, &[], 0, "What is a function?", 1000).unwrap();
        let sources = (FINAL_SOURCES
            * (SNIPPET_CHARS * SOURCE_ESCAPE_FACTOR + SOURCE_WRAPPER_CHARS))
            .div_ceil(2) as u64;
        let input = 800 + sources + student_message_bound("What is a function?");
        let embedding = (1.1 * 500.0_f64).ceil() as u64;
        assert_eq!(
            first,
            (3.3 * input as f64).ceil() as u64 + 16_500 * 1000 / 1000 + embedding
        );
        assert!(first < 90_000, "about 8 US cents: {first}");
        // The output limit counts in full: a bigger limit costs more.
        let longer =
            worst_case_cost(&sonnet, &haiku, 800, &[], 0, "What is a function?", 2000).unwrap();
        assert_eq!(longer - first, 16_500);
        // A follow-up reserves the rewrite, and the stored history with its real output tokens.
        let history = vec![
            (MessageRole::User, "x".repeat(300)),
            (MessageRole::Assistant, "y".repeat(3000)),
        ];
        let second =
            worst_case_cost(&sonnet, &haiku, 800, &history, 640, "And then?", 1000).unwrap();
        assert!(second > first + 640 * 3);
        // Without prices there is no estimate, and the demo refuses to run.
        let free: ModelEntry = serde_json::from_value(serde_json::json!({
            "id": "m", "label": "M", "vendor": "v", "provider": "fake", "modelId": "m", "residency": "au"
        }))
        .unwrap();
        assert_eq!(worst_case_cost(&free, &haiku, 800, &[], 0, "q", 1000), None);
        assert_eq!(
            worst_case_cost(&sonnet, &free, 800, &[], 0, "q", 1000),
            None
        );
    }
    #[test]
    fn the_reservation_is_not_smaller_than_the_real_request_in_every_mode() {
        use openbook_core::prompt::SourceContext;
        let c = course(true);
        let message = "<&".repeat(openbook_core::demo::MAX_MESSAGE_CHARS / 2);
        let earlier = vec![
            (MessageRole::User, "&".repeat(120)),
            (MessageRole::Assistant, "An answer with $x < 5$.".to_owned()),
            (MessageRole::User, "Is 'a' < \"b\" and c > d?".to_owned()),
            (MessageRole::Assistant, "Yes.".to_owned()),
        ];
        let history_tokens: u64 = earlier
            .iter()
            .map(|(role, text)| match role {
                MessageRole::User => student_message_bound(text),
                MessageRole::Assistant => token_bound(text),
            })
            .sum();
        let math = "Let $x < 5$ and 'y' & z > 2, so \"w\" holds. ";
        let sources: Vec<SourceContext> = (1..=FINAL_SOURCES as u32)
            .map(|n| SourceContext {
                n,
                document_name: "d".repeat(validate::MAX_FILENAME_CHARS),
                page: Some(u32::MAX),
                text: math.repeat(SNIPPET_CHARS / math.len() + 1)[..SNIPPET_CHARS].to_owned(),
            })
            .collect();
        let mut messages = earlier.clone();
        messages.push((MessageRole::User, message.clone()));
        for style in TutoringStyle::ALL {
            for mode in [
                SourceMode::NotUsed,
                SourceMode::NotCovered,
                SourceMode::Found(&sources),
            ] {
                let nonce = openbook_core::prompt::new_tag_nonce().unwrap();
                let mut system =
                    build_system_prompt_for(&c.name, &c.description, style, mode, &nonce);
                let sources_tokens = match mode {
                    SourceMode::Found(_) => {
                        let bare: Vec<SourceContext> = sources
                            .iter()
                            .map(|s| SourceContext {
                                text: String::new(),
                                ..s.clone()
                            })
                            .collect();
                        let without = build_system_prompt_for(
                            &c.name,
                            &c.description,
                            style,
                            SourceMode::Found(&bare),
                            &nonce,
                        );
                        system = without.clone();
                        source_token_bound(
                            build_system_prompt_for(&c.name, &c.description, style, mode, &nonce)
                                .chars()
                                .count()
                                - without.chars().count(),
                        )
                    }
                    _ => 0,
                };
                system.push_str(DEMO_PROMPT_NOTE);
                let turns = turns_for(&messages, mode, &nonce);
                let real = token_bound(&system)
                    + sources_tokens
                    + turns.iter().map(|t| token_bound(&t.text)).sum::<u64>();
                let reserved =
                    answer_input_tokens(system_prompt_tokens(&c, style), history_tokens, &message);
                assert!(
                    reserved >= real,
                    "{style:?}: reserved {reserved}, real {real}"
                );
            }
        }
    }
}
