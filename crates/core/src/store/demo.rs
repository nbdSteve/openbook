//! DynamoDB items of the public demo (`docs/architecture.md`, "DynamoDB table"): the reserved
//! course pointer, the daily salt, and the limit counters.

use async_trait::async_trait;
use aws_sdk_dynamodb::primitives::Blob;
use aws_sdk_dynamodb::types::AttributeValue;

use aws_sdk_dynamodb::types::{Delete, Put, TransactWriteItem, Update};

use super::{
    CourseItem, Item, META_SK, Query, Store, StoreError, cancel_is, course_pk, from_item, internal,
    is_conditional_failure, n, s, to_item, transaction_conflict,
};
use crate::demo::{
    BudgetHit, Counter, DemoBudgets, DemoConversations, DemoCounters, DemoCourse, MAX_TURNS,
    OWNER_ID, SALT_BYTES, SaltStore, StoredConversation, StoredMessage, TurnReservation,
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
            // `demo` and a ULID: the role of the demo function can read only courses with this prefix.
            id: ids::new_demo_course_id(),
            name: name.into(),
            description: description.into(),
            tutoring_style: style,
            allow_style_choice: true,
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
        // No fallback: a counter that holds less than `amount` is an error for the caller to log.
        let result = self
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
        match result {
            Ok(_) => Ok(()),
            Err(e) if is_conditional_failure(&e) => Err(StoreError::Internal(
                "a demo counter holds less than the amount to give back".into(),
            )),
            Err(e) => Err(internal(e)),
        }
    }
}

/// One counter of a turn: `ADD count :amount` when the sum stays within the limit.
fn reserve_update(
    table: &str,
    counter: &Counter,
    amount: u64,
    limit: u64,
) -> Result<TransactWriteItem, StoreError> {
    // `amount <= limit` is checked by the caller, so the room cannot underflow.
    let room = limit.saturating_sub(amount);
    let update = Update::builder()
        .table_name(table)
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
        .build()
        .map_err(internal)?;
    Ok(TransactWriteItem::builder().update(update).build())
}

/// One counter of a turn: gives `refund` back when the counter holds at least that much.
fn refund_update(
    table: &str,
    counter: &Counter,
    refund: u64,
) -> Result<TransactWriteItem, StoreError> {
    let update = Update::builder()
        .table_name(table)
        .key("pk", s(&counter.pk))
        .key("sk", s(&counter.sk))
        .update_expression("SET #c = #c - :refund")
        .condition_expression("#c >= :refund")
        .expression_attribute_names("#c", "count")
        .expression_attribute_values(":refund", n(refund))
        .build()
        .map_err(internal)?;
    Ok(TransactWriteItem::builder().update(update).build())
}

#[async_trait]
impl DemoBudgets for Store {
    async fn reserve_turn(
        &self,
        turn: &TurnReservation,
    ) -> Result<Result<(), BudgetHit>, StoreError> {
        let item = Put::builder()
            .table_name(&self.table)
            .item("pk", s(&turn.item.pk))
            .item("sk", s(&turn.item.sk))
            .item("type", s("demo_reservation"))
            .item("amount", n(turn.amount))
            .item("ttl", n(turn.item.expires_at))
            .condition_expression("attribute_not_exists(pk)")
            .build()
            .map_err(internal)?;
        let result = self
            .client
            .transact_write_items()
            // The turn ID makes a retry of this call safe: DynamoDB answers it once.
            .client_request_token(&turn.turn_id)
            .transact_items(reserve_update(
                &self.table,
                &turn.global,
                turn.amount,
                turn.global_limit,
            )?)
            .transact_items(reserve_update(
                &self.table,
                &turn.visitor,
                turn.amount,
                turn.visitor_limit,
            )?)
            .transact_items(TransactWriteItem::builder().put(item).build())
            .send()
            .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) if cancel_is(&e, 0, "ConditionalCheckFailed") => Ok(Err(BudgetHit::Global)),
            Err(e) if cancel_is(&e, 1, "ConditionalCheckFailed") => Ok(Err(BudgetHit::Visitor)),
            // Another transaction runs on the same counter. The SDK does not retry it: the caller does.
            Err(e) if transaction_conflict(&e) => Err(StoreError::Conflict),
            Err(e) => Err(internal(e)),
        }
    }

    async fn settle_turn(&self, turn: &TurnReservation, refund: u64) -> Result<bool, StoreError> {
        let delete = Delete::builder()
            .table_name(&self.table)
            .key("pk", s(&turn.item.pk))
            .key("sk", s(&turn.item.sk))
            .condition_expression("attribute_exists(pk)")
            .build()
            .map_err(internal)?;
        let mut request = self
            .client
            .transact_write_items()
            .client_request_token(format!("{}s", turn.turn_id))
            .transact_items(TransactWriteItem::builder().delete(delete).build());
        if refund > 0 {
            request = request
                .transact_items(refund_update(&self.table, &turn.global, refund)?)
                .transact_items(refund_update(&self.table, &turn.visitor, refund)?);
        }
        match request.send().await {
            Ok(_) => Ok(true),
            // The item is gone: this turn was settled before, so there is nothing to give back.
            Err(e) if cancel_is(&e, 0, "ConditionalCheckFailed") => Ok(false),
            Err(e) if transaction_conflict(&e) => Err(StoreError::Conflict),
            Err(e) => Err(internal(e)),
        }
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
        let mut messages: Vec<(String, StoredMessage)> = items
            .iter()
            .filter_map(|i| {
                let sk = text(i, "sk")?;
                sk.starts_with("M#").then_some(())?;
                let role = match text(i, "role")?.as_str() {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    _ => return None,
                };
                Some((
                    sk,
                    StoredMessage {
                        role,
                        text: text(i, "content")?,
                        output_tokens: number(i, "outputTokens")
                            .and_then(|v| u64::try_from(v).ok()),
                    },
                ))
            })
            .collect();
        messages.sort_by(|a, b| a.0.cmp(&b.0));
        let keep = messages.len().saturating_sub(HISTORY_WINDOW);
        Ok(Some(StoredConversation {
            turns: u32::try_from(number(meta, "turns").unwrap_or(0)).unwrap_or(0),
            expires_at: number(meta, "ttl").unwrap_or(0),
            messages: messages.into_iter().skip(keep).map(|(_, m)| m).collect(),
        }))
    }

    async fn append(
        &self,
        id: &str,
        question: &str,
        answer: &str,
        answer_tokens: u64,
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
        let message = |role: &str,
                       content: &str,
                       tokens: Option<u64>|
         -> Result<TransactWriteItem, StoreError> {
            let mut put = Put::builder()
                .table_name(&self.table)
                .item("pk", s(&pk))
                // IDs from one process increase, so the question sorts before the answer.
                .item("sk", s(format!("M#{}", ids::new_id())))
                .item("type", s("demo_message"))
                .item("role", s(role))
                .item("content", s(content))
                .item("ttl", n(expires_at));
            if let Some(tokens) = tokens {
                put = put.item("outputTokens", n(tokens));
            }
            Ok(TransactWriteItem::builder()
                .put(put.build().map_err(internal)?)
                .build())
        };
        let result = self
            .client
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().update(meta).build())
            .transact_items(message("user", question, None)?)
            .transact_items(message("assistant", answer, Some(answer_tokens))?)
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(e) if cancel_is(&e, 0, "ConditionalCheckFailed") => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }
}
