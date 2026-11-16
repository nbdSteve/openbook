//! Chat stream events and their SSE encoding.

use std::convert::Infallible;
use std::time::Duration;

use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use futures::Stream;
use openbook_core::model::TokenUsage;
use serde_json::{Value, json};

pub const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// One event of the chat stream, in the order of `docs/api.md`: `meta`, `delta`*, then `done` or `error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    Meta {
        conversation_id: String,
        user_message_id: String,
        assistant_message_id: String,
        title: String,
        model: String,
    },
    Delta {
        text: String,
    },
    Done {
        usage: TokenUsage,
        stop_reason: String,
    },
    Error {
        code: String,
        message: String,
    },
}

impl ChatEvent {
    pub fn name(&self) -> &'static str {
        match self {
            ChatEvent::Meta { .. } => "meta",
            ChatEvent::Delta { .. } => "delta",
            ChatEvent::Done { .. } => "done",
            ChatEvent::Error { .. } => "error",
        }
    }

    pub fn data(&self) -> Value {
        match self {
            ChatEvent::Meta {
                conversation_id,
                user_message_id,
                assistant_message_id,
                title,
                model,
            } => json!({
                "conversationId": conversation_id,
                "userMessageId": user_message_id,
                "assistantMessageId": assistant_message_id,
                "title": title,
                "model": model,
            }),
            ChatEvent::Delta { text } => json!({ "text": text }),
            ChatEvent::Done { usage, stop_reason } => json!({
                "usage": { "inputTokens": usage.input_tokens, "outputTokens": usage.output_tokens },
                "stopReason": stop_reason,
            }),
            ChatEvent::Error { code, message } => json!({ "code": code, "message": message }),
        }
    }

    /// The wire form: `event: <name>` and one `data: <json>` line.
    pub fn to_sse(&self) -> Event {
        // `data` writes one `data:` line, because a JSON string escapes its own newlines.
        Event::default()
            .event(self.name())
            .data(self.data().to_string())
    }
}

/// Wraps a stream of events as an SSE response with keep-alive comments every 15 seconds.
pub fn sse_response<S>(events: S) -> Sse<impl Stream<Item = Result<Event, Infallible>>>
where
    S: Stream<Item = ChatEvent> + Send + 'static,
{
    use futures::StreamExt;
    Sse::new(events.map(|event| Ok(event.to_sse()))).keep_alive(
        KeepAlive::new()
            .interval(KEEP_ALIVE_INTERVAL)
            .text("keep-alive"),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use futures::stream;

    use super::*;

    async fn render(events: Vec<ChatEvent>) -> (axum::http::response::Parts, String) {
        let response = sse_response(stream::iter(events)).into_response();
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, 1 << 20).await.unwrap();
        (parts, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn sample() -> Vec<ChatEvent> {
        vec![
            ChatEvent::Meta {
                conversation_id: "C".into(),
                user_message_id: "U".into(),
                assistant_message_id: "A".into(),
                title: "Hi".into(),
                model: "fake".into(),
            },
            ChatEvent::Delta {
                text: "He said \"hi\"\nand left".into(),
            },
            ChatEvent::Done {
                usage: TokenUsage {
                    input_tokens: 3,
                    output_tokens: 4,
                },
                stop_reason: "end_turn".into(),
            },
        ]
    }

    #[tokio::test]
    async fn encodes_events_in_order_with_one_data_line_each() {
        let (parts, body) = render(sample()).await;
        assert_eq!(parts.headers["content-type"], "text/event-stream");
        let expected = concat!(
            "event: meta\n",
            "data: {\"assistantMessageId\":\"A\",\"conversationId\":\"C\",\"model\":\"fake\",\"title\":\"Hi\",\"userMessageId\":\"U\"}\n\n",
            "event: delta\n",
            "data: {\"text\":\"He said \\\"hi\\\"\\nand left\"}\n\n",
            "event: done\n",
            "data: {\"stopReason\":\"end_turn\",\"usage\":{\"inputTokens\":3,\"outputTokens\":4}}\n\n",
        );
        assert_eq!(body, expected);
    }

    #[tokio::test]
    async fn newlines_in_text_never_break_the_data_line() {
        let (_, body) = render(vec![ChatEvent::Delta {
            text: "a\n\nevent: done\n".into(),
        }])
        .await;
        assert_eq!(body.lines().filter(|l| l.starts_with("event: ")).count(), 1);
        assert_eq!(body.lines().filter(|l| l.starts_with("data: ")).count(), 1);
    }

    #[tokio::test]
    async fn encodes_error_events() {
        let (_, body) = render(vec![ChatEvent::Error {
            code: "model_unavailable".into(),
            message: "x".into(),
        }])
        .await;
        assert_eq!(
            body,
            "event: error\ndata: {\"code\":\"model_unavailable\",\"message\":\"x\"}\n\n"
        );
    }

    #[test]
    fn event_names_match_the_contract() {
        let names: Vec<_> = sample().iter().map(ChatEvent::name).collect();
        assert_eq!(names, ["meta", "delta", "done"]);
    }

    #[tokio::test(start_paused = true)]
    async fn sends_keep_alive_comments_every_15_seconds() {
        use futures::StreamExt;
        let (tx, rx) = tokio::sync::mpsc::channel::<ChatEvent>(1);
        let response =
            sse_response(tokio_stream::wrappers::ReceiverStream::new(rx)).into_response();
        let mut body = response.into_body().into_data_stream();
        // No event is ready, so the first frame is the keep-alive comment after 15 seconds.
        let started = tokio::time::Instant::now();
        let frame = body.next().await.unwrap().unwrap();
        assert_eq!(&frame[..], b": keep-alive\n\n");
        assert_eq!(started.elapsed(), KEEP_ALIVE_INTERVAL);
        drop(tx);
    }
}
