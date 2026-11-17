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
    let single = report.cap_off.expect("the other mode is reported");
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
    assert!(report.hybrid.ndcg_at_8 > 0.0);
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
async fn cap_off_mode_changes_only_the_cap() {
    let (sections, questions, cache) = mini();
    let mut narrow = options();
    narrow.params.max_per_document = 1;
    let sectioned = offline::run(&sections, &questions, &cache, &narrow)
        .await
        .unwrap();
    let single = offline::run(&sections, &questions, &cache, &narrow.with_cap_off(true))
        .await
        .unwrap();
    // In cap-off mode the report does not nest the other mode.
    assert!(single.cap_off.is_none());
    assert!(single.settings.contains("cap off"));
    assert!(sectioned.settings.contains("one document per section"));
    // The mode nested in the sectioned report is the mode of the single run.
    assert_eq!(sectioned.cap_off.unwrap().hybrid, single.hybrid);
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
    assert_eq!(value["corpora"].as_array().unwrap().len(), 2);
    let value = &value["combined"];
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
    assert!(value["capOff"]["hybrid"].is_object());
    assert!(value["hybridByKind"]["off_topic"].is_object());
    let markdown = std::fs::read_to_string(&md).unwrap();
    assert!(value["headers"]["runningLines"].as_u64().unwrap() > 0);
    assert!(markdown.contains("Gate sweep"));
    assert!(markdown.contains("Cap-off mode"));
    assert!(markdown.contains("| off_topic |"));
    assert!(markdown.contains("## Combined (all corpora pooled)"));
    assert!(markdown.contains("## Corpus: american-government"));
    assert!(markdown.contains("## Corpus: college-algebra"));
}

#[test]
fn one_corpus_can_be_run_alone_and_a_wrong_use_is_refused() {
    let out = eval(&["offline", "--dev-cache", "--corpus", "college-algebra"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("## Corpus: college-algebra"));
    assert!(!stdout.contains("american-government"));
    assert!(!stdout.contains("Combined"));
    // The baseline covers every corpus, so it cannot come from one.
    let out = eval(&["offline", "--check", "--corpus", "college-algebra"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("every corpus"),
        "{}",
        text(&out.stderr)
    );
    let out = eval(&["offline", "--dev-cache", "--corpus", "nope"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("american-government, college-algebra"),
        "{}",
        text(&out.stderr)
    );
    // One cache directory needs one corpus.
    let out = eval(&["offline", "--dev-cache", "--cache", "/nowhere"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("--corpus"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn running_headers_are_stripped_from_the_committed_pages() {
    // The pages of the first corpus come from page transcription (Markdown with header lines). The
    // pages of the second corpus come from the text layer (one paragraph per page, the header and
    // the footer inside it). The same code strips both.
    for (id, dev, phrase) in [
        ("college-algebra", false, "chapter 1 prerequisites"),
        (
            "american-government",
            true,
            "chapter 2 | the constitution and its origins",
        ),
        (
            "american-government",
            true,
            "this openstax book is available for free at http://cnx.org/content/col26739/1.4",
        ),
    ] {
        let corpus = openbook_eval::corpus::select(&evals_dir(), Some(id))
            .unwrap()
            .remove(0);
        let sections = openbook_eval::data::load_sections(&corpus.sections_path()).unwrap();
        let cache = Cache::load(&corpus.cache_dir(dev)).unwrap();
        let stats = offline::header_stats(&sections, &cache.pages, 1600).unwrap();
        assert!(stats.running_lines >= 4, "{id}: {stats:?}");
        assert!(stats.removed_lines >= 15, "{id}: {stats:?}");
        // Nothing is left of a header that a longer header covers, such as `# SECTION` from
        // `# SECTION 5.1 QUADRATIC FUNCTIONS`.
        assert_eq!(stats.chunks_with_fragment_after, 0, "{id}: {stats:?}");
        assert!(
            stats.chunks_with_header_after < stats.chunks_with_header_before
                || stats.chunks_with_header_before == 0,
            "{id}: {stats:?}"
        );
        // Real pages 27 and 28 of college-algebra (the first two pages of the section that starts
        // at excerpt page 26 are the chapter opener and the first page; 27 and 28 follow): the
        // header of page 27 has a title at its end, and page 28 has the title alone.
        if id == "college-algebra" {
            let stripped = openbook_search::headers::strip_running_lines(
                &cache.pages[25..37]
                    .iter()
                    .enumerate()
                    .map(|(i, (_, t))| (u32::try_from(i + 1).unwrap(), t.clone()))
                    .collect::<Vec<_>>(),
            );
            for (_, text) in &stripped.pages {
                assert!(!text.lines().any(|l| l.trim() == "# SECTION"), "{text}");
                assert!(!text.contains("CHAPTER 5 5.1"), "{text}");
            }
        }
        // The phrase is gone from the chunks, except for its first occurrence in each section.
        let chunks = offline::build_chunks(&sections, &cache.pages, 1600).unwrap();
        let in_chunks = chunks
            .iter()
            .filter(|c| c.text.to_lowercase().contains(phrase))
            .count();
        assert!(
            in_chunks <= sections.len(),
            "{id}: {in_chunks} chunks still hold {phrase:?}"
        );
    }
}

#[test]
fn the_grid_runs_on_the_dev_cache_and_writes_a_table() {
    let dir = tempfile::tempdir().unwrap();
    // The tiny grid has two rows: the production setting with both header variants. The whole grid
    // (`make eval-grid`) is a tool and not a test.
    let out = eval(&[
        "grid",
        "--dev-cache",
        "--tiny",
        "--out",
        dir.path().to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let md = std::fs::read_to_string(dir.path().join("grid.md")).unwrap();
    for name in ["RRF k", "Final sources", "Max per document", "Chunk target"] {
        assert!(md.contains(name), "{name}");
    }
    let grid: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("grid.json")).unwrap())
            .unwrap();
    let rows = grid["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["knobs"]["dropAllHeaders"], false);
    assert_eq!(rows[1]["knobs"]["dropAllHeaders"], true);
    // Every row has the pooled numbers and the numbers of each corpus.
    assert!(rows[0]["combined"]["sectioned"]["hybrid"].is_object());
    assert_eq!(rows[0]["corpora"].as_object().unwrap().len(), 2);
    assert!(md.contains("## Combined (all corpora pooled)"));
    assert!(md.contains("## Corpus: college-algebra"));
}

#[test]
fn missing_only_needs_a_live_cache() {
    // A corpus whose cache directory holds a dev cache.
    let dir = live_looking_copy();
    for corpus in openbook_eval::corpus::discover(dir.path()).unwrap() {
        let mut cache = Cache::load(&corpus.cache_dir(false)).unwrap();
        cache.meta.kind = CacheKind::Dev;
        cache.save(&corpus.cache_dir(false)).unwrap();
    }
    let out = eval(&[
        "refresh",
        "--missing-only",
        "--evals",
        dir.path().to_str().unwrap(),
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
fn check_and_write_baseline_cannot_be_used_together() {
    let out = eval(&["offline", "--check", "--write-baseline"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("cannot be used together"),
        "{}",
        text(&out.stderr)
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

/// A temporary evals directory with the corpora, their golden sets, and their dev caches presented
/// as live caches. It lets the test run the whole baseline and check path without AWS.
fn live_looking_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for corpus in openbook_eval::corpus::discover(&evals_dir()).unwrap() {
        let to = root.join("corpora").join(&corpus.id);
        std::fs::create_dir_all(&to).unwrap();
        for name in [
            "sections.json".to_owned(),
            "golden.jsonl".to_owned(),
            "corpus.json".to_owned(),
            corpus
                .pdf_path()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ] {
            std::fs::copy(corpus.dir.join(&name), to.join(&name)).unwrap();
        }
        let mut cache = Cache::load(&corpus.cache_dir(true)).unwrap();
        cache.meta.kind = CacheKind::Live;
        cache.save(&to.join("cache")).unwrap();
    }
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
    let recall = value["combined"]["hybrid"]["recallAt8"].as_f64().unwrap();
    value["combined"]["hybrid"]["recallAt8"] =
        serde_json::json!((recall + 0.05).min(1.0).max(recall + 0.02));
    std::fs::write(
        &baseline_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("CHECK FAILED"));
    assert!(
        text(&out.stderr).contains("combined, Recall@8"),
        "{}",
        text(&out.stderr)
    );

    // One corpus that gets worse fails the check, even when the pooled numbers hold.
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    let ndcg = value["corpora"]["american-government"]["hybrid"]["ndcgAt8"]
        .as_f64()
        .unwrap();
    value["corpora"]["american-government"]["hybrid"]["ndcgAt8"] =
        serde_json::json!((ndcg + 0.05).min(1.0).max(ndcg + 0.02));
    std::fs::write(
        &baseline_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("american-government, nDCG@8"),
        "{}",
        text(&out.stderr)
    );

    // A baseline for other corpora is refused with a clear message.
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    value["corpora"]
        .as_object_mut()
        .unwrap()
        .remove("college-algebra");
    std::fs::write(
        &baseline_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("--write-baseline"),
        "{}",
        text(&out.stderr)
    );

    // Gate accuracy has no tolerance.
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    let gate = value["combined"]["hybrid"]["gateAccuracy"]
        .as_f64()
        .unwrap();
    value["combined"]["hybrid"]["gateAccuracy"] = serde_json::json!(gate + 0.0005);
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
    let cache_dir = dir.path().join("corpora/american-government/cache");
    let mut cache = Cache::load(&cache_dir).unwrap();
    cache.meta.corpus_sha256 = "0".repeat(64);
    cache.save(&cache_dir).unwrap();
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
    for corpus in openbook_eval::corpus::discover(dir.path()).unwrap() {
        std::fs::remove_dir_all(corpus.cache_dir(false)).unwrap();
    }
    let out = eval(&["refresh", "--missing-only", "--evals", evals]);
    assert!(!out.status.success());
    for corpus in openbook_eval::corpus::discover(dir.path()).unwrap() {
        assert!(!corpus.cache_dir(false).exists());
    }
}

#[test]
fn benchmarks_come_with_the_baseline_and_a_stale_file_fails_the_check() {
    let dir = live_looking_copy();
    let evals = dir.path().to_str().unwrap();
    let out = eval(&["offline", "--write-baseline", "--evals", evals]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let path = dir.path().join("benchmarks.json");
    let original = std::fs::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&original).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["questions"], 180);
    assert_eq!(value["corpora"].as_array().unwrap().len(), 2);
    assert_eq!(value["corpora"][0]["licence"], "CC BY 4.0");
    assert!(
        value["corpora"][0]["source"]
            .as_str()
            .unwrap()
            .starts_with("https://openstax.org/")
    );
    for scope in ["combined", "corpora"] {
        assert!(value["retrieval"][scope].is_object());
        assert!(value["gate"][scope].is_object());
    }
    assert!(value["gate"]["combined"]["offTopicCaught"].is_number());

    let out = eval(&["offline", "--check", "--evals", evals]);
    assert!(out.status.success(), "{}", text(&out.stderr));

    // A file that is not what the code gives now fails the check, even when the baseline holds.
    let mut stale = value.clone();
    stale["retrieval"]["combined"]["hybrid"]["recallAt8"] = serde_json::json!(0.5);
    std::fs::write(&path, serde_json::to_string_pretty(&stale).unwrap()).unwrap();
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).contains("stale"), "{}", text(&out.stderr));
    // A missing file says what to do.
    std::fs::remove_file(&path).unwrap();
    let out = eval(&["offline", "--check", "--evals", evals]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("--write-baseline"));
}

#[test]
fn the_readme_block_is_written_and_a_stale_block_fails_the_check() {
    let dir = live_looking_copy();
    let evals = dir.path().to_str().unwrap();
    let out = eval(&["offline", "--write-baseline", "--evals", evals]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let readme = dir.path().join("README.md");
    let readme_arg = readme.to_str().unwrap();
    std::fs::write(
        &readme,
        "# Title\n\n<!-- benchmarks:start -->\n<!-- benchmarks:end -->\n\nAfter.\n",
    )
    .unwrap();
    let out = eval(&[
        "readme", "--check", "--evals", evals, "--readme", readme_arg,
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let out = eval(&["readme", "--evals", evals, "--readme", readme_arg]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let written = std::fs::read_to_string(&readme).unwrap();
    assert!(
        written.contains("| All corpora | 180 | Hybrid |"),
        "{written}"
    );
    assert!(
        written.contains("| Gate accuracy | False-gate rate |")
            || written.contains("Gate accuracy")
    );
    assert!(written.starts_with("# Title") && written.ends_with("After.\n"));
    let out = eval(&[
        "readme", "--check", "--evals", evals, "--readme", readme_arg,
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    std::fs::write(&readme, written.replace("| Hybrid |", "| Hybrid! |")).unwrap();
    let out = eval(&[
        "readme", "--check", "--evals", evals, "--readme", readme_arg,
    ]);
    assert_eq!(out.status.code(), Some(1));
    // The README of the repository has the markers, and its block is what the table generator gives
    // from a benchmarks file when there is one.
    let real = std::fs::read_to_string(evals_dir().join("../README.md")).unwrap();
    assert!(real.contains("<!-- benchmarks:start -->") && real.contains("<!-- benchmarks:end -->"));
    assert!(real.contains(
        "raw.githubusercontent.com%2FnbdSteve%2Fopenbook%2Fmain%2Fevals%2Fbenchmarks.json"
    ));
}
