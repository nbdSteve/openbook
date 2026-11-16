//! The ingest queue (SQS FIFO). The API sends messages, and the worker receives them.

use aws_sdk_sqs::Client;
use aws_smithy_types::error::display::DisplayErrorContext;
use serde::{Deserialize, Serialize};

use crate::ids;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Ingest,
    Delete,
    /// Deletes a whole course: the vector index, the files, the documents, and the course item.
    DeleteCourse,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Ingest => "ingest",
            Action::Delete => "delete",
            Action::DeleteCourse => "delete_course",
        }
    }
}

/// `{ "action": "ingest", "courseId": "...", "documentId": "..." }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestMessage {
    pub action: Action,
    pub course_id: String,
    /// Empty for `delete_course`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub document_id: String,
}

impl IngestMessage {
    pub fn new(action: Action, course_id: &str, document_id: &str) -> Self {
        Self {
            action,
            course_id: course_id.to_owned(),
            document_id: document_id.to_owned(),
        }
    }

    /// A message that deletes a whole course.
    pub fn delete_course(course_id: &str) -> Self {
        Self::new(Action::DeleteCourse, course_id, "")
    }

    /// `<documentId>:<action>:<ULID>` (the course ID stands in for a course delete). The ULID makes
    /// every send unique, so a retry is not dropped.
    pub fn deduplication_id(&self) -> String {
        let subject = if self.document_id.is_empty() {
            &self.course_id
        } else {
            &self.document_id
        };
        format!("{subject}:{}:{}", self.action.as_str(), ids::new_id())
    }

    /// One writer per course: the group is the course ID.
    pub fn group_id(&self) -> &str {
        &self.course_id
    }
}

#[derive(Debug, thiserror::Error)]
#[error("queue error: {0}")]
pub struct QueueError(pub String);

#[derive(Debug, Clone)]
pub struct IngestQueue {
    client: Client,
    url: String,
}

impl IngestQueue {
    pub fn new(sdk: &aws_config::SdkConfig, url: String) -> Self {
        Self {
            client: Client::new(sdk),
            url,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn send(&self, message: &IngestMessage) -> Result<(), QueueError> {
        let body = serde_json::to_string(message).map_err(|e| QueueError(e.to_string()))?;
        self.client
            .send_message()
            .queue_url(&self.url)
            .message_body(body)
            .message_group_id(message.group_id())
            .message_deduplication_id(message.deduplication_id())
            .send()
            .await
            .map_err(|e| QueueError(DisplayErrorContext(&e).to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_have_the_documented_json() {
        let message = IngestMessage::new(Action::Ingest, "C1", "D1");
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::json!({"action": "ingest", "courseId": "C1", "documentId": "D1"})
        );
        let back: IngestMessage =
            serde_json::from_str(r#"{"action":"delete","courseId":"C","documentId":"D"}"#).unwrap();
        assert_eq!(back.action, Action::Delete);
        assert!(
            serde_json::from_str::<IngestMessage>(
                r#"{"action":"explode","courseId":"C","documentId":"D"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn group_and_deduplication_ids_follow_the_contract() {
        let message = IngestMessage::new(Action::Delete, "COURSE", "DOC");
        assert_eq!(message.group_id(), "COURSE");
        let first = message.deduplication_id();
        assert!(first.starts_with("DOC:delete:"));
        assert_eq!(first.len(), "DOC:delete:".len() + 26);
        assert_ne!(first, message.deduplication_id());
    }

    #[test]
    fn a_course_delete_has_no_document_id() {
        let message = IngestMessage::delete_course("COURSE");
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::json!({"action": "delete_course", "courseId": "COURSE"})
        );
        let back: IngestMessage =
            serde_json::from_str(r#"{"action":"delete_course","courseId":"C"}"#).unwrap();
        assert_eq!(back, IngestMessage::delete_course("C"));
        assert!(
            message
                .deduplication_id()
                .starts_with("COURSE:delete_course:")
        );
        assert_eq!(message.group_id(), "COURSE");
    }
}
