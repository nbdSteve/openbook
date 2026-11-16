use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use openbook_core::domain::Conversation;
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::extract::{ApiPath, AuthUser};
use crate::{AppState, validate};

/// Most conversations that a list returns.
const LIST_LIMIT: usize = 100;
/// Most messages that one conversation read returns (the latest ones).
const MESSAGE_LIMIT: usize = 200;

/// A stored model that left the catalog shows as the catalog default.
fn show(state: &AppState, mut conversation: Conversation) -> Conversation {
    conversation.model = state
        .models
        .catalog()
        .effective_id(&conversation.model)
        .to_owned();
    conversation
}

/// `GET /api/courses/{courseId}/conversations` (member): only the caller's own.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let (course, _) = state.require_member(&user, &course_id).await?;
    let conversations: Vec<_> = state
        .store
        .list_conversations(&user.user_id, &course.id, LIST_LIMIT)
        .await?
        .into_iter()
        .map(|c| show(&state, c))
        .collect();
    Ok(Json(json!({ "conversations": conversations })))
}

async fn own_conversation(
    state: &AppState,
    user_id: &str,
    conversation_id: &str,
) -> Result<Conversation, ApiError> {
    let id = validate::ulid_path(conversation_id, "Conversation")?;
    // The key includes the caller, so another user's conversation is simply not found.
    state
        .store
        .get_conversation(user_id, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("Conversation"))
}

/// `GET /api/conversations/{conversationId}`
pub async fn get(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(conversation_id): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let conversation = own_conversation(&state, &user.user_id, &conversation_id).await?;
    // The latest messages. A message keeps the model that wrote it, even if that model left the catalog.
    let messages = state
        .store
        .recent_messages(&conversation.id, MESSAGE_LIMIT)
        .await?;
    Ok(Json(
        json!({ "conversation": show(&state, conversation), "messages": messages }),
    ))
}

/// `DELETE /api/conversations/{conversationId}`
pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(conversation_id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let conversation = own_conversation(&state, &user.user_id, &conversation_id).await?;
    state
        .store
        .delete_conversation(&user.user_id, &conversation.id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
