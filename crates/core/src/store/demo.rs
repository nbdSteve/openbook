//! DynamoDB items of the public demo (`docs/architecture.md`, "DynamoDB table"): the reserved
//! course pointer, the daily salt, and the limit counters.

use async_trait::async_trait;
use aws_sdk_dynamodb::primitives::Blob;
use aws_sdk_dynamodb::types::AttributeValue;

use aws_sdk_dynamodb::types::{Put, TransactWriteItem, Update};

use super::{
    CourseItem, Item, META_SK, Query, Store, StoreError, cancel_is, course_pk, from_item, internal,
    is_conditional_failure, n, s, to_item,
};
use crate::demo::{
    Counter, DemoConversations, DemoCounters, DemoCourse, MAX_TURNS, OWNER_ID, SALT_BYTES,
    SaltStore, StoredConversation,
};
use crate::domain::{CourseRecord, MessageRole, TutoringStyle};
use crate::ids;
use crate::prompt::HISTORY_WINDOW;

/// The reserved partition of the course pointer.
const COURSE_PK: &str = "DEMO#COURSE";
const ACTIVE_SK: &str = "ACTIVE";
const PENDING_SK: &str = "PENDING";
const SALT_PK: &str = "DEMO#SALT";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DemoCourseItem {
    pk: String,
    sk: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(flatten)]
    course: DemoCourse,
}

impl Store {
    /// The active demo course, or `None` before the first provisioning.
    pub async fn get_demo_course(&self) -> Result<Option<DemoCourse>, StoreError> {
        match self.get(COURSE_PK, ACTIVE_SK).await? {
            Some(item) => Ok(Some(from_item::<DemoCourseItem>(item)?.course)),
            None => Ok(None),
        }
    }

    /// Makes `course` the active demo course. Provisioning calls it when the new course is ready.
    pub async fn put_demo_course(&self, course: &DemoCourse) -> Result<(), StoreError> {
        let item = DemoCourseItem {
            pk: COURSE_PK.into(),
            sk: ACTIVE_SK.into(),
            kind: "demo_course".into(),
            course: course.clone(),
        };
        self.put(to_item(&item)?).await
    }

    /// The course that a provisioning run created and has not activated, if a run was cut short.
    pub async fn get_pending_demo_course(&self) -> Result<Option<String>, StoreError> {
        Ok(self.get(COURSE_PK, PENDING_SK).await?.and_then(|mut item| {
            match item.remove("courseId") {
                Some(AttributeValue::S(id)) => Some(id),
                _ => None,
            }
        }))
    }

    pub async fn put_pending_demo_course(&self, course_id: &str) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("pk", s(COURSE_PK))
            .item("sk", s(PENDING_SK))
            .item("type", s("demo_pending"))
            .item("courseId", s(course_id))
            .send()
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub async fn delete_pending_demo_course(&self) -> Result<(), StoreError> {
        self.delete(COURSE_PK, PENDING_SK).await
    }

    /// Creates the course item of the demo. It has no membership and no invite code, so no route
    /// can reach it: every course route checks membership, and nobody is a member. The owner is
    /// `demo::OWNER_ID`, which is not a Cognito user.
    pub async fn create_demo_course(
        &self,
        name: &str,
        description: &str,
        style: TutoringStyle,
        model: &str,
    ) -> Result<CourseRecord, StoreError> {
        let now = crate::domain::now();
        let record = CourseRecord {
            id: ids::new_id(),
            name: name.into(),
            description: description.into(),
            tutoring_style: style,
            model: model.into(),
            owner_id: OWNER_ID.into(),
            invite_code: String::new(),
            bm25_version: None,
            bm25_key: None,
            deleting: false,
            created_at: now.clone(),
            updated_at: now,
        };
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(to_item(&CourseItem::new(&record))?))
            .condition_expression("attribute_not_exists(pk)")
            .send()
            .await
            .map_err(internal)?;
        Ok(record)
    }

    /// True when the course item exists and is not being deleted.
    pub async fn course_is_live(&self, course_id: &str) -> Result<bool, StoreError> {
        let item = self.get(&course_pk(course_id), META_SK).await?;
        Ok(item.is_some_and(
            |i| !matches!(i.get("status"), Some(AttributeValue::S(v)) if v == "deleting"),
        ))
    }
}

#[async_trait]
impl SaltStore for Store {
    async fn salt_for(
        &self,
        date: &str,
        fresh: [u8; SALT_BYTES],
        expires_at: i64,
    ) -> Result<Vec<u8>, StoreError> {
        let sk = format!("D#{date}");
        let read = |item: Option<Item>| match item.and_then(|mut i| i.remove("salt")) {
            Some(AttributeValue::B(blob)) => Some(blob.into_inner()),
            _ => None,
        };
        if let Some(salt) = read(self.get(SALT_PK, &sk).await?) {
            return Ok(salt);
        }
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .item("pk", s(SALT_PK))
            .item("sk", s(&sk))
            .item("type", s("demo_salt"))
            .item("salt", AttributeValue::B(Blob::new(fresh.to_vec())))
            .item("ttl", n(expires_at))
            .condition_expression("attribute_not_exists(pk)")
            .send()
            .await;
        match result {
            Ok(_) => Ok(fresh.to_vec()),
            // Another server wrote the salt first. Use that one.
            Err(e) if is_conditional_failure(&e) => read(self.get(SALT_PK, &sk).await?)
                .ok_or_else(|| StoreError::Internal("the demo salt vanished".into())),
            Err(e) => Err(internal(e)),
        }
    }
}

#[async_trait]
impl DemoCounters for Store {
    async fn reserve(
        &self,
        counter: &Counter,
        amount: u64,
        limit: u64,
    ) -> Result<bool, StoreError> {
        // The sum stays at or below the limit: `count <= limit - amount`, or no count yet.
        let Some(room) = limit.checked_sub(amount) else {
            return Ok(false);
        };
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(&counter.pk))
            .key("sk", s(&counter.sk))
            .update_expression("SET #type = :type, #ttl = if_not_exists(#ttl, :ttl) ADD #c :amount")
            .condition_expression("attribute_not_exists(#c) OR #c <= :room")
            .expression_attribute_names("#type", "type")
            .expression_attribute_names("#ttl", "ttl")
            .expression_attribute_names("#c", "count")
            .expression_attribute_values(":type", s("demo_counter"))
            .expression_attribute_values(":ttl", n(counter.expires_at))
            .expression_attribute_values(":amount", n(amount))
            .expression_attribute_values(":room", n(room))
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(e) if is_conditional_failure(&e) => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }

    async fn release(&self, counter: &Counter, amount: u64) -> Result<(), StoreError> {
        if amount == 0 {
            return Ok(());
        }
        // Never below zero: when less than `amount` is left, set it to zero.
        let first = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(&counter.pk))
            .key("sk", s(&counter.sk))
            .update_expression("SET #c = #c - :amount")
            .condition_expression("#c >= :amount")
            .expression_attribute_names("#c", "count")
            .expression_attribute_values(":amount", n(amount))
            .send()
            .await;
        match first {
            Ok(_) => Ok(()),
            Err(e) if is_conditional_failure(&e) => {
                let zero = self
                    .client
                    .update_item()
                    .table_name(&self.table)
                    .key("pk", s(&counter.pk))
                    .key("sk", s(&counter.sk))
                    .update_expression("SET #c = :zero")
                    .condition_expression("attribute_exists(#c)")
                    .expression_attribute_names("#c", "count")
                    .expression_attribute_values(":zero", n(0))
                    .send()
                    .await;
                match zero {
                    Ok(_) => Ok(()),
                    // The counter is gone: nothing to give back.
                    Err(e) if is_conditional_failure(&e) => Ok(()),
                    Err(e) => Err(internal(e)),
                }
            }
            Err(e) => Err(internal(e)),
        }
    }

    async fn charge(&self, counter: &Counter, amount: u64) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(&counter.pk))
            .key("sk", s(&counter.sk))
            .update_expression("SET #type = :type, #ttl = if_not_exists(#ttl, :ttl) ADD #c :amount")
            .expression_attribute_names("#type", "type")
            .expression_attribute_names("#ttl", "ttl")
            .expression_attribute_names("#c", "count")
            .expression_attribute_values(":type", s("demo_counter"))
            .expression_attribute_values(":ttl", n(counter.expires_at))
            .expression_attribute_values(":amount", n(amount))
            .send()
            .await
            .map_err(internal)?;
        Ok(())
    }
}

fn conversation_pk(id: &str) -> String {
    format!("DEMO#CONV#{id}")
}

#[async_trait]
impl DemoConversations for Store {
    async fn load(&self, id: &str) -> Result<Option<StoredConversation>, StoreError> {
        let mut q = Query::new("pk = :pk");
        q.values.push((":pk", s(conversation_pk(id))));
        let items = self.query(q).await?;
        let number = |item: &Item, name: &str| match item.get(name) {
            Some(AttributeValue::N(v)) => v.parse::<i64>().ok(),
            _ => None,
        };
        let text = |item: &Item, name: &str| match item.get(name) {
            Some(AttributeValue::S(v)) => Some(v.clone()),
            _ => None,
        };
        let Some(meta) = items
            .iter()
            .find(|i| text(i, "sk").as_deref() == Some("META"))
        else {
            return Ok(None);
        };
        let mut messages: Vec<(String, MessageRole, String)> = items
            .iter()
            .filter_map(|i| {
                let sk = text(i, "sk")?;
                sk.starts_with("M#").then_some(())?;
                let role = match text(i, "role")?.as_str() {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    _ => return None,
                };
                Some((sk, role, text(i, "content")?))
            })
            .collect();
        messages.sort_by(|a, b| a.0.cmp(&b.0));
        let keep = messages.len().saturating_sub(HISTORY_WINDOW);
        Ok(Some(StoredConversation {
            turns: u32::try_from(number(meta, "turns").unwrap_or(0)).unwrap_or(0),
            expires_at: number(meta, "ttl").unwrap_or(0),
            messages: messages
                .into_iter()
                .skip(keep)
                .map(|(_, role, content)| (role, content))
                .collect(),
        }))
    }

    async fn append(
        &self,
        id: &str,
        question: &str,
        answer: &str,
        expires_at: i64,
    ) -> Result<bool, StoreError> {
        let pk = conversation_pk(id);
        let meta = Update::builder()
            .table_name(&self.table)
            .key("pk", s(&pk))
            .key("sk", s("META"))
            .update_expression("SET #type = :type, #ttl = if_not_exists(#ttl, :ttl) ADD turns :one")
            .condition_expression("attribute_not_exists(turns) OR turns < :max")
            .expression_attribute_names("#type", "type")
            .expression_attribute_names("#ttl", "ttl")
            .expression_attribute_values(":type", s("demo_conversation"))
            .expression_attribute_values(":ttl", n(expires_at))
            .expression_attribute_values(":one", n(1))
            .expression_attribute_values(":max", n(MAX_TURNS))
            .build()
            .map_err(internal)?;
        let message = |role: &str, content: &str| -> Result<TransactWriteItem, StoreError> {
            let put = Put::builder()
                .table_name(&self.table)
                .item("pk", s(&pk))
                // IDs from one process increase, so the question sorts before the answer.
                .item("sk", s(format!("M#{}", ids::new_id())))
                .item("type", s("demo_message"))
                .item("role", s(role))
                .item("content", s(content))
                .item("ttl", n(expires_at))
                .build()
                .map_err(internal)?;
            Ok(TransactWriteItem::builder().put(put).build())
        };
        let result = self
            .client
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().update(meta).build())
            .transact_items(message("user", question)?)
            .transact_items(message("assistant", answer)?)
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(e) if cancel_is(&e, 0, "ConditionalCheckFailed") => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }
}
