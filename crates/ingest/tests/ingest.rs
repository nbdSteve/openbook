//! The ingest worker against AWS Emulator: SQS, S3, S3 Vectors, DynamoDB, and the real API router.
//!
//! Run them with `make test-integration`. They need `OPENBOOK_PDFIUM_PATH` (see `scripts/fetch-pdfium.sh`).
//! The models are fakes: the fake embedder hashes words, and the fake transcriber returns the
//! text layer under a `# Page <n>` heading.

#[path = "../../api/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use axum::http::StatusCode;
use openbook_core::config::{Config, Service};
use openbook_core::domain::DocumentStatus;
use openbook_core::files::Files;
use openbook_core::queue::IngestMessage;
use openbook_core::store::Store;
use openbook_ingest::pdf;
use openbook_ingest::pipeline::{
    Deps, MSG_GENERIC, MSG_NO_TEXT, MSG_PASSWORD, MSG_UNREADABLE, Outcome, Retry, WARNING_CAP,
    handle_message,
};
use openbook_ingest::transcribe::FakeTranscriber;
use openbook_search::bm25::{Bm25Cache, Bm25Store};
use openbook_search::embed::{EmbedError, Embedder, FakeEmbedder};
use openbook_search::vectors::{VectorStore, index_name};
use pdfium_render::prelude::*;
use serde_json::{Value, json};
use support::{TestApp, TestUser, start};

const BUDGET: Duration = Duration::from_secs(120);

fn text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("no string {key} in {value}"))
        .to_owned()
}

fn pdfium_dir() -> PathBuf {
    match std::env::var("OPENBOOK_PDFIUM_PATH") {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => panic!(
            "OPENBOOK_PDFIUM_PATH is not set. Run scripts/fetch-pdfium.sh and set it to the path that it prints."
        ),
    }
}

/// Makes a PDF with one text object per line, one page per entry.
fn make_pdf(pages: &[Vec<String>]) -> Vec<u8> {
    let pdfium = pdf::pdfium(&pdfium_dir()).expect("load pdfium");
    let mut document = pdfium.create_new_pdf().expect("new pdf");
    let font = document.fonts_mut().helvetica();
    for lines in pages {
        let mut page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("page");
        for (i, line) in lines.iter().enumerate() {
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(40.0),
                    PdfPoints::new(780.0 - 16.0 * i as f32),
                    line,
                    font,
                    PdfPoints::new(11.0),
                )
                .expect("text");
        }
    }
    document.save_to_bytes().expect("save pdf")
}

fn prose(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}

/// A page whose text layer looks torn apart: many one-character tokens.
fn torn_page() -> Vec<String> {
    vec![
        "x y z f g h k m n p q r s t u v w".into(),
        "a b c d e f g h j k l m n o p q r".into(),
        "x y z f g h k m n p q r s t u v w".into(),
    ]
}

fn geometry_page() -> Vec<String> {
    prose(&[
        "The Pythagorean theorem relates the three sides of a right triangle.",
        "It says that the square of the hypotenuse equals the sum of the squares of the other two sides.",
        "Builders use the theorem to check that a corner is square.",
        "The converse of the theorem also holds for every triangle.",
    ])
}

fn biology_page() -> Vec<String> {
    prose(&[
        "Photosynthesis converts light energy into chemical energy in the chloroplasts of plant cells.",
        "Chlorophyll absorbs red and blue light and reflects green light.",
        "The process makes glucose and releases oxygen as a by-product.",
        "Plants use the glucose for growth and for storing energy.",
    ])
}

fn geometry_pdf() -> Vec<u8> {
    make_pdf(&[
        geometry_page(),
        prose(&[
            "Similar triangles have equal angles and sides in the same ratio.",
            "This lets us find unknown lengths from known lengths with simple proportions.",
            "Surveyors measure tall trees and buildings with this idea every day.",
            "A shadow and the object that makes it form two similar triangles.",
        ]),
        torn_page(),
    ])
}

struct Env {
    app: TestApp,
    owner: TestUser,
    course_id: String,
    deps: Deps,
}

async fn deps_for(app: &TestApp, extra: &[(&str, &str)]) -> Deps {
    let mut vars = app.vars().clone();
    vars.insert(
        "OPENBOOK_PDFIUM_PATH".into(),
        pdfium_dir().display().to_string(),
    );
    for (k, v) in extra {
        vars.insert((*k).into(), (*v).into());
    }
    let config = Config::from_vars_for(&vars, Service::Ingest).expect("ingest config");
    let store = Store::new(&app.sdk, config.table_name.clone());
    let files = Files::new(&app.sdk, config.docs_bucket.clone(), true, None);
    let vectors = VectorStore::new(&app.sdk, config.vector_bucket.clone());
    let bm25 = Bm25Store::new(app.s3.clone(), config.docs_bucket.clone(), store.clone());
    Deps {
        config: Arc::new(config),
        store,
        s3: app.s3.clone(),
        files,
        vectors,
        bm25,
        embedder: Arc::new(FakeEmbedder),
        transcriber: Arc::new(FakeTranscriber),
        work_root: std::env::temp_dir().join(format!(
            "openbook-ingest-test-{}",
            openbook_core::ids::new_id()
        )),
    }
}

async fn env_with(extra: &[(&str, &str)]) -> Env {
    let app = start().await;
    let owner = app.user("teacher").await;
    let course = app.course(&owner, "Geometry and Biology").await;
    let deps = deps_for(&app, extra).await;
    Env {
        course_id: text(&course, "id"),
        app,
        owner,
        deps,
    }
}

async fn env() -> Env {
    env_with(&[]).await
}

impl Env {
    /// Uploads a file, calls `complete`, and returns the document ID and the queued message.
    async fn upload(&self, filename: &str, bytes: Vec<u8>) -> (String, IngestMessage) {
        let (app, id) = (&self.app, &self.course_id);
        let reply = app
            .post(
                &format!("/api/courses/{id}/documents"),
                &self.owner,
                json!({"filename": filename, "sizeBytes": bytes.len(), "contentType": "application/pdf"}),
            )
            .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
        let doc_id = text(&reply.json()["document"], "id");
        let put = app
            .http
            .put(text(&reply.json()["upload"], "url"))
            .header("content-type", "application/pdf")
            .body(bytes)
            .send()
            .await
            .expect("PUT the file");
        assert!(put.status().is_success());
        let done = app
            .post_empty(
                &format!("/api/courses/{id}/documents/{doc_id}/complete"),
                &self.owner,
            )
            .await;
        assert_eq!(done.status, StatusCode::OK, "{}", done.text);
        let mut messages = app.queued_messages().await;
        assert_eq!(messages.len(), 1, "one ingest message: {messages:?}");
        let message: IngestMessage =
            serde_json::from_value(messages.remove(0)).expect("a queue message");
        assert_eq!(message.document_id, doc_id);
        (doc_id, message)
    }

    async fn run(&self, message: &IngestMessage, receive_count: u32) -> Result<Outcome, Retry> {
        handle_message(&self.deps, message, receive_count, BUDGET).await
    }

    async fn document(&self, doc_id: &str) -> Option<Value> {
        let docs = self
            .app
            .get(
                &format!("/api/courses/{}/documents", self.course_id),
                &self.owner,
            )
            .await
            .json();
        docs["documents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == doc_id)
            .cloned()
    }

    async fn ingest(&self, filename: &str, bytes: Vec<u8>) -> (String, Outcome) {
        let (doc_id, message) = self.upload(filename, bytes).await;
        let outcome = self.run(&message, 1).await.expect("the message is handled");
        (doc_id, outcome)
    }

    async fn search_vectors(
        &self,
        query: &str,
        document: Option<&str>,
    ) -> Vec<openbook_search::vectors::VectorHit> {
        self.deps
            .vectors
            .query(
                &index_name(&self.course_id),
                &FakeEmbedder::vector(query),
                10,
                document,
            )
            .await
            .expect("query vectors")
    }

    async fn bm25_hits(&self, query: &str) -> Vec<openbook_search::bm25::Bm25Hit> {
        let course = self
            .deps
            .store
            .get_course(&self.course_id)
            .await
            .unwrap()
            .unwrap();
        let Some(key) = course.bm25_key else {
            return Vec::new();
        };
        let root = std::env::temp_dir().join(format!(
            "openbook-bm25-read-{}",
            openbook_core::ids::new_id()
        ));
        let cache = Bm25Cache::new(
            self.app.s3.clone(),
            self.deps.files.bucket().to_owned(),
            root,
        );
        cache
            .open(&self.course_id, &key)
            .await
            .expect("open the index")
            .search(query, 10)
            .unwrap()
    }

    async fn bm25_key(&self) -> Option<String> {
        self.deps
            .store
            .get_course(&self.course_id)
            .await
            .unwrap()
            .unwrap()
            .bm25_key
    }

    async fn bm25_blobs(&self) -> Vec<String> {
        self.app
            .objects_under(&format!("courses/{}/bm25/", self.course_id))
            .await
    }

    async fn page_keys(&self, doc_id: &str) -> Vec<String> {
        self.app
            .objects_under(&format!(
                "courses/{}/documents/{doc_id}/pages/",
                self.course_id
            ))
            .await
    }

    async fn course_attribute(&self, name: &str) -> Option<i64> {
        let item = self
            .app
            .dynamodb
            .get_item()
            .table_name(&self.app.table)
            .key(
                "pk",
                AttributeValue::S(format!("COURSE#{}", self.course_id)),
            )
            .key("sk", AttributeValue::S("META".into()))
            .consistent_read(true)
            .send()
            .await
            .unwrap()
            .item?;
        match item.get(name) {
            Some(AttributeValue::N(n)) => n.parse().ok(),
            _ => None,
        }
    }
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_pdf_becomes_ready_with_pages_vectors_and_a_bm25_index() {
    let e = env().await;
    let student = e.app.user("student").await;
    e.app.post("/api/join", &student, json!({"inviteCode": e.app.get(&format!("/api/courses/{}", e.course_id), &e.owner).await.json()["course"]["inviteCode"]})).await;

    let (geometry, outcome) = e.ingest("Geometry notes.pdf", geometry_pdf()).await;
    let Outcome::Ready { pages, chunks } = outcome else {
        panic!("not ready: {outcome:?}")
    };
    assert_eq!(pages, 3);
    assert!(chunks >= 2, "chunks: {chunks}");
    let (biology, outcome) = e.ingest("Biology.pdf", make_pdf(&[biology_page()])).await;
    assert!(
        matches!(outcome, Outcome::Ready { pages: 1, .. }),
        "{outcome:?}"
    );

    // The record.
    let doc = e.document(&geometry).await.expect("the document");
    assert_eq!(doc["status"], "ready");
    assert_eq!(doc["pageCount"], 3);
    assert_eq!(doc["chunkCount"], chunks);
    assert_eq!(
        doc["transcribedPages"], 1,
        "the torn page goes through the transcriber"
    );
    assert!(
        doc.get("warnings").is_none() && doc.get("error").is_none(),
        "{doc}"
    );
    // Students see ready documents.
    let listed = e
        .app
        .get(&format!("/api/courses/{}/documents", e.course_id), &student)
        .await
        .json();
    assert_eq!(listed["documents"].as_array().unwrap().len(), 2);

    // The page files.
    let keys = e.page_keys(&geometry).await;
    assert_eq!(keys.len(), 3, "{keys:?}");
    assert!(keys[0].ends_with("/pages/0001.md") && keys[2].ends_with("/pages/0003.md"));
    let page3 = e
        .app
        .s3
        .get_object()
        .bucket(&e.app.bucket)
        .key(&keys[2])
        .send()
        .await
        .unwrap();
    let page3_text = String::from_utf8(page3.body.collect().await.unwrap().to_vec()).unwrap();
    assert!(page3_text.starts_with("# Page 3\n\n"), "{page3_text}");
    let page1 = e
        .app
        .s3
        .get_object()
        .bucket(&e.app.bucket)
        .key(&keys[0])
        .send()
        .await
        .unwrap();
    assert!(
        String::from_utf8(page1.body.collect().await.unwrap().to_vec())
            .unwrap()
            .contains("Pythagorean")
    );

    // Vectors: the right document comes first, and the filter works.
    let hits = e
        .search_vectors("pythagorean theorem right triangle hypotenuse", None)
        .await;
    assert!(!hits.is_empty());
    assert_eq!(hits[0].document_id, geometry);
    assert_eq!(hits[0].page, 1);
    assert!(hits[0].text.contains("Pythagorean"));
    assert!(
        hits.windows(2).all(|w| w[0].distance <= w[1].distance),
        "nearest first"
    );
    let hits = e
        .search_vectors("photosynthesis chlorophyll glucose", None)
        .await;
    assert_eq!(hits[0].document_id, biology);
    let only = e
        .search_vectors("pythagorean theorem", Some(&biology))
        .await;
    assert!(
        only.iter().all(|h| h.document_id == biology),
        "the filter keeps only one document"
    );
    // Keys follow `<documentId>:<chunkIndex>`.
    assert!(hits.iter().all(|h| h.key.contains(':')));

    // BM25: one version per document, readable through the cache.
    assert_eq!(e.course_attribute("bm25Version").await, Some(2));
    assert_eq!(
        e.bm25_hits("pythagorean theorem").await[0].document_id,
        geometry
    );
    assert_eq!(e.bm25_hits("photosynthesis").await[0].document_id, biology);
    let blobs = e
        .app
        .objects_under(&format!("courses/{}/bm25/", e.course_id))
        .await;
    let key = e.bm25_key().await.expect("the course item has bm25Key");
    assert!(
        key.starts_with(&format!("courses/{}/bm25/v2-", e.course_id)) && key.ends_with(".tar.zst"),
        "{key}"
    );
    assert_eq!(
        key.len(),
        format!("courses/{}/bm25/v2-.tar.zst", e.course_id).len() + 26
    );
    assert!(blobs.contains(&key), "{blobs:?}");
    assert_eq!(
        blobs.len(),
        2,
        "the new blob and the previous one: {blobs:?}"
    );

    // The daily transcription counter of the owner moved by one page.
    let usage = e
        .app
        .dynamodb
        .get_item()
        .table_name(&e.app.table)
        .key("pk", AttributeValue::S(format!("USER#{}", e.owner.id)))
        .key(
            "sk",
            AttributeValue::S(format!(
                "USAGE#D#{}",
                openbook_core::quota::usage_date(chrono::Utc::now())
            )),
        )
        .consistent_read(true)
        .send()
        .await
        .unwrap()
        .item
        .expect("usage item");
    assert_eq!(
        usage.get("transcribedPages"),
        Some(&AttributeValue::N("1".into()))
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn duplicate_and_stale_messages_are_ignored() {
    let e = env().await;
    let (doc_id, message) = e.upload("Geometry notes.pdf", geometry_pdf()).await;
    let first = e.run(&message, 1).await.unwrap();
    assert!(matches!(first, Outcome::Ready { .. }));
    let before = e.search_vectors("pythagorean", None).await.len();
    let version = e.course_attribute("bm25Version").await;

    // The same message again (SQS delivers at least once).
    let again = e.run(&message, 1).await.unwrap();
    assert_eq!(again, Outcome::Ignored("the document is not queued"));
    assert_eq!(e.document(&doc_id).await.unwrap()["status"], "ready");
    assert_eq!(e.search_vectors("pythagorean", None).await.len(), before);
    assert_eq!(e.course_attribute("bm25Version").await, version);

    // Messages for things that are gone are acknowledged.
    let missing = IngestMessage::new(
        openbook_core::queue::Action::Ingest,
        &e.course_id,
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
    );
    assert!(matches!(
        e.run(&missing, 1).await.unwrap(),
        Outcome::Ignored(_)
    ));
    let missing_delete = IngestMessage::new(
        openbook_core::queue::Action::Delete,
        &e.course_id,
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
    );
    assert!(matches!(
        e.run(&missing_delete, 1).await.unwrap(),
        Outcome::Ignored(_)
    ));
    let no_course = IngestMessage::new(
        openbook_core::queue::Action::Ingest,
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        &doc_id,
    );
    assert!(matches!(
        e.run(&no_course, 1).await.unwrap(),
        Outcome::Ignored(_)
    ));
    // A delete message for a document that is not `deleting` does nothing.
    let delete = IngestMessage::new(openbook_core::queue::Action::Delete, &e.course_id, &doc_id);
    assert!(matches!(
        e.run(&delete, 1).await.unwrap(),
        Outcome::Ignored(_)
    ));
    assert_eq!(e.document(&doc_id).await.unwrap()["status"], "ready");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_transcription_cap_falls_back_to_the_text_layer_with_a_warning() {
    let e = env_with(&[("OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_PER_USER", "1")]).await;
    let pdf = make_pdf(&[geometry_page(), torn_page(), torn_page(), torn_page()]);
    let (doc_id, outcome) = e.ingest("Torn.pdf", pdf).await;
    assert!(
        matches!(outcome, Outcome::Ready { pages: 4, .. }),
        "{outcome:?}"
    );
    let doc = e.document(&doc_id).await.unwrap();
    assert_eq!(doc["status"], "ready");
    assert_eq!(doc["transcribedPages"], 1, "one page fits under the cap");
    let warnings = doc["warnings"].as_array().expect("a warning for the owner");
    assert_eq!(warnings.len(), 1);
    let warning = warnings[0].as_str().unwrap();
    assert!(warning.starts_with(WARNING_CAP), "{warning}");
    assert!(
        warning.contains("(2 pages)"),
        "two pages used the text layer: {warning}"
    );
    // The capped pages are still searchable through their text layer.
    assert!(!e.page_keys(&doc_id).await.is_empty());
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn broken_protected_blank_and_oversized_pdfs_fail_with_an_owner_safe_message() {
    let e = env_with(&[("OPENBOOK_LIMIT_PAGES_PER_DOCUMENT", "2")]).await;

    let (broken, outcome) = e
        .ingest("Broken.pdf", b"this is not a pdf at all".to_vec())
        .await;
    assert_eq!(outcome, Outcome::Failed(MSG_UNREADABLE.into()));
    let doc = e.document(&broken).await.unwrap();
    assert_eq!(doc["status"], "failed");
    assert_eq!(doc["error"], MSG_UNREADABLE);

    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/password-protected.pdf");
    let (locked, outcome) = e
        .ingest("Locked.pdf", std::fs::read(fixture).unwrap())
        .await;
    assert_eq!(outcome, Outcome::Failed(MSG_PASSWORD.into()));
    assert_eq!(e.document(&locked).await.unwrap()["error"], MSG_PASSWORD);

    let (blank, outcome) = e.ingest("Blank.pdf", make_pdf(&[vec![]])).await;
    assert_eq!(outcome, Outcome::Failed(MSG_NO_TEXT.into()));
    assert_eq!(e.document(&blank).await.unwrap()["status"], "failed");

    let (long, outcome) = e.ingest("Long.pdf", geometry_pdf()).await;
    let Outcome::Failed(message) = outcome else {
        panic!("not failed: {outcome:?}")
    };
    assert!(
        message.contains("3 pages") && message.contains("limit is 2"),
        "{message}"
    );
    assert_eq!(e.document(&long).await.unwrap()["status"], "failed");

    // Nothing of a failed document is searchable.
    assert!(e.search_vectors("pythagorean", None).await.is_empty());
    assert_eq!(e.course_attribute("bm25Version").await, None);
}

struct FlakyEmbedder {
    failures: AtomicUsize,
}

#[async_trait]
impl Embedder for FlakyEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        if self.failures.fetch_add(1, Ordering::SeqCst) < 1000 {
            return Err(EmbedError::Transient(format!(
                "throttled ({} chars)",
                text.len()
            )));
        }
        Ok(FakeEmbedder::vector(text))
    }
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn transient_errors_retry_and_the_last_attempt_fails_the_document() {
    let mut e = env().await;
    e.deps.embedder = Arc::new(FlakyEmbedder {
        failures: AtomicUsize::new(0),
    });
    let (doc_id, message) = e.upload("Geometry notes.pdf", geometry_pdf()).await;

    // Receives 1 and 2 ask SQS to deliver the message again. The document is `queued` again.
    for receive in 1..=2 {
        assert!(e.run(&message, receive).await.is_err(), "receive {receive}");
        assert_eq!(e.document(&doc_id).await.unwrap()["status"], "queued");
    }
    // The last attempt fails the document, so nothing waits for the dead-letter queue.
    let last = e.run(&message, 3).await.unwrap();
    assert_eq!(last, Outcome::Failed(MSG_GENERIC.into()));
    let doc = e.document(&doc_id).await.unwrap();
    assert_eq!(
        (doc["status"].as_str(), doc["error"].as_str()),
        (Some("failed"), Some(MSG_GENERIC))
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_retry_after_a_crash_continues_a_processing_document_and_resumes_its_pages() {
    let e = env().await;
    let (doc_id, message) = e.upload("Geometry notes.pdf", geometry_pdf()).await;

    // The first attempt wrote page 1 and then crashed: the document stays `processing`.
    assert!(
        e.deps
            .store
            .begin_processing(&e.course_id, &doc_id, false)
            .await
            .unwrap()
    );
    let key = format!("courses/{}/documents/{doc_id}/pages/0001.md", e.course_id);
    e.app
        .s3
        .put_object()
        .bucket(&e.app.bucket)
        .key(&key)
        .body(
            "Resumed page one about the Pythagorean theorem from an earlier attempt."
                .as_bytes()
                .to_vec()
                .into(),
        )
        .send()
        .await
        .unwrap();

    // A first delivery is not a retry, so a `processing` document is left alone.
    assert!(matches!(
        e.run(&message, 1).await.unwrap(),
        Outcome::Ignored(_)
    ));
    // The second delivery carries on.
    let outcome = e.run(&message, 2).await.unwrap();
    assert!(
        matches!(outcome, Outcome::Ready { pages: 3, .. }),
        "{outcome:?}"
    );
    let page1 = e
        .app
        .s3
        .get_object()
        .bucket(&e.app.bucket)
        .key(&key)
        .send()
        .await
        .unwrap();
    let body = String::from_utf8(page1.body.collect().await.unwrap().to_vec()).unwrap();
    assert!(
        body.starts_with("Resumed page one"),
        "page 1 was not written again: {body}"
    );
    let hits = e.bm25_hits("resumed earlier attempt").await;
    assert_eq!(hits[0].page, 1);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn delete_removes_vectors_bm25_entries_objects_and_the_record() {
    let e = env().await;
    let (geometry, _) = e.ingest("Geometry notes.pdf", geometry_pdf()).await;
    let (biology, _) = e.ingest("Biology.pdf", make_pdf(&[biology_page()])).await;
    assert_eq!(e.course_attribute("docCount").await, Some(2));

    let reply = e
        .app
        .delete(
            &format!("/api/courses/{}/documents/{geometry}", e.course_id),
            &e.owner,
        )
        .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text);
    assert_eq!(reply.json()["document"]["status"], "deleting");
    let mut messages = e.app.queued_messages().await;
    assert_eq!(messages.len(), 1);
    let message: IngestMessage = serde_json::from_value(messages.remove(0)).unwrap();
    assert_eq!(message.action, openbook_core::queue::Action::Delete);

    assert_eq!(e.run(&message, 1).await.unwrap(), Outcome::Deleted);

    assert!(e.document(&geometry).await.is_none(), "the record is gone");
    assert!(
        e.app
            .objects_under(&format!("courses/{}/documents/{geometry}/", e.course_id))
            .await
            .is_empty()
    );
    let hits = e
        .search_vectors("pythagorean theorem right triangle", None)
        .await;
    assert!(
        hits.iter().all(|h| h.document_id == biology),
        "no vector of the deleted document is left"
    );
    assert!(!hits.is_empty(), "the other document stays");
    assert!(
        e.bm25_hits("pythagorean").await.is_empty(),
        "the BM25 entries are gone"
    );
    assert_eq!(e.bm25_hits("photosynthesis").await[0].document_id, biology);
    assert_eq!(
        e.course_attribute("docCount").await,
        Some(1),
        "the slot is back"
    );
    assert_eq!(e.course_attribute("bm25Version").await, Some(3));
    // The new version and the previous one stay, older ones go.
    let blobs = e.bm25_blobs().await;
    let key = e.bm25_key().await.unwrap();
    assert!(key.contains("/v3-"), "{key}");
    assert!(blobs.contains(&key));
    assert!(blobs.iter().any(|k| k.contains("/v2-")), "{blobs:?}");
    assert!(!blobs.iter().any(|k| k.contains("/v1-")), "{blobs:?}");
    assert_eq!(blobs.len(), 2);

    // The same message again is harmless.
    assert!(matches!(
        e.run(&message, 2).await.unwrap(),
        Outcome::Ignored(_)
    ));
}

struct DeletingEmbedder {
    store: Store,
    course_id: String,
    document_id: String,
    done: AtomicUsize,
}

#[async_trait]
impl Embedder for DeletingEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        // The owner presses delete while the worker embeds.
        if self.done.fetch_add(1, Ordering::SeqCst) == 0 {
            self.store
                .mark_deleting(&self.course_id, &self.document_id)
                .await
                .unwrap();
        }
        Ok(FakeEmbedder::vector(text))
    }
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_delete_during_processing_leaves_no_vectors_and_the_delete_message_finishes_it() {
    let mut e = env().await;
    let (doc_id, message) = e.upload("Geometry notes.pdf", geometry_pdf()).await;
    e.deps.embedder = Arc::new(DeletingEmbedder {
        store: e.deps.store.clone(),
        course_id: e.course_id.clone(),
        document_id: doc_id.clone(),
        done: AtomicUsize::new(0),
    });
    let outcome = e.run(&message, 1).await.unwrap();
    assert!(matches!(outcome, Outcome::Ignored(_)), "{outcome:?}");
    assert_eq!(e.document(&doc_id).await.unwrap()["status"], "deleting");
    assert!(
        e.search_vectors("pythagorean", None).await.is_empty(),
        "the worker removed its vectors"
    );

    let delete = IngestMessage::new(openbook_core::queue::Action::Delete, &e.course_id, &doc_id);
    assert_eq!(e.run(&delete, 1).await.unwrap(), Outcome::Deleted);
    assert!(e.document(&doc_id).await.is_none());
    assert!(
        e.app
            .objects_under(&format!("courses/{}/documents/{doc_id}/", e.course_id))
            .await
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn deleting_a_course_goes_through_the_worker_and_removes_everything() {
    let e = env().await;
    let student = e.app.user("student").await;
    let invite = e
        .app
        .get(&format!("/api/courses/{}", e.course_id), &e.owner)
        .await
        .json()["course"]["inviteCode"]
        .clone();
    e.app
        .post("/api/join", &student, json!({"inviteCode": invite}))
        .await;
    let (doc_id, outcome) = e.ingest("Geometry notes.pdf", geometry_pdf()).await;
    assert!(matches!(outcome, Outcome::Ready { .. }));
    // A second document waits in the queue when the owner deletes the course.
    let (waiting, waiting_message) = e.upload("Biology.pdf", make_pdf(&[biology_page()])).await;
    let index = index_name(&e.course_id);
    assert!(!e.search_vectors("pythagorean", None).await.is_empty());
    let client = aws_sdk_s3vectors::Client::new(&e.app.sdk);
    assert!(
        client
            .get_index()
            .vector_bucket_name(&e.app.vector_bucket)
            .index_name(&index)
            .send()
            .await
            .is_ok()
    );
    assert_eq!(e.course_attribute("courses").await, None);

    let reply = e
        .app
        .delete(&format!("/api/courses/{}", e.course_id), &e.owner)
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text);
    let mut messages = e.app.queued_messages().await;
    assert_eq!(messages.len(), 1);
    let message: IngestMessage = serde_json::from_value(messages.remove(0)).unwrap();
    assert_eq!(message.action, openbook_core::queue::Action::DeleteCourse);
    assert_eq!(message.course_id, e.course_id);

    // Members get 404 at once, and nothing new can start.
    for user in [&e.owner, &student] {
        assert_eq!(
            e.app
                .get(&format!("/api/courses/{}", e.course_id), user)
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    // Ingest and delete messages for a deleting course are dropped. The waiting document is not processed.
    let dropped = e.run(&waiting_message, 1).await.unwrap();
    assert_eq!(dropped, Outcome::Ignored("the course is being deleted"));
    let delete_doc =
        IngestMessage::new(openbook_core::queue::Action::Delete, &e.course_id, &doc_id);
    assert_eq!(
        e.run(&delete_doc, 1).await.unwrap(),
        Outcome::Ignored("the course is being deleted")
    );
    assert!(
        !e.search_vectors("pythagorean", None).await.is_empty(),
        "nothing was deleted yet"
    );

    // The worker removes everything.
    assert_eq!(e.run(&message, 1).await.unwrap(), Outcome::CourseDeleted);
    let gone = client
        .get_index()
        .vector_bucket_name(&e.app.vector_bucket)
        .index_name(&index)
        .send()
        .await;
    assert!(gone.is_err(), "the index is deleted");
    assert!(e.search_vectors("pythagorean", None).await.is_empty());
    assert!(
        e.app
            .objects_under(&format!("courses/{}/", e.course_id))
            .await
            .is_empty(),
        "documents, pages, and BM25 blobs are gone"
    );
    let left = e
        .app
        .get_item_sk_prefixes(&format!("COURSE#{}", e.course_id))
        .await;
    assert!(left.iter().all(|sk| sk.starts_with("Q#")), "{left:?}");
    assert_eq!(e.course_attribute("docCount").await, None);
    assert!(
        e.deps
            .store
            .get_course(&e.course_id)
            .await
            .unwrap()
            .is_none()
    );
    let _ = waiting;

    // The same message again, and messages for the vanished course, are acknowledged.
    assert!(matches!(
        e.run(&message, 2).await.unwrap(),
        Outcome::Ignored(_)
    ));
    assert!(matches!(
        e.run(&waiting_message, 2).await.unwrap(),
        Outcome::Ignored(_)
    ));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_status_flow_is_visible_to_the_owner() {
    let e = env().await;
    let (doc_id, message) = e.upload("Geometry notes.pdf", geometry_pdf()).await;
    assert_eq!(e.document(&doc_id).await.unwrap()["status"], "queued");
    e.run(&message, 1).await.unwrap();
    let doc = e.document(&doc_id).await.unwrap();
    assert_eq!(doc["status"], DocumentStatus::Ready.as_json());
}

trait AsJson {
    fn as_json(&self) -> String;
}

impl AsJson for DocumentStatus {
    fn as_json(&self) -> String {
        serde_json::to_value(self)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }
}

struct RecordingTranscriber {
    images: std::sync::Mutex<Vec<Vec<u8>>>,
}

#[async_trait]
impl openbook_ingest::transcribe::Transcriber for RecordingTranscriber {
    async fn transcribe(
        &self,
        page_number: u32,
        image: &[u8],
        text_layer: &str,
    ) -> Result<String, openbook_ingest::transcribe::TranscribeError> {
        self.images.lock().unwrap().push(image.to_vec());
        Ok(format!(
            "# Scan {page_number}\n\nA scanned page about the Pythagorean theorem. {text_layer}"
        ))
    }
}

/// A page that is one big noisy image: a scan with grain.
fn make_scan_pdf() -> Vec<u8> {
    let pdfium = pdf::pdfium(&pdfium_dir()).expect("load pdfium");
    let mut document = pdfium.create_new_pdf().expect("new pdf");
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut noise = image::RgbImage::new(1600, 2200);
    for pixel in noise.pixels_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *pixel = image::Rgb([(state >> 8) as u8, (state >> 24) as u8, (state >> 40) as u8]);
    }
    let mut page = document
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .expect("page");
    let object = PdfPageImageObject::new_with_size(
        &document,
        &image::DynamicImage::ImageRgb8(noise),
        PdfPoints::new(595.0),
        PdfPoints::new(842.0),
    )
    .expect("image object");
    page.objects_mut()
        .add_image_object(object)
        .expect("add image");
    document.save_to_bytes().expect("save pdf")
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_noisy_scanned_page_goes_to_the_model_as_a_small_grayscale_jpeg() {
    let mut e = env().await;
    let recorder = Arc::new(RecordingTranscriber {
        images: std::sync::Mutex::new(Vec::new()),
    });
    e.deps.transcriber = recorder.clone();
    let (doc_id, outcome) = e.ingest("Scan.pdf", make_scan_pdf()).await;
    assert!(
        matches!(outcome, Outcome::Ready { pages: 1, .. }),
        "{outcome:?}"
    );
    let images = recorder.images.lock().unwrap().clone();
    assert_eq!(images.len(), 1, "the scan was transcribed");
    let bytes = &images[0];
    assert!(
        bytes.len() <= 3_500_000,
        "{} bytes is over the limit",
        bytes.len()
    );
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "a JPEG");
    let decoded = image::load_from_memory(bytes).unwrap();
    assert!(
        decoded.width().max(decoded.height()) <= 1568,
        "{}x{}",
        decoded.width(),
        decoded.height()
    );
    assert_eq!(decoded.color().channel_count(), 1, "grayscale");
    let doc = e.document(&doc_id).await.unwrap();
    assert_eq!(doc["transcribedPages"], 1);
    assert!(doc.get("warnings").is_none());
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn after_three_receives_the_document_fails_without_opening_the_pdf() {
    let mut e = env().await;
    // pdfium cannot load here. A worker that opened the PDF would ask for a retry instead of failing it.
    let nowhere = std::env::temp_dir().join("openbook-no-pdfium-here");
    e.deps = deps_for(
        &e.app,
        &[("OPENBOOK_PDFIUM_PATH", &nowhere.display().to_string())],
    )
    .await;
    let (doc_id, message) = e.upload("Geometry notes.pdf", geometry_pdf()).await;
    let outcome = e.run(&message, 4).await.unwrap();
    assert_eq!(outcome, Outcome::Failed(MSG_GENERIC.into()));
    let doc = e.document(&doc_id).await.unwrap();
    assert_eq!(
        (doc["status"].as_str(), doc["error"].as_str()),
        (Some("failed"), Some(MSG_GENERIC))
    );
    assert!(
        e.page_keys(&doc_id).await.is_empty(),
        "the PDF was not read"
    );

    // A document that was `processing` when the worker crashed (receives 4 and 5) fails the same way.
    let (second, message) = e.upload("Second.pdf", geometry_pdf()).await;
    assert!(
        e.deps
            .store
            .begin_processing(&e.course_id, &second, false)
            .await
            .unwrap()
    );
    assert_eq!(
        e.run(&message, 5).await.unwrap(),
        Outcome::Failed(MSG_GENERIC.into())
    );
    assert_eq!(e.document(&second).await.unwrap()["status"], "failed");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_delete_removes_vectors_up_to_the_planned_chunk_count() {
    let e = env().await;
    let (doc_id, outcome) = e.ingest("Geometry notes.pdf", geometry_pdf()).await;
    let Outcome::Ready { chunks, .. } = outcome else {
        panic!("{outcome:?}")
    };
    // The plan is on the document item before the vectors are written, and it matches the result.
    let item = e
        .app
        .dynamodb
        .get_item()
        .table_name(&e.app.table)
        .key("pk", AttributeValue::S(format!("COURSE#{}", e.course_id)))
        .key("sk", AttributeValue::S(format!("DOC#{doc_id}")))
        .consistent_read(true)
        .send()
        .await
        .unwrap()
        .item
        .unwrap();
    assert_eq!(
        item.get("plannedChunkCount"),
        Some(&AttributeValue::N(chunks.to_string()))
    );

    // A run that planned more chunks than it finished (it crashed in the middle): extra vectors exist.
    let extra = 4;
    let records: Vec<openbook_search::vectors::VectorRecord> = (chunks..chunks + extra)
        .map(|i| openbook_search::vectors::VectorRecord {
            key: openbook_search::vectors::vector_key(&doc_id, i),
            embedding: FakeEmbedder::vector("leftover pythagorean text"),
            document_id: doc_id.clone(),
            page: 1,
            heading: String::new(),
            text: "leftover pythagorean text".into(),
        })
        .collect();
    e.deps
        .vectors
        .put(&index_name(&e.course_id), &records)
        .await
        .unwrap();
    e.deps
        .store
        .set_planned_chunks(&e.course_id, &doc_id, chunks + extra)
        .await
        .unwrap();
    assert!(e.search_vectors("pythagorean", Some(&doc_id)).await.len() as u32 >= chunks + extra);

    e.app
        .delete(
            &format!("/api/courses/{}/documents/{doc_id}", e.course_id),
            &e.owner,
        )
        .await;
    let mut messages = e.app.queued_messages().await;
    let message: IngestMessage = serde_json::from_value(messages.pop().unwrap()).unwrap();
    assert_eq!(e.run(&message, 1).await.unwrap(), Outcome::Deleted);
    assert!(
        e.search_vectors("pythagorean", None).await.is_empty(),
        "no leftover vector"
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn two_writers_cannot_overwrite_each_others_bm25_blob() {
    let e = env().await;
    // Both writers start from "no index", so both write version 1. Their blobs have different keys.
    let chunk = |doc: &str, text: &str| openbook_search::bm25::Bm25Chunk {
        chunk_id: format!("{doc}:0"),
        document_id: doc.to_owned(),
        page: 1,
        heading: String::new(),
        text: text.to_owned(),
    };
    let root = std::env::temp_dir().join(format!(
        "openbook-two-writers-{}",
        openbook_core::ids::new_id()
    ));
    let (dir_a, dir_b) = (root.join("a"), root.join("b"));
    let (a, b) = tokio::join!(
        e.deps
            .bm25
            .update(&e.course_id, &dir_a, &[], vec![chunk("A", "alpha content")]),
        e.deps
            .bm25
            .update(&e.course_id, &dir_b, &[], vec![chunk("B", "bravo content")]),
    );
    assert_eq!(
        a.is_ok() as u8 + b.is_ok() as u8,
        1,
        "exactly one writer wins: {a:?} {b:?}"
    );
    let (version, winner_key) = a.or(b).unwrap();
    assert_eq!(version, 1);
    assert_eq!(e.bm25_key().await, Some(winner_key.clone()));
    assert_eq!(e.course_attribute("bm25Version").await, Some(1));
    // The winner's blob is intact and readable. The loser left no blob.
    let blobs = e.bm25_blobs().await;
    assert_eq!(blobs, vec![winner_key], "{blobs:?}");
    let hits = e.bm25_hits("content").await;
    assert_eq!(hits.len(), 1);
    assert!(["A", "B"].contains(&hits[0].document_id.as_str()));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_resumed_run_reports_the_same_fallback_warnings() {
    let mut e = env_with(&[("OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_PER_USER", "1")]).await;
    let pdf = make_pdf(&[geometry_page(), torn_page(), torn_page(), torn_page()]);
    // The first attempt writes every page (one transcribed, two on the text layer), then the embedder fails.
    e.deps.embedder = Arc::new(FlakyEmbedder {
        failures: AtomicUsize::new(0),
    });
    let (doc_id, message) = e.upload("Torn.pdf", pdf).await;
    assert!(e.run(&message, 1).await.is_err());
    let keys = e.page_keys(&doc_id).await;
    assert_eq!(keys.len(), 4, "all pages are in S3 for the retry");
    let usage_before = transcribed_counter(&e).await;

    // The retry reads the pages and their metadata. It spends no new transcription and finds the same warnings.
    e.deps.embedder = Arc::new(FakeEmbedder);
    let outcome = e.run(&message, 2).await.unwrap();
    assert!(
        matches!(outcome, Outcome::Ready { pages: 4, .. }),
        "{outcome:?}"
    );
    let doc = e.document(&doc_id).await.unwrap();
    assert_eq!(
        doc["transcribedPages"], 1,
        "the transcribed page is counted again"
    );
    let warnings = doc["warnings"]
        .as_array()
        .expect("the warning survives the retry");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].as_str().unwrap().starts_with(WARNING_CAP));
    assert!(
        warnings[0].as_str().unwrap().contains("(2 pages)"),
        "{warnings:?}"
    );
    assert_eq!(
        transcribed_counter(&e).await,
        usage_before,
        "no page was transcribed twice"
    );
}

async fn transcribed_counter(e: &Env) -> i64 {
    let item = e
        .app
        .dynamodb
        .get_item()
        .table_name(&e.app.table)
        .key("pk", AttributeValue::S(format!("USER#{}", e.owner.id)))
        .key(
            "sk",
            AttributeValue::S(format!(
                "USAGE#D#{}",
                openbook_core::quota::usage_date(chrono::Utc::now())
            )),
        )
        .consistent_read(true)
        .send()
        .await
        .unwrap()
        .item;
    match item.as_ref().and_then(|i| i.get("transcribedPages")) {
        Some(AttributeValue::N(n)) => n.parse().unwrap(),
        _ => 0,
    }
}
