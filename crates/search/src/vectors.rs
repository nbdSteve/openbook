//! The S3 Vectors store: one index per course in the stage's vector bucket.

use std::collections::HashMap;

use aws_sdk_s3vectors::Client;
use aws_sdk_s3vectors::types::{
    DataType, DistanceMetric, MetadataConfiguration, PutInputVector, VectorData,
};
use aws_smithy_types::error::display::DisplayErrorContext;
use aws_smithy_types::{Document, Number};

use crate::embed::DIMENSIONS;

/// Most vectors in one put, and most keys in one delete.
pub const BATCH_SIZE: usize = 500;

#[derive(Debug, Clone, thiserror::Error)]
#[error("vector store error: {0}")]
pub struct VectorError(pub String);

fn err(error: &dyn std::error::Error) -> VectorError {
    VectorError(DisplayErrorContext(error).to_string())
}

/// The index name for a course: `c-<courseId in lower case>`.
pub fn index_name(course_id: &str) -> String {
    format!("c-{}", course_id.to_lowercase())
}

/// The vector key of a chunk: `<documentId>:<chunkIndex>`.
pub fn vector_key(document_id: &str, chunk_index: u32) -> String {
    format!("{document_id}:{chunk_index}")
}

/// One vector with its metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorRecord {
    pub key: String,
    pub embedding: Vec<f32>,
    pub document_id: String,
    /// The first page of the chunk (1-based).
    pub page: u32,
    pub heading: String,
    pub text: String,
}

/// A query result. A smaller `distance` means a closer match (cosine distance).
#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub key: String,
    pub distance: f32,
    pub document_id: String,
    pub page: u32,
    pub heading: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct VectorStore {
    client: Client,
    bucket: String,
}

impl VectorStore {
    pub fn new(sdk: &aws_config::SdkConfig, bucket: String) -> Self {
        Self {
            client: Client::new(sdk),
            bucket,
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Creates the index when it does not exist: float32, 1024 dimensions, cosine, with `text` and
    /// `heading` as non-filterable metadata keys.
    pub async fn ensure_index(&self, index: &str) -> Result<(), VectorError> {
        let existing = self
            .client
            .get_index()
            .vector_bucket_name(&self.bucket)
            .index_name(index)
            .send()
            .await;
        match existing {
            Ok(_) => return Ok(()),
            Err(e)
                if e.as_service_error()
                    .is_some_and(|s| s.is_not_found_exception()) => {}
            Err(e) => return Err(err(&e)),
        }
        let metadata = MetadataConfiguration::builder()
            .non_filterable_metadata_keys("text")
            .non_filterable_metadata_keys("heading")
            .build()
            .map_err(|e| err(&e))?;
        let created = self
            .client
            .create_index()
            .vector_bucket_name(&self.bucket)
            .index_name(index)
            .data_type(DataType::Float32)
            .dimension(DIMENSIONS as i32)
            .distance_metric(DistanceMetric::Cosine)
            .metadata_configuration(metadata)
            .send()
            .await;
        match created {
            Ok(_) => Ok(()),
            // Another call created it first.
            Err(e)
                if e.as_service_error()
                    .is_some_and(|s| s.is_conflict_exception()) =>
            {
                Ok(())
            }
            Err(e) => Err(err(&e)),
        }
    }

    /// Puts vectors in batches of 500. A put with an existing key replaces it.
    pub async fn put(&self, index: &str, records: &[VectorRecord]) -> Result<(), VectorError> {
        for batch in records.chunks(BATCH_SIZE) {
            let mut request = self
                .client
                .put_vectors()
                .vector_bucket_name(&self.bucket)
                .index_name(index);
            for record in batch {
                let metadata = Document::Object(HashMap::from([
                    (
                        "documentId".to_owned(),
                        Document::String(record.document_id.clone()),
                    ),
                    (
                        "page".to_owned(),
                        Document::Number(Number::PosInt(u64::from(record.page))),
                    ),
                    (
                        "heading".to_owned(),
                        Document::String(record.heading.clone()),
                    ),
                    ("text".to_owned(), Document::String(record.text.clone())),
                ]));
                let vector = PutInputVector::builder()
                    .key(&record.key)
                    .data(VectorData::Float32(record.embedding.clone()))
                    .metadata(metadata)
                    .build()
                    .map_err(|e| err(&e))?;
                request = request.vectors(vector);
            }
            request.send().await.map_err(|e| err(&e))?;
        }
        Ok(())
    }

    /// Deletes vectors by key. A missing index or key is not an error.
    pub async fn delete_keys(&self, index: &str, keys: &[String]) -> Result<(), VectorError> {
        for batch in keys.chunks(BATCH_SIZE) {
            let result = self
                .client
                .delete_vectors()
                .vector_bucket_name(&self.bucket)
                .index_name(index)
                .set_keys(Some(batch.to_vec()))
                .send()
                .await;
            match result {
                Ok(_) => {}
                Err(e)
                    if e.as_service_error()
                        .is_some_and(|s| s.is_not_found_exception()) =>
                {
                    return Ok(());
                }
                Err(e) => return Err(err(&e)),
            }
        }
        Ok(())
    }

    /// The nearest vectors to `query`, nearest first. With `document_id`, only that document's chunks.
    pub async fn query(
        &self,
        index: &str,
        query: &[f32],
        top_k: u32,
        document_id: Option<&str>,
    ) -> Result<Vec<VectorHit>, VectorError> {
        let mut request = self
            .client
            .query_vectors()
            .vector_bucket_name(&self.bucket)
            .index_name(index)
            .top_k(i32::try_from(top_k).unwrap_or(i32::MAX))
            .query_vector(VectorData::Float32(query.to_vec()))
            .return_metadata(true)
            .return_distance(true);
        if let Some(id) = document_id {
            request = request.filter(Document::Object(HashMap::from([(
                "documentId".to_owned(),
                Document::String(id.to_owned()),
            )])));
        }
        let out = match request.send().await {
            Ok(out) => out,
            // No index means no vectors yet.
            Err(e)
                if e.as_service_error()
                    .is_some_and(|s| s.is_not_found_exception()) =>
            {
                return Ok(Vec::new());
            }
            Err(e) => return Err(err(&e)),
        };
        let mut hits: Vec<VectorHit> = out.vectors().iter().map(hit_from).collect();
        hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        Ok(hits)
    }

    /// Deletes a whole index. A missing index is not an error.
    pub async fn delete_index(&self, index: &str) -> Result<(), VectorError> {
        let result = self
            .client
            .delete_index()
            .vector_bucket_name(&self.bucket)
            .index_name(index)
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e)
                if e.as_service_error()
                    .is_some_and(|s| s.is_not_found_exception()) =>
            {
                Ok(())
            }
            Err(e) => Err(err(&e)),
        }
    }
}

fn hit_from(vector: &aws_sdk_s3vectors::types::QueryOutputVector) -> VectorHit {
    let metadata = vector.metadata();
    let string = |name: &str| -> String {
        match metadata {
            Some(Document::Object(map)) => match map.get(name) {
                Some(Document::String(s)) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    };
    let page = match metadata {
        Some(Document::Object(map)) => match map.get("page") {
            Some(Document::Number(Number::PosInt(n))) => u32::try_from(*n).unwrap_or(0),
            Some(Document::Number(Number::Float(f))) => *f as u32,
            _ => 0,
        },
        _ => 0,
    };
    VectorHit {
        key: vector.key().to_owned(),
        distance: vector.distance().unwrap_or(f32::MAX),
        document_id: string("documentId"),
        page,
        heading: string("heading"),
        text: string("text"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_keys_follow_the_contract() {
        assert_eq!(
            index_name("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            "c-01arz3ndektsv4rrffq69g5fav"
        );
        assert_eq!(vector_key("DOC", 7), "DOC:7");
    }
}
