//! The ingest and delete steps, as `docs/retrieval.md` describes them.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aws_sdk_s3::Client as S3Client;
use aws_smithy_types::error::display::DisplayErrorContext;
use chrono::Utc;
use futures::StreamExt;
use openbook_core::config::Config;
use openbook_core::domain::{CourseRecord, Document, DocumentStatus};
use openbook_core::files::{Files, document_prefix, source_key};
use openbook_core::queue::{Action, IngestMessage};
use openbook_core::quota::{
    Reservation, release_transcribed_page, reserve_transcribed_page, usage_date,
};
use openbook_core::store::{IngestResult, Store};
use openbook_search::bm25::{Bm25Chunk, Bm25Store};
use openbook_search::chunker::{chunk_pages, embed_text};
use openbook_search::embed::{EmbedError, Embedder, embed_all};
use openbook_search::vectors::{VectorRecord, VectorStore, index_name, vector_key};
use tokio_stream::wrappers::ReceiverStream;

use crate::pdf::{self, PageData, PdfError};
use crate::transcribe::{TranscribeError, Transcriber};

/// The last attempt: after this many receives, a transient error fails the document.
pub const MAX_RECEIVES: u32 = 3;
/// Pages that are in flight at once.
pub const PAGE_CONCURRENCY: usize = 8;
/// S3 object metadata key that marks a transcribed page, so a resumed run counts it.
const TRANSCRIBED_METADATA: &str = "transcribed";

pub const MSG_UNREADABLE: &str =
    "This PDF could not be opened. It may be damaged. Try another copy of the file.";
pub const MSG_PASSWORD: &str =
    "This PDF is password protected. Remove the password and upload it again.";
pub const MSG_NO_TEXT: &str = "No text could be found in this PDF.";
pub const MSG_GENERIC: &str = "Processing failed. Delete the document and upload it again.";
pub const MSG_INDEXING: &str = "The document could not be indexed. Delete it and upload it again.";
pub const WARNING_CAP: &str = "The daily limit for page transcription was reached, so some pages use only the text layer of the PDF. Math on those pages may be garbled.";
pub const WARNING_FAILED_PAGES: &str =
    "Some pages could not be transcribed, so they use only the text layer of the PDF.";

/// Everything the worker needs. Tests build it with fakes for the models.
pub struct Deps {
    pub config: Arc<Config>,
    pub store: Store,
    pub s3: S3Client,
    pub files: Files,
    pub vectors: VectorStore,
    pub bm25: Bm25Store,
    pub embedder: Arc<dyn Embedder>,
    pub transcriber: Arc<dyn Transcriber>,
    /// A directory for downloads and index work.
    pub work_root: PathBuf,
}

/// The handler failed in a way that a retry can fix. SQS delivers the message again.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Retry(pub String);

/// What happened to a message. Used for logs and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The document is `ready`.
    Ready { pages: u32, chunks: u32 },
    /// The document is `failed`, with this owner-safe message.
    Failed(String),
    /// The message was stale, a duplicate, or for something that no longer exists.
    Ignored(&'static str),
    /// The document was deleted (a `delete` message).
    Deleted,
}

#[derive(Debug)]
enum IngestError {
    /// The owner can fix it (or nothing can). The message is owner-safe.
    Permanent(String),
    /// Throttling or a timeout. A retry can work.
    Transient(String),
    /// The owner deleted the document while it was processed.
    Cancelled,
}

fn transient(error: &dyn std::error::Error) -> IngestError {
    IngestError::Transient(DisplayErrorContext(error).to_string())
}

/// Handles one message. `receive_count` is `ApproximateReceiveCount`. `budget` is how long it may run.
pub async fn handle_message(
    deps: &Deps,
    message: &IngestMessage,
    receive_count: u32,
    budget: Duration,
) -> Result<Outcome, Retry> {
    match message.action {
        Action::Ingest => handle_ingest(deps, message, receive_count, budget).await,
        Action::Delete => handle_delete(deps, message).await,
    }
}

async fn handle_ingest(
    deps: &Deps,
    message: &IngestMessage,
    receive_count: u32,
    budget: Duration,
) -> Result<Outcome, Retry> {
    let (course_id, document_id) = (&message.course_id, &message.document_id);
    let retry_err = |e: &dyn std::error::Error| Retry(e.to_string());
    let Some(course) = deps
        .store
        .get_course(course_id)
        .await
        .map_err(|e| retry_err(&e))?
    else {
        return Ok(Outcome::Ignored("the course no longer exists"));
    };
    let Some(document) = deps
        .store
        .get_document(course_id, document_id)
        .await
        .map_err(|e| retry_err(&e))?
    else {
        return Ok(Outcome::Ignored("the document no longer exists"));
    };
    // A message for a document that is not `queued` is a duplicate or stale. After a crash, the
    // document is still `processing` and the next receive carries on (the FIFO group has one worker).
    let retry = receive_count > 1;
    let acceptable = document.status == DocumentStatus::Queued
        || (retry && document.status == DocumentStatus::Processing);
    if !acceptable {
        return Ok(Outcome::Ignored("the document is not queued"));
    }
    if !deps
        .store
        .begin_processing(course_id, document_id, retry)
        .await
        .map_err(|e| retry_err(&e))?
    {
        return Ok(Outcome::Ignored("the document is not queued"));
    }

    let work_dir = deps.work_root.join(document_id);
    let result =
        match tokio::time::timeout(budget, run_ingest(deps, &course, &document, &work_dir)).await {
            Ok(result) => result,
            Err(_) => Err(IngestError::Transient("the time budget ran out".into())),
        };
    let _ = tokio::fs::remove_dir_all(&work_dir).await;

    match result {
        Ok(done) => {
            let moved = deps
                .store
                .finish_ingest(course_id, document_id, &done)
                .await
                .map_err(|e| retry_err(&e))?;
            if moved {
                tracing::info!(
                    pages = done.page_count,
                    chunks = done.chunk_count,
                    transcribed = done.transcribed_pages,
                    "document is ready"
                );
                Ok(Outcome::Ready {
                    pages: done.page_count,
                    chunks: done.chunk_count,
                })
            } else {
                // The owner deleted it while it was processed. Remove what this run wrote.
                tracing::warn!(
                    "the document left `processing` before the end; removing its vectors"
                );
                remove_vectors(deps, course_id, document_id, done.chunk_count).await;
                Ok(Outcome::Ignored(
                    "the document changed while it was processed",
                ))
            }
        }
        Err(IngestError::Cancelled) => Ok(Outcome::Ignored(
            "the document was deleted while it was processed",
        )),
        Err(IngestError::Permanent(text)) => {
            tracing::warn!(error = %text, "document failed");
            deps.store
                .fail_document(course_id, document_id, &text)
                .await
                .map_err(|e| retry_err(&e))?;
            Ok(Outcome::Failed(text))
        }
        Err(IngestError::Transient(text)) => {
            tracing::error!(error = %text, receive_count, "transient ingest error");
            if receive_count >= MAX_RECEIVES {
                deps.store
                    .fail_document(course_id, document_id, MSG_GENERIC)
                    .await
                    .map_err(|e| retry_err(&e))?;
                Ok(Outcome::Failed(MSG_GENERIC.into()))
            } else {
                // The retry accepts only `queued` documents (or `processing` after a crash).
                deps.store
                    .requeue(course_id, document_id)
                    .await
                    .map_err(|e| retry_err(&e))?;
                Err(Retry(text))
            }
        }
    }
}

async fn remove_vectors(deps: &Deps, course_id: &str, document_id: &str, chunks: u32) {
    let keys: Vec<String> = (0..chunks).map(|i| vector_key(document_id, i)).collect();
    if let Err(error) = deps
        .vectors
        .delete_keys(&index_name(course_id), &keys)
        .await
    {
        tracing::error!(%error, "could not remove vectors");
    }
}

/// The text of one finished page.
struct PageResult {
    number: u32,
    markdown: String,
    transcribed: bool,
}

fn page_key(course_id: &str, document_id: &str, number: u32) -> String {
    format!(
        "{}pages/{number:04}.md",
        document_prefix(course_id, document_id)
    )
}

async fn existing_pages(deps: &Deps, prefix: &str) -> Result<HashSet<u32>, IngestError> {
    let mut found = HashSet::new();
    let mut token: Option<String> = None;
    loop {
        let page = deps
            .s3
            .list_objects_v2()
            .bucket(deps.files.bucket())
            .prefix(format!("{prefix}pages/"))
            .set_continuation_token(token.take())
            .send()
            .await
            .map_err(|e| transient(&e))?;
        for object in page.contents() {
            let number = object
                .key()
                .and_then(|k| k.rsplit('/').next())
                .and_then(|name| name.strip_suffix(".md"))
                .and_then(|n| n.parse::<u32>().ok());
            if let Some(number) = number {
                found.insert(number);
            }
        }
        match page.next_continuation_token() {
            Some(next) if page.is_truncated().unwrap_or(false) => token = Some(next.to_owned()),
            _ => return Ok(found),
        }
    }
}

async fn read_existing_page(
    deps: &Deps,
    key: String,
    number: u32,
) -> Result<PageResult, IngestError> {
    let out = deps
        .s3
        .get_object()
        .bucket(deps.files.bucket())
        .key(key)
        .send()
        .await
        .map_err(|e| transient(&e))?;
    let transcribed = out
        .metadata()
        .is_some_and(|m| m.get(TRANSCRIBED_METADATA).is_some_and(|v| v == "1"));
    let bytes = out
        .body
        .collect()
        .await
        .map_err(|e| transient(&e))?
        .into_bytes();
    Ok(PageResult {
        number,
        markdown: String::from_utf8_lossy(&bytes).into_owned(),
        transcribed,
    })
}

#[derive(Default)]
struct PageStats {
    capped: u32,
    failed: u32,
}

async fn process_page(
    deps: &Deps,
    course_id: &str,
    document_id: &str,
    owner_id: &str,
    page: PageData,
) -> Result<(PageResult, PageStats), IngestError> {
    let mut stats = PageStats::default();
    let mut markdown = page.text.clone();
    let mut transcribed = false;
    if let Some(png) = &page.png {
        let limits = &deps.config.limits;
        let date = usage_date(Utc::now());
        let reserved = reserve_transcribed_page(
            &deps.store,
            owner_id,
            &date,
            limits.transcribed_pages_per_user,
            limits.transcribed_pages_global,
        )
        .await
        .map_err(|e| transient(&e))?;
        if reserved == Reservation::Reserved {
            match deps
                .transcriber
                .transcribe(page.number, png, &page.text)
                .await
            {
                Ok(text) => {
                    markdown = text;
                    transcribed = true;
                }
                Err(TranscribeError::Transient(error)) => {
                    release_transcribed_page(&deps.store, owner_id, &date).await;
                    return Err(IngestError::Transient(error));
                }
                Err(TranscribeError::Permanent(error)) => {
                    tracing::warn!(page = page.number, %error, "transcription failed; using the text layer");
                    release_transcribed_page(&deps.store, owner_id, &date).await;
                    stats.failed += 1;
                }
            }
        } else {
            tracing::warn!(
                page = page.number,
                ?reserved,
                "transcription cap reached; using the text layer"
            );
            stats.capped += 1;
        }
    }
    let mut request = deps
        .s3
        .put_object()
        .bucket(deps.files.bucket())
        .key(page_key(course_id, document_id, page.number))
        .content_type("text/markdown; charset=utf-8")
        .body(markdown.clone().into_bytes().into());
    if transcribed {
        request = request.metadata(TRANSCRIBED_METADATA, "1");
    }
    request.send().await.map_err(|e| transient(&e))?;
    Ok((
        PageResult {
            number: page.number,
            markdown,
            transcribed,
        },
        stats,
    ))
}

fn pdf_error(error: PdfError, limit: u32) -> IngestError {
    match error {
        PdfError::Unreadable => IngestError::Permanent(MSG_UNREADABLE.into()),
        PdfError::Password => IngestError::Permanent(MSG_PASSWORD.into()),
        PdfError::TooManyPages { pages, .. } => IngestError::Permanent(format!(
            "This PDF has {pages} pages. The limit is {limit} pages for one document."
        )),
        PdfError::Library(text) => IngestError::Transient(text),
    }
}

async fn run_ingest(
    deps: &Deps,
    course: &CourseRecord,
    document: &Document,
    work_dir: &std::path::Path,
) -> Result<IngestResult, IngestError> {
    let course_id = &course.id;
    let document_id = &document.id;
    let limits = &deps.config.limits;
    let prefix = document_prefix(course_id, document_id);

    // Step 2: download the PDF.
    tokio::fs::create_dir_all(work_dir)
        .await
        .map_err(|e| IngestError::Transient(e.to_string()))?;
    let pdf_path = work_dir.join("source.pdf");
    let source = deps
        .s3
        .get_object()
        .bucket(deps.files.bucket())
        .key(source_key(course_id, document_id))
        .send()
        .await
        .map_err(|e| transient(&e))?;
    let bytes = source
        .body
        .collect()
        .await
        .map_err(|e| transient(&e))?
        .into_bytes();
    tokio::fs::write(&pdf_path, &bytes)
        .await
        .map_err(|e| IngestError::Transient(e.to_string()))?;
    drop(bytes);

    // Step 3: open it, and refuse what cannot work.
    let lib_dir = pdf::library_dir(deps.config.pdfium_path.as_deref());
    let page_total = {
        let (lib_dir, pdf_path, limit) =
            (lib_dir.clone(), pdf_path.clone(), limits.pages_per_document);
        tokio::task::spawn_blocking(move || pdf::page_count(&lib_dir, &pdf_path, limit))
            .await
            .map_err(|e| IngestError::Transient(e.to_string()))?
            .map_err(|e| pdf_error(e, limits.pages_per_document))?
    };

    // Steps 4 to 6: read, transcribe, and write the pages. Pages that exist in S3 are not done again.
    let done_before = existing_pages(deps, &prefix).await?;
    let skip: HashSet<u32> = done_before
        .iter()
        .copied()
        .filter(|n| *n <= page_total)
        .collect();
    let (tx, rx) = tokio::sync::mpsc::channel::<PageData>(PAGE_CONCURRENCY);
    let producer = {
        let (lib_dir, pdf_path, skip) = (lib_dir.clone(), pdf_path.clone(), skip.clone());
        tokio::task::spawn_blocking(move || pdf::extract_pages(&lib_dir, &pdf_path, &skip, &tx))
    };
    let mut results: BTreeMap<u32, PageResult> = BTreeMap::new();
    let mut capped = 0u32;
    let mut failed_pages = 0u32;
    {
        let mut stream = ReceiverStream::new(rx)
            .map(|page| process_page(deps, course_id, document_id, &course.owner_id, page))
            .buffer_unordered(PAGE_CONCURRENCY);
        while let Some(item) = stream.next().await {
            let (page, stats) = item?;
            capped += stats.capped;
            failed_pages += stats.failed;
            results.insert(page.number, page);
        }
    }
    producer
        .await
        .map_err(|e| IngestError::Transient(e.to_string()))?
        .map_err(|e| pdf_error(e, limits.pages_per_document))?;
    // Pages from an earlier attempt.
    let earlier: Vec<u32> = skip.iter().copied().collect();
    let mut earlier_stream = futures::stream::iter(earlier)
        .map(|n| read_existing_page(deps, page_key(course_id, document_id, n), n))
        .buffer_unordered(PAGE_CONCURRENCY);
    while let Some(page) = earlier_stream.next().await {
        let page = page?;
        results.insert(page.number, page);
    }
    drop(earlier_stream);
    if results.len() != page_total as usize {
        return Err(IngestError::Transient(format!(
            "expected {page_total} pages and found {}",
            results.len()
        )));
    }
    let transcribed_pages =
        u32::try_from(results.values().filter(|p| p.transcribed).count()).unwrap_or(u32::MAX);

    // Step 7: chunk.
    let pages: Vec<(u32, String)> = results
        .values()
        .map(|p| (p.number, p.markdown.clone()))
        .collect();
    let chunks = chunk_pages(&pages);
    if chunks.is_empty() {
        return Err(IngestError::Permanent(MSG_NO_TEXT.into()));
    }

    // Stop here when the owner deleted the document in the meantime.
    let current = deps
        .store
        .get_document(course_id, document_id)
        .await
        .map_err(|e| transient(&e))?;
    if current.is_none_or(|d| d.status != DocumentStatus::Processing) {
        return Err(IngestError::Cancelled);
    }

    // Steps 8 and 9: embed.
    let title = document
        .filename
        .strip_suffix(".pdf")
        .or_else(|| document.filename.strip_suffix(".PDF"))
        .unwrap_or(&document.filename)
        .to_owned();
    let texts: Vec<String> = chunks.iter().map(|c| embed_text(&title, c)).collect();
    let embeddings = embed_all(deps.embedder.as_ref(), &texts)
        .await
        .map_err(|e| match e {
            EmbedError::Transient(text) => IngestError::Transient(text),
            EmbedError::Permanent(text) => {
                tracing::error!(error = %text, "embedding failed");
                IngestError::Permanent(MSG_INDEXING.into())
            }
        })?;

    // Step 10: vectors.
    let index = index_name(course_id);
    deps.vectors
        .ensure_index(&index)
        .await
        .map_err(|e| transient(&e))?;
    let records: Vec<VectorRecord> = chunks
        .iter()
        .zip(embeddings)
        .map(|(chunk, embedding)| VectorRecord {
            key: vector_key(document_id, chunk.index),
            embedding,
            document_id: document_id.clone(),
            page: chunk.first_page,
            heading: chunk.heading_path.clone(),
            text: chunk.text.clone(),
        })
        .collect();
    deps.vectors
        .put(&index, &records)
        .await
        .map_err(|e| transient(&e))?;

    // Step 11: BM25. A failure here leaves vectors that the next attempt overwrites.
    let bm25_chunks: Vec<Bm25Chunk> = chunks
        .iter()
        .map(|c| Bm25Chunk {
            chunk_id: vector_key(document_id, c.index),
            document_id: document_id.clone(),
            page: u64::from(c.first_page),
            heading: c.heading_path.clone(),
            text: c.text.clone(),
        })
        .collect();
    deps.bm25
        .update(
            course_id,
            &work_dir.join("bm25"),
            std::slice::from_ref(document_id),
            bm25_chunks,
        )
        .await
        .map_err(|e| transient(&e))?;

    let mut warnings = Vec::new();
    if capped > 0 {
        warnings.push(format!("{WARNING_CAP} ({capped} pages)"));
    }
    if failed_pages > 0 {
        warnings.push(format!("{WARNING_FAILED_PAGES} ({failed_pages} pages)"));
    }
    Ok(IngestResult {
        page_count: page_total,
        chunk_count: u32::try_from(chunks.len()).unwrap_or(u32::MAX),
        transcribed_pages,
        warnings,
    })
}

async fn handle_delete(deps: &Deps, message: &IngestMessage) -> Result<Outcome, Retry> {
    let (course_id, document_id) = (&message.course_id, &message.document_id);
    let retry_err = |e: &dyn std::error::Error| Retry(e.to_string());
    let Some(course) = deps
        .store
        .get_course(course_id)
        .await
        .map_err(|e| retry_err(&e))?
    else {
        return Ok(Outcome::Ignored("the course no longer exists"));
    };
    let Some(document) = deps
        .store
        .get_document(course_id, document_id)
        .await
        .map_err(|e| retry_err(&e))?
    else {
        return Ok(Outcome::Ignored("the document no longer exists"));
    };
    if document.status != DocumentStatus::Deleting {
        return Ok(Outcome::Ignored("the document is not being deleted"));
    }

    // Vectors by key: `<documentId>:0` to `<documentId>:<chunkCount - 1>`.
    let keys: Vec<String> = (0..document.chunk_count.unwrap_or(0))
        .map(|i| vector_key(document_id, i))
        .collect();
    deps.vectors
        .delete_keys(&index_name(course_id), &keys)
        .await
        .map_err(|e| retry_err(&e))?;
    // The BM25 entries, when the course has an index.
    if course.bm25_version.is_some() {
        deps.bm25
            .update(
                course_id,
                &deps
                    .work_root
                    .join(format!("{document_id}-delete"))
                    .join("bm25"),
                std::slice::from_ref(document_id),
                Vec::new(),
            )
            .await
            .map_err(|e| retry_err(&e))?;
    }
    deps.files
        .delete_prefix(&document_prefix(course_id, document_id))
        .await
        .map_err(|e| retry_err(&e))?;
    deps.store
        .delete_document(course_id, document_id)
        .await
        .map_err(|e| retry_err(&e))?;
    Ok(Outcome::Deleted)
}
