use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::AppState;
use crate::extract::AuthUser;

/// `GET /api/models`: the model catalog in display order.
pub async fn list(State(state): State<AppState>, AuthUser(_user): AuthUser) -> Json<Value> {
    Json(json!({ "models": state.models.catalog().infos() }))
}
