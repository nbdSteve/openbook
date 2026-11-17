//! End-to-end tests of the harness: a small corpus with known answers, the committed dev cache,
//! and the command line (the regression check and its exit code).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use openbook_eval::cache::{Cache, CacheKind, EmbeddingStore, FORMAT, Meta, rewrite_key};
use openbook_eval::data::{Kind, Question, Section};
use openbook_eval::offline::{self, Options};
use openbook_eval::refresh::{CHUNK_TARGET_TOKENS, texts_to_embed};
use openbook_search::embed::{DIMENSIONS, FakeEmbedder};
use openbook_search::retrieval::GateRule;

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

/// The numbers of production for the fake embedder that the small corpus uses.
fn options() -> Options {
    Options::production("fake")
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
        question("q5", Kind::OffTopic, "hello there", &[], None),
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
    for text in texts_to_embed(&sections, &pages, &queries, &CHUNK_TARGET_TOKENS).unwrap() {
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
    let report = offline::run(&sections, &questions, &cache, &options())
        .await
        .unwrap();
    assert_eq!(
        (report.questions, report.covered, report.not_covered),
        (5, 3, 2)
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
    assert_eq!(report.hybrid_by_kind["off_topic"].gated_rate, 1.0);
    // The gate is reported by group and for every question.
    let group = |name: &str| {
        report
            .gate_by_group
            .iter()
            .find(|g| g.group.starts_with(name))
            .unwrap()
    };
    assert_eq!(group("covered").hybrid, 0.0);
    assert_eq!(group("off_topic").questions, 1);
    assert_eq!(group("not_covered and off_topic").questions, 2);
    assert_eq!(report.diagnostics.len(), 5);
    let earnings = &report.diagnostics[3];
    assert_eq!((earnings.id.as_str(), earnings.gated), ("q4", true));
    assert_eq!(earnings.lexical_hits, 0);
    assert!(earnings.best_dense_distance.is_some());
    let apples = &report.diagnostics[0];
    assert!(apples.matched_terms > 0 && apples.term_coverage > 0.0);
    assert!(report.gate_sweep.len() > 20);
    // Both document modes are reported.
    let single = report.single_document.expect("the other mode is reported");
    assert_eq!(single.hybrid.recall_at_8, 1.0);
    assert_eq!(report.hybrid_by_kind["lookup"].questions, 1);
}

#[tokio::test]
async fn the_gate_number_changes_the_gate_accuracy() {
    let (sections, questions, cache) = mini();
    // A distance limit of 0 gates every question that has no BM25 hit. The earnings question stays gated.
    let mut strict = options();
    strict.params.gate = GateRule::AnyLexicalHit { max_distance: 0.0 };
    let report = offline::run(&sections, &questions, &cache, &strict)
        .await
        .unwrap();
    assert_eq!(report.hybrid.missed_gate_rate, 0.0);
    // The fusion count changes what the hybrid run returns, not the dense-only gate rule.
    let mut narrow = options();
    narrow.params.final_sources = 1;
    narrow.params.max_per_document = 1;
    let report = offline::run(&sections, &questions, &cache, &narrow)
        .await
        .unwrap();
    // One source is returned, and the question has one relevant chunk at rank 1 in this corpus.
    assert!(report.hybrid.recall_at_8 > 0.0);
    // nDCG reads a ten-deep list, so it is still more than zero for the questions that miss.
    assert!(report.hybrid.ndcg_at_10 > 0.0);
}

#[tokio::test]
async fn a_missing_embedding_is_an_error_and_not_a_skipped_question() {
    let (sections, mut questions, cache) = mini();
    questions.push(question(
        "q6",
        Kind::Lookup,
        "A question that was never embedded",
        &[1],
        None,
    ));
    let error = offline::run(&sections, &questions, &cache, &options())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("q6"), "{error}");
    assert!(error.contains("never embedded"), "{error}");
    assert!(error.contains("Refresh the cache"), "{error}");
}

#[tokio::test]
async fn a_missing_rewrite_is_an_error() {
    let (sections, questions, mut cache) = mini();
    cache.rewrites.clear();
    let error = offline::run(&sections, &questions, &cache, &options())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no rewrite for follow-up q3"), "{error}");
}

#[tokio::test]
async fn a_label_that_no_chunk_holds_is_a_data_error() {
    let (sections, mut questions, cache) = mini();
    questions[0].relevant_pages = vec![9];
    let error = offline::run(&sections, &questions, &cache, &options())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no chunk holds the labeled pages"),
        "{error}"
    );
}

#[tokio::test]
async fn single_document_mode_changes_only_the_cap() {
    let (sections, questions, cache) = mini();
    let mut narrow = options();
    narrow.params.max_per_document = 1;
    let sectioned = offline::run(&sections, &questions, &cache, &narrow)
        .await
        .unwrap();
    let single = offline::run(
        &sections,
        &questions,
        &cache,
        &narrow.with_single_document(true),
    )
    .await
    .unwrap();
    // In single-document mode the report does not nest the other mode.
    assert!(single.single_document.is_none());
    assert!(single.settings.contains("single document"));
    assert!(sectioned.settings.contains("one document per section"));
    // The mode nested in the sectioned report is the mode of the single run.
    assert_eq!(sectioned.single_document.unwrap().hybrid, single.hybrid);
    // The gate does not depend on the mode.
    assert_eq!(sectioned.hybrid.gate_accuracy, single.hybrid.gate_accuracy);
}

#[tokio::test]
async fn the_run_is_deterministic() {
    let (sections, questions, cache) = mini();
    let a = offline::run(&sections, &questions, &cache, &options())
        .await
        .unwrap();
    let b = offline::run(&sections, &questions, &cache, &options())
        .await
        .unwrap();
    assert_eq!(a.to_json().unwrap(), b.to_json().unwrap());
}

#[test]
fn chunks_know_their_pages_in_the_excerpt() {
    let (sections, _, cache) = mini();
    let chunks = offline::build_chunks(&sections, &cache.pages, 1600).unwrap();
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
fn the_report_has_the_gate_inputs_the_sweep_and_both_modes() {
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("report.json");
    let md = dir.path().join("report.md");
    let out = eval(&[
        "offline",
        "--dev-cache",
        "--json",
        json.to_str().unwrap(),
        "--markdown",
        md.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    let first = &value["diagnostics"][0];
    for field in [
        "id",
        "kind",
        "bestDenseDistance",
        "topBm25Score",
        "termCoverage",
        "queryTerms",
        "indexedTerms",
        "matchedTerms",
    ] {
        assert!(first.get(field).is_some(), "{field}");
    }
    assert!(value["gateSweep"].as_array().unwrap().len() > 20);
    assert!(value["singleDocument"]["hybrid"].is_object());
    assert!(value["hybridByKind"]["off_topic"].is_object());
    let markdown = std::fs::read_to_string(&md).unwrap();
    assert!(markdown.contains("Gate sweep"));
    assert!(markdown.contains("Single-document mode"));
    assert!(markdown.contains("| off_topic |"));
}

#[test]
fn the_grid_runs_on_the_dev_cache_and_writes_a_table() {
    let dir = tempfile::tempdir().unwrap();
    let out = eval(&["grid", "--dev-cache", "--out", dir.path().to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let md = std::fs::read_to_string(dir.path().join("grid.md")).unwrap();
    for name in ["RRF k", "Final sources", "Max per document", "Chunk target"] {
        assert!(md.contains(name), "{name}");
    }
    let grid: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("grid.json")).unwrap())
            .unwrap();
    // 3 chunk targets x 3 k x 3 final x 2 candidates x 4 max per document x 2 weights.
    assert_eq!(grid["rows"].as_array().unwrap().len(), 432);
}

#[test]
fn missing_only_needs_a_live_cache() {
    let out = eval(&[
        "refresh",
        "--missing-only",
        "--cache",
        evals_dir().join("cache-dev").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("live cache"),
        "{}",
        text(&out.stderr)
    );
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
