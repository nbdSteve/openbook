//! API errors. They render as `{"error":{"code","message"}}` with the statuses in `docs/api.md`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use openbook_core::auth::AuthError;
use openbook_core::files::FilesError;
use openbook_core::store::StoreError;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("limit reached: {0}")]
    LimitReached(String),
    #[error("too large: {0}")]
    TooLarge(String),
    #[error("quota exceeded")]
    QuotaExceeded,
    /// Too many failed joins today. It has the `quota_exceeded` code too.
    #[error("too many failed attempts")]
    TooManyAttempts,
    #[error("capacity exhausted")]
    CapacityExhausted,
    #[error("model unavailable")]
    ModelUnavailable,
    /// Details are logged where the error is created. Clients get a generic message.
    #[error("internal error")]
    Internal,
}

/// Message that clients see for the `model_unavailable` error, also in the SSE `error` event.
pub const MODEL_UNAVAILABLE_MESSAGE: &str =
    "The model is not available right now. Try again in a moment.";
pub const INTERNAL_MESSAGE: &str = "Something went wrong. Try again.";

impl ApiError {
    /// Logs the details and returns the generic internal error.
    pub fn internal(details: impl std::fmt::Display) -> Self {
        tracing::error!(error = %details, "internal error");
        ApiError::Internal
    }

    pub fn not_found(what: &str) -> Self {
        ApiError::NotFound(format!("{what} not found"))
    }

    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthenticated => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) | ApiError::LimitReached(_) => StatusCode::CONFLICT,
            ApiError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::QuotaExceeded | ApiError::TooManyAttempts => StatusCode::TOO_MANY_REQUESTS,
            ApiError::CapacityExhausted | ApiError::ModelUnavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ApiError::InvalidRequest(_) => "invalid_request",
            ApiError::Unauthenticated => "unauthenticated",
            ApiError::Forbidden(_) => "forbidden",
            ApiError::NotFound(_) => "not_found",
            ApiError::Conflict(_) => "conflict",
            ApiError::LimitReached(_) => "limit_reached",
            ApiError::TooLarge(_) => "too_large",
            ApiError::QuotaExceeded | ApiError::TooManyAttempts => "quota_exceeded",
            ApiError::CapacityExhausted => "capacity_exhausted",
            ApiError::ModelUnavailable => "model_unavailable",
            ApiError::Internal => "internal",
        }
    }

    /// The message for clients.
    pub fn message(&self) -> String {
        match self {
            ApiError::InvalidRequest(m)
            | ApiError::Forbidden(m)
            | ApiError::NotFound(m)
            | ApiError::Conflict(m)
            | ApiError::LimitReached(m)
            | ApiError::TooLarge(m) => m.clone(),
            ApiError::Unauthenticated => "Sign in to continue".into(),
            ApiError::QuotaExceeded => {
                "You have used all of today's messages. Try again tomorrow.".into()
            }
            ApiError::TooManyAttempts => {
                "Too many invite codes did not work today. Try again tomorrow.".into()
            }
            ApiError::CapacityExhausted => {
                "Ask has reached its daily limit. Try again tomorrow.".into()
            }
            ApiError::ModelUnavailable => MODEL_UNAVAILABLE_MESSAGE.into(),
            ApiError::Internal => INTERNAL_MESSAGE.into(),
        }
    }

    /// The JSON body.
    pub fn body(&self) -> serde_json::Value {
        json!({ "error": { "code": self.code(), "message": self.message() } })
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status(), Json(self.body())).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::LimitReached => ApiError::LimitReached("A limit was reached".into()),
            other => ApiError::internal(other),
        }
    }
}

impl From<FilesError> for ApiError {
    fn from(error: FilesError) -> Self {
        ApiError::internal(error)
    }
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Invalid(reason) => {
                tracing::debug!(%reason, "token rejected");
                ApiError::Unauthenticated
            }
            AuthError::Unavailable(reason) => ApiError::internal(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_and_codes_match_the_contract() {
        let cases = [
            (ApiError::InvalidRequest("x".into()), 400, "invalid_request"),
            (ApiError::Unauthenticated, 401, "unauthenticated"),
            (ApiError::Forbidden("x".into()), 403, "forbidden"),
            (ApiError::NotFound("x".into()), 404, "not_found"),
            (ApiError::Conflict("x".into()), 409, "conflict"),
            (ApiError::LimitReached("x".into()), 409, "limit_reached"),
            (ApiError::TooLarge("x".into()), 413, "too_large"),
            (ApiError::QuotaExceeded, 429, "quota_exceeded"),
            (ApiError::TooManyAttempts, 429, "quota_exceeded"),
            (ApiError::CapacityExhausted, 503, "capacity_exhausted"),
            (ApiError::ModelUnavailable, 503, "model_unavailable"),
            (ApiError::Internal, 500, "internal"),
        ];
        for (error, status, code) in cases {
            assert_eq!(error.status().as_u16(), status, "{code}");
            assert_eq!(error.code(), code);
            assert_eq!(error.body()["error"]["code"], code);
            assert!(
                !error.body()["error"]["message"]
                    .as_str()
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn internal_errors_do_not_leak_details() {
        let error = ApiError::from(StoreError::Internal(
            "table secret-table-name is broken".into(),
        ));
        assert_eq!(error, ApiError::Internal);
        assert!(!error.message().contains("secret-table-name"));
        let error = ApiError::from(AuthError::Unavailable("http://jwks.internal/keys".into()));
        assert_eq!(error, ApiError::Internal);
        assert!(!error.message().contains("jwks"));
    }

    #[test]
    fn token_problems_are_401() {
        let error = ApiError::from(AuthError::Invalid("token expired at 1234".into()));
        assert_eq!(error, ApiError::Unauthenticated);
        assert!(!error.message().contains("1234"));
    }

    #[tokio::test]
    async fn renders_json() {
        let response = ApiError::NotFound("Course not found".into()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            json!({"error": {"code": "not_found", "message": "Course not found"}})
        );
    }
}
