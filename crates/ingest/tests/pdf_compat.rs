//! PDF compatibility: the ingest worker against PDFs of many versions and producers.
//!
//! The fixtures live in `tests/fixtures/pdf-compat/` and come from `scripts/make-pdf-fixtures.sh`.
//! `tests/fixtures/pdf-compat/README.md` says what each one is. To add a case, add a line to
//! `CASES` (and the fixture to the script).
//!
//! Run it with `make test-integration`. It needs `OPENBOOK_PDFIUM_PATH` (see
//! `scripts/fetch-pdfium.sh`). The models are fakes: the fake embedder hashes words, and the test
//! transcriber returns the text layer under a `# Page <n>` heading. For a page with no text layer
//! (a scan) it returns a fixed sentence, as a vision model would read it from the image.

#[path = "../../api/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use openbook_core::config::{Config, Service};
use openbook_core::files::Files;
use openbook_core::queue::IngestMessage;
use openbook_core::store::Store;
use openbook_ingest::pipeline::{Deps, MSG_PASSWORD, MSG_UNREADABLE, Outcome, handle_message};
use openbook_ingest::transcribe::{TranscribeError, Transcriber};
use openbook_search::bm25::Bm25Store;
use openbook_search::embed::FakeEmbedder;
use openbook_search::vectors::VectorStore;
use serde_json::{Value, json};
use support::{TestApp, TestUser, start};

const BUDGET: Duration = Duration::from_secs(120);

/// What the test transcriber says about a page that has no text layer.
const SCAN_READING: &str =
    "Scanned Lecture Notes. The Pythagorean theorem links the three sides of a right triangle.";

/// What a fixture must do.
enum Expect {
    /// `ready` with this many pages, this text in the stored page Markdown (in this page order), and
    /// a number of transcribed pages inside the range.
    Ready {
        pages: u64,
        /// Text that must be in the Markdown of the whole document.
        present: &'static [&'static str],
        /// Text that must not be there.
        absent: &'static [&'static str],
        /// Text that must be in the Markdown of page `n` (1-based).
        on_page: &'static [(usize, &'static str)],
        /// The least and the most pages that go through the transcriber.
        transcribed: (u64, u64),
    },
    /// `failed` with exactly this owner-safe message.
    Failed(&'static str),
}

struct Case {
    file: &'static str,
    expect: Expect,
    /// A real ingestion bug that this fixture shows. The case is skipped and named in the output.
    known_bug: Option<&'static str>,
}

const fn ready(pages: u64, present: &'static [&'static str], transcribed: (u64, u64)) -> Expect {
    Expect::Ready {
        pages,
        present,
        absent: &[],
        on_page: &[],
        transcribed,
    }
}

const NONE: (u64, u64) = (0, 0);

const CASES: &[Case] = &[
    Case {
        file: "latex-plain.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "Mitochondria produce most of the chemical energy",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "latex-math.pdf",
        // The math makes the text layer poor, so the page must go to the transcriber.
        expect: ready(1, &["Quadratic Formulas", "quadratic formula"], (1, 1)),
        known_bug: None,
    },
    Case {
        file: "quartz-cupsfilter.pdf",
        expect: ready(
            1,
            &[
                "Photosynthesis Summary",
                "Chlorophyll absorbs red and blue light",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "gs-13.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "gs-14.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "gs-17.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "gs-pdfa2b.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "qpdf-objstm-xrefstream.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "qpdf-linearized.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "qpdf-pdf20.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "incremental-update.pdf",
        expect: Expect::Ready {
            pages: 1,
            present: &["Final Lecture Sheet", "the newest revision of the sheet"],
            absent: &["Draft Lecture Sheet", "first revision"],
            on_page: &[],
            transcribed: NONE,
        },
        known_bug: None,
    },
    Case {
        file: "qpdf-encrypted-owner-only.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "qpdf-encrypted-user-password.pdf",
        expect: Expect::Failed(MSG_PASSWORD),
        known_bug: None,
    },
    Case {
        file: "scanned-image-only.pdf",
        // No text layer: the transcriber must run, and its reading is what gets stored.
        expect: ready(1, &["Scanned Lecture Notes", "Pythagorean theorem"], (1, 1)),
        known_bug: None,
    },
    Case {
        file: "qpdf-rotated-90.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "qpdf-rotated-180.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "qpdf-rotated-270.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "multipage-3.pdf",
        expect: Expect::Ready {
            pages: 3,
            present: &[],
            absent: &[],
            on_page: &[
                (1, "The Nile flows north"),
                (2, "The Andes form the longest mountain range"),
                (3, "The Sahara is the largest hot desert"),
            ],
            transcribed: NONE,
        },
        known_bug: None,
    },
    Case {
        file: "greek-xelatex.pdf",
        // Greek words are not math symbols, so the text layer is enough and the page is not transcribed.
        // XeTeX writes the accents as combining marks (NFD), so the checks use unaccented stems.
        expect: ready(1, &["Εισαγωγ", "Γεωμετρ", "Πυθαγ", "πλευρ"], NONE),
        known_bug: None,
    },
    Case {
        file: "chinese-xelatex.pdf",
        expect: ready(
            1,
            &["细胞生物学笔记", "线粒体为细胞提供大部分化学能量"],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "damaged-repairable-xref.pdf",
        expect: ready(
            1,
            &[
                "Cell Biology Notes",
                "The nucleus stores the genetic material",
            ],
            NONE,
        ),
        known_bug: None,
    },
    Case {
        file: "damaged-unopenable.pdf",
        expect: Expect::Failed(MSG_UNREADABLE),
        known_bug: None,
    },
];

/// Returns the text layer under a heading, or `SCAN_READING` when the page has no text layer.
/// It remembers the pages that it was asked to read, and whether each had an image.
struct TestTranscriber {
    seen: Mutex<Vec<(u32, bool)>>,
}

#[async_trait]
impl Transcriber for TestTranscriber {
    async fn transcribe(
        &self,
        page_number: u32,
        image: &[u8],
        text_layer: &str,
    ) -> Result<String, TranscribeError> {
        self.seen
            .lock()
            .map_err(|_| TranscribeError::Permanent("poisoned".into()))?
            .push((page_number, image.starts_with(&[0xFF, 0xD8])));
        let body = if text_layer.trim().is_empty() {
            SCAN_READING
        } else {
            text_layer.trim()
        };
        Ok(format!("# Page {page_number}\n\n{body}"))
    }
}

fn pdfium_dir() -> PathBuf {
    match std::env::var("OPENBOOK_PDFIUM_PATH") {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => panic!(
            "OPENBOOK_PDFIUM_PATH is not set. Run scripts/fetch-pdfium.sh and set it to the path that it prints."
        ),
    }
}

fn text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("no string {key} in {value}"))
        .to_owned()
}

struct Env {
    app: TestApp,
    owner: TestUser,
    course_id: String,
    deps: Deps,
    transcriber: Arc<TestTranscriber>,
}

async fn env() -> Env {
    let app = start().await;
    let owner = app.user("teacher").await;
    let course = app.course(&owner, "PDF compatibility").await;
    let mut vars = app.vars().clone();
    vars.insert(
        "OPENBOOK_PDFIUM_PATH".into(),
        pdfium_dir().display().to_string(),
    );
    let config = Config::from_vars_for(&vars, Service::Ingest).expect("ingest config");
    let store = Store::new(&app.sdk, config.table_name.clone());
    let files = Files::new(&app.sdk, config.docs_bucket.clone(), true, None);
    let vectors = VectorStore::new(&app.sdk, config.vector_bucket.clone());
    let bm25 = Bm25Store::new(app.s3.clone(), config.docs_bucket.clone(), store.clone());
    let transcriber = Arc::new(TestTranscriber {
        seen: Mutex::new(Vec::new()),
    });
    let deps = Deps {
        config: Arc::new(config),
        store,
        s3: app.s3.clone(),
        files,
        vectors,
        bm25,
        embedder: Arc::new(FakeEmbedder),
        transcriber: transcriber.clone(),
        work_root: std::env::temp_dir().join(format!(
            "openbook-ingest-test-{}",
            openbook_core::ids::new_id()
        )),
    };
    Env {
        course_id: text(&course, "id"),
        app,
        owner,
        deps,
        transcriber,
    }
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

    async fn document(&self, doc_id: &str) -> Value {
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
            .expect("documents")
            .iter()
            .find(|d| d["id"] == doc_id)
            .cloned()
            .expect("the document is listed")
    }

    /// The stored Markdown of each page, in page order.
    async fn pages(&self, doc_id: &str) -> Vec<String> {
        let mut pages = Vec::new();
        for key in self
            .app
            .objects_under(&format!(
                "courses/{}/documents/{doc_id}/pages/",
                self.course_id
            ))
            .await
        {
            let object = self
                .app
                .s3
                .get_object()
                .bucket(&self.app.bucket)
                .key(&key)
                .send()
                .await
                .expect("get the page");
            let bytes = object.body.collect().await.expect("read the page").to_vec();
            pages.push(String::from_utf8(bytes).expect("UTF-8 Markdown"));
        }
        pages
    }
}

/// Collapses runs of white space, so a line break in the text layer does not matter.
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Runs one fixture. Returns every problem that it finds.
async fn check(e: &Env, case: &Case, fixtures: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let bytes = std::fs::read(fixtures.join(case.file)).expect("read the fixture");
    let seen_before = e.transcriber.seen.lock().expect("lock").len();
    let (doc_id, message) = e.upload(case.file, bytes).await;
    let outcome = match handle_message(&e.deps, &message, 1, BUDGET).await {
        Ok(outcome) => outcome,
        Err(retry) => return vec![format!("the worker asked for a retry: {retry:?}")],
    };
    let doc = e.document(&doc_id).await;
    match &case.expect {
        Expect::Failed(message) => {
            if outcome != Outcome::Failed((*message).into()) {
                problems.push(format!("outcome {outcome:?}, wanted Failed({message:?})"));
            }
            if doc["status"] != "failed" || doc["error"] != *message {
                problems.push(format!("document: {doc}"));
            }
            if !e.pages(&doc_id).await.is_empty() {
                problems.push("a failed document has page files".into());
            }
        }
        Expect::Ready {
            pages,
            present,
            absent,
            on_page,
            transcribed,
        } => {
            let Outcome::Ready { pages: got, chunks } = outcome else {
                return vec![format!("outcome {outcome:?}, wanted Ready")];
            };
            if got as u64 != *pages {
                problems.push(format!("outcome says {got} pages, wanted {pages}"));
            }
            if chunks == 0 {
                problems.push("no chunks".into());
            }
            if doc["status"] != "ready" || doc["pageCount"] != *pages {
                problems.push(format!("document: {doc}"));
            }
            if doc.get("warnings").is_some() || doc.get("error").is_some() {
                problems.push(format!("warnings or error on a ready document: {doc}"));
            }
            let stored = e.pages(&doc_id).await;
            if stored.len() as u64 != *pages {
                problems.push(format!("{} page files, wanted {pages}", stored.len()));
            }
            let all = flat(&stored.join("\n\n"));
            for want in *present {
                if !all.contains(want) {
                    problems.push(format!("text missing: {want:?}. Stored: {all:?}"));
                }
            }
            for unwanted in *absent {
                if all.contains(unwanted) {
                    problems.push(format!("old text is still there: {unwanted:?}"));
                }
            }
            for (page, want) in *on_page {
                let have = stored.get(page - 1).map(|p| flat(p)).unwrap_or_default();
                if !have.contains(want) {
                    problems.push(format!("page {page} lacks {want:?}. It has {have:?}"));
                }
            }
            let used = doc["transcribedPages"].as_u64().unwrap_or(0);
            if used < transcribed.0 || used > transcribed.1 {
                problems.push(format!(
                    "{used} transcribed pages, wanted {}..={}",
                    transcribed.0, transcribed.1
                ));
            }
            let seen = e.transcriber.seen.lock().expect("lock");
            let asked = &seen[seen_before..];
            if asked.len() as u64 != used {
                problems.push(format!(
                    "the transcriber saw {} pages, the record says {used}",
                    asked.len()
                ));
            }
            if asked.iter().any(|(_, jpeg)| !jpeg) {
                problems.push("a page went to the transcriber without a JPEG".into());
            }
        }
    }
    problems
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn every_pdf_fixture_has_its_expected_outcome() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf-compat");
    let e = env().await;
    let mut failures = Vec::new();
    for case in CASES {
        if let Some(reason) = case.known_bug {
            eprintln!("SKIPPED {}: known ingestion bug: {reason}", case.file);
            continue;
        }
        let problems = check(&e, case, &fixtures).await;
        if problems.is_empty() {
            eprintln!("ok      {}", case.file);
        } else {
            eprintln!("FAILED  {}", case.file);
            failures.push(format!("{}:\n  {}", case.file, problems.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[test]
fn every_fixture_file_has_a_case_and_every_case_has_a_file() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf-compat");
    let mut on_disk: Vec<String> = std::fs::read_dir(&fixtures)
        .expect("fixtures folder")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".pdf"))
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = CASES.iter().map(|c| c.file.to_owned()).collect();
    listed.sort();
    assert_eq!(on_disk, listed);
}

/// A rotated page reads the same text at every rotation, and it renders upright: the picture is
/// landscape for 90 and 270 degrees, as the page looks in a viewer.
#[test]
#[ignore = "needs OPENBOOK_PDFIUM_PATH; run make test-integration"]
fn rotated_pages_read_and_render_upright() {
    let dir = match std::env::var("OPENBOOK_PDFIUM_PATH") {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => panic!("OPENBOOK_PDFIUM_PATH is not set. Run scripts/fetch-pdfium.sh."),
    };
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf-compat");
    let plain =
        openbook_ingest::pdf::page_text(&dir, &fixtures.join("latex-plain.pdf"), 1).unwrap();
    assert!(plain.contains("Cell Biology Notes"));
    for (file, landscape) in [
        ("latex-plain.pdf", false),
        ("qpdf-rotated-90.pdf", true),
        ("qpdf-rotated-180.pdf", false),
        ("qpdf-rotated-270.pdf", true),
    ] {
        let path = fixtures.join(file);
        let text = openbook_ingest::pdf::page_text(&dir, &path, 1).unwrap();
        assert_eq!(
            text, plain,
            "{file}: the text layer differs from the unrotated page"
        );
        let jpeg = openbook_ingest::pdf::render_page(&dir, &path, 1)
            .unwrap()
            .expect("a rendering");
        let image = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(
            image.width() > image.height(),
            landscape,
            "{file}: rendered {}x{}",
            image.width(),
            image.height()
        );
        // A4 proportions, whatever the rotation.
        let (long, short) = (
            image.width().max(image.height()),
            image.width().min(image.height()),
        );
        let ratio = long as f64 / short as f64;
        assert!((ratio - 1.414).abs() < 0.02, "{file}: ratio {ratio}");
    }
}
