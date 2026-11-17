//! S3 helpers: presigned uploads, object checks, and prefix deletes.

use std::time::Duration;

use aws_sdk_s3::Client;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_smithy_types::error::display::DisplayErrorContext;

/// How long a presigned upload URL works.
pub const PRESIGN_EXPIRY: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, thiserror::Error)]
#[error("s3 error: {0}")]
pub struct FilesError(pub String);

fn err(error: &dyn std::error::Error) -> FilesError {
    FilesError(DisplayErrorContext(error).to_string())
}

/// A presigned `PUT` for the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresignedUpload {
    pub url: String,
    pub expires_at: String,
}

/// The S3 key of a document's source PDF.
pub fn source_key(course_id: &str, document_id: &str) -> String {
    format!("courses/{course_id}/documents/{document_id}/source.pdf")
}

/// The prefix of one document's objects.
pub fn document_prefix(course_id: &str, document_id: &str) -> String {
    format!("courses/{course_id}/documents/{document_id}/")
}

/// The prefix of everything that belongs to a course.
pub fn course_prefix(course_id: &str) -> String {
    format!("courses/{course_id}/")
}

/// The docs bucket.
#[derive(Debug, Clone)]
pub struct Files {
    client: Client,
    /// Used only to presign. Set when presigned URLs need another base URL (local runs).
    presign_client: Option<Client>,
    /// Path of the presign endpoint, for example `/__aws`. It is not part of the signature.
    presign_prefix: String,
    bucket: String,
}

/// Splits `scheme://host[:port]/path` into the origin and the path without a trailing slash.
fn split_endpoint(url: &str) -> (&str, &str) {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    match url[after_scheme..].find('/') {
        Some(i) => (
            &url[..after_scheme + i],
            url[after_scheme + i..].trim_end_matches('/'),
        ),
        None => (url, ""),
    }
}

impl Files {
    /// `presign_endpoint` is the local-only base URL for presigned URLs. It uses path-style addressing.
    pub fn new(
        sdk: &aws_config::SdkConfig,
        bucket: String,
        custom_endpoint: bool,
        presign_endpoint: Option<&str>,
    ) -> Self {
        let mut conf = aws_sdk_s3::config::Builder::from(sdk);
        if custom_endpoint {
            conf = conf.force_path_style(true);
        }
        let client = Client::from_conf(conf.build());
        let presign_client = presign_endpoint.map(|url| {
            // The SDK drops the separator after an endpoint path, so presign against the origin only.
            // The path prefix goes back in `presign_put`.
            let (origin, _) = split_endpoint(url);
            let conf = aws_sdk_s3::config::Builder::from(sdk)
                .endpoint_url(origin)
                .force_path_style(true)
                .build();
            Client::from_conf(conf)
        });
        let presign_prefix = presign_endpoint
            .map(|url| split_endpoint(url).1.to_owned())
            .unwrap_or_default();
        Self {
            client,
            presign_client,
            presign_prefix,
            bucket,
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Presigns a `PUT` for `key`. It signs `content-type` and `content-length`.
    pub async fn presign_put(
        &self,
        key: &str,
        content_type: &str,
        size_bytes: u64,
    ) -> Result<PresignedUpload, FilesError> {
        let client = self.presign_client.as_ref().unwrap_or(&self.client);
        let length = i64::try_from(size_bytes).map_err(|e| err(&e))?;
        let config = PresigningConfig::expires_in(PRESIGN_EXPIRY).map_err(|e| err(&e))?;
        // Take the time before signing, so the reported expiry is never later than the real one.
        let expires_at = crate::domain::timestamp(chrono::Utc::now() + PRESIGN_EXPIRY);
        let request = client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .content_length(length)
            .presigned(config)
            .await
            .map_err(|e| err(&e))?;
        let mut url = request.uri().to_owned();
        if self.presign_client.is_some() && !self.presign_prefix.is_empty() {
            // Local only: the dev server proxy strips the prefix again, and the emulator runs without auth.
            let origin_len = split_endpoint(&url).0.len();
            url = format!(
                "{}{}{}",
                &url[..origin_len],
                self.presign_prefix,
                &url[origin_len..]
            );
        }
        Ok(PresignedUpload { url, expires_at })
    }

    /// Writes an object. The API never does this (the browser uploads with a presigned URL). The demo
    /// provisioning command does it for the demo PDFs, which are in the repository.
    pub async fn put_object(
        &self,
        key: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<(), FilesError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(bytes.into())
            .send()
            .await
            .map_err(|e| err(&e))?;
        Ok(())
    }

    /// The size of the object, or `None` when it does not exist.
    pub async fn object_size(&self, key: &str) -> Result<Option<u64>, FilesError> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(out) => Ok(Some(
                u64::try_from(out.content_length().unwrap_or(0)).unwrap_or(0),
            )),
            Err(e) if e.as_service_error().is_some_and(|s| s.is_not_found()) => Ok(None),
            Err(e) => Err(err(&e)),
        }
    }

    /// Deletes every object under `prefix`.
    pub async fn delete_prefix(&self, prefix: &str) -> Result<(), FilesError> {
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(token.take())
                .send()
                .await
                .map_err(|e| err(&e))?;
            let ids: Vec<ObjectIdentifier> = page
                .contents()
                .iter()
                .filter_map(|o| o.key())
                .filter_map(|k| ObjectIdentifier::builder().key(k).build().ok())
                .collect();
            if !ids.is_empty() {
                let delete = Delete::builder()
                    .set_objects(Some(ids))
                    .quiet(true)
                    .build()
                    .map_err(|e| err(&e))?;
                let out = self
                    .client
                    .delete_objects()
                    .bucket(&self.bucket)
                    .delete(delete)
                    .send()
                    .await
                    .map_err(|e| err(&e))?;
                if let Some(first) = out.errors().first() {
                    return Err(FilesError(format!(
                        "could not delete {}: {}",
                        first.key().unwrap_or("?"),
                        first.message().unwrap_or("?")
                    )));
                }
            }
            match page.next_continuation_token() {
                Some(next) if page.is_truncated().unwrap_or(false) => token = Some(next.to_owned()),
                _ => return Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use aws_credential_types::Credentials;
    use aws_sdk_s3::config::Region;

    use super::*;

    fn sdk() -> aws_config::SdkConfig {
        aws_config::SdkConfig::builder()
            .region(Region::new("ap-southeast-2"))
            .credentials_provider(
                aws_credential_types::provider::SharedCredentialsProvider::new(Credentials::new(
                    "AKIDEXAMPLE",
                    "secret",
                    None,
                    None,
                    "test",
                )),
            )
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .build()
    }

    #[test]
    fn keys_follow_the_layout() {
        assert_eq!(source_key("C", "D"), "courses/C/documents/D/source.pdf");
        assert_eq!(document_prefix("C", "D"), "courses/C/documents/D/");
        assert_eq!(course_prefix("C"), "courses/C/");
    }

    #[tokio::test]
    async fn presigned_put_signs_content_type_and_length() {
        let files = Files::new(&sdk(), "bucket".into(), false, None);
        let upload = files
            .presign_put("courses/C/documents/D/source.pdf", "application/pdf", 1234)
            .await
            .unwrap();
        assert!(upload.url.starts_with(
            "https://bucket.s3.ap-southeast-2.amazonaws.com/courses/C/documents/D/source.pdf?"
        ));
        let signed = upload
            .url
            .split("X-Amz-SignedHeaders=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let signed = signed.replace("%3B", ";");
        assert!(signed.contains("content-type"), "{signed}");
        assert!(signed.contains("content-length"), "{signed}");
        assert!(upload.url.contains("X-Amz-Expires=900"));
        assert!(upload.expires_at.ends_with('Z'));
    }

    #[tokio::test]
    async fn presign_endpoint_uses_its_base_url_with_path_style() {
        let files = Files::new(
            &sdk(),
            "bucket".into(),
            true,
            Some("http://localhost:5173/__aws"),
        );
        let upload = files
            .presign_put("k/source.pdf", "application/pdf", 5)
            .await
            .unwrap();
        assert!(
            upload
                .url
                .starts_with("http://localhost:5173/__aws/bucket/k/source.pdf?"),
            "{}",
            upload.url
        );
    }
}
