//! Domain types. They serialize exactly as the types in `docs/api.md` (camelCase).

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// Formats a timestamp as RFC 3339 in UTC with millisecond precision.
pub fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The current time as an API timestamp.
pub fn now() -> String {
    timestamp(Utc::now())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Student,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TutoringStyle {
    #[default]
    Socratic,
    Hints,
    StepByStep,
    Concepts,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    Uploading,
    Queued,
    Processing,
    Ready,
    Failed,
    Deleting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

/// The signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub daily_messages: u32,
    pub max_courses_owned: u32,
    pub max_documents_per_course: u32,
    pub max_upload_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub date: String,
    pub messages_used: u32,
    pub messages_limit: u32,
}

/// A course as the API shows it to one caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Course {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tutoring_style: TutoringStyle,
    /// The course default model (a catalog ID).
    pub model: String,
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A course as stored (no caller role).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tutoring_style: TutoringStyle,
    /// The stored catalog ID. It may no longer be in the catalog.
    pub model: String,
    pub owner_id: String,
    pub invite_code: String,
    /// Version of the course BM25 index. `None` until the first document is indexed.
    pub bm25_version: Option<u64>,
    /// The S3 key of the BM25 blob of `bm25_version`.
    pub bm25_key: Option<String>,
    /// The owner deleted the course. The worker is removing it.
    pub deleting: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl CourseRecord {
    /// The API view for a caller with `role`. Only owners see the invite code.
    /// `model` is the catalog ID to show (the stored one, or the catalog default when it left the catalog).
    pub fn view(&self, role: Role, model: &str) -> Course {
        Course {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            tutoring_style: self.tutoring_style,
            model: model.to_owned(),
            role,
            invite_code: (role == Role::Owner).then(|| self.invite_code.clone()),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub user_id: String,
    pub email: String,
    pub role: Role,
    pub joined_at: String,
}

/// A course membership as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub course_id: String,
    pub user_id: String,
    pub email: String,
    pub role: Role,
    pub joined_at: String,
}

impl Membership {
    pub fn member(&self) -> Member {
        Member {
            user_id: self.user_id.clone(),
            email: self.email.clone(),
            role: self.role,
            joined_at: self.joined_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: String,
    pub course_id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub status: DocumentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcribed_pages: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    /// The number of chunks the worker planned to write. Set before the first vector is written.
    #[serde(skip)]
    pub planned_chunk_count: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub course_id: String,
    pub title: String,
    /// Catalog ID of the last answer.
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A retrieved passage (phase 3). The API sends none before then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub n: u32,
    pub document_id: String,
    pub document_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<Source>>,
    /// Assistant messages only: the catalog ID that wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub user_id: String,
    pub email: String,
    pub question: String,
    pub conversation_id: String,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn course_serializes_as_documented() {
        let record = CourseRecord {
            id: "C".into(),
            name: "Algebra".into(),
            description: String::new(),
            tutoring_style: TutoringStyle::StepByStep,
            model: "claude".into(),
            owner_id: "u".into(),
            invite_code: "ABCD2345".into(),
            bm25_version: None,
            bm25_key: None,
            deleting: false,
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let owner = serde_json::to_value(record.view(Role::Owner, "claude")).unwrap();
        assert_eq!(
            owner,
            json!({
                "id": "C", "name": "Algebra", "description": "", "tutoringStyle": "step_by_step", "model": "claude",
                "role": "owner", "inviteCode": "ABCD2345", "createdAt": "t", "updatedAt": "t"
            })
        );
        let student = serde_json::to_value(record.view(Role::Student, "claude")).unwrap();
        assert!(student.get("inviteCode").is_none());
        assert_eq!(student["role"], "student");
    }

    #[test]
    fn document_omits_empty_optionals() {
        let doc = Document {
            id: "D".into(),
            course_id: "C".into(),
            filename: "a.pdf".into(),
            size_bytes: 5,
            status: DocumentStatus::Uploading,
            error: None,
            page_count: None,
            chunk_count: None,
            transcribed_pages: None,
            warnings: None,
            planned_chunk_count: None,
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let value = serde_json::to_value(doc).unwrap();
        assert_eq!(value["sizeBytes"], 5);
        assert_eq!(value["status"], "uploading");
        assert!(value.get("error").is_none() && value.get("pageCount").is_none());
    }

    #[test]
    fn limits_and_usage_use_camel_case() {
        let limits = serde_json::to_value(Limits {
            daily_messages: 1,
            max_courses_owned: 2,
            max_documents_per_course: 3,
            max_upload_bytes: 4,
        })
        .unwrap();
        assert_eq!(
            limits,
            json!({"dailyMessages":1,"maxCoursesOwned":2,"maxDocumentsPerCourse":3,"maxUploadBytes":4})
        );
        let usage = serde_json::to_value(Usage {
            date: "d".into(),
            messages_used: 1,
            messages_limit: 2,
        })
        .unwrap();
        assert_eq!(
            usage,
            json!({"date":"d","messagesUsed":1,"messagesLimit":2})
        );
    }

    #[test]
    fn timestamps_have_milliseconds_and_z() {
        let at = DateTime::parse_from_rfc3339("2026-11-16T01:02:03.456789Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(timestamp(at), "2026-11-16T01:02:03.456Z");
    }

    #[test]
    fn style_ids_match_the_contract() {
        for (style, id) in [
            (TutoringStyle::Socratic, "socratic"),
            (TutoringStyle::Hints, "hints"),
            (TutoringStyle::StepByStep, "step_by_step"),
            (TutoringStyle::Concepts, "concepts"),
            (TutoringStyle::Direct, "direct"),
        ] {
            assert_eq!(serde_json::to_value(style).unwrap(), json!(id));
        }
    }
}
