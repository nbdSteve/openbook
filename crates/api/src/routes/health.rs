use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::AppState;

/// `GET /api/health` (no auth).
pub async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(
        json!({ "ok": true, "stage": state.config.stage.as_str(), "version": env!("CARGO_PKG_VERSION") }),
    )
}
