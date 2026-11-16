//! End-to-end tests of the harness: a small corpus with known answers, the committed dev cache,
//! and the command line (the regression check and its exit code).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use openbook_eval::cache::{Cache, CacheKind, EmbeddingStore, FORMAT, Meta, rewrite_key};
use openbook_eval::data::{Kind, Question, Section};
use openbook_eval::offline;
use openbook_eval::refresh::texts_to_embed;
use openbook_search::embed::{DIMENSIONS, FakeEmbedder};
use openbook_search::retrieval::Params;

fn section(id: &str, a: u32, b: u32) -> Section {
    Section {
        id: id.into(),
        title: id.into(),
        first_page: a,
        last_page: b,
    }
}

fn question(id: &str, kind: Kind, text: &str, pages: &[u32], context: Option<&str>) -> Question {
    Question {
        id: id.into(),
        question: text.into(),
        relevant_pages: pages.to_vec(),
        kind,
        context: context.map(str::to_owned),
    }
}

/// Four pages in two sections, and a cache that holds fake embeddings of every text.
fn mini() -> (Vec<Section>, Vec<Question>, Cache) {
    let sections = vec![section("fruit", 1, 2), section("rocks", 3, 4)];
    let pages = vec![
        (
            1,
            "# Apples\n\nApples are red fruit that grow on trees in orchards.".to_owned(),
        ),
        (
            2,
            "# Bananas\n\nBananas are yellow fruit from tropical plants.".to_owned(),
        ),
        (
            3,
            "# Granite\n\nGranite is a hard igneous rock with large crystals.".to_owned(),
        ),
        (
            4,
            "# Basalt\n\nBasalt is a dark volcanic rock that forms from lava.".to_owned(),
        ),
    ];
    let questions = vec![
        question(
            "q1",
            Kind::Lookup,
            "Which fruit is yellow and tropical?",
            &[2],
            None,
        ),
        question(
            "q2",
            Kind::Paraphrase,
            "What dark volcanic rock forms from lava?",
            &[4],
            None,
        ),
        question(
            "q3",
            Kind::Followup,
            "What colour is it?",
            &[1],
            Some("Tell me about apples"),
        ),
        question(
            "q4",
            Kind::NotCovered,
            "Quarterly earnings of the company",
            &[],
            None,
        ),
    ];
    let mut rewrites = BTreeMap::new();
    let mut queries = Vec::new();
    for q in &questions {
        match &q.context {
            Some(context) => {
                let joined = format!("{context} {}", q.question);
                rewrites.insert(
                    rewrite_key("dev-concat", context, &q.question),
                    joined.clone(),
                );
                queries.push(joined);
            }
            None => queries.push(q.question.clone()),
        }
    }
    let mut embeddings = EmbeddingStore::new("fake", DIMENSIONS);
    for text in texts_to_embed(&sections, &pages, &queries).unwrap() {
        embeddings
            .insert(&text, &FakeEmbedder::vector(&text))
            .unwrap();
    }
    let cache = Cache {
        meta: Meta {
            format: FORMAT,
            kind: CacheKind::Dev,
            corpus: "mini.pdf".into(),
            corpus_sha256: "00".into(),
            embedding_model: "fake".into(),
            transcribe_model: None,
            rewrite_model: "dev-concat".into(),
            pages: 4,
            transcribed_pages: 0,
            fallback_pages: vec![],
            latency: None,
        },
        pages,
        embeddings,
        rewrites,
    };
    (sections, questions, cache)
}

#[tokio::test]
async fn a_small_corpus_with_known_answers_scores_as_expected() {
    let (sections, questions, cache) = mini();
    let report = offline::run(&sections, &questions, &cache, &Params::default())
        .await
        .unwrap();
    assert_eq!(
        (report.questions, report.covered, report.not_covered),
        (4, 3, 1)
    );
    // Every covered question finds its page, in every setup.
    for metrics in [report.dense, report.bm25, report.hybrid] {
        assert_eq!(metrics.recall_at_5, 1.0);
        assert_eq!(metrics.recall_at_8, 1.0);
    }
    assert!(report.misses.is_empty(), "{:?}", report.misses);
    // The question about earnings shares no word with the corpus: BM25 has no hit and the dense
    // distance is far, so the gate cuts it, and covered questions pass the gate.
    assert_eq!(report.hybrid.gate_accuracy, 1.0);
    assert_eq!(report.hybrid.false_gate_rate, 0.0);
    assert_eq!(report.hybrid.missed_gate_rate, 0.0);
    assert_eq!(report.hybrid_by_kind["not_covered"].gated_rate, 1.0);
    assert_eq!(report.hybrid_by_kind["lookup"].questions, 1);
}

#[tokio::test]
async fn the_gate_number_changes_the_gate_accuracy() {
    let (sections, questions, cache) = mini();
    // A distance limit of 0 gates every question that has no BM25 hit. The earnings question stays gated.
    let strict = Params {
        gate_max_distance: 0.0,
        ..Params::default()
    };
    let report = offline::run(&sections, &questions, &cache, &strict)
        .await
        .unwrap();
    assert_eq!(report.hybrid.missed_gate_rate, 0.0);
    // The fusion count changes what the hybrid run returns, not the dense-only gate rule.
    let narrow = Params {
        final_sources: 1,
        max_per_document: 1,
        ..Params::default()
    };
    let report = offline::run(&sections, &questions, &cache, &narrow)
        .await
        .unwrap();
    // The ranked lists are ten deep for nDCG, so Recall@8 still sees them.
    assert_eq!(report.hybrid.recall_at_8, 1.0);
}

#[tokio::test]
async fn a_missing_embedding_is_an_error_and_not_a_skipped_question() {
    let (sections, mut questions, cache) = mini();
    questions.push(question(
        "q5",
        Kind::Lookup,
        "A question that was never embedded",
        &[1],
        None,
    ));
    let error = offline::run(&sections, &questions, &cache, &Params::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("q5"), "{error}");
    assert!(error.contains("never embedded"), "{error}");
    assert!(error.contains("Refresh the cache"), "{error}");
}

#[tokio::test]
async fn a_missing_rewrite_is_an_error() {
    let (sections, questions, mut cache) = mini();
    cache.rewrites.clear();
    let error = offline::run(&sections, &questions, &cache, &Params::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no rewrite for follow-up q3"), "{error}");
}

#[tokio::test]
async fn a_label_that_no_chunk_holds_is_a_data_error() {
    let (sections, mut questions, cache) = mini();
    questions[0].relevant_pages = vec![9];
    let error = offline::run(&sections, &questions, &cache, &Params::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no chunk holds the labeled pages"),
        "{error}"
    );
}

#[tokio::test]
async fn the_run_is_deterministic() {
    let (sections, questions, cache) = mini();
    let a = offline::run(&sections, &questions, &cache, &Params::default())
        .await
        .unwrap();
    let b = offline::run(&sections, &questions, &cache, &Params::default())
        .await
        .unwrap();
    assert_eq!(a.to_json().unwrap(), b.to_json().unwrap());
}

#[test]
fn chunks_know_their_pages_in_the_excerpt() {
    let (sections, _, cache) = mini();
    let chunks = offline::build_chunks(&sections, &cache.pages).unwrap();
    let rocks: Vec<_> = chunks.iter().filter(|c| c.document_id == "rocks").collect();
    assert!(!rocks.is_empty());
    for chunk in &rocks {
        assert!(chunk.span.0 >= 3 && chunk.span.1 <= 4, "{:?}", chunk.span);
        assert!(chunk.key.starts_with("rocks:"));
    }
    assert!(
        chunks
            .iter()
            .any(|c| c.span == (4, 4) && c.text.contains("Basalt"))
    );
    assert!(
        chunks
            .iter()
            .all(|c| c.embed_text.starts_with(&c.document_id))
    );
}

// ---- The committed dev cache and the command line

fn evals_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals")
}

fn eval(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_openbook-eval"))
        .args(args)
        .output()
        .expect("the binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn the_committed_dev_cache_runs_and_says_that_it_is_a_dev_cache() {
    let out = eval(&["offline", "--dev-cache"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("DEV CACHE"));
    assert!(stdout.contains("| Recall@8 |"));
    assert!(stdout.contains("| Metric | Dense only | BM25 only | Hybrid |"));
    // Two runs print the same table.
    assert_eq!(stdout, text(&eval(&["offline", "--dev-cache"]).stdout));
}

#[test]
fn a_dev_cache_cannot_give_or_check_a_baseline() {
    for flag in ["--check", "--write-baseline"] {
        let out = eval(&["offline", "--dev-cache", flag]);
        assert!(!out.status.success());
        assert!(
            text(&out.stderr).contains("never gives a baseline"),
            "{}",
            text(&out.stderr)
        );
    }
    assert!(
        !evals_dir().join("baseline.json").exists() || {
            // A real baseline may exist once the live refresh ran. It must not come from a dev cache.
            let text = std::fs::read_to_string(evals_dir().join("baseline.json")).unwrap();
            !text.contains("\"cacheKind\": \"dev\"")
        }
    );
}

#[test]
fn tuning_flags_cannot_be_used_with_the_check() {
    let out = eval(&["offline", "--dev-cache", "--rrf-k", "30"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let out = eval(&["offline", "--check", "--rrf-k", "30"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("tuning flags"));
}

/// A temporary evals directory with the corpus, the golden set, and the dev cache presented as a
/// live cache. It lets the test run the whole baseline and check path without AWS.
fn live_looking_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let from = evals_dir();
    std::fs::create_dir_all(root.join("corpus")).unwrap();
    for name in [
        "corpus/sections.json",
        "corpus/college-algebra-excerpt.pdf",
        "golden.jsonl",
    ] {
        std::fs::copy(from.join(name), root.join(name)).unwrap();
    }
    let mut cache = Cache::load(&from.join("cache-dev")).unwrap();
    cache.meta.kind = CacheKind::Live;
    cache.save(&root.join("cache")).unwrap();
    dir
}

#[test]
fn the_check_passes_on_its_own_baseline_and_fails_on_a_regression() {
    let dir = live_looking_copy();
    let evals = dir.path().to_str().unwrap();

    // No baseline yet: the check says what to do.
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("--write-baseline"),
        "{}",
        text(&out.stderr)
    );

    let out = eval(&["offline", "--write-baseline", "--evals", evals]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let baseline_path = dir.path().join("baseline.json");
    assert!(baseline_path.exists());

    let out = eval(&["offline", "--check", "--evals", evals]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("Check passed"));

    // A baseline that is better than the code (a regression for the code) fails the check.
    let original = std::fs::read_to_string(&baseline_path).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    let recall = value["hybrid"]["recallAt8"].as_f64().unwrap();
    value["hybrid"]["recallAt8"] = serde_json::json!((recall + 0.05).min(1.0).max(recall + 0.02));
    std::fs::write(
        &baseline_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("CHECK FAILED"));
    assert!(text(&out.stderr).contains("Recall@8"));

    // Gate accuracy has no tolerance.
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    let gate = value["hybrid"]["gateAccuracy"].as_f64().unwrap();
    value["hybrid"]["gateAccuracy"] = serde_json::json!(gate + 0.0005);
    std::fs::write(
        &baseline_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("Gate accuracy"));
}

#[test]
fn a_cache_for_another_corpus_is_refused() {
    let dir = live_looking_copy();
    let evals = dir.path().to_str().unwrap();
    let mut cache = Cache::load(&dir.path().join("cache")).unwrap();
    cache.meta.corpus_sha256 = "0".repeat(64);
    cache.save(&dir.path().join("cache")).unwrap();
    let out = eval(&["offline", "--evals", evals]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("another corpus file"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn the_live_refresh_does_not_start_without_the_go_ahead() {
    // With no PDFium the plan cannot even be made, and that is also a refusal to call AWS. With
    // PDFium the plan is printed and the run stops. In both cases the exit code is not 0 and no
    // cache is written.
    let dir = live_looking_copy();
    let evals = dir.path().to_str().unwrap();
    std::fs::remove_dir_all(dir.path().join("cache")).unwrap();
    let out = eval(&["refresh", "--evals", evals]);
    assert!(!out.status.success());
    assert!(!dir.path().join("cache").exists());
}
