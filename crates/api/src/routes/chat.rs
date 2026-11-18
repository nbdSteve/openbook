//! `POST /api/courses/{courseId}/chat`: a Server-Sent Events stream.
//!
//! Everything that can fail with a normal JSON error (validation, membership, quota) happens before the
//! stream starts. The model call then runs in a spawned task that sends events to a channel. The task
//! saves the assistant message before it sends `done`, so persistence does not depend on the client.

use std::sync::Arc;
use std::time::Duration;

use crate::retrieval::{Retrieval, run_turn};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use openbook_core::auth::Principal;
use openbook_core::domain::{
    Conversation, CourseRecord, MessageRole, Question, Role, TutoringStyle, effective_style,
};
use openbook_core::ids;
use openbook_core::model::{ChatModel, ChatRequest, ModelEvent, TokenUsage, estimate_tokens};
use openbook_core::prompt::{
    HISTORY_WINDOW, build_system_prompt_for, title_from_message, turns_for,
};
use openbook_core::quota::{
    Reservation, ReserveLimits, release_message, reserve_message, usage_date,
};
use openbook_core::store::{NewMessage, Store};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

use crate::error::{ApiError, INTERNAL_MESSAGE, MODEL_UNAVAILABLE_MESSAGE};
use crate::extract::{ApiJson, ApiPath, AuthUser};
use crate::sse::{ChatEvent, sse_response};
use crate::{AppState, validate};

/// Events wait in this channel until the SSE connection takes them.
pub(crate) const EVENT_BUFFER: usize = 256;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatBody {
    conversation_id: Option<String>,
    message: String,
    model: Option<String>,
    style: Option<TutoringStyle>,
}

/// Everything the background task needs.
struct ChatRun {
    store: Store,
    provider: Arc<dyn ChatModel>,
    user: Principal,
    conversation_id: String,
    user_message_id: String,
    assistant_message_id: String,
    title: String,
    model_id: String,
    style: TutoringStyle,
    /// The conversation so far, ending with the new student message.
    history: Vec<(MessageRole, String)>,
    course: CourseRecord,
    retrieval: Arc<Retrieval>,
    /// The fast model, for the query rewrite of later turns.
    rewriter: Option<Arc<dyn ChatModel>>,
    /// When the message was reserved. Usage is recorded against this day.
    at: DateTime<Utc>,
    timeouts: ChatTimeouts,
}

/// How long a model call may take. The overall limit is below the Lambda timeout.
#[derive(Debug, Clone, Copy)]
pub struct ChatTimeouts {
    /// The whole answer.
    pub overall: Duration,
    /// From the start of the call to the first delta.
    pub first_delta: Duration,
    /// Between two deltas.
    pub gap: Duration,
}

impl Default for ChatTimeouts {
    fn default() -> Self {
        Self {
            overall: Duration::from_secs(100),
            first_delta: Duration::from_secs(45),
            gap: Duration::from_secs(30),
        }
    }
}

/// Picks the catalog model: the request, else the conversation's, else the course default.
pub fn pick_model<'a>(
    catalog: &'a openbook_core::catalog::Catalog,
    requested: Option<&'a str>,
    conversation_model: Option<&'a str>,
    course_model: &'a str,
) -> &'a str {
    requested
        .or_else(|| conversation_model.filter(|m| catalog.get(m).is_some()))
        .unwrap_or_else(|| catalog.effective_id(course_model))
}

pub async fn chat(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
    ApiJson(body): ApiJson<ChatBody>,
) -> Result<Response, ApiError> {
    let message = validate::chat_message(&body.message)?;
    let requested = body
        .model
        .as_deref()
        .map(|id| super::courses::known_model(&state, id))
        .transpose()?;
    let conversation_id = body
        .conversation_id
        .as_deref()
        .map(|id| validate::ulid_path(id, "Conversation"))
        .transpose()?;

    let (course, membership) = state.require_member(&user, &course_id).await?;
    let existing = match &conversation_id {
        Some(id) => Some(
            state
                .store
                .get_conversation(&user.user_id, id)
                .await?
                .filter(|c| c.course_id == course.id)
                .ok_or_else(|| ApiError::not_found("Conversation"))?,
        ),
        None => None,
    };

    let catalog = state.models.catalog();
    let model_id = pick_model(
        catalog,
        requested.as_deref(),
        existing.as_ref().map(|c| c.model.as_str()),
        &course.model,
    )
    .to_owned();
    let style = effective_style(&course, body.style, existing.as_ref().and_then(|c| c.style));
    let provider = state
        .models
        .provider(&model_id)
        .ok_or_else(|| ApiError::internal(format!("no provider for model {model_id}")))?;

    // Reserve one message. From here on, a failure must give the reservation back.
    let limits = &state.config.limits;
    let at = Utc::now();
    let date = usage_date(at);
    match reserve_message(
        &state.store,
        &user.user_id,
        &date,
        ReserveLimits {
            user_messages: limits.daily_messages_per_user,
            global_messages: limits.daily_messages_global,
            global_tokens: limits.daily_tokens_global,
        },
    )
    .await?
    {
        Reservation::Reserved => {}
        Reservation::UserLimit => return Err(ApiError::QuotaExceeded),
        Reservation::GlobalCap => return Err(ApiError::CapacityExhausted),
    }

    let prepared = prepare(
        &state,
        &user,
        &course,
        membership.role,
        existing,
        &message,
        (&model_id, style),
    )
    .await;
    let prepared = match prepared {
        Ok(p) => p,
        Err(error) => {
            release_message(&state.store, &user.user_id, &date).await;
            return Err(error);
        }
    };

    let run = ChatRun {
        store: state.store.clone(),
        provider,
        user,
        conversation_id: prepared.conversation_id,
        user_message_id: prepared.user_message_id,
        assistant_message_id: prepared.assistant_message_id,
        title: prepared.title,
        model_id,
        style,
        history: prepared.history,
        course,
        retrieval: state.retrieval.clone(),
        rewriter: state.models.provider(state.models.catalog().fast_id()),
        at,
        timeouts: state.chat_timeouts,
    };
    let (tx, rx) = mpsc::channel(EVENT_BUFFER);
    // The task keeps the request span, so its log lines carry the request ID.
    tokio::spawn(run_chat(run, tx).instrument(tracing::Span::current()));
    Ok(sse_response(ReceiverStream::new(rx)).into_response())
}

struct Prepared {
    conversation_id: String,
    user_message_id: String,
    assistant_message_id: String,
    title: String,
    history: Vec<(MessageRole, String)>,
}

/// Loads the history, creates the conversation if needed, and saves the user message (and the question).
async fn prepare(
    state: &AppState,
    user: &Principal,
    course: &CourseRecord,
    role: Role,
    existing: Option<Conversation>,
    message: &str,
    choice: (&str, TutoringStyle),
) -> Result<Prepared, ApiError> {
    let (model_id, style) = choice;
    let mut history: Vec<(MessageRole, String)> = match &existing {
        Some(conversation) => state
            .store
            .recent_messages(&conversation.id, HISTORY_WINDOW)
            .await?
            .into_iter()
            .map(|m| (m.role, m.content))
            .collect(),
        None => Vec::new(),
    };

    let now = openbook_core::domain::now();
    let (conversation_id, title) = match existing {
        Some(conversation) => (conversation.id, conversation.title),
        None => {
            let conversation = Conversation {
                id: ids::new_id(),
                course_id: course.id.clone(),
                title: title_from_message(message),
                model: model_id.to_owned(),
                style: Some(style),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            state
                .store
                .create_conversation(&user.user_id, &conversation)
                .await?;
            (conversation.id, conversation.title)
        }
    };

    // IDs from one process increase, so the user message sorts before the answer.
    let user_message_id = ids::new_id();
    let assistant_message_id = ids::new_id();
    state
        .store
        .add_message(
            &conversation_id,
            &NewMessage {
                id: user_message_id.clone(),
                role: MessageRole::User,
                content: message.to_owned(),
                created_at: now.clone(),
                sources: None,
                trace: None,
                model: None,
                style: None,
                usage: TokenUsage::default(),
            },
        )
        .await?;

    if role == Role::Student {
        let question = Question {
            id: ids::new_id(),
            user_id: user.user_id.clone(),
            email: user.email.clone(),
            question: message.to_owned(),
            conversation_id: conversation_id.clone(),
            created_at: now,
        };
        state.store.add_question(&course.id, &question).await?;
    }

    history.push((MessageRole::User, message.to_owned()));
    Ok(Prepared {
        conversation_id,
        user_message_id,
        assistant_message_id,
        title,
        history,
    })
}

/// Sends an event. A closed channel means the client left; the task goes on, so the answer is still saved.
pub(crate) async fn emit(tx: &mpsc::Sender<ChatEvent>, event: ChatEvent) {
    let _ = tx.send(event).await;
}

pub(crate) fn error_event(code: &str, message: &str) -> ChatEvent {
    ChatEvent::Error {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

/// Why a model call ended without a full answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    Error,
    TimedOut,
}

/// Waits for the next model event within the time that is left.
pub(crate) async fn next_event(
    stream: &mut openbook_core::model::ModelStream,
    wait: Duration,
    deadline: Instant,
) -> Result<Option<Result<ModelEvent, openbook_core::model::ModelError>>, Failure> {
    let wait = wait.min(deadline.saturating_duration_since(Instant::now()));
    match tokio::time::timeout(wait, stream.next()).await {
        Ok(item) => Ok(item),
        Err(_) => Err(Failure::TimedOut),
    }
}

fn sum_usage(a: TokenUsage, b: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: a.input_tokens + b.input_tokens,
        output_tokens: a.output_tokens + b.output_tokens,
    }
}

/// The message went back, but retrieval still spent tokens. Record them without counting a message.
async fn record_retrieval_only(run: &ChatRun, usage: TokenUsage) {
    if usage != TokenUsage::default() {
        run.store
            .record_tokens_only(&run.user.user_id, run.at, usage)
            .await;
    }
}

async fn run_chat(run: ChatRun, tx: mpsc::Sender<ChatEvent>) {
    let started = Instant::now();
    let deadline = started + run.timeouts.overall;
    let date = usage_date(run.at);
    // `meta` goes first, at once.
    emit(
        &tx,
        ChatEvent::Meta {
            conversation_id: run.conversation_id.clone(),
            user_message_id: run.user_message_id.clone(),
            assistant_message_id: run.assistant_message_id.clone(),
            title: run.title.clone(),
            model: run.model_id.clone(),
            style: run.style,
        },
    )
    .await;
    tracing::info!(model = %run.model_id, style = run.style.id(), "chat started");

    // Retrieval. It never fails the chat: at worst the answer has no sources.
    let earlier = &run.history[..run.history.len().saturating_sub(1)];
    let message = run
        .history
        .last()
        .map(|(_, t)| t.as_str())
        .unwrap_or_default();
    let documents = match run.store.list_documents(&run.course.id).await {
        Ok(documents) => documents,
        Err(error) => {
            tracing::error!(%error, "could not list the documents for retrieval");
            Vec::new()
        }
    };
    let turn = run_turn(
        &run.retrieval,
        run.rewriter.as_deref(),
        &run.course,
        &documents,
        earlier,
        message,
    )
    .await;
    if !turn.sources.is_empty() {
        emit(
            &tx,
            ChatEvent::Sources {
                sources: turn.sources.clone(),
            },
        )
        .await;
    }
    let request = ChatRequest {
        system: build_system_prompt_for(
            &run.course.name,
            &run.course.description,
            run.style,
            turn.mode(),
        ),
        turns: turns_for(&run.history, turn.mode()),
        ..ChatRequest::default()
    };
    // Tokens that retrieval spent (the rewrite and the query embedding) are counted on every way out.
    let retrieval_usage = turn.usage;
    let input_chars = request.system.chars().count()
        + request
            .turns
            .iter()
            .map(|t| t.text.chars().count())
            .sum::<usize>();

    let log_end = |outcome: &str, usage: TokenUsage| {
        tracing::info!(
            outcome,
            model = %run.model_id,
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            duration_ms = started.elapsed().as_millis() as u64,
            "chat finished"
        );
    };

    // The call itself must start within the first-delta time.
    let first_wait = run.timeouts.first_delta.min(run.timeouts.overall);
    let mut stream = match tokio::time::timeout(first_wait, run.provider.stream(request)).await {
        Ok(Ok(stream)) => stream,
        outcome => {
            if let Ok(Err(error)) = &outcome {
                tracing::error!(%error, model = %run.model_id, "model call failed to start");
            } else {
                tracing::error!(model = %run.model_id, "model call timed out before it started");
            }
            release_message(&run.store, &run.user.user_id, &date).await;
            record_retrieval_only(&run, retrieval_usage).await;
            log_end("model_failed_to_start", TokenUsage::default());
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
            run.timeouts.first_delta
        } else {
            run.timeouts.gap
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
                tracing::error!(%error, model = %run.model_id, "model stream failed");
                failure = Some(Failure::Error);
                break;
            }
            Ok(None) => break,
            Err(timed_out) => {
                tracing::error!(model = %run.model_id, "model stream timed out");
                failure = Some(timed_out);
                break;
            }
        }
    }

    let (usage, stop_reason) = match (finish, failure) {
        (Some(done), None) => done,
        (_, failure) => {
            let outcome = match failure {
                Some(Failure::TimedOut) => "timed_out",
                _ => "model_failed",
            };
            if text.is_empty() {
                // Nothing reached the student, so the message does not count.
                release_message(&run.store, &run.user.user_id, &date).await;
                record_retrieval_only(&run, retrieval_usage).await;
                log_end(outcome, TokenUsage::default());
            } else {
                // The model produced text and cost money. Record the estimated tokens and the message.
                let usage = TokenUsage {
                    input_tokens: estimate_tokens(input_chars),
                    output_tokens: estimate_tokens(text.chars().count()),
                };
                run.store
                    .record_usage(&run.user.user_id, run.at, sum_usage(usage, retrieval_usage))
                    .await;
                log_end(outcome, usage);
            }
            emit(
                &tx,
                error_event("model_unavailable", MODEL_UNAVAILABLE_MESSAGE),
            )
            .await;
            return;
        }
    };

    // Save everything before `done`: in Lambda the invocation can end as soon as the stream closes.
    let created_at = openbook_core::domain::now();
    let saved = run
        .store
        .add_message(
            &run.conversation_id,
            &NewMessage {
                id: run.assistant_message_id.clone(),
                role: MessageRole::Assistant,
                content: text,
                created_at: created_at.clone(),
                sources: (!turn.sources.is_empty()).then(|| turn.sources.clone()),
                trace: turn.trace.clone(),
                model: Some(run.model_id.clone()),
                style: Some(run.style),
                usage,
            },
        )
        .await;
    if let Err(error) = saved {
        tracing::error!(%error, "could not save the assistant message");
        run.store
            .record_usage(&run.user.user_id, run.at, sum_usage(usage, retrieval_usage))
            .await;
        log_end("save_failed", usage);
        emit(&tx, error_event("internal", INTERNAL_MESSAGE)).await;
        return;
    }
    match run
        .store
        .touch_conversation(
            &run.user.user_id,
            &run.conversation_id,
            &run.model_id,
            run.style,
            &created_at,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            // The user deleted the conversation during the chat. Let the new messages expire.
            if let Err(error) = run
                .store
                .expire_conversation_messages(&run.conversation_id)
                .await
            {
                tracing::error!(%error, "could not expire the messages of a deleted conversation");
            }
        }
        Err(error) => tracing::error!(%error, "could not update the conversation"),
    }
    run.store
        .record_usage(&run.user.user_id, run.at, sum_usage(usage, retrieval_usage))
        .await;
    log_end("done", usage);

    emit(&tx, ChatEvent::Done { usage, stop_reason }).await;
}

#[cfg(test)]
mod tests {
    use openbook_core::catalog::Catalog;

    use super::*;

    fn catalog() -> Catalog {
        let entry = |id: &str| {
            format!(
                r#"{{"id":"{id}","label":"{id}","vendor":"v","provider":"fake","modelId":"{id}","residency":"au"}}"#
            )
        };
        Catalog::parse(
            &format!("[{},{},{}]", entry("a"), entry("b"), entry("c")),
            "a",
            "a",
        )
        .unwrap()
    }

    #[test]
    fn request_beats_conversation_beats_course() {
        let c = catalog();
        assert_eq!(pick_model(&c, Some("c"), Some("b"), "a"), "c");
        assert_eq!(pick_model(&c, None, Some("b"), "a"), "b");
        assert_eq!(pick_model(&c, None, None, "b"), "b");
    }

    #[test]
    fn models_that_left_the_catalog_fall_back_to_the_default() {
        let c = catalog();
        assert_eq!(pick_model(&c, None, Some("gone"), "b"), "b");
        assert_eq!(pick_model(&c, None, None, "gone"), "a");
        assert_eq!(pick_model(&c, None, Some("gone"), "gone"), "a");
    }
}
