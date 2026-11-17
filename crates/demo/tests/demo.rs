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
use openbook_core::files::Files;
use openbook_core::model::{ChatModel, ChatRequest, FakeChat, ModelError, ModelStream};
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
        "coverage": "geometry and photosynthesis",
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

    fn calls(&self) -> usize {
        self.recorder.requests.lock().unwrap().len()
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

async fn counter(env: &Demo, pk: &str) -> u32 {
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
            total += n.parse::<u32>().unwrap();
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
    assert_eq!(active.examples.len(), 4);
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
    assert_eq!(meta["model"], "fake-a");
    assert_eq!(meta["attribution"].as_array().unwrap().len(), 2);
    assert_eq!(meta["attribution"][0]["license"], "CC BY 4.0");
    assert!(
        meta.get("conversationId").is_none(),
        "no conversation exists"
    );
    let sources = sse[1].1["sources"].as_array().unwrap();
    assert!(sources.iter().any(|s| s["documentName"] == "geometry.pdf"));
    assert!(text_of(&reply).contains("Pythagorean"));
    assert!(sse.last().unwrap().1["stopReason"].is_string());

    // One model call, one turn, the fixed output limit, and the server prompt.
    let requests = env.recorder.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].turns.len(), 1, "single turn");
    assert_eq!(
        requests[0].turns[0].text,
        "What does the Pythagorean theorem say?"
    );
    assert_eq!(requests[0].max_tokens, Some(700));
    assert!(requests[0].system.contains("<source n=\"1\""));
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_question_the_materials_do_not_cover_gets_the_fixed_reply_and_no_model_call() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);
    let reply = ask(&router, "xyzzy plugh quux", PEER).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert_eq!(names(&reply).first().map(String::as_str), Some("meta"));
    assert!(!names(&reply).contains(&"sources".to_owned()));
    let text = text_of(&reply);
    assert!(
        text.contains("This demo covers geometry and photosynthesis."),
        "{text}"
    );
    assert!(text.contains("- What does the Pythagorean theorem say?"));
    assert!(text.contains("- What is photosynthesis?"));
    let done = reply.sse().pop().unwrap();
    assert_eq!(done.0, "done");
    assert_eq!(done.1["stopReason"], "not_covered");
    assert_eq!(env.calls(), 0, "no model call");
    // The global model-call counter went back, and the visitor counters stay.
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 0);
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
        json!({"message": "hi", "model": "fake-b"}),
        json!({"message": "hi", "maxTokens": 5000}),
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
    assert_eq!(env.calls(), 0);
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
    assert_eq!(env.calls(), 0);
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
    assert_eq!(env.calls(), 2, "a refused question makes no model call");
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
async fn the_global_limit_counts_model_calls_only_and_returns_demo_busy() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[
        ("OPENBOOK_DEMO_LIMIT_GLOBAL_PER_DAY", "2"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "50"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY", "50"),
    ]);
    // Fixed replies use no model call, so they never fill the global limit.
    for _ in 0..4 {
        let reply = ask(&router, "xyzzy plugh quux", "192.0.2.1:1").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.sse().pop().unwrap().1["stopReason"], "not_covered");
    }
    for visitor in ["192.0.2.2:1", "192.0.2.3:1"] {
        assert_eq!(
            ask(&router, "What is photosynthesis?", visitor)
                .await
                .status,
            StatusCode::OK
        );
    }
    let busy = ask(&router, "What is photosynthesis?", "192.0.2.4:1").await;
    assert_eq!(busy.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy.error_code(), "demo_busy");
    assert!(
        busy.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("busy")
    );
    assert_eq!(env.calls(), 2);
    // A fixed reply needs no model call, but the server cannot know that before it searches, so it
    // refuses every question while the global limit is full.
    let again = ask(&router, "xyzzy plugh quux", "192.0.2.4:1").await;
    assert_eq!(again.error_code(), "demo_busy");
    // The refused visitor did not lose a question: with room, a limit of 1 per window still allows one.
    let roomy = env.router(&[
        ("OPENBOOK_DEMO_LIMIT_GLOBAL_PER_DAY", "3"),
        ("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "1"),
    ]);
    assert_eq!(
        ask(&roomy, "What is photosynthesis?", "192.0.2.4:1")
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn a_model_that_fails_to_start_gives_the_question_back() {
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
    assert_eq!(counter(&env, "DEMO#GLOBAL").await, 0);
}

// ---------------------------------------------------------------------------------------------
// Isolation and privacy
// ---------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs AWS Emulator; run make test-integration"]
async fn the_demo_route_writes_only_limit_counters_and_the_salt() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[("OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN", "3")]);
    let (before, objects) = env.snapshot().await;
    // An answer, a fixed reply, a refusal by the limit, and bad input.
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
            key.0.starts_with("DEMO#V#") || key.0 == "DEMO#GLOBAL" || key.0 == "DEMO#SALT",
            "the demo route wrote {key:?}"
        );
    }
    // No conversation, message, question, or usage item exists for anyone.
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
async fn the_salt_is_daily_and_no_address_or_hash_is_stored_or_logged() {
    let env = demo_env().await;
    env.provision().await;
    let router = env.router(&[]);

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
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .finish(),
    );
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
    assert!(logs.contains("outcome=\"not_covered\""), "{logs}");
    assert!(
        logs.contains("duration_ms=") && logs.contains("input_tokens="),
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
