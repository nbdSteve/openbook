//! Demo provisioning: creates or refreshes the public demo course through the normal ingestion path.
//!
//! The command makes a course item, uploads the PDFs of the manifest to the docs bucket, and sends
//! the same `ingest` messages that the API sends. The worker then runs the real pipeline: pages,
//! chunks, embeddings, vectors, and the BM25 index. When every document is `ready`, the command
//! points the reserved table item `DEMO#COURSE / ACTIVE` at the new course. The API finds the demo
//! course only through that item, so no ID is committed anywhere.
//!
//! A run is safe to repeat. The same content gives "up to date". New content builds a second course,
//! switches the item, and then deletes the old course, so the demo has no gap.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use openbook_core::demo::{Attribution, DemoCourse};
use openbook_core::domain::{Document, DocumentStatus, TutoringStyle};
use openbook_core::files::{Files, source_key};
use openbook_core::ids;
use openbook_core::queue::{Action, IngestMessage, IngestQueue};
use openbook_core::store::{Store, StoreError};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_NAME_CHARS: usize = 80;
const MAX_DESCRIPTION_CHARS: usize = 500;
const MAX_PDF_BYTES: usize = 25 * 1024 * 1024;
/// The longest example question, in words.
const MAX_EXAMPLE_WORDS: usize = 5;
/// The manifest changes the fingerprint when this changes, for a change of the pipeline.
const FINGERPRINT_VERSION: &str = "openbook-demo-v1";

#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{0}")]
    Files(#[from] openbook_core::files::FilesError),
    #[error("{0}")]
    Queue(#[from] openbook_core::queue::QueueError),
    #[error("ingestion failed for {document}: {reason}")]
    IngestFailed { document: String, reason: String },
    #[error("the documents were not ready after {0:?}. Is the ingest worker running?")]
    Timeout(Duration),
}

// ---------------------------------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub tutoring_style: TutoringStyle,
    /// Four to six example questions, half from each book, in alternating order. The landing page
    /// shows them (the CDK app reads them from this file for the public web config).
    pub examples: Vec<String>,
    pub sources: Vec<SourceSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSpec {
    /// Paths relative to the manifest file. The first file that exists is used, so a move of the
    /// evaluation corpora does not break the demo.
    pub files: Vec<String>,
    /// The name that the document has in the course (the UI shows it as the source name), for
    /// example "College Algebra.pdf". Without it, the name of the file. It is part of the fingerprint,
    /// so a new name makes the next `make demo-provision` build the course again.
    #[serde(default)]
    pub document_name: Option<String>,
    pub attribution: Attribution,
}

#[derive(Debug, Clone)]
pub struct Pdf {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// A manifest with its PDFs read.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub manifest: Manifest,
    /// One PDF for each source, in the order of the sources.
    pub pdfs: Vec<Pdf>,
}

impl Loaded {
    pub fn load(path: &Path) -> Result<Loaded, DemoError> {
        let bad = |message: String| DemoError::Manifest(message);
        let text = std::fs::read_to_string(path)
            .map_err(|e| bad(format!("cannot read {}: {e}", path.display())))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| bad(format!("{} is not valid: {e}", path.display())))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut pdfs = Vec::new();
        for source in &manifest.sources {
            let found: Option<PathBuf> = source
                .files
                .iter()
                .map(|f| base.join(f))
                .find(|candidate| candidate.is_file());
            let Some(file) = found else {
                return Err(bad(format!(
                    "no file of the source \"{}\" exists (tried {})",
                    source.attribution.title,
                    source.files.join(", ")
                )));
            };
            let bytes = std::fs::read(&file)
                .map_err(|e| bad(format!("cannot read {}: {e}", file.display())))?;
            let filename = source.document_name.clone().unwrap_or_else(|| {
                file.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned()
            });
            pdfs.push(Pdf { filename, bytes });
        }
        let loaded = Loaded { manifest, pdfs };
        loaded.check()?;
        Ok(loaded)
    }

    /// The rules that the demo needs. They protect the public pages from a bad edit.
    pub fn check(&self) -> Result<(), DemoError> {
        let bad = |message: &str| Err(DemoError::Manifest(message.to_owned()));
        let m = &self.manifest;
        if m.name.trim().is_empty() || m.name.chars().count() > MAX_NAME_CHARS {
            return bad("name must be 1 to 80 characters");
        }
        if m.description.chars().count() > MAX_DESCRIPTION_CHARS {
            return bad("description must be at most 500 characters");
        }
        if !(4..=6).contains(&m.examples.len()) || m.examples.iter().any(|e| e.trim().is_empty()) {
            return bad("examples must be 4 to 6 questions");
        }
        // The landing page shows them as chips with a tight word budget.
        if m.examples
            .iter()
            .any(|e| e.split_whitespace().count() > MAX_EXAMPLE_WORDS)
        {
            return bad("an example question has more than 5 words");
        }
        if m.sources.is_empty() || self.pdfs.len() != m.sources.len() {
            return bad("the manifest needs at least one source with a file");
        }
        for (source, pdf) in m.sources.iter().zip(&self.pdfs) {
            let a = &source.attribution;
            let fields = [&a.title, &a.authors, &a.publisher, &a.copyright, &a.license];
            if fields.iter().any(|f| f.trim().is_empty())
                || !a.license_url.starts_with("https://")
                || !a.source_url.starts_with("https://")
            {
                return bad("every source needs a full attribution with https links");
            }
            if !pdf.filename.to_lowercase().ends_with(".pdf") || pdf.bytes.is_empty() {
                return bad("every source file must be a non-empty .pdf");
            }
            if pdf.bytes.len() > MAX_PDF_BYTES {
                return bad("a source file is larger than 25 MiB");
            }
        }
        Ok(())
    }

    /// A hash of everything that decides what the demo course holds: the text of the manifest
    /// (not the file paths, and not the examples, which the course does not store) and the bytes of
    /// every PDF.
    pub fn fingerprint(&self) -> String {
        let m = &self.manifest;
        let text = serde_json::json!({
            "name": m.name,
            "description": m.description,
            "tutoringStyle": m.tutoring_style,
            "attribution": m.sources.iter().map(|s| &s.attribution).collect::<Vec<_>>(),
        });
        let mut hash = Sha256::new();
        hash.update(FINGERPRINT_VERSION.as_bytes());
        hash.update(text.to_string().as_bytes());
        for pdf in &self.pdfs {
            hash.update(pdf.filename.as_bytes());
            hash.update(Sha256::digest(&pdf.bytes));
        }
        openbook_core::demo::hex(&hash.finalize())
    }

    pub fn attribution(&self) -> Vec<Attribution> {
        self.manifest
            .sources
            .iter()
            .map(|s| s.attribution.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Provisioning
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The active course already has this content, and every document is ready.
    UpToDate { course_id: String },
    /// A new course is active. `replaced` is the old course that was sent to be deleted.
    Provisioned {
        course_id: String,
        replaced: Option<String>,
    },
}

pub struct Provisioner {
    pub store: Store,
    pub files: Files,
    pub queue: IngestQueue,
    /// The catalog ID that the course item stores. The demo route uses the fast model anyway.
    pub model: String,
    /// How long to wait for the worker.
    pub wait: Duration,
    /// How often to look at the documents.
    pub poll: Duration,
}

impl Provisioner {
    pub async fn provision(&self, loaded: &Loaded, force: bool) -> Result<Outcome, DemoError> {
        loaded.check()?;
        let fingerprint = loaded.fingerprint();
        let active = self.store.get_demo_course().await?;

        let pending = self.store.get_pending_demo_course().await?;
        if let Some(active) = &active
            && !force
            && active.fingerprint == fingerprint
            && self
                .is_current(&active.course_id, loaded.pdfs.len())
                .await?
        {
            // A run that stopped after it switched the demo left its note. The course is the active one.
            if pending.as_deref() == Some(active.course_id.as_str()) {
                self.store.delete_pending_demo_course().await?;
            }
            return Ok(Outcome::UpToDate {
                course_id: active.course_id.clone(),
            });
        }

        // An earlier run that stopped half way left a course behind. Remove it first, but never the
        // active course: a run that switched the demo and stopped before it cleared its note left the
        // active ID here, and that course serves the visitors.
        if let Some(stale) = pending {
            if active.as_ref().is_none_or(|a| a.course_id != stale) {
                tracing::info!(course_id = %stale, "removing the course of an unfinished run");
                self.retire(&stale).await?;
            }
            self.store.delete_pending_demo_course().await?;
        }

        let m = &loaded.manifest;
        let course = self
            .store
            .create_demo_course(&m.name, &m.description, m.tutoring_style, &self.model)
            .await?;
        self.store.put_pending_demo_course(&course.id).await?;
        tracing::info!(course_id = %course.id, "created the demo course");

        for pdf in &loaded.pdfs {
            self.submit(&course.id, pdf).await?;
        }
        self.wait_ready(&course.id, loaded.pdfs.len()).await?;

        let demo = DemoCourse {
            course_id: course.id.clone(),
            fingerprint,
            attribution: loaded.attribution(),
            provisioned_at: openbook_core::domain::now(),
        };
        self.store.put_demo_course(&demo).await?;
        self.store.delete_pending_demo_course().await?;
        tracing::info!(course_id = %course.id, "the demo course is active");

        let replaced = match active {
            Some(old) if old.course_id != course.id => {
                self.retire(&old.course_id).await?;
                Some(old.course_id)
            }
            _ => None,
        };
        Ok(Outcome::Provisioned {
            course_id: course.id,
            replaced,
        })
    }

    /// The course exists, is not being deleted, and has exactly `count` documents, all ready.
    async fn is_current(&self, course_id: &str, count: usize) -> Result<bool, DemoError> {
        if !self.store.course_is_live(course_id).await? {
            return Ok(false);
        }
        let documents = self.store.list_documents(course_id).await?;
        Ok(documents.len() == count && documents.iter().all(|d| d.status == DocumentStatus::Ready))
    }

    /// The same steps as `POST .../documents` and `.../complete`, with the file written directly.
    async fn submit(&self, course_id: &str, pdf: &Pdf) -> Result<(), DemoError> {
        let now = openbook_core::domain::now();
        let document = Document {
            id: ids::new_id(),
            course_id: course_id.to_owned(),
            filename: pdf.filename.clone(),
            size_bytes: pdf.bytes.len() as u64,
            status: DocumentStatus::Uploading,
            error: None,
            page_count: None,
            chunk_count: None,
            transcribed_pages: None,
            warnings: None,
            planned_chunk_count: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let key = source_key(course_id, &document.id);
        self.store
            .create_document(&document, &key, u32::MAX)
            .await?;
        self.files
            .put_object(&key, "application/pdf", pdf.bytes.clone())
            .await?;
        if let Some(queued) = self.store.mark_queued(course_id, &document.id).await? {
            self.queue
                .send(&IngestMessage::new(Action::Ingest, course_id, &queued.id))
                .await?;
            tracing::info!(document = %pdf.filename, "queued for ingestion");
        }
        Ok(())
    }

    async fn wait_ready(&self, course_id: &str, count: usize) -> Result<(), DemoError> {
        let started = Instant::now();
        loop {
            let documents = self.store.list_documents(course_id).await?;
            if let Some(failed) = documents
                .iter()
                .find(|d| d.status == DocumentStatus::Failed)
            {
                return Err(DemoError::IngestFailed {
                    document: failed.filename.clone(),
                    reason: failed.error.clone().unwrap_or_else(|| "unknown".into()),
                });
            }
            if documents.len() == count
                && documents.iter().all(|d| d.status == DocumentStatus::Ready)
            {
                return Ok(());
            }
            if started.elapsed() >= self.wait {
                return Err(DemoError::Timeout(self.wait));
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Marks the course `deleting` and sends the same message as `DELETE /api/courses/{id}`.
    /// The worker removes the vectors, the files, the documents, and the course item.
    async fn retire(&self, course_id: &str) -> Result<(), DemoError> {
        if self.store.mark_course_deleting(course_id).await? {
            self.queue
                .send(&IngestMessage::delete_course(course_id))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribution() -> Attribution {
        Attribution {
            title: "Book".into(),
            authors: "A. Author".into(),
            publisher: "Press".into(),
            copyright: "2020 Press".into(),
            license: "CC BY 4.0".into(),
            license_url: "https://creativecommons.org/licenses/by/4.0/".into(),
            source_url: "https://example.org/book".into(),
            changes: "Pages selected.".into(),
        }
    }

    fn loaded() -> Loaded {
        Loaded {
            manifest: Manifest {
                name: "Demo".into(),
                description: "A demo.".into(),
                tutoring_style: TutoringStyle::Direct,
                examples: ["One?", "Two?", "Three?", "Four?"].map(Into::into).to_vec(),
                sources: vec![SourceSpec {
                    files: vec!["a.pdf".into()],
                    document_name: None,
                    attribution: attribution(),
                }],
            },
            pdfs: vec![Pdf {
                filename: "a.pdf".into(),
                bytes: b"%PDF-1.4 one".to_vec(),
            }],
        }
    }

    #[test]
    fn the_fingerprint_follows_the_content_and_not_the_paths() {
        let base = loaded();
        let same = {
            let mut l = loaded();
            l.manifest.sources[0].files = vec!["elsewhere/a.pdf".into()];
            l
        };
        assert_eq!(base.fingerprint(), same.fingerprint());
        assert_eq!(base.fingerprint().len(), 64);

        let mut other_bytes = loaded();
        other_bytes.pdfs[0].bytes.push(b'x');
        assert_ne!(base.fingerprint(), other_bytes.fingerprint());
        // The name of the document is part of the content: the UI shows it.
        let mut other_name = loaded();
        other_name.pdfs[0].filename = "A Book.pdf".into();
        assert_ne!(base.fingerprint(), other_name.fingerprint());
        let mut other_text = loaded();
        other_text.manifest.description = "Other things.".into();
        assert_ne!(base.fingerprint(), other_text.fingerprint());
        let mut other_credit = loaded();
        other_credit.manifest.sources[0].attribution.changes = "More.".into();
        assert_ne!(base.fingerprint(), other_credit.fingerprint());
    }

    #[test]
    fn the_manifest_rules_protect_the_public_page() {
        assert!(loaded().check().is_ok());
        type Change = Box<dyn Fn(&mut Loaded)>;
        let breaks: Vec<(&str, Change)> = vec![
            ("name", Box::new(|l| l.manifest.name = " ".into())),
            (
                "a long example",
                Box::new(|l| {
                    l.manifest.examples[0] = "What is the product rule of exponents?".into()
                }),
            ),
            (
                "three examples",
                Box::new(|l| l.manifest.examples.truncate(3)),
            ),
            (
                "no licence link",
                Box::new(|l| l.manifest.sources[0].attribution.license_url = "x".into()),
            ),
            (
                "no authors",
                Box::new(|l| l.manifest.sources[0].attribution.authors = String::new()),
            ),
            (
                "not a pdf",
                Box::new(|l| l.pdfs[0].filename = "a.txt".into()),
            ),
            ("empty file", Box::new(|l| l.pdfs[0].bytes.clear())),
            ("no sources", Box::new(|l| l.manifest.sources.clear())),
        ];
        for (what, change) in breaks {
            let mut l = loaded();
            change(&mut l);
            assert!(l.check().is_err(), "{what} must be refused");
        }
    }

    #[test]
    fn the_real_manifest_is_valid_when_its_files_exist() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/course.json");
        let loaded = Loaded::load(&path).expect("the committed manifest loads");
        assert_eq!(loaded.pdfs.len(), 2, "two OpenStax excerpts");
        assert!((4..=6).contains(&loaded.manifest.examples.len()));
        // Alternating books: the even ones are from the first book (College Algebra), the odd ones
        // from the second (American Government).
        assert_eq!(loaded.manifest.examples.len() % 2, 0, "half from each book");
        // The documents have the titles of the books as names, not the names of the files.
        let names: Vec<_> = loaded.pdfs.iter().map(|p| p.filename.as_str()).collect();
        assert_eq!(names, ["College Algebra.pdf", "American Government 2e.pdf"]);
        for source in &loaded.manifest.sources {
            assert_eq!(source.attribution.license, "CC BY 4.0");
            assert!(source.attribution.source_url.contains("openstax.org"));
        }
    }

    #[test]
    fn a_missing_file_is_named() {
        let dir = std::env::temp_dir().join(format!("openbook-demo-{}", ids::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = serde_json::json!({
            "name": "Demo", "description": "", "tutoringStyle": "direct",
            "examples": ["One?", "Two?", "Three?", "Four?"],
            "sources": [{ "files": ["nope.pdf"], "attribution": attribution() }]
        });
        std::fs::write(dir.join("course.json"), manifest.to_string()).unwrap();
        let error = Loaded::load(&dir.join("course.json"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Book") && error.contains("nope.pdf"),
            "{error}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
