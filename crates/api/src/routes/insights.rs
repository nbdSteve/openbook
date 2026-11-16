use axum::Json;
use axum::extract::State;
use chrono::{Duration, Utc};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::ApiError;
use crate::extract::{ApiPath, AuthUser};

const LATEST_QUESTIONS: usize = 100;

/// `GET /api/courses/{courseId}/insights` (owner)
pub async fn insights(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let (course, _) = state.require_owner(&user, &course_id).await?;
    let since = Utc::now() - Duration::days(7);
    let (questions, stats) = tokio::join!(
        state.store.list_questions(&course.id, LATEST_QUESTIONS),
        state.store.question_stats(&course.id, since)
    );
    let (questions, stats) = (questions?, stats?);
    Ok(Json(json!({
        "questions": questions,
        "totals": {
            "questionsLast7Days": stats.questions,
            "activeStudentsLast7Days": stats.active_students,
        },
    })))
}
