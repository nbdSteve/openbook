//! DynamoDB store: one table, the keys in `docs/architecture.md`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::{
    AttributeValue, Delete, DeleteRequest, KeysAndAttributes, Put, TransactWriteItem, Update,
    WriteRequest,
};
use aws_smithy_types::error::display::DisplayErrorContext;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::Principal;
use crate::domain::{
    Conversation, CourseRecord, Document, DocumentStatus, Membership, Message, MessageRole,
    Question, Role, Source, TutoringStyle,
};
use crate::ids;
use crate::model::TokenUsage;
use crate::quota::{PageCounter, UsageCounter, UsageScope, usage_date, usage_month};

type Item = HashMap<String, AttributeValue>;

const DAILY_USAGE_TTL_DAYS: i64 = 40;
const QUESTION_TTL_DAYS: i64 = 180;
const INVITE_ATTEMPTS: usize = 8;
const BATCH_SIZE: usize = 25;
const BATCH_RETRIES: u32 = 8;
/// The most items that one statistics query reads (a guard against a huge course).
const STATS_MAX_ITEMS: usize = 20_000;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store error: {0}")]
    Internal(String),
    /// A per-user or per-course limit counter is at its maximum.
    #[error("limit reached")]
    LimitReached,
}

fn internal(error: impl std::error::Error) -> StoreError {
    StoreError::Internal(DisplayErrorContext(&error).to_string())
}

fn is_conditional_failure<E: ProvideErrorMetadata, R>(error: &SdkError<E, R>) -> bool {
    error.code() == Some("ConditionalCheckFailedException")
}

fn s(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

fn n(value: impl ToString) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

// Key builders.
fn user_pk(user_id: &str) -> String {
    format!("USER#{user_id}")
}
fn course_pk(course_id: &str) -> String {
    format!("COURSE#{course_id}")
}
fn conv_pk(conversation_id: &str) -> String {
    format!("CONV#{conversation_id}")
}
fn invite_pk(code: &str) -> String {
    format!("INVITE#{code}")
}
fn member_sk(user_id: &str) -> String {
    format!("MEMBER#{user_id}")
}
fn doc_sk(document_id: &str) -> String {
    format!("DOC#{document_id}")
}
fn conv_sk(conversation_id: &str) -> String {
    format!("CONV#{conversation_id}")
}
fn convs_gsi_pk(user_id: &str, course_id: &str) -> String {
    format!("CONVS#{user_id}#{course_id}")
}
fn usage_daily_sk(date: &str) -> String {
    format!("USAGE#D#{date}")
}
fn usage_monthly_sk(month: &str) -> String {
    format!("USAGE#M#{month}")
}
const META_SK: &str = "META";
/// Per-user counters (owned courses). One item per user.
const COUNTERS_SK: &str = "COUNTERS";
const ORPHAN_MESSAGE_TTL_SECONDS: i64 = 24 * 3600;
const GLOBAL_PK: &str = "GLOBAL";

fn epoch_in_days(days: i64) -> i64 {
    (Utc::now() + chrono::Duration::days(days)).timestamp()
}

// ---------------------------------------------------------------------------------------------
// Stored item shapes
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CourseItem {
    pk: String,
    sk: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    description: String,
    tutoring_style: TutoringStyle,
    model: String,
    owner_id: String,
    invite_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bm25_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bm25_key: Option<String>,
    /// `deleting` while the worker removes the course.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    created_at: String,
    updated_at: String,
}

impl CourseItem {
    fn new(record: &CourseRecord) -> Self {
        Self {
            pk: course_pk(&record.id),
            sk: META_SK.into(),
            kind: "course".into(),
            name: record.name.clone(),
            description: record.description.clone(),
            tutoring_style: record.tutoring_style,
            model: record.model.clone(),
            owner_id: record.owner_id.clone(),
            invite_code: record.invite_code.clone(),
            bm25_version: record.bm25_version,
            bm25_key: record.bm25_key.clone(),
            status: record.deleting.then(|| "deleting".to_owned()),
            created_at: record.created_at.clone(),
            updated_at: record.updated_at.clone(),
        }
    }

    fn record(self) -> CourseRecord {
        CourseRecord {
            id: self
                .pk
                .strip_prefix("COURSE#")
                .unwrap_or(&self.pk)
                .to_owned(),
            name: self.name,
            description: self.description,
            tutoring_style: self.tutoring_style,
            model: self.model,
            owner_id: self.owner_id,
            invite_code: self.invite_code,
            bm25_version: self.bm25_version,
            bm25_key: self.bm25_key,
            deleting: self.status.as_deref() == Some("deleting"),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MembershipItem {
    pk: String,
    sk: String,
    gsi1pk: String,
    gsi1sk: String,
    #[serde(rename = "type")]
    kind: String,
    course_id: String,
    user_id: String,
    email: String,
    role: Role,
    joined_at: String,
}

impl MembershipItem {
    fn new(m: &Membership) -> Self {
        Self {
            pk: course_pk(&m.course_id),
            sk: member_sk(&m.user_id),
            gsi1pk: user_pk(&m.user_id),
            gsi1sk: format!("COURSE#{}#{}", m.joined_at, m.course_id),
            kind: "membership".into(),
            course_id: m.course_id.clone(),
            user_id: m.user_id.clone(),
            email: m.email.clone(),
            role: m.role,
            joined_at: m.joined_at.clone(),
        }
    }

    fn membership(self) -> Membership {
        Membership {
            course_id: self.course_id,
            user_id: self.user_id,
            email: self.email,
            role: self.role,
            joined_at: self.joined_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InviteItem {
    pk: String,
    sk: String,
    #[serde(rename = "type")]
    kind: String,
    course_id: String,
    created_at: String,
}

impl InviteItem {
    fn new(code: &str, course_id: &str, created_at: &str) -> Self {
        Self {
            pk: invite_pk(code),
            sk: "INVITE".into(),
            kind: "invite".into(),
            course_id: course_id.into(),
            created_at: created_at.into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentItem {
    pk: String,
    sk: String,
    #[serde(rename = "type")]
    kind: String,
    filename: String,
    size_bytes: u64,
    s3_key: String,
    status: DocumentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chunk_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcribed_pages: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warnings: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    planned_chunk_count: Option<u32>,
    created_at: String,
    updated_at: String,
}

impl DocumentItem {
    fn new(d: &Document, s3_key: &str) -> Self {
        Self {
            pk: course_pk(&d.course_id),
            sk: doc_sk(&d.id),
            kind: "document".into(),
            filename: d.filename.clone(),
            size_bytes: d.size_bytes,
            s3_key: s3_key.into(),
            status: d.status,
            error: d.error.clone(),
            page_count: d.page_count,
            chunk_count: d.chunk_count,
            transcribed_pages: d.transcribed_pages,
            warnings: d.warnings.clone(),
            planned_chunk_count: d.planned_chunk_count,
            created_at: d.created_at.clone(),
            updated_at: d.updated_at.clone(),
        }
    }

    fn document(self) -> Document {
        Document {
            id: self.sk.strip_prefix("DOC#").unwrap_or(&self.sk).to_owned(),
            course_id: self
                .pk
                .strip_prefix("COURSE#")
                .unwrap_or(&self.pk)
                .to_owned(),
            filename: self.filename,
            size_bytes: self.size_bytes,
            status: self.status,
            error: self.error,
            page_count: self.page_count,
            chunk_count: self.chunk_count,
            transcribed_pages: self.transcribed_pages,
            warnings: self.warnings,
            planned_chunk_count: self.planned_chunk_count,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationItem {
    pk: String,
    sk: String,
    gsi1pk: String,
    gsi1sk: String,
    #[serde(rename = "type")]
    kind: String,
    course_id: String,
    title: String,
    model: String,
    created_at: String,
    updated_at: String,
}

impl ConversationItem {
    fn new(user_id: &str, c: &Conversation) -> Self {
        Self {
            pk: user_pk(user_id),
            sk: conv_sk(&c.id),
            gsi1pk: convs_gsi_pk(user_id, &c.course_id),
            gsi1sk: format!("{}#{}", c.updated_at, c.id),
            kind: "conversation".into(),
            course_id: c.course_id.clone(),
            title: c.title.clone(),
            model: c.model.clone(),
            created_at: c.created_at.clone(),
            updated_at: c.updated_at.clone(),
        }
    }

    fn conversation(self) -> Conversation {
        Conversation {
            id: self.sk.strip_prefix("CONV#").unwrap_or(&self.sk).to_owned(),
            course_id: self.course_id,
            title: self.title,
            model: self.model,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageItem {
    pk: String,
    sk: String,
    #[serde(rename = "type")]
    kind: String,
    role: MessageRole,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sources: Option<Vec<Source>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    created_at: String,
}

impl MessageItem {
    fn message(self) -> Message {
        Message {
            id: self.sk.strip_prefix("MSG#").unwrap_or(&self.sk).to_owned(),
            role: self.role,
            content: self.content,
            created_at: self.created_at,
            sources: self.sources,
            model: self.model,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuestionItem {
    pk: String,
    sk: String,
    #[serde(rename = "type")]
    kind: String,
    user_id: String,
    email: String,
    question: String,
    conversation_id: String,
    created_at: String,
    ttl: i64,
}

impl QuestionItem {
    fn question(self) -> Question {
        Question {
            id: self.sk.strip_prefix("Q#").unwrap_or(&self.sk).to_owned(),
            user_id: self.user_id,
            email: self.email,
            question: self.question,
            conversation_id: self.conversation_id,
            created_at: self.created_at,
        }
    }
}

fn to_item<T: Serialize>(value: &T) -> Result<Item, StoreError> {
    serde_dynamo::to_item(value).map_err(internal)
}

fn from_item<T: serde::de::DeserializeOwned>(item: Item) -> Result<T, StoreError> {
    serde_dynamo::from_item(item).map_err(internal)
}

/// Changes to a course. `None` leaves a field as it is.
#[derive(Debug, Clone, Default)]
pub struct CoursePatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tutoring_style: Option<TutoringStyle>,
    pub model: Option<String>,
}

/// A newly saved message.
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: String,
    pub sources: Option<Vec<Source>>,
    /// Assistant messages only.
    pub model: Option<String>,
    pub usage: TokenUsage,
}

/// Counts of questions in a time window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuestionStats {
    pub questions: u32,
    pub active_students: u32,
}

struct Query<'a> {
    index: Option<&'a str>,
    key_condition: &'a str,
    names: Vec<(&'a str, &'a str)>,
    values: Vec<(&'a str, AttributeValue)>,
    forward: bool,
    max_items: usize,
    projection: Option<&'a str>,
}

impl<'a> Query<'a> {
    fn new(key_condition: &'a str) -> Self {
        Self {
            index: None,
            key_condition,
            names: Vec::new(),
            values: Vec::new(),
            forward: true,
            max_items: usize::MAX,
            projection: None,
        }
    }
}

/// The DynamoDB table.
#[derive(Debug, Clone)]
pub struct Store {
    client: Client,
    table: String,
}

impl Store {
    pub fn new(sdk: &aws_config::SdkConfig, table: String) -> Self {
        Self {
            client: Client::new(sdk),
            table,
        }
    }

    pub fn table(&self) -> &str {
        &self.table
    }

    // -----------------------------------------------------------------------------------------
    // Generic helpers
    // -----------------------------------------------------------------------------------------

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            // Authorization reads must see the latest write.
            .consistent_read(true)
            .send()
            .await
            .map_err(internal)?;
        Ok(out.item)
    }

    async fn put(&self, item: Item) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .send()
            .await
            .map_err(internal)?;
        Ok(())
    }

    async fn delete(&self, pk: &str, sk: &str) -> Result<(), StoreError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .send()
            .await
            .map_err(internal)?;
        Ok(())
    }

    async fn query(&self, q: Query<'_>) -> Result<Vec<Item>, StoreError> {
        let mut items: Vec<Item> = Vec::new();
        let mut start: Option<Item> = None;
        loop {
            let mut request = self
                .client
                .query()
                .table_name(&self.table)
                .set_index_name(q.index.map(str::to_owned))
                .key_condition_expression(q.key_condition)
                .scan_index_forward(q.forward)
                .set_exclusive_start_key(start.take());
            for (name, target) in &q.names {
                request = request.expression_attribute_names(*name, *target);
            }
            for (name, value) in &q.values {
                request = request.expression_attribute_values(*name, value.clone());
            }
            if let Some(projection) = q.projection {
                request = request.projection_expression(projection);
            }
            if q.max_items != usize::MAX {
                let remaining = q.max_items.saturating_sub(items.len());
                request = request.limit(i32::try_from(remaining).unwrap_or(i32::MAX));
            }
            let out = request.send().await.map_err(internal)?;
            items.extend(out.items.unwrap_or_default());
            if items.len() >= q.max_items {
                items.truncate(q.max_items);
                return Ok(items);
            }
            match out.last_evaluated_key {
                Some(key) if !key.is_empty() => start = Some(key),
                _ => return Ok(items),
            }
        }
    }

    /// Deletes items in groups of 25 and retries what DynamoDB did not process.
    async fn batch_delete(&self, keys: Vec<(String, String)>) -> Result<(), StoreError> {
        for chunk in keys.chunks(BATCH_SIZE) {
            let mut pending: Vec<WriteRequest> = chunk
                .iter()
                .map(|(pk, sk)| {
                    let delete = DeleteRequest::builder()
                        .key("pk", s(pk))
                        .key("sk", s(sk))
                        .build()
                        .map_err(internal)?;
                    Ok(WriteRequest::builder().delete_request(delete).build())
                })
                .collect::<Result<_, StoreError>>()?;
            let mut attempt = 0u32;
            while !pending.is_empty() {
                let out = self
                    .client
                    .batch_write_item()
                    .request_items(&self.table, pending)
                    .send()
                    .await
                    .map_err(internal)?;
                pending = out
                    .unprocessed_items
                    .and_then(|mut m| m.remove(&self.table))
                    .unwrap_or_default();
                if !pending.is_empty() {
                    attempt += 1;
                    if attempt > BATCH_RETRIES {
                        return Err(StoreError::Internal(
                            "batch delete left unprocessed items".into(),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(25 * 2u64.pow(attempt))).await;
                }
            }
        }
        Ok(())
    }

    /// Every `(pk, sk)` under a partition key, except the sort keys in `keep`.
    async fn partition_keys(
        &self,
        pk: &str,
        keep: &[&str],
    ) -> Result<Vec<(String, String)>, StoreError> {
        let mut q = Query::new("pk = :pk");
        q.values.push((":pk", s(pk)));
        q.projection = Some("pk, sk");
        let items = self.query(q).await?;
        Ok(items
            .into_iter()
            .filter_map(|mut item| match (item.remove("pk"), item.remove("sk")) {
                (Some(AttributeValue::S(pk)), Some(AttributeValue::S(sk)))
                    if !keep.contains(&sk.as_str()) =>
                {
                    Some((pk, sk))
                }
                _ => None,
            })
            .collect())
    }

    // -----------------------------------------------------------------------------------------
    // Profile
    // -----------------------------------------------------------------------------------------

    /// Creates the profile on the first call. Later calls only read.
    pub async fn ensure_profile(&self, principal: &Principal) -> Result<(), StoreError> {
        let pk = user_pk(&principal.user_id);
        if self.get(&pk, "PROFILE").await?.is_some() {
            return Ok(());
        }
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .item("pk", s(pk))
            .item("sk", s("PROFILE"))
            .item("type", s("profile"))
            .item("email", s(&principal.email))
            .item("createdAt", s(crate::domain::now()))
            .condition_expression("attribute_not_exists(pk)")
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_conditional_failure(&e) => Ok(()),
            Err(e) => Err(internal(e)),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Courses
    // -----------------------------------------------------------------------------------------

    /// Creates a course, the owner membership, and the invite code in one transaction.
    pub async fn create_course(
        &self,
        owner: &Principal,
        name: &str,
        description: &str,
        style: TutoringStyle,
        model: &str,
        max_owned: u32,
    ) -> Result<CourseRecord, StoreError> {
        let now = crate::domain::now();
        let id = ids::new_id();
        for _ in 0..INVITE_ATTEMPTS {
            let record = CourseRecord {
                id: id.clone(),
                name: name.into(),
                description: description.into(),
                tutoring_style: style,
                model: model.into(),
                owner_id: owner.user_id.clone(),
                invite_code: ids::new_invite_code(),
                bm25_version: None,
                bm25_key: None,
                deleting: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            let membership = Membership {
                course_id: id.clone(),
                user_id: owner.user_id.clone(),
                email: owner.email.clone(),
                role: Role::Owner,
                joined_at: now.clone(),
            };
            let put =
                |item: Item, condition: Option<&str>| -> Result<TransactWriteItem, StoreError> {
                    let put = Put::builder()
                        .table_name(&self.table)
                        .set_item(Some(item))
                        .set_condition_expression(condition.map(str::to_owned))
                        .build()
                        .map_err(internal)?;
                    Ok(TransactWriteItem::builder().put(put).build())
                };
            // The counter keeps the limit exact when creates run in parallel.
            let counter = Update::builder()
                .table_name(&self.table)
                .key("pk", s(user_pk(&owner.user_id)))
                .key("sk", s(COUNTERS_SK))
                .update_expression("ADD #c :one")
                .condition_expression("attribute_not_exists(#c) OR #c < :max")
                .expression_attribute_names("#c", "courses")
                .expression_attribute_values(":one", n(1))
                .expression_attribute_values(":max", n(max_owned))
                .build()
                .map_err(internal)?;
            let result = self
                .client
                .transact_write_items()
                .transact_items(put(
                    to_item(&CourseItem::new(&record))?,
                    Some("attribute_not_exists(pk)"),
                )?)
                .transact_items(put(to_item(&MembershipItem::new(&membership))?, None)?)
                .transact_items(put(
                    to_item(&InviteItem::new(&record.invite_code, &id, &now))?,
                    Some("attribute_not_exists(pk)"),
                )?)
                .transact_items(TransactWriteItem::builder().update(counter).build())
                .send()
                .await;
            match result {
                Ok(_) => return Ok(record),
                Err(e) if cancel_is(&e, 3, "ConditionalCheckFailed") => {
                    return Err(StoreError::LimitReached);
                }
                Err(e) if cancel_is(&e, 2, "ConditionalCheckFailed") => continue,
                Err(e) if transaction_conflict(&e) => backoff().await,
                Err(e) => return Err(internal(e)),
            }
        }
        Err(StoreError::Internal(
            "could not find a free invite code".into(),
        ))
    }

    pub async fn get_course(&self, course_id: &str) -> Result<Option<CourseRecord>, StoreError> {
        match self.get(&course_pk(course_id), META_SK).await? {
            Some(item) => Ok(Some(from_item::<CourseItem>(item)?.record())),
            None => Ok(None),
        }
    }

    pub async fn get_membership(
        &self,
        course_id: &str,
        user_id: &str,
    ) -> Result<Option<Membership>, StoreError> {
        match self.get(&course_pk(course_id), &member_sk(user_id)).await? {
            Some(item) => Ok(Some(from_item::<MembershipItem>(item)?.membership())),
            None => Ok(None),
        }
    }

    /// The user's memberships, newest first.
    pub async fn list_memberships(&self, user_id: &str) -> Result<Vec<Membership>, StoreError> {
        let mut q = Query::new("gsi1pk = :pk AND begins_with(gsi1sk, :prefix)");
        q.index = Some("gsi1");
        q.values = vec![(":pk", s(user_pk(user_id))), (":prefix", s("COURSE#"))];
        q.forward = false;
        let items = self.query(q).await?;
        items
            .into_iter()
            .map(|i| Ok(from_item::<MembershipItem>(i)?.membership()))
            .collect()
    }

    /// Courses by ID (missing courses are skipped). The order of the result is not defined.
    pub async fn get_courses(
        &self,
        course_ids: &[String],
    ) -> Result<Vec<CourseRecord>, StoreError> {
        let mut out = Vec::new();
        for chunk in course_ids.chunks(100) {
            let mut keys: Vec<Item> = chunk
                .iter()
                .map(|id| {
                    HashMap::from([
                        ("pk".to_owned(), s(course_pk(id))),
                        ("sk".to_owned(), s(META_SK)),
                    ])
                })
                .collect();
            let mut attempt = 0u32;
            while !keys.is_empty() {
                let request = KeysAndAttributes::builder()
                    .set_keys(Some(keys))
                    .consistent_read(true)
                    .build()
                    .map_err(internal)?;
                let response = self
                    .client
                    .batch_get_item()
                    .request_items(&self.table, request)
                    .send()
                    .await
                    .map_err(internal)?;
                if let Some(mut tables) = response.responses {
                    for item in tables.remove(&self.table).unwrap_or_default() {
                        out.push(from_item::<CourseItem>(item)?.record());
                    }
                }
                keys = response
                    .unprocessed_keys
                    .and_then(|mut m| m.remove(&self.table))
                    .map(|k| k.keys)
                    .unwrap_or_default();
                if !keys.is_empty() {
                    attempt += 1;
                    if attempt > BATCH_RETRIES {
                        return Err(StoreError::Internal(
                            "batch get left unprocessed keys".into(),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(25 * 2u64.pow(attempt))).await;
                }
            }
        }
        Ok(out)
    }

    /// Applies a patch. Returns `None` when the course does not exist.
    pub async fn update_course(
        &self,
        course_id: &str,
        patch: &CoursePatch,
    ) -> Result<Option<CourseRecord>, StoreError> {
        let mut sets = vec!["#updatedAt = :updatedAt"];
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(META_SK))
            .expression_attribute_names("#updatedAt", "updatedAt")
            .expression_attribute_values(":updatedAt", s(crate::domain::now()));
        if let Some(name) = &patch.name {
            sets.push("#name = :name");
            request = request
                .expression_attribute_names("#name", "name")
                .expression_attribute_values(":name", s(name));
        }
        if let Some(description) = &patch.description {
            sets.push("#description = :description");
            request = request
                .expression_attribute_names("#description", "description")
                .expression_attribute_values(":description", s(description));
        }
        if let Some(style) = patch.tutoring_style {
            let value =
                serde_dynamo::to_attribute_value::<_, AttributeValue>(style).map_err(internal)?;
            sets.push("#style = :style");
            request = request
                .expression_attribute_names("#style", "tutoringStyle")
                .expression_attribute_values(":style", value);
        }
        if let Some(model) = &patch.model {
            sets.push("#model = :model");
            request = request
                .expression_attribute_names("#model", "model")
                .expression_attribute_values(":model", s(model));
        }
        let result = request
            .update_expression(format!("SET {}", sets.join(", ")))
            .condition_expression("attribute_exists(pk)")
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
            .send()
            .await;
        match result {
            Ok(out) => match out.attributes {
                Some(item) => Ok(Some(from_item::<CourseItem>(item)?.record())),
                None => Ok(None),
            },
            Err(e) if is_conditional_failure(&e) => Ok(None),
            Err(e) => Err(internal(e)),
        }
    }

    /// Deletes the invite item. This is the first step of a course delete, so the code stops working at once.
    pub async fn delete_invite(&self, course: &CourseRecord) -> Result<(), StoreError> {
        self.delete(&invite_pk(&course.invite_code), "INVITE").await
    }

    /// Deletes the memberships and documents of a course, then the course item, the owner membership,
    /// and the owner's course counter in one transaction. A failed run can be repeated.
    /// Question items (`Q#`) stay until their TTL: nothing can reach them once the course is gone.
    pub async fn delete_course_data(&self, course: &CourseRecord) -> Result<(), StoreError> {
        let pk = course_pk(&course.id);
        let owner_member = member_sk(&course.owner_id);
        let mut q = Query::new("pk = :pk AND sk < :questions");
        q.values = vec![(":pk", s(&pk)), (":questions", s("Q#"))];
        q.projection = Some("pk, sk");
        let items = self.query(q).await?;
        let keys: Vec<(String, String)> = items
            .into_iter()
            .filter_map(|mut item| match (item.remove("pk"), item.remove("sk")) {
                (Some(AttributeValue::S(pk)), Some(AttributeValue::S(sk)))
                    if sk != META_SK && sk != owner_member =>
                {
                    Some((pk, sk))
                }
                _ => None,
            })
            .collect();
        self.batch_delete(keys).await?;

        let delete = |sk: &str, condition: Option<&str>| -> Result<TransactWriteItem, StoreError> {
            let delete = Delete::builder()
                .table_name(&self.table)
                .key("pk", s(&pk))
                .key("sk", s(sk))
                .set_condition_expression(condition.map(str::to_owned))
                .build()
                .map_err(internal)?;
            Ok(TransactWriteItem::builder().delete(delete).build())
        };
        let counter = Update::builder()
            .table_name(&self.table)
            .key("pk", s(user_pk(&course.owner_id)))
            .key("sk", s(COUNTERS_SK))
            .update_expression("ADD #c :minusOne")
            .expression_attribute_names("#c", "courses")
            .expression_attribute_values(":minusOne", n(-1))
            .build()
            .map_err(internal)?;
        let result = self
            .client
            .transact_write_items()
            .transact_items(delete(META_SK, Some("attribute_exists(pk)"))?)
            .transact_items(delete(&owner_member, None)?)
            .transact_items(TransactWriteItem::builder().update(counter).build())
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            // The course item is already gone: an earlier run finished.
            Err(e) if cancel_is(&e, 0, "ConditionalCheckFailed") => Ok(()),
            Err(e) => Err(internal(e)),
        }
    }

    /// Replaces the invite code. The old code stops working in the same transaction.
    pub async fn rotate_invite_code(&self, course_id: &str) -> Result<Option<String>, StoreError> {
        for _ in 0..INVITE_ATTEMPTS {
            let Some(course) = self.get_course(course_id).await? else {
                return Ok(None);
            };
            let new_code = ids::new_invite_code();
            let now = crate::domain::now();
            let new_invite = Put::builder()
                .table_name(&self.table)
                .set_item(Some(to_item(&InviteItem::new(&new_code, course_id, &now))?))
                .condition_expression("attribute_not_exists(pk)")
                .build()
                .map_err(internal)?;
            let old_invite = Delete::builder()
                .table_name(&self.table)
                .key("pk", s(invite_pk(&course.invite_code)))
                .key("sk", s("INVITE"))
                .build()
                .map_err(internal)?;
            let update = Update::builder()
                .table_name(&self.table)
                .key("pk", s(course_pk(course_id)))
                .key("sk", s(META_SK))
                .update_expression("SET inviteCode = :new, updatedAt = :now")
                .condition_expression("inviteCode = :old")
                .expression_attribute_values(":new", s(&new_code))
                .expression_attribute_values(":old", s(&course.invite_code))
                .expression_attribute_values(":now", s(&now))
                .build()
                .map_err(internal)?;
            let result = self
                .client
                .transact_write_items()
                .transact_items(TransactWriteItem::builder().put(new_invite).build())
                .transact_items(TransactWriteItem::builder().delete(old_invite).build())
                .transact_items(TransactWriteItem::builder().update(update).build())
                .send()
                .await;
            match result {
                Ok(_) => return Ok(Some(new_code)),
                // The new code is taken, or another rotation ran first. Read and try again.
                Err(e)
                    if cancel_is(&e, 0, "ConditionalCheckFailed")
                        || cancel_is(&e, 2, "ConditionalCheckFailed") =>
                {
                    continue;
                }
                Err(e) if transaction_conflict(&e) => backoff().await,
                Err(e) => return Err(internal(e)),
            }
        }
        Err(StoreError::Internal(
            "could not rotate the invite code".into(),
        ))
    }

    /// The course ID for an invite code (already normalized).
    pub async fn find_invite(&self, code: &str) -> Result<Option<String>, StoreError> {
        match self.get(&invite_pk(code), "INVITE").await? {
            Some(item) => {
                let invite: InviteItem = from_item(item)?;
                Ok(Some(invite.course_id))
            }
            None => Ok(None),
        }
    }

    /// Adds a member. If the user is already a member, returns the existing membership.
    pub async fn add_member(
        &self,
        course_id: &str,
        user: &Principal,
        role: Role,
    ) -> Result<Membership, StoreError> {
        let membership = Membership {
            course_id: course_id.into(),
            user_id: user.user_id.clone(),
            email: user.email.clone(),
            role,
            joined_at: crate::domain::now(),
        };
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(to_item(&MembershipItem::new(&membership))?))
            .condition_expression("attribute_not_exists(pk)")
            .send()
            .await;
        match result {
            Ok(_) => Ok(membership),
            Err(e) if is_conditional_failure(&e) => self
                .get_membership(course_id, &user.user_id)
                .await?
                .ok_or_else(|| StoreError::Internal("membership vanished".into())),
            Err(e) => Err(internal(e)),
        }
    }

    pub async fn list_members(&self, course_id: &str) -> Result<Vec<Membership>, StoreError> {
        let mut q = Query::new("pk = :pk AND begins_with(sk, :prefix)");
        q.values = vec![(":pk", s(course_pk(course_id))), (":prefix", s("MEMBER#"))];
        let items = self.query(q).await?;
        let mut members = items
            .into_iter()
            .map(|i| Ok(from_item::<MembershipItem>(i)?.membership()))
            .collect::<Result<Vec<_>, StoreError>>()?;
        members.sort_by(|a, b| {
            // The owner first, then students by join time.
            (a.role != Role::Owner)
                .cmp(&(b.role != Role::Owner))
                .then_with(|| a.joined_at.cmp(&b.joined_at))
                .then_with(|| a.user_id.cmp(&b.user_id))
        });
        Ok(members)
    }

    pub async fn remove_member(&self, course_id: &str, user_id: &str) -> Result<(), StoreError> {
        self.delete(&course_pk(course_id), &member_sk(user_id))
            .await
    }

    // -----------------------------------------------------------------------------------------
    // Documents
    // -----------------------------------------------------------------------------------------

    /// Saves a document and counts it on the course item. The condition keeps the limit exact
    /// when uploads start in parallel.
    pub async fn create_document(
        &self,
        document: &Document,
        s3_key: &str,
        max_documents: u32,
    ) -> Result<(), StoreError> {
        let put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(to_item(&DocumentItem::new(document, s3_key))?))
            .build()
            .map_err(internal)?;
        let counter = Update::builder()
            .table_name(&self.table)
            .key("pk", s(course_pk(&document.course_id)))
            .key("sk", s(META_SK))
            .update_expression("ADD #d :one")
            .condition_expression(
                "attribute_exists(pk) AND (attribute_not_exists(#d) OR #d < :max)",
            )
            .expression_attribute_names("#d", "docCount")
            .expression_attribute_values(":one", n(1))
            .expression_attribute_values(":max", n(max_documents))
            .build()
            .map_err(internal)?;
        let result = self
            .client
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().put(put).build())
            .transact_items(TransactWriteItem::builder().update(counter).build())
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if cancel_is(&e, 1, "ConditionalCheckFailed") => Err(StoreError::LimitReached),
            Err(e) => Err(internal(e)),
        }
    }

    pub async fn get_document(
        &self,
        course_id: &str,
        document_id: &str,
    ) -> Result<Option<Document>, StoreError> {
        match self
            .get(&course_pk(course_id), &doc_sk(document_id))
            .await?
        {
            Some(item) => Ok(Some(from_item::<DocumentItem>(item)?.document())),
            None => Ok(None),
        }
    }

    /// Documents, newest first.
    pub async fn list_documents(&self, course_id: &str) -> Result<Vec<Document>, StoreError> {
        let mut q = Query::new("pk = :pk AND begins_with(sk, :prefix)");
        q.values = vec![(":pk", s(course_pk(course_id))), (":prefix", s("DOC#"))];
        q.forward = false;
        let items = self.query(q).await?;
        items
            .into_iter()
            .map(|i| Ok(from_item::<DocumentItem>(i)?.document()))
            .collect()
    }

    /// Moves a document from `uploading` to `queued` and returns it.
    /// Returns `None` when it was not `uploading`.
    pub async fn mark_queued(
        &self,
        course_id: &str,
        document_id: &str,
    ) -> Result<Option<Document>, StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(doc_sk(document_id)))
            .update_expression("SET #status = :queued, updatedAt = :now")
            .condition_expression("#status = :uploading")
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":queued", s("queued"))
            .expression_attribute_values(":uploading", s("uploading"))
            .expression_attribute_values(":now", s(crate::domain::now()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew)
            .send()
            .await;
        match result {
            Ok(out) => match out.attributes {
                Some(item) => Ok(Some(from_item::<DocumentItem>(item)?.document())),
                None => Ok(None),
            },
            Err(e) if is_conditional_failure(&e) => Ok(None),
            Err(e) => Err(internal(e)),
        }
    }

    /// Deletes a document and lowers the course counter.
    pub async fn delete_document(
        &self,
        course_id: &str,
        document_id: &str,
    ) -> Result<(), StoreError> {
        let delete = Delete::builder()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(doc_sk(document_id)))
            .condition_expression("attribute_exists(pk)")
            .build()
            .map_err(internal)?;
        let counter = Update::builder()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(META_SK))
            .update_expression("ADD #d :minusOne")
            .condition_expression("attribute_exists(pk)")
            .expression_attribute_names("#d", "docCount")
            .expression_attribute_values(":minusOne", n(-1))
            .build()
            .map_err(internal)?;
        let result = self
            .client
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().delete(delete).build())
            .transact_items(TransactWriteItem::builder().update(counter).build())
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            // The document is already gone.
            Err(e) if cancel_is(&e, 0, "ConditionalCheckFailed") => Ok(()),
            // The course is gone: delete the document item alone.
            Err(e) if cancel_is(&e, 1, "ConditionalCheckFailed") => {
                self.delete(&course_pk(course_id), &doc_sk(document_id))
                    .await
            }
            Err(e) => Err(internal(e)),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Conversations and messages
    // -----------------------------------------------------------------------------------------

    pub async fn create_conversation(
        &self,
        user_id: &str,
        conversation: &Conversation,
    ) -> Result<(), StoreError> {
        self.put(to_item(&ConversationItem::new(user_id, conversation))?)
            .await
    }

    /// The user's own conversation, or `None`.
    pub async fn get_conversation(
        &self,
        user_id: &str,
        conversation_id: &str,
    ) -> Result<Option<Conversation>, StoreError> {
        match self
            .get(&user_pk(user_id), &conv_sk(conversation_id))
            .await?
        {
            Some(item) => Ok(Some(from_item::<ConversationItem>(item)?.conversation())),
            None => Ok(None),
        }
    }

    /// The user's conversations in a course, most recently updated first.
    pub async fn list_conversations(
        &self,
        user_id: &str,
        course_id: &str,
        limit: usize,
    ) -> Result<Vec<Conversation>, StoreError> {
        let mut q = Query::new("gsi1pk = :pk");
        q.index = Some("gsi1");
        q.values = vec![(":pk", s(convs_gsi_pk(user_id, course_id)))];
        q.forward = false;
        q.max_items = limit;
        let items = self.query(q).await?;
        items
            .into_iter()
            .map(|i| Ok(from_item::<ConversationItem>(i)?.conversation()))
            .collect()
    }

    /// Sets `model`, `updatedAt`, and `gsi1sk` after a chat message.
    /// Returns false when the conversation was deleted in the meantime.
    pub async fn touch_conversation(
        &self,
        user_id: &str,
        conversation_id: &str,
        model: &str,
        updated_at: &str,
    ) -> Result<bool, StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(user_pk(user_id)))
            .key("sk", s(conv_sk(conversation_id)))
            .update_expression("SET updatedAt = :at, gsi1sk = :sk, #model = :model")
            .condition_expression("attribute_exists(pk)")
            .expression_attribute_names("#model", "model")
            .expression_attribute_values(":model", s(model))
            .expression_attribute_values(":at", s(updated_at))
            .expression_attribute_values(":sk", s(format!("{updated_at}#{conversation_id}")))
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(e) if is_conditional_failure(&e) => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }

    /// Gives the messages of a deleted conversation a TTL. A chat that was running while the user
    /// deleted the conversation writes messages that nothing can reach.
    pub async fn expire_conversation_messages(
        &self,
        conversation_id: &str,
    ) -> Result<(), StoreError> {
        let ttl = (Utc::now().timestamp()) + ORPHAN_MESSAGE_TTL_SECONDS;
        for (pk, sk) in self.partition_keys(&conv_pk(conversation_id), &[]).await? {
            self.client
                .update_item()
                .table_name(&self.table)
                .key("pk", s(pk))
                .key("sk", s(sk))
                .update_expression("SET #ttl = :ttl")
                .condition_expression("attribute_exists(pk)")
                .expression_attribute_names("#ttl", "ttl")
                .expression_attribute_values(":ttl", n(ttl))
                .send()
                .await
                .map_err(internal)?;
        }
        Ok(())
    }

    /// Deletes the conversation and its messages. Returns false when the user has no such conversation.
    pub async fn delete_conversation(
        &self,
        user_id: &str,
        conversation_id: &str,
    ) -> Result<bool, StoreError> {
        if self
            .get_conversation(user_id, conversation_id)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        let keys = self.partition_keys(&conv_pk(conversation_id), &[]).await?;
        self.batch_delete(keys).await?;
        self.delete(&user_pk(user_id), &conv_sk(conversation_id))
            .await?;
        Ok(true)
    }

    pub async fn add_message(
        &self,
        conversation_id: &str,
        message: &NewMessage,
    ) -> Result<(), StoreError> {
        let item = MessageItem {
            pk: conv_pk(conversation_id),
            sk: format!("MSG#{}", message.id),
            kind: "message".into(),
            role: message.role,
            content: message.content.clone(),
            sources: message.sources.clone(),
            model: message.model.clone(),
            input_tokens: message.usage.input_tokens,
            output_tokens: message.usage.output_tokens,
            created_at: message.created_at.clone(),
        };
        self.put(to_item(&item)?).await
    }

    /// The last `limit` messages, oldest first.
    pub async fn recent_messages(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        let mut q = Query::new("pk = :pk AND begins_with(sk, :prefix)");
        q.values = vec![(":pk", s(conv_pk(conversation_id))), (":prefix", s("MSG#"))];
        q.forward = false;
        q.max_items = limit;
        let items = self.query(q).await?;
        let mut messages = items
            .into_iter()
            .map(|i| Ok(from_item::<MessageItem>(i)?.message()))
            .collect::<Result<Vec<_>, StoreError>>()?;
        messages.reverse();
        Ok(messages)
    }

    // -----------------------------------------------------------------------------------------
    // Questions (teacher insights)
    // -----------------------------------------------------------------------------------------

    pub async fn add_question(
        &self,
        course_id: &str,
        question: &Question,
    ) -> Result<(), StoreError> {
        let item = QuestionItem {
            pk: course_pk(course_id),
            sk: format!("Q#{}", question.id),
            kind: "question".into(),
            user_id: question.user_id.clone(),
            email: question.email.clone(),
            question: question.question.clone(),
            conversation_id: question.conversation_id.clone(),
            created_at: question.created_at.clone(),
            ttl: epoch_in_days(QUESTION_TTL_DAYS),
        };
        self.put(to_item(&item)?).await
    }

    /// The latest questions, newest first.
    pub async fn list_questions(
        &self,
        course_id: &str,
        limit: usize,
    ) -> Result<Vec<Question>, StoreError> {
        let mut q = Query::new("pk = :pk AND begins_with(sk, :prefix)");
        q.values = vec![(":pk", s(course_pk(course_id))), (":prefix", s("Q#"))];
        q.forward = false;
        q.max_items = limit;
        let items = self.query(q).await?;
        items
            .into_iter()
            .map(|i| Ok(from_item::<QuestionItem>(i)?.question()))
            .collect()
    }

    /// Question and distinct-student counts since `since`.
    pub async fn question_stats(
        &self,
        course_id: &str,
        since: DateTime<Utc>,
    ) -> Result<QuestionStats, StoreError> {
        let floor = ids::ulid_floor(u64::try_from(since.timestamp_millis()).unwrap_or(0));
        let mut q = Query::new("pk = :pk AND sk BETWEEN :lo AND :hi");
        q.values = vec![
            (":pk", s(course_pk(course_id))),
            (":lo", s(format!("Q#{floor}"))),
            // `~` sorts after every ULID character.
            (":hi", s("Q#~")),
        ];
        q.projection = Some("userId");
        q.max_items = STATS_MAX_ITEMS;
        let items = self.query(q).await?;
        let students: HashSet<&str> = items
            .iter()
            .filter_map(|i| match i.get("userId") {
                Some(AttributeValue::S(id)) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        Ok(QuestionStats {
            questions: u32::try_from(items.len()).unwrap_or(u32::MAX),
            active_students: u32::try_from(students.len()).unwrap_or(u32::MAX),
        })
    }

    // -----------------------------------------------------------------------------------------
    // Usage
    // -----------------------------------------------------------------------------------------

    fn usage_keys(scope: &UsageScope, date: &str) -> (String, String) {
        match scope {
            UsageScope::User(id) => (user_pk(id), usage_daily_sk(date)),
            UsageScope::Global => (GLOBAL_PK.to_owned(), usage_daily_sk(date)),
        }
    }

    /// Messages used by the user on `date`.
    pub async fn daily_messages(&self, user_id: &str, date: &str) -> Result<u32, StoreError> {
        let item = self.get(&user_pk(user_id), &usage_daily_sk(date)).await?;
        Ok(item
            .and_then(|i| match i.get("messages") {
                Some(AttributeValue::N(v)) => v.parse::<i64>().ok(),
                _ => None,
            })
            .map(|v| u32::try_from(v.max(0)).unwrap_or(u32::MAX))
            .unwrap_or(0))
    }

    /// Failed join attempts by the user on `date`.
    pub async fn failed_joins(&self, user_id: &str, date: &str) -> Result<u32, StoreError> {
        let item = self
            .get(&user_pk(user_id), &format!("JOINFAIL#D#{date}"))
            .await?;
        Ok(item
            .and_then(|i| match i.get("attempts") {
                Some(AttributeValue::N(v)) => v.parse::<u32>().ok(),
                _ => None,
            })
            .unwrap_or(0))
    }

    /// Counts one failed join attempt. Best effort: a failure is logged.
    pub async fn record_failed_join(&self, user_id: &str, date: &str) {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(user_pk(user_id)))
            .key("sk", s(format!("JOINFAIL#D#{date}")))
            .update_expression(
                "SET #type = :type, #ttl = if_not_exists(#ttl, :ttl) ADD attempts :one",
            )
            .expression_attribute_names("#type", "type")
            .expression_attribute_names("#ttl", "ttl")
            .expression_attribute_values(":type", s("joinfail"))
            .expression_attribute_values(":ttl", n(epoch_in_days(DAILY_USAGE_TTL_DAYS)))
            .expression_attribute_values(":one", n(1))
            .send()
            .await;
        if let Err(error) = result {
            tracing::error!(error = %DisplayErrorContext(&error), "could not record a failed join");
        }
    }

    /// Adds token counts to the user's daily and monthly items and to the global daily item.
    /// Best effort: failures are logged.
    pub async fn record_usage(&self, user_id: &str, at: DateTime<Utc>, usage: TokenUsage) {
        let date = usage_date(at);
        let month = usage_month(at);
        let daily_user =
            self.add_tokens(user_pk(user_id), usage_daily_sk(&date), usage, false, true);
        let monthly_user = self.add_tokens(
            user_pk(user_id),
            usage_monthly_sk(&month),
            usage,
            true,
            false,
        );
        let daily_global = self.add_tokens(
            GLOBAL_PK.to_owned(),
            usage_daily_sk(&date),
            usage,
            false,
            true,
        );
        let (a, b, c) = futures::join!(daily_user, monthly_user, daily_global);
        for result in [a, b, c] {
            if let Err(error) = result {
                tracing::error!(%error, "could not record token usage");
            }
        }
    }

    async fn add_tokens(
        &self,
        pk: String,
        sk: String,
        usage: TokenUsage,
        count_message: bool,
        with_ttl: bool,
    ) -> Result<(), StoreError> {
        let mut set = String::from("SET #type = :type");
        let mut add = String::from("ADD inputTokens :in, outputTokens :out");
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .expression_attribute_names("#type", "type")
            .expression_attribute_values(":type", s("usage"))
            .expression_attribute_values(":in", n(usage.input_tokens))
            .expression_attribute_values(":out", n(usage.output_tokens));
        if with_ttl {
            set.push_str(", #ttl = if_not_exists(#ttl, :ttl)");
            request = request
                .expression_attribute_names("#ttl", "ttl")
                .expression_attribute_values(":ttl", n(epoch_in_days(DAILY_USAGE_TTL_DAYS)));
        }
        if count_message {
            add.push_str(", messages :one");
            request = request.expression_attribute_values(":one", n(1));
        }
        request
            .update_expression(format!("{set} {add}"))
            .send()
            .await
            .map_err(internal)?;
        Ok(())
    }
}

#[async_trait]
impl UsageCounter for Store {
    async fn reserve(
        &self,
        scope: &UsageScope,
        date: &str,
        limit: u32,
    ) -> Result<bool, StoreError> {
        self.reserve_counter(scope, date, limit, "messages").await
    }

    async fn global_tokens(&self, date: &str) -> Result<u64, StoreError> {
        let item = self.get(GLOBAL_PK, &usage_daily_sk(date)).await?;
        let number = |name: &str| match item.as_ref().and_then(|i| i.get(name)) {
            Some(AttributeValue::N(v)) => v.parse::<u64>().unwrap_or(0),
            _ => 0,
        };
        Ok(number("inputTokens").saturating_add(number("outputTokens")))
    }

    async fn release(&self, scope: &UsageScope, date: &str) -> Result<(), StoreError> {
        self.release_counter(scope, date, "messages").await
    }
}

#[async_trait]
impl PageCounter for Store {
    async fn reserve_page(
        &self,
        scope: &UsageScope,
        date: &str,
        limit: u32,
    ) -> Result<bool, StoreError> {
        self.reserve_counter(scope, date, limit, "transcribedPages")
            .await
    }

    async fn release_page(&self, scope: &UsageScope, date: &str) -> Result<(), StoreError> {
        self.release_counter(scope, date, "transcribedPages").await
    }
}

impl Store {
    /// Adds one to a daily counter unless it is at `limit`.
    async fn reserve_counter(
        &self,
        scope: &UsageScope,
        date: &str,
        limit: u32,
        attribute: &str,
    ) -> Result<bool, StoreError> {
        let (pk, sk) = Self::usage_keys(scope, date);
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .update_expression("SET #type = :type, #ttl = if_not_exists(#ttl, :ttl) ADD #attr :one")
            .condition_expression("attribute_not_exists(#attr) OR #attr < :limit")
            .expression_attribute_names("#type", "type")
            .expression_attribute_names("#ttl", "ttl")
            .expression_attribute_names("#attr", attribute)
            .expression_attribute_values(":type", s("usage"))
            .expression_attribute_values(":ttl", n(epoch_in_days(DAILY_USAGE_TTL_DAYS)))
            .expression_attribute_values(":one", n(1))
            .expression_attribute_values(":limit", n(limit))
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(e) if is_conditional_failure(&e) => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }

    async fn release_counter(
        &self,
        scope: &UsageScope,
        date: &str,
        attribute: &str,
    ) -> Result<(), StoreError> {
        let (pk, sk) = Self::usage_keys(scope, date);
        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .update_expression("ADD #attr :minusOne")
            .expression_attribute_names("#attr", attribute)
            .expression_attribute_values(":minusOne", n(-1))
            .send()
            .await
            .map_err(internal)?;
        Ok(())
    }

    // -----------------------------------------------------------------------------------------
    // Ingestion
    // -----------------------------------------------------------------------------------------

    async fn update_document_status(
        &self,
        course_id: &str,
        document_id: &str,
        update: &str,
        condition: &str,
        values: Vec<(&str, AttributeValue)>,
        ret: Option<aws_sdk_dynamodb::types::ReturnValue>,
    ) -> Result<Option<Item>, StoreError> {
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(doc_sk(document_id)))
            .update_expression(update)
            .condition_expression(condition)
            .expression_attribute_values(":now", s(crate::domain::now()));
        // DynamoDB rejects names and values that no expression uses, so pass only the used ones.
        let used = |token: &str| update.contains(token) || condition.contains(token);
        for (token, attribute) in [
            ("#status", "status"),
            ("#error", "error"),
            ("#warnings", "warnings"),
        ] {
            if used(token) {
                request = request.expression_attribute_names(token, attribute);
            }
        }
        for (name, value) in values {
            if used(name) {
                request = request.expression_attribute_values(name, value);
            }
        }
        if let Some(ret) = ret {
            request = request.return_values(ret);
        }
        match request.send().await {
            Ok(out) => Ok(Some(out.attributes.unwrap_or_default())),
            Err(e) if is_conditional_failure(&e) => Ok(None),
            Err(e) => Err(internal(e)),
        }
    }

    /// Moves a document to `processing`. It must be `queued`, or `processing` when `retry` is true
    /// (the earlier attempt crashed or timed out). Returns false when the message is stale.
    pub async fn begin_processing(
        &self,
        course_id: &str,
        document_id: &str,
        retry: bool,
    ) -> Result<bool, StoreError> {
        let condition = if retry {
            "#status = :queued OR #status = :processing"
        } else {
            "#status = :queued"
        };
        let done = self
            .update_document_status(
                course_id,
                document_id,
                "SET #status = :processing, updatedAt = :now REMOVE #error, #warnings",
                condition,
                vec![(":queued", s("queued")), (":processing", s("processing"))],
                None,
            )
            .await?;
        Ok(done.is_some())
    }

    /// Puts a document back to `queued` after a transient failure, so the retry accepts it.
    pub async fn requeue(&self, course_id: &str, document_id: &str) -> Result<(), StoreError> {
        self.update_document_status(
            course_id,
            document_id,
            "SET #status = :queued, updatedAt = :now",
            "#status = :processing",
            vec![(":queued", s("queued")), (":processing", s("processing"))],
            None,
        )
        .await?;
        Ok(())
    }

    /// Sets `ready` with the results. Returns false when the document is no longer `processing`
    /// (for example, the owner deleted it).
    pub async fn finish_ingest(
        &self,
        course_id: &str,
        document_id: &str,
        result: &IngestResult,
    ) -> Result<bool, StoreError> {
        let warnings = if result.warnings.is_empty() {
            None
        } else {
            Some(AttributeValue::L(
                result.warnings.iter().map(|w| s(w.as_str())).collect(),
            ))
        };
        let mut sets = String::from(
            "SET #status = :ready, updatedAt = :now, pageCount = :pages, chunkCount = :chunks, transcribedPages = :transcribed",
        );
        let mut removes = String::from("REMOVE #error");
        let mut values = vec![
            (":ready", s("ready")),
            (":processing", s("processing")),
            (":pages", n(result.page_count)),
            (":chunks", n(result.chunk_count)),
            (":transcribed", n(result.transcribed_pages)),
        ];
        match warnings {
            Some(list) => {
                sets.push_str(", #warnings = :warnings");
                values.push((":warnings", list));
            }
            None => removes.push_str(", #warnings"),
        }
        let update = format!("{sets} {removes}");
        let done = self
            .update_document_status(
                course_id,
                document_id,
                &update,
                "#status = :processing",
                values,
                None,
            )
            .await?;
        Ok(done.is_some())
    }

    /// Sets `failed` with an owner-safe message. Returns false when the document is not `queued` or `processing`.
    pub async fn fail_document(
        &self,
        course_id: &str,
        document_id: &str,
        message: &str,
    ) -> Result<bool, StoreError> {
        let done = self
            .update_document_status(
                course_id,
                document_id,
                "SET #status = :failed, #error = :message, updatedAt = :now",
                "#status = :queued OR #status = :processing",
                vec![
                    (":failed", s("failed")),
                    (":message", s(message)),
                    (":queued", s("queued")),
                    (":processing", s("processing")),
                ],
                None,
            )
            .await?;
        Ok(done.is_some())
    }

    /// Moves a document from `queued` back to `uploading` when the ingest message could not be sent.
    pub async fn revert_queued(
        &self,
        course_id: &str,
        document_id: &str,
    ) -> Result<(), StoreError> {
        self.update_document_status(
            course_id,
            document_id,
            "SET #status = :uploading, updatedAt = :now",
            "#status = :queued",
            vec![(":uploading", s("uploading")), (":queued", s("queued"))],
            None,
        )
        .await?;
        Ok(())
    }

    /// Sets `deleting`. Returns the document as it was, or `None` when it was already `deleting`
    /// (or does not exist).
    pub async fn mark_deleting(
        &self,
        course_id: &str,
        document_id: &str,
    ) -> Result<Option<Document>, StoreError> {
        let old = self
            .update_document_status(
                course_id,
                document_id,
                "SET #status = :deleting, updatedAt = :now",
                "attribute_exists(pk) AND #status <> :deleting",
                vec![(":deleting", s("deleting"))],
                Some(aws_sdk_dynamodb::types::ReturnValue::AllOld),
            )
            .await?;
        match old {
            Some(item) if !item.is_empty() => Ok(Some(from_item::<DocumentItem>(item)?.document())),
            _ => Ok(None),
        }
    }

    /// Undoes `mark_deleting` when the delete message could not be sent.
    pub async fn restore_status(
        &self,
        course_id: &str,
        document_id: &str,
        status: DocumentStatus,
    ) -> Result<(), StoreError> {
        let value =
            serde_dynamo::to_attribute_value::<_, AttributeValue>(status).map_err(internal)?;
        self.update_document_status(
            course_id,
            document_id,
            "SET #status = :previous, updatedAt = :now",
            "#status = :deleting",
            vec![(":previous", value), (":deleting", s("deleting"))],
            None,
        )
        .await?;
        Ok(())
    }

    /// Moves the course BM25 version forward, with the key of its blob, in one conditional update.
    /// `expected` is the version that the writer started from. Returns false when another writer
    /// moved it first.
    pub async fn set_bm25_version(
        &self,
        course_id: &str,
        expected: Option<u64>,
        new: u64,
        key: &str,
    ) -> Result<bool, StoreError> {
        let mut request = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(META_SK))
            .update_expression("SET bm25Version = :new, bm25Key = :key")
            .expression_attribute_values(":new", n(new))
            .expression_attribute_values(":key", s(key));
        request = match expected {
            None => request
                .condition_expression("attribute_exists(pk) AND attribute_not_exists(bm25Version)"),
            Some(old) => request
                .condition_expression("attribute_exists(pk) AND bm25Version = :old")
                .expression_attribute_values(":old", n(old)),
        };
        match request.send().await {
            Ok(_) => Ok(true),
            Err(e) if is_conditional_failure(&e) => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }

    /// Marks the course `deleting`. Members get `404` from then on. Returns false when the course
    /// does not exist.
    pub async fn mark_course_deleting(&self, course_id: &str) -> Result<bool, StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(META_SK))
            .update_expression("SET #status = :deleting, updatedAt = :now")
            .condition_expression("attribute_exists(pk)")
            .expression_attribute_names("#status", "status")
            .expression_attribute_values(":deleting", s("deleting"))
            .expression_attribute_values(":now", s(crate::domain::now()))
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(e) if is_conditional_failure(&e) => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }

    /// Deletes every membership of the course (the owner's too), so the course leaves every list.
    pub async fn delete_memberships(&self, course_id: &str) -> Result<(), StoreError> {
        let mut q = Query::new("pk = :pk AND begins_with(sk, :prefix)");
        q.values = vec![(":pk", s(course_pk(course_id))), (":prefix", s("MEMBER#"))];
        q.projection = Some("pk, sk");
        let keys: Vec<(String, String)> = self
            .query(q)
            .await?
            .into_iter()
            .filter_map(|mut item| match (item.remove("pk"), item.remove("sk")) {
                (Some(AttributeValue::S(pk)), Some(AttributeValue::S(sk))) => Some((pk, sk)),
                _ => None,
            })
            .collect();
        self.batch_delete(keys).await
    }

    /// Records how many chunks the worker is about to write, before the first vector goes out.
    pub async fn set_planned_chunks(
        &self,
        course_id: &str,
        document_id: &str,
        planned: u32,
    ) -> Result<(), StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(course_pk(course_id)))
            .key("sk", s(doc_sk(document_id)))
            .update_expression("SET plannedChunkCount = :planned")
            .condition_expression("attribute_exists(pk)")
            .expression_attribute_values(":planned", n(planned))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_conditional_failure(&e) => Ok(()),
            Err(e) => Err(internal(e)),
        }
    }

    /// Sets `updatedAt` of a `deleting` document, so a repeated `DELETE` can tell how long ago the
    /// last `delete` message was sent. Returns false when the document is not `deleting`.
    pub async fn touch_deleting(
        &self,
        course_id: &str,
        document_id: &str,
    ) -> Result<bool, StoreError> {
        let done = self
            .update_document_status(
                course_id,
                document_id,
                "SET updatedAt = :now",
                "#status = :deleting",
                vec![(":deleting", s("deleting"))],
                None,
            )
            .await?;
        Ok(done.is_some())
    }
}

/// What a finished ingest reports.
#[derive(Debug, Clone, Default)]
pub struct IngestResult {
    pub page_count: u32,
    pub chunk_count: u32,
    pub transcribed_pages: u32,
    pub warnings: Vec<String>,
}

/// True when a transaction was cancelled and item `index` had the cancellation reason `code`.
fn cancel_is<R>(error: &SdkError<TransactWriteItemsError, R>, index: usize, code: &str) -> bool {
    match error.as_service_error() {
        Some(TransactWriteItemsError::TransactionCanceledException(cancelled)) => cancelled
            .cancellation_reasons()
            .get(index)
            .and_then(|reason| reason.code())
            .is_some_and(|c| c == code),
        _ => false,
    }
}

/// True when another transaction touched the same items at the same time. A retry can succeed.
fn transaction_conflict<R>(error: &SdkError<TransactWriteItemsError, R>) -> bool {
    match error.as_service_error() {
        Some(TransactWriteItemsError::TransactionCanceledException(cancelled)) => cancelled
            .cancellation_reasons()
            .iter()
            .any(|r| r.code() == Some("TransactionConflict")),
        Some(TransactWriteItemsError::TransactionInProgressException(_)) => true,
        _ => false,
    }
}

async fn backoff() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}
