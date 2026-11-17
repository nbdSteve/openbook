//! Demo provisioning and the demo route against AWS Emulator, with the real ingest worker.
//!
//! Run them with `make test-integration`. They need `OPENBOOK_PDFIUM_PATH`.

#[path = "../../api/tests/support/mod.rs"]
mod support;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use axum::Router;
use axum::http::{Method, StatusCode};
use openbook_core::config::{Config, Service};
use openbook_core::demo::{self, DemoBudgets, DemoConversations, TurnReservation};
use openbook_core::files::Files;
use openbook_core::model::{ChatModel, ChatRequest, FakeChat, ModelError, ModelStream};
use openbook_core::prompt::{NOT_COVERED_NOTICE, REWRITE_SYSTEM_PROMPT};
use openbook_core::queue::IngestQueue;
use openbook_core::store::Store;
use openbook_demo::{DemoError, Loaded, Outcome, Provisioner};
use openbook_ingest::pdf;
use openbook_ingest::pipeline::Deps;
use openbook_ingest::transcribe::FakeTranscriber;
use openbook_search::bm25::Bm25Store;
use openbook_search::embed::FakeEmbedder;
use openbook_search::vectors::VectorStore;
use pdfium_render::prelude::*;
use serde_json::{Value, json};
use support::{TestApp, send_anonymous, send_to, start};

const PEER: &str = "203.0.113.9:50000";

/// A fast model that records each request and answers like the fake model.
struct Recorder {
    inner: FakeChat,
    requests: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl ChatModel for Recorder {
    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.stream(request).await
    }
}

fn make_pdf(pages: &[&[&str]]) -> Vec<u8> {
    let dir = PathBuf::from(std::env::var("OPENBOOK_PDFIUM_PATH").expect("OPENBOOK_PDFIUM_PATH"));
    let pdfium = pdf::pdfium(&dir).expect("load pdfium");
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

const GEOMETRY: &[&str] = &[
    "The Pythagorean theorem relates the three sides of a right triangle.",
    "It says that the square of the hypotenuse equals the sum of the squares of the other two sides.",
    "Builders use the theorem to check that a corner is square.",
];
const BIOLOGY: &[&str] = &[
    "Photosynthesis converts light energy into chemical energy in the chloroplasts of plant cells.",
    "Chlorophyll absorbs red and blue light and reflects green light.",
    "The process makes glucose and releases oxygen as a by-product.",
];

fn attribution(title: &str) -> Value {
    json!({
        "title": title, "authors": "A. Author", "publisher": "Test Press",
        "copyright": "2020 Test Press", "license": "CC BY 4.0",
        "licenseUrl": "https://creativecommons.org/licenses/by/4.0/",
        "sourceUrl": "https://example.org/book", "changes": "Pages selected."
    })
}

fn write_manifest(dir: &Path, biology_pages: &[&[&str]]) {
    std::fs::write(dir.join("geometry.pdf"), make_pdf(&[GEOMETRY])).unwrap();
    std::fs::write(dir.join("biology.pdf"), make_pdf(biology_pages)).unwrap();
    let manifest = json!({
        "name": "Test demo", "description": "Geometry and biology.", "tutoringStyle": "direct",
        "examples": [
            "What does the Pythagorean theorem say?", "What is photosynthesis?",
            "What is a right triangle?", "What do chloroplasts do?"
        ],
        "sources": [
            { "files": ["geometry.pdf"], "attribution": attribution("Geometry Book") },
            { "files": ["biology.pdf"], "attribution": attribution("Biology Book") }
        ]
    });
    std::fs::write(dir.join("course.json"), manifest.to_string()).unwrap();
}

const PRICED_MODELS: &str = r#"[
  {"id":"fake-a","label":"Fake Sonnet","vendor":"openbook","provider":"fake","modelId":"fake-a","residency":"au","maxOutputTokens":2000,"vision":true,"inputUsdPerMillion":3,"outputUsdPerMillion":15},
  {"id":"fake-b","label":"Fake Haiku","vendor":"openbook","provider":"fake","modelId":"fake-b","residency":"au","maxOutputTokens":2000,"vision":true,"inputUsdPerMillion":1,"outputUsdPerMillion":5}
]"#;

struct Demo {
    app: TestApp,
    dir: tempfile::TempDir,
    provisioner: Provisioner,
    recorder: Arc<Recorder>,
    vars: HashMap<String, String>,
    /// Dropping it stops the worker thread.
    _worker: tokio::sync::oneshot::Sender<()>,
}

async fn demo_env() -> Demo {
    let app = start().await;
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path(), &[BIOLOGY]);

    let mut vars = app.vars().clone();
    // The default chat model answers, the fast model rewrites follow-ups, and both have prices.
    vars.insert("OPENBOOK_MODELS".into(), PRICED_MODELS.into());
    vars.insert("OPENBOOK_DEFAULT_CHAT_MODEL".into(), "fake-a".into());
    vars.insert("OPENBOOK_FAST_MODEL".into(), "fake-b".into());
    vars.insert(
        "OPENBOOK_PDFIUM_PATH".into(),
        std::env::var("OPENBOOK_PDFIUM_PATH").expect("OPENBOOK_PDFIUM_PATH"),
    );
    let config = Config::from_vars_for(&vars, Service::Ingest).expect("ingest config");
    let store = Store::new(&app.sdk, config.table_name.clone());
    let files = Files::new(&app.sdk, config.docs_bucket.clone(), true, None);
    let deps = Arc::new(Deps {
        store: store.clone(),
        s3: app.s3.clone(),
        files: files.clone(),
        vectors: VectorStore::new(&app.sdk, config.vector_bucket.clone()),
        bm25: Bm25Store::new(app.s3.clone(), config.docs_bucket.clone(), store.clone()),
        embedder: Arc::new(FakeEmbedder),
        transcriber: Arc::new(FakeTranscriber),
        work_root: std::env::temp_dir().join(format!(
            "openbook-demo-test-{}",
            openbook_core::ids::new_id()
        )),
        config: Arc::new(config),
    });
    let queue = IngestQueue::new(&app.sdk, app.queue_url.clone());
    // The worker runs on its own thread: its future is not `Send` for `tokio::spawn`.
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    {
        let (deps, queue) = (deps.clone(), queue.clone());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("worker runtime");
            runtime.block_on(async {
                tokio::select! {
                    _ = openbook_ingest::worker::poll(&deps, &queue) => {}
                    _ = stopped => {}
                }
            });
        });
    }
    let provisioner = Provisioner {
        store,
        files,
        queue,
        model: "fake-a".into(),
        wait: Duration::from_secs(120),
        poll: Duration::from_millis(200),
    };
    vars.insert("OPENBOOK_DEMO_ENABLED".into(), "true".into());
    Demo {
        app,
        dir,
        provisioner,
        recorder: Arc::new(Recorder {
            inner: FakeChat::with_delay("fake-a", Duration::ZERO),
            requests: Mutex::new(Vec::new()),
        }),
        vars,
        _worker: stop,
    }
}

impl Demo {
    fn loaded(&self) -> Loaded {
        Loaded::load(&self.dir.path().join("course.json")).expect("load the manifest")
    }

    async fn provision(&self) -> Outcome {
        self.provisioner
            .provision(&self.loaded(), false)
            .await
            .expect("provision")
    }

    /// The state of a router with the priced catalog and the recorder, to change a config field.
    fn state(&self, extra: &[(&str, &str)]) -> openbook_api::AppState {
        let mut vars = self.vars.clone();
        for (k, v) in extra {
            vars.insert((*k).into(), (*v).into());
        }
        self.app
            .state_with_fast(&vars, ("fake-a".into(), self.recorder.clone()))
    }

    fn router(&self, extra: &[(&str, &str)]) -> Router {
        let mut vars = self.vars.clone();
        for (k, v) in extra {
            vars.insert((*k).into(), (*v).into());
        }
        self.app
            .router_with_fast(&vars, ("fake-a".into(), self.recorder.clone()))
    }

    async fn active_course(&self) -> String {
        self.provisioner
            .store
            .get_demo_course()
            .await
            .unwrap()
            .expect("an active demo course")
            .course_id
    }

    /// Every item of the table except the demo counters, as `(pk, sk) -> attributes`, and the S3 keys.
    async fn snapshot(&self) -> (BTreeMap<(String, String), String>, Vec<String>) {
        let mut items = BTreeMap::new();
        let mut start = None;
        loop {
            let out = self
                .app
                .dynamodb
                .scan()
                .table_name(&self.app.table)
                .consistent_read(true)
                .set_exclusive_start_key(start.take())
                .send()
                .await
                .unwrap();
            for item in out.items() {
                let text = |name: &str| match item.get(name) {
                    Some(AttributeValue::S(v)) => v.clone(),
                    _ => String::new(),
                };
                let sorted: BTreeMap<_, _> =
                    item.iter().map(|(k, v)| (k.clone(), canon(v))).collect();
                items.insert((text("pk"), text("sk")), format!("{sorted:?}"));
            }
            match out.last_evaluated_key {
                Some(key) if !key.is_empty() => start = Some(key),
                _ => break,
            }
        }
        (items, self.app.objects_under("").await)
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The log of every test of this file. One global subscriber, installed once: a subscriber per
/// thread makes tracing cache "nobody listens" for a call site that another thread hit first, and
/// then the lines are lost. The checks only look for text that no test may log, or for lines that
/// every run of the route writes.
fn log_capture() -> Capture {
    static CAPTURE: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
    CAPTURE
        .get_or_init(|| {
            let capture = Capture::default();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(capture.clone())
                .with_ansi(false)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
            capture
        })
        .clone()
}

/// An attribute value as text, with the keys of maps in order, so equal items give equal text.
fn canon(value: &AttributeValue) -> String {
    match value {
        AttributeValue::M(map) => {
            let sorted: BTreeMap<_, _> = map.iter().map(|(k, v)| (k.clone(), canon(v))).collect();
            format!("M{sorted:?}")
        }
        AttributeValue::L(list) => format!("L{:?}", list.iter().map(canon).collect::<Vec<_>>()),
        other => format!("{other:?}"),
    }
}

async fn ask(router: &Router, message: &str, peer: &str) -> support::Reply {
    send_anonymous(
        router,
        "/api/demo/chat",
        Some(json!({ "message": message })),
        Some(peer),
        &[],
    )
    .await
}

fn names(reply: &support::Reply) -> Vec<String> {
    reply.sse().into_iter().map(|(n, _)| n).collect()
}

fn text_of(reply: &support::Reply) -> String {
    reply
        .sse()
        .iter()
        .filter(|(n, _)| n == "delta")
        .map(|(_, d)| d["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The requests that the chat model got, without the rewrites.
fn answer_requests(env: &Demo) -> Vec<ChatRequest> {
    env.recorder
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.system != REWRITE_SYSTEM_PROMPT)
        .cloned()
        .collect()
}

async fn counter(env: &Demo, pk: &str) -> u64 {
    let sks = env.app.get_item_sk_prefixes(pk).await;
    let mut total = 0;
    for sk in sks {
        let out = env
            .app
            .dynamodb
            .get_item()
            .table_name(&env.app.table)
            .key("pk", AttributeValue::S(pk.into()))
            .key("sk", AttributeValue::S(sk))
            .consistent_read(true)
            .send()
            .await
            .unwrap();
        if let Some(AttributeValue::N(n)) = out.item().and_then(|i| i.get("count")) {
            total += n.parse::<u64>().unwrap();
        }
    }
    total
}

// ---------------------------------------------------------------------------------------------
// Provisioning
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn provisioning_runs_the_normal_ingestion_path() {
    let env = demo_env().await;
    let Outcome::Provisioned {
        course_id,
        replaced,
    } = env.provision().await
    else {
        panic!("a first run provisions");
    };
    assert_eq!(replaced, None);
    assert_eq!(env.active_course().await, course_id);

    let store = &env.provisioner.store;
    let documents = store.list_documents(&course_id).await.unwrap();
    assert_eq!(documents.len(), 2);
    for document in &documents {
        assert_eq!(format!("{:?}", document.status), "Ready");
        assert!(
            document.chunk_count.unwrap_or(0) > 0,
            "the pipeline made chunks"
        );
        assert!(document.page_count.unwrap_or(0) > 0);
    }
    let course = store.get_course(&course_id).await.unwrap().unwrap();
    assert!(course.bm25_key.is_some(), "the worker built the BM25 index");
    assert!(course.invite_code.is_empty(), "no invite code");
    assert_eq!(course.owner_id, openbook_core::demo::OWNER_ID);
    assert!(
        !env.app
            .get_item_sk_prefixes(&format!("COURSE#{course_id}"))
            .await
            .iter()
            .any(|sk| sk.starts_with("MEMBER#")),
        "no membership"
    );
    let active = store.get_demo_course().await.unwrap().unwrap();
    assert_eq!(active.attribution.len(), 2);
    assert_eq!(active.attribution[0].license, "CC BY 4.0");
    assert!(store.get_pending_demo_course().await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn provisioning_twice_changes_nothing_and_new_content_replaces_the_course() {
    let env = demo_env().await;
    let Outcome::Provisioned {
        course_id: first, ..
    } = env.provision().await
    else {
        panic!("provisions");
    };
    let before = env.snapshot().await;
    assert_eq!(
        env.provision().await,
        Outcome::UpToDate {
            course_id: first.clone()
        }
    );
    assert_eq!(
        env.snapshot().await,
        before,
        "an up-to-date run writes nothing"
    );

    // Another page in one PDF is new content.
    write_manifest(
        env.dir.path(),
        &[BIOLOGY, &["Mitochondria make most of the ATP of a cell."]],
    );
    let Outcome::Provisioned {
        course_id: second,
        replaced,
    } = env.provision().await
    else {
        panic!("new content provisions");
    };
    assert_ne!(first, second);
    assert_eq!(replaced.as_deref(), Some(first.as_str()));
    assert_eq!(env.active_course().await, second);

    // The worker deletes the old course.
    let store = &env.provisioner.store;
    let mut gone = false;
    for _ in 0..150 {
        if store.get_course(&first).await.unwrap().is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(gone, "the old demo course was deleted");
    assert!(
        env.app
            .objects_under(&format!("courses/{first}/"))
            .await
            .is_empty()
    );
    assert!(
        !env.app
            .objects_under(&format!("courses/{second}/"))
            .await
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_cut_short_run_is_cleaned_up_and_a_failed_ingest_keeps_the_old_course() {
    let env = demo_env().await;
    let store = &env.provisioner.store;
    // A run that stopped after it made the course.
    let stale = store
        .create_demo_course(
            "Stale",
            "",
            openbook_core::domain::TutoringStyle::Direct,
            "fake-a",
        )
        .await
        .unwrap();
    store.put_pending_demo_course(&stale.id).await.unwrap();
    let Outcome::Provisioned {
        course_id: first, ..
    } = env.provision().await
    else {
        panic!("provisions");
    };
    assert_ne!(first, stale.id);
    assert!(store.get_pending_demo_course().await.unwrap().is_none());
    let mut gone = false;
    for _ in 0..150 {
        if store.get_course(&stale.id).await.unwrap().is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(gone, "the stale course was deleted");

    // A PDF that cannot be read fails ingestion. The active course does not change.
    std::fs::write(env.dir.path().join("biology.pdf"), b"this is not a pdf").unwrap();
    let error = env
        .provisioner
        .provision(&env.loaded(), false)
        .await
        .unwrap_err();
    assert!(matches!(error, DemoError::IngestFailed { .. }), "{error}");
    assert_eq!(env.active_course().await, first);
    assert!(store.get_pending_demo_course().await.unwrap().is_some());
}

// ---------------------------------------------------------------------------------------------
// The route
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_demo_question_streams_an_answer_with_sources_and_credits() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let reply = ask(&router, "What does the Pythagorean theorem say?", PEER).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert!(reply.content_type.starts_with("text/event-stream"));
    let events = names(&reply);
    assert_eq!(events.first().map(String::as_str), Some("meta"));
    assert_eq!(events[1], "sources");
    assert_eq!(events.last().map(String::as_str), Some("done"));
    assert!(events[2..events.len() - 1].iter().all(|e| e == "delta"));

    let sse = reply.sse();
    let meta = &sse[0].1;
    // The default chat model answers, not the fast model.
    assert_eq!(meta["model"], "fake-a");
    assert!(text_of(&reply).contains("from the model `fake-a`"));
    assert!(demo::is_conversation_id(
        meta["conversationId"].as_str().unwrap()
    ));
    assert_eq!(meta["attribution"].as_array().unwrap().len(), 2);
    assert_eq!(meta["attribution"][0]["license"], "CC BY 4.0");
    let sources = sse[1].1["sources"].as_array().unwrap();
    assert!(sources.iter().any(|s| s["documentName"] == "geometry.pdf"));
    assert!(text_of(&reply).contains("Pythagorean"));
    assert!(sse.last().unwrap().1["stopReason"].is_string());

    // One model call, one turn, the catalog output limit, and the server prompt.
    let requests = answer_requests(&env);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].turns.len(), 1, "first turn");
    assert_eq!(
        requests[0].turns[0].text,
        "What does the Pythagorean theorem say?"
    );
    assert_eq!(requests[0].max_tokens, Some(1000), "the demo output limit");
    assert!(requests[0].system.contains("<source n=\"1\""));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_question_the_materials_do_not_cover_gets_a_real_answer_with_the_not_covered_notice() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let reply = ask(&router, "xyzzy plugh quux", PEER).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    // The same events as a covered question, without the sources event.
    let events = names(&reply);
    assert_eq!(events.first().map(String::as_str), Some("meta"));
    assert!(!events.contains(&"sources".to_owned()));
    assert_eq!(events.last().map(String::as_str), Some("done"));
    // The model was called, like in the course chat, with the normal notice.
    let requests = answer_requests(&env);
    assert_eq!(requests.len(), 1, "the model answers");
    assert!(requests[0].system.contains(NOT_COVERED_NOTICE));
    assert!(!requests[0].system.contains("<source n="));
    assert!(text_of(&reply).contains("do not cover this question"));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn input_checks_reject_bad_requests_before_anything_is_counted() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    for message in ["", "   \n ", &"x".repeat(301)] {
        let reply = ask(&router, message, PEER).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{message:?}");
        assert_eq!(reply.error_code(), "invalid_request");
    }
    // Extra fields: no history, no model, no limits from the client.
    for body in [
        json!({"message": "hi", "history": [{"role": "user", "content": "x"}]}),
        json!({"message": "hi", "messages": []}),
        json!({"message": "hi", "model": "fake-b"}),
        json!({"message": "hi", "maxTokens": 5000}),
        json!({"message": "hi", "conversationId": 5}),
        json!({"message": 5}),
        json!({}),
    ] {
        let reply = send_anonymous(
            &router,
            "/api/demo/chat",
            Some(body.clone()),
            Some(PEER),
            &[],
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(reply.error_code(), "invalid_request");
    }
    let reply = send_anonymous(&router, "/api/demo/chat", None, Some(PEER), &[]).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        send_to(&router, Method::GET, "/api/demo/chat", None, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.app.items_under("DEMO#GLOBAL").await,
        0,
        "nothing was counted"
    );
    assert_eq!(env.app.items_under("DEMO#SALT").await, 0);
    assert!(env.recorder.requests.lock().unwrap().is_empty());
    // 300 characters is fine.
    let ok = ask(&router, &format!("Pythagorean {}", "a".repeat(288)), PEER).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_kill_switch_and_a_missing_course_and_a_missing_address() {
    let env = demo_env().await;
    // Not provisioned yet: a clear error, not a crash, and nothing counted.
    let router = env.router(&[]);
    let reply = ask(&router, "What is photosynthesis?", PEER).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.error_code(), "demo_unavailable");
    assert_eq!(env.app.items_under("DEMO#GLOBAL").await, 0);

    env.provision().await;
    // Off: 404, even for a body that is not valid.
    let off = env.router(&[("OPENBOOK_DEMO_ENABLED", "false")]);
    let reply = ask(&off, "What is photosynthesis?", PEER).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.error_code(), "demo_unavailable");
    let reply = send_anonymous(
        &off,
        "/api/demo/chat",
        Some(json!({"nope": 1})),
        Some(PEER),
        &[],
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(env.app.items_under("DEMO#GLOBAL").await, 0);

    // No usable client address (no socket address here): fail closed. A header is not trusted
    // outside Lambda.
    let reply = send_anonymous(
        &router,
        "/api/demo/chat",
        Some(json!({"message": "What is photosynthesis?"})),
        None,
        &[("cloudfront-viewer-address", "198.51.100.1:4000")],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.error_code(), "demo_unavailable");
    assert_eq!(env.app.items_under("DEMO#GLOBAL").await, 0);
    assert!(env.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_visitor_limits_return_demo_limit() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "2"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY", "3"),
    ]);
    for _ in 0..2 {
        assert_eq!(
            ask(&router, "What is photosynthesis?", PEER).await.status,
            StatusCode::OK
        );
    }
    let limited = ask(&router, "What is photosynthesis?", PEER).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.error_code(), "demo_rate_limited");
    assert!(
        limited.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("demo limit")
    );
    let retry = limited.json()["error"]["retryAfterSeconds"]
        .as_u64()
        .unwrap();
    assert!(
        (1..=600).contains(&retry),
        "retry within the window: {retry}"
    );
    assert_eq!(
        answer_requests(&env).len(),
        2,
        "a refused question makes no model call"
    );
    // Another visitor is fine, and the same /64 is the same visitor.
    assert_eq!(
        ask(&router, "What is photosynthesis?", "198.51.100.4:1")
            .await
            .status,
        StatusCode::OK
    );
    for peer in ["[2001:db8:1:2::1]:1", "[2001:db8:1:2:ffff::9]:1"] {
        let status = ask(&router, "What is photosynthesis?", peer).await.status;
        if peer.contains("::1]") {
            assert_eq!(status, StatusCode::OK);
        } else {
            assert_eq!(status, StatusCode::OK, "second question of the /64");
        }
    }
    assert_eq!(
        ask(
            &router,
            "What is photosynthesis?",
            "[2001:db8:1:2:aaaa::7]:1"
        )
        .await
        .status,
        StatusCode::TOO_MANY_REQUESTS,
        "the third question of the same /64 in the window"
    );

    // The daily limit (the window limit is out of the way).
    let router = env.router(&[
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "100"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY", "3"),
    ]);
    for _ in 0..3 {
        assert_eq!(
            ask(&router, "What is photosynthesis?", "192.0.2.77:1")
                .await
                .status,
            StatusCode::OK
        );
    }
    let limited = ask(&router, "What is photosynthesis?", "192.0.2.77:1").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.error_code(), "demo_rate_limited");
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_daily_budget_refunds_what_a_turn_did_not_spend_and_then_returns_demo_busy() {
    let env = demo_env().await;
    env.provision().await;
    // Room for a few turns: one worst-case reservation is about 0.05 US dollars, a real turn about 0.004.
    let router = env.router(&[
        ("OPENBOOK_DEMO_BUDGET_USD_PER_DAY", "0.06"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "50"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY", "50"),
    ]);
    let first = ask(&router, "What is photosynthesis?", "192.0.2.2:1").await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    // The reservation went back: the counter holds the real cost only.
    let spent = counter(&env, "DEMO#GLOBAL").await;
    assert!(
        (1..20_000).contains(&spent),
        "settled to the real cost, not the reservation: {spent}"
    );
    let reserved_worst_case = 40_000;
    assert!(spent < reserved_worst_case);
    // Without the refund, the second turn would not fit in 0.06 US dollars.
    let second = ask(
        &router,
        "What does the Pythagorean theorem say?",
        "192.0.2.3:1",
    )
    .await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);

    // Keep going until the budget cannot hold a worst-case turn.
    let mut busy = None;
    for visitor in 4..20 {
        let reply = ask(
            &router,
            "What is photosynthesis?",
            &format!("192.0.2.{visitor}:1"),
        )
        .await;
        if reply.status == StatusCode::SERVICE_UNAVAILABLE {
            busy = Some(reply);
            break;
        }
        assert_eq!(reply.status, StatusCode::OK);
    }
    let busy = busy.expect("the budget ran out");
    assert_eq!(busy.error_code(), "demo_busy");
    assert!(
        busy.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("busy")
    );
    // The refused visitor did not lose a question, and no model was called for the refused turn.
    let before = answer_requests(&env).len();
    let again = ask(&router, "What is photosynthesis?", "192.0.2.99:1").await;
    assert_eq!(again.error_code(), "demo_busy");
    assert_eq!(answer_requests(&env).len(), before);
    assert_eq!(counter(&env, "DEMO#V#").await, 0, "exact partitions only");
    // A bigger budget lets the same visitor ask at once.
    let roomy = env.router(&[
        ("OPENBOOK_DEMO_BUDGET_USD_PER_DAY", "2"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "1"),
    ]);
    assert_eq!(
        ask(&roomy, "What is photosynthesis?", "192.0.2.99:1")
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn budget_reservations_in_the_table_are_atomic_idempotent_and_start_again_each_utc_day() {
    let env = demo_env().await;
    let store = env.provisioner.store.clone();
    let limits = openbook_core::config::DemoConfig {
        enabled: true,
        visitor_per_window: 10,
        visitor_per_day: 40,
        budget_micro_usd_per_day: 1000,
        visitor_budget_micro_usd_per_day: 800,
        max_output_tokens: 1000,
    };
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-17T23:59:58Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    // Eight visitors at once try to reserve 600 of 1000. Only one can fit.
    let visitors: Vec<String> = (0..8).map(|i| format!("v{i}")).collect();
    let attempts = futures::future::join_all(
        visitors
            .iter()
            .map(|v| demo::reserve_budget(&store, v, now, 600, &limits)),
    )
    .await;
    let mut held = Vec::new();
    for attempt in attempts {
        match attempt.unwrap() {
            Ok(reservation) => held.push(reservation),
            Err(hit) => assert_eq!(hit, demo::BudgetHit::Global),
        }
    }
    assert_eq!(held.len(), 1, "no overspend");
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 600);
    // One visitor cap of 800: the same visitor cannot reserve 500 more, though the day has 400 left.
    let holder = held.remove(0);
    assert_eq!(
        demo::reserve_budget(&store, "w", now, 500, &limits)
            .await
            .unwrap()
            .unwrap_err(),
        demo::BudgetHit::Global
    );
    // Settling with the real cost gives the rest back, once.
    holder.settle(&store, 100).await;
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 100);

    // The same turn settled twice gives back only once (a retry, or two tasks of one turn).
    let turn_id = openbook_core::ids::new_id().to_lowercase();
    let turn = TurnReservation {
        turn_id: turn_id.clone(),
        global: demo::budget_counter(now),
        visitor: demo::visitor_budget_counter("z", now),
        amount: 300,
        global_limit: 1000,
        visitor_limit: 800,
        item: openbook_core::demo::Counter {
            pk: format!("DEMO#RES#{}", turn_id),
            sk: "R".into(),
            expires_at: chrono::Utc::now().timestamp() + 3600,
        },
    };
    assert_eq!(store.reserve_turn(&turn).await.unwrap(), Ok(()));
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 400);
    assert!(store.settle_turn(&turn, 250).await.unwrap());
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 150);
    // A second settlement (a retry with the same token, or another task of the same turn, with the
    // same amount or another one) never gives back twice: the item of the turn is gone.
    let _ = store.settle_turn(&turn, 250).await;
    let _ = store.settle_turn(&turn, 100).await;
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 150, "no double refund");
    assert_eq!(counter(&env, "DEMO#V#z").await, 50);
    assert!(env.app.get_item_sk_prefixes(&turn.item.pk).await.is_empty());

    // A refund that the counter cannot hold is an error that changes nothing (no fallback).
    use openbook_core::demo::DemoCounters;
    assert!(
        store
            .release(&demo::budget_counter(now), 5000)
            .await
            .is_err()
    );
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 150);

    // Two seconds later it is the next UTC day, and the budget is whole again.
    let next = now + chrono::Duration::seconds(2);
    assert!(
        demo::reserve_budget(&store, "w", next, 800, &limits)
            .await
            .unwrap()
            .is_ok()
    );
    assert_eq!(
        env.app.get_item_sk_prefixes("DEMO#GLOBAL").await,
        vec!["D#2026-11-17".to_owned(), "D#2026-11-18".to_owned()]
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_model_that_fails_to_start_gives_the_question_and_the_money_back() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let reply = ask(
        &router,
        "What does the Pythagorean theorem say? [fake:fail-start]",
        PEER,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let last = reply.sse().pop().unwrap();
    assert_eq!(last.0, "error");
    assert_eq!(last.1["code"], "model_unavailable");
    // Only the query embedding cost anything, and the visitor keeps the question.
    assert!(counter(&env, "DEMO#GLOBAL").await < 100);
    let partitions = env.app.get_item_sk_prefixes("DEMO#GLOBAL").await;
    assert_eq!(partitions.len(), 1);
    // A turn that failed leaves no conversation behind.
    let (items, _) = env.snapshot().await;
    assert!(items.keys().all(|(pk, _)| !pk.starts_with("DEMO#CONV#")));
}

// ---------------------------------------------------------------------------------------------
// Isolation and privacy
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_demo_route_writes_only_demo_items_and_no_s3_object() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "3")]);
    let (before, objects) = env.snapshot().await;
    // Answers, a refusal by the limit, and bad input.
    assert_eq!(
        ask(&router, "What does the Pythagorean theorem say?", PEER)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        ask(&router, "xyzzy plugh quux", PEER).await.status,
        StatusCode::OK
    );
    assert_eq!(
        ask(&router, "What is photosynthesis?", PEER).await.status,
        StatusCode::OK
    );
    assert_eq!(
        ask(&router, "What is photosynthesis?", PEER).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(ask(&router, "", PEER).await.status, StatusCode::BAD_REQUEST);
    let (after, objects_after) = env.snapshot().await;

    assert_eq!(objects, objects_after, "no S3 object changed");
    for (key, value) in &before {
        assert_eq!(after.get(key), Some(value), "{key:?} changed");
    }
    for key in after.keys().filter(|k| !before.contains_key(*k)) {
        assert!(
            key.0.starts_with("DEMO#V#")
                || key.0.starts_with("DEMO#CONV#")
                || key.0 == "DEMO#GLOBAL"
                || key.0 == "DEMO#SALT",
            "the demo route wrote {key:?}"
        );
    }
    // No user, conversation, question, or usage item exists for anyone.
    assert!(
        after
            .keys()
            .all(|(pk, _)| !pk.starts_with("CONV#") && !pk.starts_with("USER#"))
    );
    assert!(
        after
            .keys()
            .all(|(_, sk)| !sk.starts_with("Q#") && !sk.starts_with("USAGE#"))
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_follow_up_uses_the_conversation_that_the_server_kept() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let first = ask(&router, "What does the Pythagorean theorem say?", PEER).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text);
    let id = first.sse()[0].1["conversationId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(demo::is_conversation_id(&id));
    assert_eq!(id.len(), 48, "192 bits");

    let second = send_anonymous(
        &router,
        "/api/demo/chat",
        Some(json!({"message": "And how do builders use it?", "conversationId": id})),
        Some(PEER),
        &[],
    )
    .await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    assert_eq!(
        second.sse()[0].1["conversationId"],
        id.as_str(),
        "the same conversation"
    );
    // The server sent the earlier turn to the model: question, answer, question.
    let answers = answer_requests(&env);
    assert_eq!(answers.len(), 2);
    let turns = &answers[1].turns;
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].text, "What does the Pythagorean theorem say?");
    assert!(turns[1].text.contains("Pythagorean"));
    assert_eq!(turns[2].text, "And how do builders use it?");

    // The conversation is stored for 24 hours, with two turns.
    let stored = env.provisioner.store.load(&id).await.unwrap().unwrap();
    assert_eq!(stored.turns, 2);
    assert_eq!(stored.messages.len(), 4);
    let in_a_day = chrono_now() + 24 * 3600;
    assert!(
        (stored.expires_at - in_a_day).abs() < 120,
        "{}",
        stored.expires_at
    );
    // It has no user, no GSI key, and every message expires with it.
    let pk = format!("DEMO#CONV#{id}");
    for item in env
        .app
        .dynamodb
        .query()
        .table_name(&env.app.table)
        .key_condition_expression("pk = :pk")
        .expression_attribute_values(":pk", AttributeValue::S(pk))
        .send()
        .await
        .unwrap()
        .items()
    {
        assert!(!item.contains_key("gsi1pk") && !item.contains_key("gsi1sk"));
        assert!(!item.contains_key("userId"));
        assert_eq!(
            item.get("ttl"),
            Some(&AttributeValue::N(stored.expires_at.to_string()))
        );
    }
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn an_unknown_forged_or_expired_conversation_starts_fresh() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let first = ask(&router, "What does the Pythagorean theorem say?", PEER).await;
    let id = first.sse()[0].1["conversationId"]
        .as_str()
        .unwrap()
        .to_owned();

    let with = |conversation_id: String| {
        let router = router.clone();
        async move {
            send_anonymous(
                &router,
                "/api/demo/chat",
                Some(json!({"message": "What is photosynthesis?", "conversationId": conversation_id})),
                Some(PEER),
                &[],
            )
            .await
        }
    };
    // A guessed ID, a wrong shape, and an upper-case copy never reach the stored conversation.
    for forged in [
        "0".repeat(48),
        id[..47].to_owned(),
        id.to_uppercase(),
        format!("{id}0"),
        "../../../etc/passwd".to_owned(),
    ] {
        let reply = with(forged.clone()).await;
        assert_eq!(reply.status, StatusCode::OK, "{forged}: {}", reply.text);
        let new_id = reply.sse()[0].1["conversationId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(new_id, id);
        assert!(demo::is_conversation_id(&new_id));
    }
    for request in answer_requests(&env).iter().skip(1) {
        assert_eq!(request.turns.len(), 1, "no history from a forged ID");
    }

    // After the TTL the conversation is gone, even if DynamoDB still holds the item.
    env.app
        .dynamodb
        .update_item()
        .table_name(&env.app.table)
        .key("pk", AttributeValue::S(format!("DEMO#CONV#{id}")))
        .key("sk", AttributeValue::S("META".into()))
        .update_expression("SET #ttl = :past")
        .expression_attribute_names("#ttl", "ttl")
        .expression_attribute_values(":past", AttributeValue::N((chrono_now() - 10).to_string()))
        .send()
        .await
        .unwrap();
    let reply = with(id.clone()).await;
    assert_ne!(reply.sse()[0].1["conversationId"], id.as_str());
    let last = answer_requests(&env).pop().unwrap();
    assert_eq!(
        last.turns.len(),
        1,
        "an expired conversation has no history"
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_conversation_has_at_most_ten_turns() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "50"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY", "50"),
    ]);
    let first = ask(&router, "What is photosynthesis?", PEER).await;
    let id = first.sse()[0].1["conversationId"]
        .as_str()
        .unwrap()
        .to_owned();
    for turn in 2..=10 {
        let reply = send_anonymous(
            &router,
            "/api/demo/chat",
            Some(json!({"message": format!("Tell me more, part {turn}"), "conversationId": id})),
            Some(PEER),
            &[],
        )
        .await;
        assert_eq!(
            reply.sse()[0].1["conversationId"],
            id.as_str(),
            "turn {turn}"
        );
    }
    assert_eq!(
        env.provisioner
            .store
            .load(&id)
            .await
            .unwrap()
            .unwrap()
            .turns,
        10
    );
    // The eleventh question starts a fresh conversation with a new ID and no history.
    let eleventh = send_anonymous(
        &router,
        "/api/demo/chat",
        Some(json!({"message": "One more?", "conversationId": id})),
        Some(PEER),
        &[],
    )
    .await;
    assert_ne!(eleventh.sse()[0].1["conversationId"], id.as_str());
    assert_eq!(answer_requests(&env).last().unwrap().turns.len(), 1);
    assert_eq!(
        env.provisioner
            .store
            .load(&id)
            .await
            .unwrap()
            .unwrap()
            .turns,
        10
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn demo_conversations_are_in_no_user_route() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let first = ask(&router, "What does the Pythagorean theorem say?", PEER).await;
    let id = first.sse()[0].1["conversationId"]
        .as_str()
        .unwrap()
        .to_owned();
    let teacher = env.app.user("teacher").await;
    let own = env.app.course(&teacher, "Own course").await;
    let own_id = own["id"].as_str().unwrap().to_owned();
    // The ID is no ULID, and it is not stored under a user, so no conversation route finds it.
    for path in [
        format!("/api/conversations/{id}"),
        format!("/api/conversations/{}", openbook_core::ids::new_id()),
    ] {
        assert_eq!(
            send_to(&router, Method::GET, &path, Some(&teacher.token), None)
                .await
                .status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send_to(&router, Method::DELETE, &path, Some(&teacher.token), None)
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    let list = env
        .app
        .get(&format!("/api/courses/{own_id}/conversations"), &teacher)
        .await
        .json();
    assert_eq!(list["conversations"].as_array().unwrap().len(), 0);
    // No GSI query by user finds it either.
    let (items, _) = env.snapshot().await;
    assert!(items.keys().any(|(pk, _)| pk == &format!("DEMO#CONV#{id}")));
    assert!(
        items
            .iter()
            .filter(|((pk, _), _)| pk.starts_with("DEMO#CONV#"))
            .all(|(_, v)| !v.contains("gsi1pk"))
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_salt_is_daily_and_no_address_or_hash_is_stored_or_logged() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);

    let capture = log_capture();
    assert_eq!(
        ask(&router, "What does the Pythagorean theorem say?", PEER)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        ask(&router, "xyzzy plugh quux", PEER).await.status,
        StatusCode::OK
    );
    // Wait for the spawned turn to log its line.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let salts = env.app.get_item_sk_prefixes("DEMO#SALT").await;
    assert_eq!(salts.len(), 1, "one salt for today: {salts:?}");
    let out = env
        .app
        .dynamodb
        .get_item()
        .table_name(&env.app.table)
        .key("pk", AttributeValue::S("DEMO#SALT".into()))
        .key("sk", AttributeValue::S(salts[0].clone()))
        .send()
        .await
        .unwrap();
    let item = out.item().unwrap();
    match item.get("salt") {
        Some(AttributeValue::B(blob)) => assert_eq!(blob.as_ref().len(), 32),
        other => panic!("a 32 byte salt, got {other:?}"),
    }
    let ttl: i64 = match item.get("ttl") {
        Some(AttributeValue::N(n)) => n.parse().unwrap(),
        other => panic!("a ttl, got {other:?}"),
    };
    let in_two_days = chrono_now() + 2 * 24 * 3600;
    assert!(
        (ttl - in_two_days).abs() < 120,
        "the salt expires in 2 days"
    );

    // The visitor hash is in the key of the counter. Neither it nor the address is anywhere else.
    let (all, _) = env.snapshot().await;
    let hash = all
        .keys()
        .find_map(|(pk, _)| pk.strip_prefix("DEMO#V#").map(str::to_owned))
        .expect("a visitor counter");
    assert_eq!(hash.len(), 32);
    for (key, value) in &all {
        assert!(!value.contains("203.0.113.9"), "address stored in {key:?}");
        assert!(
            key.0 == format!("DEMO#V#{hash}") || !value.contains(&hash),
            "hash outside its counter: {key:?}"
        );
    }
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("demo_turn"), "{logs}");
    assert!(
        logs.contains("outcome=done") || logs.contains("outcome=\"done\""),
        "{logs}"
    );
    assert!(
        logs.contains("duration_ms=")
            && logs.contains("input_tokens=")
            && logs.contains("cost_micro_usd="),
        "{logs}"
    );
    assert!(!logs.contains("203.0.113"), "the address is logged: {logs}");
    assert!(!logs.contains(&hash), "the hash is logged: {logs}");
    assert!(
        !logs.contains("Pythagorean theorem say"),
        "the question is logged"
    );
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn nobody_can_reach_the_demo_course_through_the_normal_routes() {
    let env = demo_env().await;
    env.provision().await;
    let course = env.active_course().await;
    let documents = env.provisioner.store.list_documents(&course).await.unwrap();
    let document = documents[0].id.clone();
    let router = env.router(&[]);
    // Teachers and students of other courses are non-members: 404 on every course route.
    let teacher = env.app.user("teacher").await;
    let student = env.app.user("student").await;
    let own = env.app.course(&teacher, "Own course").await;
    let own_id = own["id"].as_str().unwrap().to_owned();
    for user in [&teacher, &student] {
        let routes: Vec<(Method, String, Option<Value>)> = vec![
            (Method::GET, format!("/api/courses/{course}"), None),
            (
                Method::PATCH,
                format!("/api/courses/{course}"),
                Some(json!({"name": "Mine"})),
            ),
            (Method::DELETE, format!("/api/courses/{course}"), None),
            (
                Method::POST,
                format!("/api/courses/{course}/invite-code"),
                None,
            ),
            (Method::GET, format!("/api/courses/{course}/members"), None),
            (
                Method::GET,
                format!("/api/courses/{course}/documents"),
                None,
            ),
            (
                Method::POST,
                format!("/api/courses/{course}/documents"),
                Some(
                    json!({"filename": "a.pdf", "sizeBytes": 10, "contentType": "application/pdf"}),
                ),
            ),
            (
                Method::POST,
                format!("/api/courses/{course}/documents/{document}/complete"),
                None,
            ),
            (
                Method::DELETE,
                format!("/api/courses/{course}/documents/{document}"),
                None,
            ),
            (
                Method::GET,
                format!("/api/courses/{course}/conversations"),
                None,
            ),
            (
                Method::POST,
                format!("/api/courses/{course}/chat"),
                Some(json!({"message": "hi"})),
            ),
            (Method::GET, format!("/api/courses/{course}/insights"), None),
            (
                Method::DELETE,
                format!("/api/courses/{course}/members/{}", user.id),
                None,
            ),
        ];
        for (method, path, body) in routes {
            let reply = send_to(&router, method.clone(), &path, Some(&user.token), body).await;
            assert_eq!(
                reply.status,
                StatusCode::NOT_FOUND,
                "{method} {path}: {}",
                reply.text
            );
            assert_eq!(reply.error_code(), "not_found");
        }
        // The demo course is in nobody's list, and there is no invite code to join with.
        let list = env.app.get("/api/courses", user).await.json();
        assert!(
            list["courses"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["id"] != course.as_str()),
            "{list}"
        );
        for code in ["", "AAAAAAAA", "DEMO", "demo"] {
            let reply = send_to(
                &router,
                Method::POST,
                "/api/join",
                Some(&user.token),
                Some(json!({"inviteCode": code})),
            )
            .await;
            assert!(
                matches!(
                    reply.status,
                    StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
                ),
                "join with {code:?}: {}",
                reply.text
            );
        }
    }
    // The owner of the demo course is no Cognito user, so a token cannot carry its ID.
    assert_eq!(openbook_core::demo::OWNER_ID, "demo");
    let reply = send_to(
        &router,
        Method::GET,
        &format!("/api/courses/{own_id}"),
        Some(&teacher.token),
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "normal courses still work");
    // A normal chat in a normal course still reads only its own course.
    assert!(
        env.provisioner
            .store
            .get_membership(&course, "demo")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn nothing_else_became_anonymous() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let id = openbook_core::ids::new_id();
    let other = openbook_core::ids::new_id();
    let routes = [
        (Method::GET, "/api/me".to_owned()),
        (Method::GET, "/api/models".to_owned()),
        (Method::GET, "/api/courses".to_owned()),
        (Method::POST, "/api/courses".to_owned()),
        (Method::POST, "/api/join".to_owned()),
        (Method::GET, format!("/api/courses/{id}")),
        (Method::PATCH, format!("/api/courses/{id}")),
        (Method::DELETE, format!("/api/courses/{id}")),
        (Method::POST, format!("/api/courses/{id}/invite-code")),
        (Method::GET, format!("/api/courses/{id}/members")),
        (Method::DELETE, format!("/api/courses/{id}/members/{other}")),
        (Method::GET, format!("/api/courses/{id}/documents")),
        (Method::POST, format!("/api/courses/{id}/documents")),
        (
            Method::POST,
            format!("/api/courses/{id}/documents/{other}/complete"),
        ),
        (
            Method::DELETE,
            format!("/api/courses/{id}/documents/{other}"),
        ),
        (Method::GET, format!("/api/courses/{id}/conversations")),
        (Method::GET, format!("/api/conversations/{other}")),
        (Method::DELETE, format!("/api/conversations/{other}")),
        (Method::POST, format!("/api/courses/{id}/chat")),
        (Method::GET, format!("/api/courses/{id}/insights")),
    ];
    for (method, path) in routes {
        let reply = send_to(
            &router,
            method.clone(),
            &path,
            None,
            Some(json!({"message": "hi", "name": "x", "inviteCode": "x"})),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} must need sign-in"
        );
        assert_eq!(reply.error_code(), "unauthenticated");
    }
    // A demo-looking course ID with the demo course's real ID still needs sign-in.
    let course = env.active_course().await;
    let reply = send_to(
        &router,
        Method::POST,
        &format!("/api/courses/{course}/chat"),
        None,
        Some(json!({"message": "hi"})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn one_visitor_cannot_spend_the_day_and_gets_demo_rate_limited() {
    let env = demo_env().await;
    env.provision().await;
    // The cap of one visitor is 0.06 USD: about one worst-case reservation and a few real turns.
    let router = env.router(&[
        ("OPENBOOK_DEMO_VISITOR_BUDGET_USD_PER_DAY", "0.06"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "50"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY", "50"),
    ]);
    let mut refused = None;
    for _ in 0..40 {
        let reply = ask(&router, "What is photosynthesis?", "192.0.2.50:1").await;
        if reply.status == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(reply);
            break;
        }
        assert_eq!(reply.status, StatusCode::OK);
    }
    let refused = refused.expect("the visitor cap stopped the visitor before 40 questions");
    assert_eq!(refused.error_code(), "demo_rate_limited");
    assert!(
        refused.json()["error"]["retryAfterSeconds"]
            .as_u64()
            .unwrap()
            >= 1
    );
    // The day still has room for everybody else.
    let other = ask(&router, "What is photosynthesis?", "192.0.2.51:1").await;
    assert_eq!(other.status, StatusCode::OK, "{}", other.text);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_stream_that_fails_keeps_the_whole_reservation() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let reply = ask(
        &router,
        "What does the Pythagorean theorem say? [fake:fail-after-delta]",
        PEER,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.sse().pop().unwrap().0, "error");
    // The usage of a failed stream is not known, so the money is not given back: the counter holds the
    // reservation, which is far above a real turn (about 4,000 millionths of a dollar).
    let kept = counter(&env, "DEMO#GLOBAL").await;
    assert!(kept > 20_000, "the whole reservation is kept: {kept}");
    // The item of the turn is gone, so the settlement is final.
    let (items, _) = env.snapshot().await;
    assert!(items.keys().all(|(pk, _)| !pk.starts_with("DEMO#RES#")));
    // A failed turn is not saved as a conversation.
    assert!(items.keys().all(|(pk, _)| !pk.starts_with("DEMO#CONV#")));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_client_that_leaves_still_settles_the_turn() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let request = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/api/demo/chat")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({"message": "What does the Pythagorean theorem say?"}).to_string(),
        ))
        .unwrap();
    let mut request = request;
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        PEER.parse::<std::net::SocketAddr>().unwrap(),
    ));
    let response = tower::ServiceExt::oneshot(router.clone(), request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // The client leaves: the body is dropped before one event is read.
    drop(response);
    let mut settled = false;
    for _ in 0..100 {
        let (items, _) = env.snapshot().await;
        let open = items.keys().any(|(pk, _)| pk.starts_with("DEMO#RES#"));
        let spent = counter(&env, "DEMO#GLOBAL").await;
        if !open && (1..20_000).contains(&spent) {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        settled,
        "the turn settled to its real cost after the client left"
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn in_lambda_the_header_is_the_visitor_and_a_missing_header_fails_closed() {
    let _ = log_capture();
    let env = demo_env().await;
    env.provision().await;
    // The same state with `in_lambda` on: the address is the CloudFront header and nothing else.
    let router = {
        let mut state = env.state(&[]);
        let mut config = (*state.config).clone();
        config.in_lambda = true;
        state.config = Arc::new(config);
        openbook_api::build_router(state)
    };
    let body = json!({"message": "What does the Pythagorean theorem say?"});
    // No header: 503, nothing counted, even with a socket address.
    let reply = send_anonymous(
        &router,
        "/api/demo/chat",
        Some(body.clone()),
        Some(PEER),
        &[],
    )
    .await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.error_code(), "demo_unavailable");
    // A header that is not an address: the same.
    for bad in ["", "unknown", "not-an-address:80"] {
        let reply = send_anonymous(
            &router,
            "/api/demo/chat",
            Some(body.clone()),
            Some(PEER),
            &[("cloudfront-viewer-address", bad)],
        )
        .await;
        assert_eq!(reply.error_code(), "demo_unavailable", "{bad:?}");
    }
    assert_eq!(env.app.items_under("DEMO#GLOBAL").await, 0);
    // A good header works without a socket address, and two addresses are two visitors.
    let one = send_anonymous(
        &router,
        "/api/demo/chat",
        Some(body.clone()),
        None,
        &[("cloudfront-viewer-address", "198.51.100.1:4000")],
    )
    .await;
    let logs = String::from_utf8(log_capture().0.lock().unwrap().clone()).unwrap();
    assert_eq!(one.status, StatusCode::OK, "{} {logs}", one.text);
    assert!(
        one.error_code_or_none().is_none(),
        "answered: {}",
        one.text.chars().take(80).collect::<String>()
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_api_modes_mount_the_demo_alone_or_everything_but_the_demo() {
    let env = demo_env().await;
    env.provision().await;
    let with_mode = |mode: openbook_core::config::ApiMode| {
        let mut state = env.state(&[]);
        let mut config = (*state.config).clone();
        config.api_mode = mode;
        state.config = Arc::new(config);
        openbook_api::build_router(state)
    };
    let teacher = env.app.user("teacher").await;
    let body = || Some(json!({"message": "What is photosynthesis?"}));
    // The main function does not serve the demo: the route does not exist there.
    let full = with_mode(openbook_core::config::ApiMode::Full);
    let reply = send_anonymous(&full, "/api/demo/chat", body(), Some(PEER), &[]).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.error_code(), "not_found");
    assert_eq!(
        send_to(&full, Method::GET, "/api/me", Some(&teacher.token), None)
            .await
            .status,
        StatusCode::OK
    );
    // The demo function serves health and the demo, and no other route, signed in or not.
    let demo = with_mode(openbook_core::config::ApiMode::Demo);
    assert_eq!(
        send_to(&demo, Method::GET, "/api/health", None, None)
            .await
            .status,
        StatusCode::OK
    );
    for path in ["/api/me", "/api/courses", "/api/models"] {
        assert_eq!(
            send_to(&demo, Method::GET, path, Some(&teacher.token), None)
                .await
                .status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    let reply = send_anonymous(&demo, "/api/demo/chat", body(), Some(PEER), &[]).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn provisioning_never_retires_the_active_course_that_a_pending_note_names() {
    let env = demo_env().await;
    let Outcome::Provisioned { course_id, .. } = env.provision().await else {
        panic!("provisions");
    };
    // A run that switched the demo and stopped before it cleared its note.
    let store = &env.provisioner.store;
    store.put_pending_demo_course(&course_id).await.unwrap();
    assert_eq!(
        env.provision().await,
        Outcome::UpToDate {
            course_id: course_id.clone()
        }
    );
    assert!(store.get_pending_demo_course().await.unwrap().is_none());
    assert!(
        store.course_is_live(&course_id).await.unwrap(),
        "the active course is not retired"
    );
    // The same with new content: the note is cleared and the course stays until the switch.
    store.put_pending_demo_course(&course_id).await.unwrap();
    write_manifest(
        env.dir.path(),
        &[BIOLOGY, &["Mitochondria make most of the ATP of a cell."]],
    );
    let Outcome::Provisioned { replaced, .. } = env.provision().await else {
        panic!("new content provisions");
    };
    assert_eq!(replaced.as_deref(), Some(course_id.as_str()));
    assert!(store.get_pending_demo_course().await.unwrap().is_none());
}
