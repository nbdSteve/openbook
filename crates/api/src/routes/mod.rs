//! Route handlers, one module per area of `docs/api.md`.

use crate::error::ApiError;

pub mod chat;
pub mod conversations;
pub mod courses;
pub mod documents;
pub mod health;
pub mod insights;
pub mod me;
pub mod models;

/// Unknown routes get the JSON error shape too.
pub async fn not_found() -> ApiError {
    ApiError::NotFound("Not found".into())
}
