//! DynamoDB items of the public demo (`docs/architecture.md`, "DynamoDB table"): the reserved
//! course pointer, the daily salt, and the limit counters.

use async_trait::async_trait;
use aws_sdk_dynamodb::primitives::Blob;
use aws_sdk_dynamodb::types::AttributeValue;

use super::{
    CourseItem, Item, META_SK, Store, StoreError, course_pk, from_item, internal,
    is_conditional_failure, n, s, to_item,
};
use crate::demo::{Counter, DemoCounters, DemoCourse, OWNER_ID, SALT_BYTES, SaltStore};
use crate::domain::{CourseRecord, TutoringStyle};
use crate::ids;

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
    async fn reserve(&self, counter: &Counter, limit: u32) -> Result<bool, StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(&counter.pk))
            .key("sk", s(&counter.sk))
            .update_expression("SET #type = :type, #ttl = if_not_exists(#ttl, :ttl) ADD #c :one")
            .condition_expression("attribute_not_exists(#c) OR #c < :limit")
            .expression_attribute_names("#type", "type")
            .expression_attribute_names("#ttl", "ttl")
            .expression_attribute_names("#c", "count")
            .expression_attribute_values(":type", s("demo_counter"))
            .expression_attribute_values(":ttl", n(counter.expires_at))
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

    async fn release(&self, counter: &Counter) -> Result<(), StoreError> {
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", s(&counter.pk))
            .key("sk", s(&counter.sk))
            .update_expression("ADD #c :minusOne")
            .condition_expression("#c > :zero")
            .expression_attribute_names("#c", "count")
            .expression_attribute_values(":minusOne", n(-1))
            .expression_attribute_values(":zero", n(0))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            // The counter is already at zero or gone.
            Err(e) if is_conditional_failure(&e) => Ok(()),
            Err(e) => Err(internal(e)),
        }
    }
}
