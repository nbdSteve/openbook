//! One structured log line per request, and the `x-request-id` header.
//!
//! The line has the method, the route template (never the raw path), the status, and the latency.
//! It never has tokens, emails, message text, or query strings. Handlers log inside the request span,
//! so their lines carry the request ID.

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use openbook_core::ids;
use tracing::Instrument;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// The Lambda request ID in Lambda, else a new ID.
fn request_id(request: &Request) -> String {
    request
        .extensions()
        .get::<lambda_http::Context>()
        .map(|context| context.request_id.clone())
        .unwrap_or_else(ids::new_id)
}

pub async fn log_request(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let id = request_id(&request);
    let method = request.method().to_string();
    // The template, for example `/api/courses/{course_id}/chat`. Unmatched requests log no path.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let span = tracing::info_span!("request", request_id = %id, %method, %route);

    let mut response = next.run(request).instrument(span.clone()).await;
    let status = response.status().as_u16();
    span.in_scope(|| {
        tracing::info!(
            status,
            duration_ms = started.elapsed().as_millis() as u64,
            "request"
        );
    });
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use tower::ServiceExt;

    use super::*;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/api/courses/{course_id}/chat",
                get(|| async {
                    tracing::info!("inside the handler");
                    "ok"
                }),
            )
            .layer(axum::middleware::from_fn(log_request))
    }

    #[tokio::test]
    async fn logs_the_template_not_the_path_and_never_secrets() {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let request = HttpRequest::builder()
            .uri("/api/courses/01ARZ3NDEKTSV4RRFFQ69G5FAV/chat?secret=hunter2")
            .header("x-auth-token", "TOKEN-VALUE")
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        let header = response.headers()[REQUEST_ID_HEADER]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(header.len(), 26);

        let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("/api/courses/{course_id}/chat"), "{logs}");
        assert!(logs.contains("status=200"), "{logs}");
        assert!(logs.contains("duration_ms="), "{logs}");
        assert!(logs.contains(&header), "request ID in the line: {logs}");
        // Handler lines carry the request ID too.
        let handler_line = logs
            .lines()
            .find(|l| l.contains("inside the handler"))
            .unwrap();
        assert!(handler_line.contains(&header), "{handler_line}");
        for secret in ["01ARZ3NDEKTSV4RRFFQ69G5FAV", "hunter2", "TOKEN-VALUE"] {
            assert!(!logs.contains(secret), "{secret} leaked: {logs}");
        }
    }

    #[tokio::test]
    async fn unmatched_requests_log_no_path() {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let request = HttpRequest::builder()
            .uri("/nothing/secret-path")
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), 404);
        let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(
            logs.contains("unmatched") && !logs.contains("secret-path"),
            "{logs}"
        );
        assert!(response.headers().contains_key(REQUEST_ID_HEADER));
    }
}
