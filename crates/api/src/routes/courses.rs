use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use openbook_core::domain::{Role, TutoringStyle};
use openbook_core::ids;
use openbook_core::queue::IngestMessage;
use openbook_core::quota::usage_date;
use openbook_core::store::{CoursePatch, StoreError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::extract::{ApiJson, ApiPath, AuthUser};
use crate::{AppState, validate};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCourse {
    name: String,
    description: Option<String>,
    tutoring_style: Option<TutoringStyle>,
    allow_style_choice: Option<bool>,
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCourse {
    name: Option<String>,
    description: Option<String>,
    tutoring_style: Option<TutoringStyle>,
    allow_style_choice: Option<bool>,
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinCourse {
    invite_code: String,
}

/// A model ID from a request must be in the catalog.
pub fn known_model(state: &AppState, id: &str) -> Result<String, ApiError> {
    match state.models.catalog().get(id) {
        Some(entry) => Ok(entry.id.clone()),
        None => Err(ApiError::InvalidRequest(format!("Unknown model {id:?}"))),
    }
}

/// `GET /api/courses`
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Json<Value>, ApiError> {
    let memberships = state.store.list_memberships(&user.user_id).await?;
    let ids: Vec<String> = memberships.iter().map(|m| m.course_id.clone()).collect();
    let mut records: HashMap<String, _> = state
        .store
        .get_courses(&ids)
        .await?
        .into_iter()
        .filter(|c| !c.deleting)
        .map(|c| (c.id.clone(), c))
        .collect();
    let courses: Vec<_> = memberships
        .iter()
        .filter_map(|m| {
            records
                .remove(&m.course_id)
                .map(|c| state.course_view(&c, m.role))
        })
        .collect();
    Ok(Json(json!({ "courses": courses })))
}

/// `POST /api/courses`
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiJson(body): ApiJson<CreateCourse>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let name = validate::course_name(&body.name)?;
    let description = validate::description(body.description.as_deref().unwrap_or(""))?;
    let style = body.tutoring_style.unwrap_or_default();
    let model = match &body.model {
        Some(id) => known_model(&state, id)?,
        None => state.models.catalog().default_chat_id().to_owned(),
    };

    let max = state.config.limits.courses_owned;
    let course = state
        .store
        .create_course(&user, &name, &description, style, &model, max)
        .await
        .map_err(|e| match e {
            StoreError::LimitReached => {
                ApiError::LimitReached(format!("You can own at most {max} courses"))
            }
            other => other.into(),
        })?;
    let course = match body.allow_style_choice {
        Some(allow) => state
            .store
            .update_course(
                &course.id,
                &CoursePatch {
                    allow_style_choice: Some(allow),
                    ..CoursePatch::default()
                },
            )
            .await?
            .ok_or_else(|| ApiError::not_found("Course"))?,
        None => course,
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({ "course": state.course_view(&course, Role::Owner) })),
    ))
}

/// `GET /api/courses/{courseId}`
pub async fn get(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let (course, membership) = state.require_member(&user, &course_id).await?;
    Ok(Json(
        json!({ "course": state.course_view(&course, membership.role) }),
    ))
}

/// `PATCH /api/courses/{courseId}` (owner)
pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
    ApiJson(body): ApiJson<UpdateCourse>,
) -> Result<Json<Value>, ApiError> {
    let (course, membership) = state.require_owner(&user, &course_id).await?;
    let patch = CoursePatch {
        name: body
            .name
            .as_deref()
            .map(validate::course_name)
            .transpose()?,
        description: body
            .description
            .as_deref()
            .map(validate::description)
            .transpose()?,
        tutoring_style: body.tutoring_style,
        allow_style_choice: body.allow_style_choice,
        model: body
            .model
            .as_deref()
            .map(|id| known_model(&state, id))
            .transpose()?,
    };
    let updated = state
        .store
        .update_course(&course.id, &patch)
        .await?
        .ok_or_else(|| ApiError::not_found("Course"))?;
    Ok(Json(
        json!({ "course": state.course_view(&updated, membership.role) }),
    ))
}

/// `DELETE /api/courses/{courseId}` (owner)
///
/// The API marks the course `deleting` (members get `404` at once), deletes the invite and the
/// memberships, and sends `delete_course`. The worker deletes the vector index, the files, the
/// documents, and the course item, so a delete cannot race an ingestion in the same course.
/// The owner can repeat the request when it failed halfway.
pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let course_id = validate::ulid_path(&course_id, "Course")?;
    let course = state
        .store
        .get_course(&course_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Course"))?;
    if course.owner_id != user.user_id {
        // Members who are not the owner get 403. Everyone else, and everyone once the delete started, gets 404.
        state.require_member(&user, &course_id).await?;
        return Err(ApiError::Forbidden(
            "Only the course owner can do this".into(),
        ));
    }
    state.store.mark_course_deleting(&course.id).await?;
    // The invite stops working at once, and the course leaves every list.
    state.store.delete_invite(&course).await?;
    state.store.delete_memberships(&course.id).await?;
    if let Err(error) = state
        .queue
        .send(&IngestMessage::delete_course(&course.id))
        .await
    {
        // The course stays `deleting`. The owner can send the request again.
        tracing::error!(%error, "could not send the delete_course message");
        return Err(ApiError::Unavailable);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/courses/{courseId}/invite-code` (owner)
pub async fn rotate_invite_code(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let (course, _) = state.require_owner(&user, &course_id).await?;
    let code = state
        .store
        .rotate_invite_code(&course.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Course"))?;
    Ok(Json(json!({ "inviteCode": code })))
}

/// `POST /api/join`
pub async fn join(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiJson(body): ApiJson<JoinCourse>,
) -> Result<Json<Value>, ApiError> {
    let date = usage_date(Utc::now());
    let limit = state.config.limits.failed_joins_per_day;
    if state.store.failed_joins(&user.user_id, &date).await? >= limit {
        return Err(ApiError::TooManyAttempts);
    }
    let unknown = || ApiError::not_found("Invite code");
    let found = async {
        // The block returns `Result<_, StoreError>` so that `?` has one error type.
        let Some(code) = ids::normalize_invite_code(&body.invite_code) else {
            return Ok(None);
        };
        let Some(course_id) = state.store.find_invite(&code).await? else {
            return Ok(None);
        };
        // A course that the worker is deleting cannot be joined.
        Ok::<_, StoreError>(
            state
                .store
                .get_course(&course_id)
                .await?
                .filter(|c| !c.deleting),
        )
    }
    .await?;
    let Some(course) = found else {
        state.store.record_failed_join(&user.user_id, &date).await;
        return Err(unknown());
    };
    // Joining twice returns the existing membership, so an owner keeps the owner role.
    let membership = state
        .store
        .add_member(&course.id, &user, Role::Student)
        .await?;
    Ok(Json(
        json!({ "course": state.course_view(&course, membership.role) }),
    ))
}

/// `GET /api/courses/{courseId}/members` (owner)
pub async fn members(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let (course, _) = state.require_owner(&user, &course_id).await?;
    let members: Vec<_> = state
        .store
        .list_members(&course.id)
        .await?
        .iter()
        .map(|m| m.member())
        .collect();
    Ok(Json(json!({ "members": members })))
}

/// `DELETE /api/courses/{courseId}/members/{userId}`
/// An owner removes a student. A student removes only themselves. Nobody removes the owner.
pub async fn remove_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath((course_id, target_id)): ApiPath<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (course, membership) = state.require_member(&user, &course_id).await?;
    let target_id = validate::user_id_path(&target_id)?;
    if membership.role == Role::Student && target_id != user.user_id {
        return Err(ApiError::Forbidden("You can only remove yourself".into()));
    }
    let target = if target_id == user.user_id {
        membership
    } else {
        state
            .store
            .get_membership(&course.id, &target_id)
            .await?
            .ok_or_else(|| ApiError::not_found("Member"))?
    };
    if target.role == Role::Owner {
        return Err(ApiError::Conflict(
            "The course owner cannot be removed".into(),
        ));
    }
    state
        .store
        .remove_member(&course.id, &target.user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
