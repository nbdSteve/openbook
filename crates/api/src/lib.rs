//! Openbook HTTP API: one axum router for Lambda and for local runs.

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use openbook_core::auth::{Principal, TokenVerifier};
use openbook_core::config::Config;
use openbook_core::domain::{Course, CourseRecord, Membership, Role};
use openbook_core::files::Files;
use openbook_core::model::ModelRegistry;
use openbook_core::queue::IngestQueue;
use openbook_core::store::Store;

pub mod error;
pub mod extract;
pub mod logging;
pub mod retrieval;
pub mod routes;
pub mod sse;
pub mod validate;

use error::ApiError;

/// Everything a handler needs. Cloning is cheap.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub files: Files,
    pub verifier: Arc<TokenVerifier>,
    pub models: ModelRegistry,
    /// Hybrid retrieval for chat: S3 Vectors and the BM25 reader cache.
    pub retrieval: Arc<retrieval::Retrieval>,
    /// The ingest queue: `complete` and document `DELETE` send messages to it.
    pub queue: IngestQueue,
    /// Time limits for model calls.
    pub chat_timeouts: routes::chat::ChatTimeouts,
    /// The daily salt of the demo visitor hash, kept in memory after the first read.
    pub demo_visitors: Arc<openbook_core::demo::Visitors>,
}

impl AppState {
    /// The course as one caller sees it. A stored model that left the catalog shows as the default.
    pub fn course_view(&self, record: &CourseRecord, role: Role) -> Course {
        let catalog = self.models.catalog();
        record.view(role, catalog.effective_id(&record.model))
    }

    /// Loads the course and the caller's membership. A non-member gets `404`.
    pub async fn require_member(
        &self,
        user: &Principal,
        course_id: &str,
    ) -> Result<(CourseRecord, Membership), ApiError> {
        let course_id = validate::ulid_path(course_id, "Course")?;
        let (membership, course) = tokio::join!(
            self.store.get_membership(&course_id, &user.user_id),
            self.store.get_course(&course_id)
        );
        match (membership?, course?) {
            // A course that the worker is deleting does not exist for its members.
            (Some(membership), Some(course)) if !course.deleting => Ok((course, membership)),
            _ => Err(ApiError::not_found("Course")),
        }
    }

    /// Like `require_member`, and the caller must own the course (`403` for a student).
    pub async fn require_owner(
        &self,
        user: &Principal,
        course_id: &str,
    ) -> Result<(CourseRecord, Membership), ApiError> {
        let (course, membership) = self.require_member(user, course_id).await?;
        if membership.role != Role::Owner {
            return Err(ApiError::Forbidden(
                "Only the course owner can do this".into(),
            ));
        }
        Ok((course, membership))
    }
}

/// The header that every response of the demo route has. A CloudFront Function on the demo behavior
/// uses it to tell a response of this server from one that Lambda or CloudFront made (a throttle,
/// a timeout), and turns the second kind into a clean `demo_busy` (`infra/lib/demo-response.ts`).
pub const DEMO_MARKER_HEADER: &str = "x-openbook-demo";

async fn mark_demo(mut response: axum::response::Response) -> axum::response::Response {
    response.headers_mut().insert(
        DEMO_MARKER_HEADER,
        axum::http::HeaderValue::from_static("1"),
    );
    response
}

/// Builds the router with the routes of `docs/api.md` under `/api` that the mode of the process
/// mounts (`OPENBOOK_API_MODE`): every route (local runs), every route except the demo (the main
/// Lambda function), or only health and the demo (the demo Lambda function).
pub fn build_router(state: AppState) -> Router {
    use openbook_core::config::ApiMode;
    use routes::{chat, conversations, courses, demo, documents, health, insights, me, models};
    let mode = state.config.api_mode;
    let mut router = Router::new().route("/api/health", get(health::health));
    if mode != ApiMode::Demo {
        router = router
            .route("/api/me", get(me::me))
            .route("/api/models", get(models::list))
            .route("/api/courses", get(courses::list).post(courses::create))
            .route(
                "/api/courses/{course_id}",
                get(courses::get)
                    .patch(courses::update)
                    .delete(courses::delete),
            )
            .route(
                "/api/courses/{course_id}/invite-code",
                post(courses::rotate_invite_code),
            )
            .route("/api/join", post(courses::join))
            .route("/api/courses/{course_id}/members", get(courses::members))
            .route(
                "/api/courses/{course_id}/members/{user_id}",
                axum::routing::delete(courses::remove_member),
            )
            .route(
                "/api/courses/{course_id}/documents",
                get(documents::list).post(documents::create),
            )
            .route(
                "/api/courses/{course_id}/documents/{document_id}/complete",
                post(documents::complete),
            )
            .route(
                "/api/courses/{course_id}/documents/{document_id}",
                axum::routing::delete(documents::delete),
            )
            .route(
                "/api/courses/{course_id}/conversations",
                get(conversations::list),
            )
            .route(
                "/api/conversations/{conversation_id}",
                get(conversations::get).delete(conversations::delete),
            )
            .route("/api/courses/{course_id}/chat", post(chat::chat))
            .route("/api/courses/{course_id}/insights", get(insights::insights));
    }
    if mode != ApiMode::Full {
        // The only route that has no sign-in. It reads the demo course and nothing else.
        router = router.route(
            "/api/demo/chat",
            post(demo::chat).layer(axum::middleware::map_response(mark_demo)),
        );
    }
    router
        .fallback(routes::not_found)
        .method_not_allowed_fallback(routes::not_found)
        .layer(axum::middleware::from_fn(logging::log_request))
        .with_state(state)
}
