//! `POST /api/demo/chat`: one anonymous question about the demo course, answered as a stream.
//!
//! This is the only route without sign-in, so it is narrow on purpose:
//! - It reads the demo course (found through the reserved table item) and nothing else.
//! - It writes only the limit counters (and the daily salt, once a day). No conversation, no message,
//!   no question record, no usage record.
//! - It is single-turn. The body has one field, `message`. The server fixes the model, the output
//!   limit, and the prompt.
//! - The visitor is an HMAC of the client address with a daily salt. Neither is logged or stored.
//!
//! Everything that can fail with a normal JSON error happens before the stream starts.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequest, Request, State};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use openbook_core::catalog::ModelEntry;
use openbook_core::demo::{
    self, DemoCourse, LimitHit, MAX_OUTPUT_TOKENS, NOT_COVERED_STOP_REASON, Reservation,
    not_covered_reply, parse_viewer_address,
};
use openbook_core::domain::{CourseRecord, Document, DocumentStatus, MessageRole};
use openbook_core::model::{ChatModel, ChatRequest, ChatTurn, ModelEvent, TokenUsage};
use openbook_core::prompt::build_system_prompt_for;
use openbook_core::store::Store;
use openbook_search::retrieval::Outcome;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

use super::chat::{ChatTimeouts, EVENT_BUFFER, Failure, emit, error_event, next_event};
use crate::error::{ApiError, MODEL_UNAVAILABLE_MESSAGE};
use crate::extract::ApiJson;
use crate::retrieval::{Retrieval, run_turn};
use crate::sse::{ChatEvent, sse_response};
use crate::{AppState, validate};

/// CloudFront adds this header to every request that it sends to the origin, and it replaces a value
/// that the viewer sends. The origin request policy must forward it (`infra/lib/openbook-stack.ts`).
pub const VIEWER_ADDRESS_HEADER: &str = "cloudfront-viewer-address";

/// The server adds this to the course prompt. The demo has a hard output limit, so the answer must
/// end before it.
const DEMO_PROMPT_NOTE: &str = "\n\nThis is a short public demo, and your answer has a strict length limit. \
     Answer in at most about 250 words, and finish your last sentence.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DemoBody {
    message: String,
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

/// One structured line per demo turn. It never has the address, the hash, or the question.
fn log_turn(
    outcome: &str,
    model: &str,
    usage: TokenUsage,
    started: Instant,
    limit: Option<LimitHit>,
) {
    tracing::info!(
        event = "demo_turn",
        outcome,
        model,
        input_tokens = usage.input_tokens,
        output_tokens = usage.output_tokens,
        duration_ms = started.elapsed().as_millis() as u64,
        limit = limit.map_or("none", LimitHit::as_str),
        "demo turn"
    );
}

/// Everything the background task needs.
struct DemoRun {
    store: Store,
    provider: Arc<dyn ChatModel>,
    model_id: String,
    max_tokens: i32,
    course: CourseRecord,
    demo: DemoCourse,
    documents: Vec<Document>,
    retrieval: Arc<Retrieval>,
    message: String,
    reservation: Reservation,
    timeouts: ChatTimeouts,
    started: Instant,
}

pub async fn chat(State(state): State<AppState>, request: Request) -> Result<Response, ApiError> {
    // The kill switch comes first: with the demo off, this route is as if it did not exist.
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

    let fast_id = state.models.catalog().fast_id().to_owned();
    let Some(address) = address else {
        // No address, no visitor, no limit. Fail closed.
        tracing::error!("the demo request has no usable client address");
        log_turn(
            "no_client_address",
            &fast_id,
            TokenUsage::default(),
            started,
            None,
        );
        return Err(ApiError::DemoUnavailable);
    };

    let unavailable = |reason: &str| {
        log_turn(reason, &fast_id, TokenUsage::default(), started, None);
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
    let Some(provider) = state.models.provider(&fast_id) else {
        return Err(ApiError::internal(format!(
            "no provider for model {fast_id}"
        )));
    };
    let max_tokens = state
        .models
        .catalog()
        .get(&fast_id)
        .map_or(MAX_OUTPUT_TOKENS, |e: &ModelEntry| {
            e.max_tokens().min(MAX_OUTPUT_TOKENS)
        });

    let now = Utc::now();
    let visitor = state.demo_visitors.id(&state.store, now, address).await?;
    let reservation = match demo::reserve(&state.store, &visitor, now, &state.config.demo).await? {
        Ok(reservation) => reservation,
        Err(hit) => {
            log_turn("limit", &fast_id, TokenUsage::default(), started, Some(hit));
            return Err(match hit {
                LimitHit::Global => ApiError::DemoBusy,
                LimitHit::VisitorWindow | LimitHit::VisitorDay => ApiError::DemoRateLimited {
                    retry_after_seconds: hit.retry_after_seconds(now),
                },
            });
        }
    };

    let run = DemoRun {
        store: state.store.clone(),
        provider,
        model_id: fast_id,
        max_tokens,
        course,
        demo: demo_course,
        documents,
        retrieval: state.retrieval.clone(),
        message,
        reservation,
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
        model_id,
        max_tokens,
        course,
        demo,
        documents,
        retrieval,
        message,
        reservation,
        timeouts,
        started,
    } = run;
    let deadline = started + timeouts.overall;
    emit(
        &tx,
        ChatEvent::DemoMeta {
            model: model_id.clone(),
            attribution: demo.attribution.clone(),
        },
    )
    .await;

    // Single turn: no earlier messages and no rewrite, so retrieval makes no model call.
    let turn = run_turn(&retrieval, None, &course, &documents, &[], &message).await;
    if turn.outcome != Outcome::Found {
        // Nothing to answer from. The fixed reply costs no model call, so the global call goes back.
        reservation.release_global(&store).await;
        let outcome = match turn.outcome {
            Outcome::NotCovered => "not_covered",
            _ => "retrieval_failed",
        };
        emit(
            &tx,
            ChatEvent::Delta {
                text: not_covered_reply(&demo.coverage, &demo.examples),
            },
        )
        .await;
        log_turn(outcome, &model_id, TokenUsage::default(), started, None);
        emit(
            &tx,
            ChatEvent::Done {
                usage: TokenUsage::default(),
                stop_reason: NOT_COVERED_STOP_REASON.to_owned(),
            },
        )
        .await;
        return;
    }

    emit(
        &tx,
        ChatEvent::Sources {
            sources: turn.sources.clone(),
        },
    )
    .await;
    let mut system = build_system_prompt_for(
        &course.name,
        &course.description,
        course.tutoring_style,
        turn.mode(),
    );
    system.push_str(DEMO_PROMPT_NOTE);
    let request = ChatRequest {
        system,
        turns: vec![ChatTurn {
            role: MessageRole::User,
            text: message,
        }],
        max_tokens: Some(max_tokens),
        temperature: None,
    };

    let first_wait = timeouts.first_delta.min(timeouts.overall);
    let mut stream = match tokio::time::timeout(first_wait, provider.stream(request)).await {
        Ok(Ok(stream)) => stream,
        outcome => {
            if let Ok(Err(error)) = &outcome {
                tracing::error!(%error, model = %model_id, "demo model call failed to start");
            }
            reservation.release_all(&store).await;
            log_turn(
                "model_failed_to_start",
                &model_id,
                TokenUsage::default(),
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

    let mut produced = false;
    let mut finish: Option<(TokenUsage, String)> = None;
    let mut failure: Option<Failure> = None;
    loop {
        let wait = if produced {
            timeouts.gap
        } else {
            timeouts.first_delta
        };
        match next_event(&mut stream, wait, deadline).await {
            Ok(Some(Ok(ModelEvent::Delta(delta)))) => {
                produced = produced || !delta.is_empty();
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
            log_turn("done", &model_id, usage, started, None);
            emit(&tx, ChatEvent::Done { usage, stop_reason }).await;
        }
        (_, failure) => {
            let outcome = match failure {
                Some(Failure::TimedOut) => "timed_out",
                _ => "model_failed",
            };
            if !produced {
                // Nothing reached the visitor, so the question does not count.
                reservation.release_all(&store).await;
            }
            log_turn(outcome, &model_id, TokenUsage::default(), started, None);
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
}
