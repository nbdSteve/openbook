//! Request extractors: the `x-auth-token` caller and a JSON body with API errors.

use axum::body::to_bytes;
use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::request::Parts;
use openbook_core::auth::Principal;
use serde::de::DeserializeOwned;

use crate::AppState;
use crate::error::ApiError;

/// The header that carries the Cognito ID token. `Authorization` belongs to CloudFront OAC.
pub const AUTH_HEADER: &str = "x-auth-token";
const MAX_BODY_BYTES: usize = 64 * 1024;

/// The signed-in caller.
pub struct AuthUser(pub Principal);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTH_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(ApiError::Unauthenticated)?;
        Ok(AuthUser(state.verifier.verify(token).await?))
    }
}

/// Path parameters. A rejection (for example bad percent-encoding) is the JSON `404 not_found`.
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(ApiPath(value)),
            Err(_) => Err(ApiError::NotFound("Not found".into())),
        }
    }
}

/// A JSON body. Problems become `400 invalid_request`.
pub struct ApiJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bytes = to_bytes(request.into_body(), MAX_BODY_BYTES)
            .await
            .map_err(|_| {
                ApiError::InvalidRequest("The request body is too large or unreadable".into())
            })?;
        let value = serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::InvalidRequest(format!("The request body is not valid: {e}")))?;
        Ok(ApiJson(value))
    }
}
