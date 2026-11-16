use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use openbook_core::domain::{Document, DocumentStatus, Role};
use openbook_core::files::source_key;
use openbook_core::ids;
use openbook_core::queue::{Action, IngestMessage};
use openbook_core::store::StoreError;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::extract::{ApiJson, ApiPath, AuthUser};
use crate::{AppState, validate};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDocument {
    filename: String,
    size_bytes: u64,
    content_type: String,
}

/// `GET /api/courses/{courseId}/documents` (member). Students see only `ready` documents.
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
) -> Result<Json<Value>, ApiError> {
    let (course, membership) = state.require_member(&user, &course_id).await?;
    let mut documents = state.store.list_documents(&course.id).await?;
    if membership.role != Role::Owner {
        documents.retain(|d| d.status == DocumentStatus::Ready);
    }
    Ok(Json(json!({ "documents": documents })))
}

/// `POST /api/courses/{courseId}/documents` (owner)
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(course_id): ApiPath<String>,
    ApiJson(body): ApiJson<CreateDocument>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (course, _) = state.require_owner(&user, &course_id).await?;
    let filename = validate::pdf_filename(&body.filename)?;
    validate::pdf_content_type(&body.content_type)?;
    let size = validate::upload_size(body.size_bytes, state.config.limits.upload_bytes)?;

    let now = openbook_core::domain::now();
    let document = Document {
        id: ids::new_id(),
        course_id: course.id.clone(),
        filename,
        size_bytes: size,
        status: DocumentStatus::Uploading,
        error: None,
        page_count: None,
        chunk_count: None,
        transcribed_pages: None,
        warnings: None,
        created_at: now.clone(),
        updated_at: now,
    };
    let key = source_key(&course.id, &document.id);
    let upload = state
        .files
        .presign_put(&key, "application/pdf", size)
        .await?;
    let max = state.config.limits.documents_per_course;
    state
        .store
        .create_document(&document, &key, max)
        .await
        .map_err(|e| match e {
            StoreError::LimitReached => {
                ApiError::LimitReached(format!("A course can have at most {max} documents"))
            }
            other => other.into(),
        })?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "document": document,
            "upload": {
                "url": upload.url,
                "method": "PUT",
                "headers": { "content-type": "application/pdf" },
                "expiresAt": upload.expires_at,
            },
        })),
    ))
}

async fn load_document(
    state: &AppState,
    course_id: &str,
    document_id: &str,
) -> Result<Document, ApiError> {
    let document_id = validate::ulid_path(document_id, "Document")?;
    state
        .store
        .get_document(course_id, &document_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Document"))
}

/// `POST /api/courses/{courseId}/documents/{documentId}/complete` (owner)
pub async fn complete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath((course_id, document_id)): ApiPath<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let (course, _) = state.require_owner(&user, &course_id).await?;
    let document = load_document(&state, &course.id, &document_id).await?;
    if document.status != DocumentStatus::Uploading {
        return Ok(Json(json!({ "document": document })));
    }

    let key = source_key(&course.id, &document.id);
    match state.files.object_size(&key).await? {
        None => return Err(ApiError::Conflict("The file has not been uploaded".into())),
        Some(size) if size != document.size_bytes => {
            return Err(ApiError::Conflict(
                "The uploaded file size does not match sizeBytes".into(),
            ));
        }
        Some(_) => {}
    }

    let document = match state.store.mark_queued(&course.id, &document.id).await? {
        Some(queued) => {
            let message = IngestMessage::new(Action::Ingest, &course.id, &queued.id);
            if let Err(error) = state.queue.send(&message).await {
                // Put the document back, so that the client can call `complete` again.
                tracing::error!(%error, "could not send the ingest message");
                if let Err(revert) = state.store.revert_queued(&course.id, &queued.id).await {
                    tracing::error!(%revert, "could not put the document back to uploading");
                }
                return Err(ApiError::Unavailable);
            }
            queued
        }
        // Another request moved it first.
        None => load_document(&state, &course.id, &document.id).await?,
    };
    Ok(Json(json!({ "document": document })))
}

/// `DELETE /api/courses/{courseId}/documents/{documentId}` (owner)
/// Sets `deleting` and sends the `delete` message. The worker deletes the vectors, the BM25 entries,
/// the S3 objects, and the record. Deleting a document that is already `deleting` returns it unchanged.
pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath((course_id, document_id)): ApiPath<(String, String)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (course, _) = state.require_owner(&user, &course_id).await?;
    let document = load_document(&state, &course.id, &document_id).await?;
    if document.status == DocumentStatus::Deleting {
        return Ok((StatusCode::ACCEPTED, Json(json!({ "document": document }))));
    }
    let Some(previous) = state.store.mark_deleting(&course.id, &document.id).await? else {
        // Another request set it first.
        let document = load_document(&state, &course.id, &document.id).await?;
        return Ok((StatusCode::ACCEPTED, Json(json!({ "document": document }))));
    };
    let message = IngestMessage::new(Action::Delete, &course.id, &document.id);
    if let Err(error) = state.queue.send(&message).await {
        tracing::error!(%error, "could not send the delete message");
        if let Err(restore) = state
            .store
            .restore_status(&course.id, &document.id, previous.status)
            .await
        {
            tracing::error!(%restore, "could not restore the document status");
        }
        return Err(ApiError::Unavailable);
    }
    let document = load_document(&state, &course.id, &document.id).await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "document": document }))))
}
