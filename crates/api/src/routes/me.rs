use axum::Json;
use axum::extract::State;
use chrono::Utc;
use openbook_core::domain::{Limits, Usage, User};
use openbook_core::quota::usage_date;
use serde_json::{Value, json};

use crate::AppState;
use crate::error::ApiError;
use crate::extract::AuthUser;

/// `GET /api/me`. The first call for a user creates their profile.
pub async fn me(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Json<Value>, ApiError> {
    state.store.ensure_profile(&user).await?;
    let date = usage_date(Utc::now());
    let used = state.store.daily_messages(&user.user_id, &date).await?;
    let limits = &state.config.limits;
    Ok(Json(json!({
        "user": User { id: user.user_id, email: user.email },
        "usage": Usage { date, messages_used: used, messages_limit: limits.daily_messages_per_user },
        "limits": Limits {
            daily_messages: limits.daily_messages_per_user,
            max_courses_owned: limits.courses_owned,
            max_documents_per_course: limits.documents_per_course,
            max_upload_bytes: limits.upload_bytes,
        },
    })))
}
