//! The retrieval core that the API and the evaluation share (`docs/evaluation.md`).
//!
//! It holds the pure parts: query cleaning, reciprocal rank fusion, the per-document cap, and the
//! relevance gate. Two small traits stand for the dense and the lexical backend, so the API (S3
//! Vectors and Tantivy from S3) and the evaluation (an in-memory index) run exactly this code.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;

// ---- The numbers. They are tuned against the evaluation set. Keep them here.
/// Candidates from the dense search.
pub const DENSE_TOP_K: u32 = 40;
/// Candidates from BM25.
pub const LEXICAL_TOP_K: usize = 40;
/// Reciprocal rank fusion: `score = sum over lists of 1 / (RRF_K + rank)`, with rank from 1.
pub const RRF_K: f64 = 60.0;
/// Sources that go to the model and the student.
pub const FINAL_SOURCES: usize = 8;
/// Most sources from one document.
pub const MAX_PER_DOCUMENT: usize = 4;
/// The gate: with no BM25 hit, a best dense cosine distance above this means no sources.
/// With Titan v2, a relevant passage had distances near 0.68 and an unrelated question near 0.94.
/// Phase 4 tunes this number against the evaluation set.
pub const GATE_MAX_DISTANCE: f32 = 0.85;

/// The tunable numbers of fusion and the gate. `Default` is what production uses. The evaluation
/// changes them to compare settings; the API never does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub rrf_k: f64,
    pub final_sources: usize,
    pub max_per_document: usize,
    pub gate_max_distance: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            rrf_k: RRF_K,
            final_sources: FINAL_SOURCES,
            max_per_document: MAX_PER_DOCUMENT,
            gate_max_distance: GATE_MAX_DISTANCE,
        }
    }
}
/// A search query is cut to this many characters.
pub const MAX_QUERY_CHARS: usize = 500;
/// Each retriever gets this long. A slow one counts as failed.
pub const RETRIEVER_TIMEOUT: Duration = Duration::from_secs(3);

/// A passage that a retriever found.
#[derive(Debug, Clone, PartialEq)]
pub struct Passage {
    /// `<documentId>:<chunkIndex>`.
    pub key: String,
    pub document_id: String,
    pub page: u32,
    pub heading: String,
    pub text: String,
}

/// A candidate from one retriever. `value` is the cosine distance (dense, smaller is closer) or the
/// BM25 score (lexical, larger is better). The list order is the rank.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub passage: Passage,
    pub value: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fused {
    pub passage: Passage,
    pub score: f64,
    pub dense_rank: Option<usize>,
    pub lexical_rank: Option<usize>,
}

/// Fuses two ranked lists with reciprocal rank fusion. Ranks start at 1 in list order. The result
/// is in fused order, at most `FINAL_SOURCES` long, and has at most `MAX_PER_DOCUMENT` per document.
pub fn fuse(dense: &[Hit], lexical: &[Hit]) -> Vec<Fused> {
    fuse_with(&Params::default(), dense, lexical)
}

/// `fuse` with explicit numbers.
pub fn fuse_with(params: &Params, dense: &[Hit], lexical: &[Hit]) -> Vec<Fused> {
    let mut by_key: HashMap<&str, Fused> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (list, is_dense) in [(dense, true), (lexical, false)] {
        for (index, hit) in list.iter().enumerate() {
            let rank = index + 1;
            let entry = by_key.entry(hit.passage.key.as_str()).or_insert_with(|| {
                order.push(hit.passage.key.as_str());
                Fused {
                    passage: hit.passage.clone(),
                    score: 0.0,
                    dense_rank: None,
                    lexical_rank: None,
                }
            });
            // A key can repeat inside one list. Only its first place counts.
            let slot = if is_dense {
                &mut entry.dense_rank
            } else {
                &mut entry.lexical_rank
            };
            if slot.is_none() {
                *slot = Some(rank);
                entry.score += 1.0 / (params.rrf_k + rank as f64);
            }
        }
    }
    let mut all: Vec<Fused> = order.iter().filter_map(|k| by_key.remove(k)).collect();
    // Best score first. Ties go to the better rank in either list, then to the key, so the order is stable.
    all.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| best_rank(a).cmp(&best_rank(b)))
            .then_with(|| a.passage.key.cmp(&b.passage.key))
    });
    let mut per_document: HashMap<String, usize> = HashMap::new();
    let mut kept = Vec::new();
    for candidate in all {
        let count = per_document
            .entry(candidate.passage.document_id.clone())
            .or_insert(0);
        if *count >= params.max_per_document {
            continue;
        }
        *count += 1;
        kept.push(candidate);
        if kept.len() == params.final_sources {
            break;
        }
    }
    kept
}

fn best_rank(f: &Fused) -> usize {
    [f.dense_rank, f.lexical_rank]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(usize::MAX)
}

/// The relevance gate. With no BM25 hit, a best dense distance above `GATE_MAX_DISTANCE` (or no dense
/// result at all) gives no sources.
pub fn passes_gate(best_dense_distance: Option<f32>, lexical_hits: usize) -> bool {
    passes_gate_with(&Params::default(), best_dense_distance, lexical_hits)
}

/// `passes_gate` with explicit numbers.
pub fn passes_gate_with(
    params: &Params,
    best_dense_distance: Option<f32>,
    lexical_hits: usize,
) -> bool {
    let dense_is_close = best_dense_distance.is_some_and(|d| d <= params.gate_max_distance);
    dense_is_close || lexical_hits > 0
}

/// Cuts text to `max` characters (never inside a character).
pub fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// How retrieval ended for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Retrieval did not run (no ready documents) or both retrievers failed.
    NotUsed,
    Found,
    /// The gate found nothing.
    NotCovered,
}

/// One line, no quotes, one space between words, at most `MAX_QUERY_CHARS`. `None` when empty.
pub fn clean_query(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let unquoted = line.trim_matches(|c| matches!(c, '"' | '\'' | '`' | '“' | '”'));
    let words = unquoted.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped = clip(&words, MAX_QUERY_CHARS);
    (!clipped.is_empty()).then_some(clipped)
}

/// The result of a dense search.
#[derive(Debug, Clone, Default)]
pub struct DenseResult {
    /// Candidates, closest first.
    pub hits: Vec<Hit>,
    /// Input tokens of the query embedding.
    pub embed_tokens: u64,
}

/// The dense backend: embeds the query and finds the closest chunks of one course.
#[async_trait]
pub trait DenseSearch: Send + Sync {
    async fn search(&self, query: &str) -> Result<DenseResult, String>;
}

/// The lexical backend: BM25 over the chunks of one course.
#[async_trait]
pub trait LexicalSearch: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<Hit>, String>;
}

/// What the two retrievers returned. A retriever that failed has `None` and an entry in `errors`.
#[derive(Debug, Clone, Default)]
pub struct Searched {
    pub embed_tokens: u64,
    pub dense: Option<Vec<Hit>>,
    pub lexical: Option<Vec<Hit>>,
    pub dense_ms: Option<u64>,
    pub lexical_ms: Option<u64>,
    pub errors: Vec<String>,
}

/// Runs both retrievers in parallel. Each gets `timeout`; a slow one counts as failed.
pub async fn search_both(
    dense: &dyn DenseSearch,
    lexical: &dyn LexicalSearch,
    query: &str,
    timeout: Duration,
) -> Searched {
    async fn timed<T>(
        timeout: Duration,
        future: impl std::future::Future<Output = Result<T, String>>,
    ) -> (Result<T, String>, u64) {
        let started = Instant::now();
        let result = match tokio::time::timeout(timeout, future).await {
            Ok(result) => result,
            Err(_) => Err("timed out".to_owned()),
        };
        (result, started.elapsed().as_millis() as u64)
    }
    let ((dense, dense_ms), (lexical, lexical_ms)) = tokio::join!(
        timed(timeout, dense.search(query)),
        timed(timeout, lexical.search(query))
    );
    let mut errors = Vec::new();
    let mut embed_tokens = 0;
    let dense = dense.map(|found| {
        embed_tokens = found.embed_tokens;
        found.hits
    });
    let mut keep = |name: &str, result: Result<Vec<Hit>, String>| match result {
        Ok(hits) => Some(hits),
        Err(error) => {
            tracing::warn!(retriever = name, %error, "retriever failed");
            errors.push(format!("{name} failed"));
            None
        }
    };
    let dense = keep("dense", dense);
    let lexical = keep("lexical", lexical);
    Searched {
        embed_tokens,
        dense,
        lexical,
        dense_ms: Some(dense_ms),
        lexical_ms: Some(lexical_ms),
        errors,
    }
}

/// The decision for one query.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub outcome: Outcome,
    /// Both lists as given (empty when the outcome is `NotUsed`).
    pub dense: Vec<Hit>,
    pub lexical: Vec<Hit>,
    /// The fused sources. Empty unless the outcome is `Found`.
    pub fused: Vec<Fused>,
}

/// Applies the relevance gate and the fusion to the two candidate lists. `None` means the retriever
/// failed. When dense search failed and BM25 found nothing, the gate cannot judge without the dense
/// result, so the outcome is `NotUsed` and not `NotCovered`.
pub fn decide(dense: Option<Vec<Hit>>, lexical: Option<Vec<Hit>>) -> Decision {
    decide_with(&Params::default(), dense, lexical)
}

/// `decide` with explicit numbers.
pub fn decide_with(
    params: &Params,
    dense: Option<Vec<Hit>>,
    lexical: Option<Vec<Hit>>,
) -> Decision {
    if dense.is_none() && lexical.as_ref().is_none_or(Vec::is_empty) {
        return Decision {
            outcome: Outcome::NotUsed,
            dense: Vec::new(),
            lexical: Vec::new(),
            fused: Vec::new(),
        };
    }
    let (dense, lexical) = (dense.unwrap_or_default(), lexical.unwrap_or_default());
    let best_dense = dense.first().map(|h| h.value);
    let fused = if passes_gate_with(params, best_dense, lexical.len()) {
        fuse_with(params, &dense, &lexical)
    } else {
        Vec::new()
    };
    let outcome = if fused.is_empty() {
        Outcome::NotCovered
    } else {
        Outcome::Found
    };
    Decision {
        outcome,
        dense,
        lexical,
        fused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(doc: &str, n: u32, value: f32) -> Hit {
        Hit {
            passage: Passage {
                key: format!("{doc}:{n}"),
                document_id: doc.to_owned(),
                page: n + 1,
                heading: String::new(),
                text: format!("text of {doc} {n}"),
            },
            value,
        }
    }

    fn keys(fused: &[Fused]) -> Vec<&str> {
        fused.iter().map(|f| f.passage.key.as_str()).collect()
    }

    #[test]
    fn rrf_adds_one_over_sixty_plus_rank_for_each_list() {
        let dense = [hit("A", 0, 0.1), hit("B", 0, 0.2)];
        let lexical = [hit("B", 0, 9.0), hit("C", 0, 5.0)];
        let fused = fuse(&dense, &lexical);
        // B is rank 2 in dense and rank 1 in lexical: it wins over A (rank 1 in one list only).
        assert_eq!(keys(&fused), ["B:0", "A:0", "C:0"]);
        let b = &fused[0];
        assert!((b.score - (1.0 / 62.0 + 1.0 / 61.0)).abs() < 1e-12);
        assert_eq!((b.dense_rank, b.lexical_rank), (Some(2), Some(1)));
        let a = &fused[1];
        assert!((a.score - 1.0 / 61.0).abs() < 1e-12);
        assert_eq!((a.dense_rank, a.lexical_rank), (Some(1), None));
        assert_eq!(fused[2].lexical_rank, Some(2));
    }

    #[test]
    fn ties_are_broken_by_the_better_rank_then_by_key() {
        // A:0 is first in dense, B:0 is first in lexical: equal scores, equal best rank, so the key decides.
        let fused = fuse(&[hit("A", 0, 0.1)], &[hit("B", 0, 9.0)]);
        assert_eq!(keys(&fused), ["A:0", "B:0"]);
        // The same score again, with the keys the other way round: the key still decides.
        let fused = fuse(&[hit("Z", 0, 0.1)], &[hit("Y", 0, 9.0), hit("X", 0, 8.0)]);
        assert_eq!(keys(&fused), ["Y:0", "Z:0", "X:0"]);
        // The result does not depend on the order of the inputs inside a tie.
        let again = fuse(&[hit("Z", 0, 0.1)], &[hit("Y", 0, 9.0), hit("X", 0, 8.0)]);
        assert_eq!(keys(&fused), keys(&again));
    }

    #[test]
    fn at_most_eight_sources_and_four_per_document() {
        let dense: Vec<Hit> = (0..40)
            .map(|i| hit("A", i, 0.1 + i as f32 * 0.001))
            .collect();
        let lexical: Vec<Hit> = (0..40)
            .map(|i| hit("A", i, 9.0 - i as f32 * 0.01))
            .collect();
        let fused = fuse(&dense, &lexical);
        assert_eq!(
            fused.len(),
            MAX_PER_DOCUMENT,
            "one document gives at most four"
        );
        // Other documents fill the rest, in fused order.
        let more: Vec<Hit> = (0..10)
            .map(|i| hit(&format!("D{}", i % 5), i, 0.5))
            .collect();
        let mixed = fuse(&[dense.clone(), more.clone()].concat(), &lexical);
        assert_eq!(mixed.len(), FINAL_SOURCES);
        let mut per: HashMap<&str, usize> = HashMap::new();
        for f in &mixed {
            *per.entry(f.passage.document_id.as_str()).or_default() += 1;
        }
        assert!(per.values().all(|n| *n <= MAX_PER_DOCUMENT), "{per:?}");
        assert!(
            mixed.windows(2).all(|w| w[0].score >= w[1].score),
            "fused order"
        );
    }

    #[test]
    fn the_cap_skips_the_extra_chunks_of_a_busy_document_and_keeps_going() {
        let dense = [
            hit("A", 0, 0.1),
            hit("A", 1, 0.1),
            hit("A", 2, 0.1),
            hit("A", 3, 0.1),
            hit("A", 4, 0.1),
            hit("B", 0, 0.1),
        ];
        let fused = fuse(&dense, &[]);
        assert_eq!(keys(&fused), ["A:0", "A:1", "A:2", "A:3", "B:0"]);
    }

    #[test]
    fn repeated_keys_in_one_list_count_once_and_empty_lists_work() {
        let fused = fuse(&[hit("A", 0, 0.1), hit("A", 0, 0.2)], &[]);
        assert_eq!(fused.len(), 1);
        assert!((fused[0].score - 1.0 / 61.0).abs() < 1e-12);
        assert!(fuse(&[], &[]).is_empty());
    }

    #[test]
    fn the_gate_needs_a_close_dense_result_or_a_bm25_hit() {
        assert!(passes_gate(Some(0.2), 0));
        assert!(passes_gate(Some(0.85), 0), "the limit itself passes");
        assert!(!passes_gate(Some(0.86), 0));
        assert!(passes_gate(Some(0.9), 1), "one BM25 hit is enough");
        // Distances from real Titan v2 traces on a quadratics PDF.
        assert!(
            passes_gate(Some(0.678), 0),
            "a relevant question with no BM25 hit passes"
        );
        assert!(passes_gate(Some(0.688), 0));
        assert!(passes_gate(Some(0.455), 0));
        assert!(
            !passes_gate(Some(0.944), 0),
            "an unrelated question with no BM25 hit is gated"
        );
        assert!(!passes_gate(Some(0.964), 0));
        assert!(passes_gate(Some(0.944), 2), "BM25 hits keep the sources");
        assert!(passes_gate(None, 3));
        assert!(!passes_gate(None, 0));
    }

    #[test]
    fn queries_are_cleaned() {
        assert_eq!(
            clean_query("  \"pythagorean theorem  proof\"\nextra line"),
            Some("pythagorean theorem proof".into())
        );
        assert_eq!(clean_query("`code`"), Some("code".into()));
        assert_eq!(clean_query(" \n  "), None);
        assert_eq!(clean_query("\"\""), None);
        assert!(clean_query(&"w ".repeat(1000)).unwrap().chars().count() <= MAX_QUERY_CHARS);
    }

    #[test]
    fn clip_never_cuts_inside_a_character() {
        assert_eq!(clip("éééé", 2), "éé");
        assert_eq!(clip("short", 100), "short");
    }

    #[test]
    fn decide_gates_fuses_and_tells_a_failure_from_a_gap() {
        let near = vec![hit("A", 0, 0.5)];
        let far = vec![hit("A", 0, 0.95)];
        let found = decide(Some(near.clone()), Some(Vec::new()));
        assert_eq!(found.outcome, Outcome::Found);
        assert_eq!(keys(&found.fused), ["A:0"]);
        // A far dense result and no BM25 hit: the gate says the course does not cover it.
        let gap = decide(Some(far.clone()), Some(Vec::new()));
        assert_eq!(gap.outcome, Outcome::NotCovered);
        assert!(gap.fused.is_empty());
        // A BM25 hit keeps the sources even when dense is far.
        let kept = decide(Some(far), Some(vec![hit("B", 1, 3.0)]));
        assert_eq!(kept.outcome, Outcome::Found);
        // Dense failed and BM25 found nothing: the gate cannot judge.
        assert_eq!(decide(None, Some(Vec::new())).outcome, Outcome::NotUsed);
        assert_eq!(decide(None, None).outcome, Outcome::NotUsed);
        // Dense failed but BM25 found something: the gate passes (no dense distance, one lexical hit).
        assert_eq!(
            decide(None, Some(vec![hit("B", 1, 3.0)])).outcome,
            Outcome::Found
        );
    }

    struct Stub(Result<Vec<Hit>, String>);

    #[async_trait]
    impl DenseSearch for Stub {
        async fn search(&self, _query: &str) -> Result<DenseResult, String> {
            self.0.clone().map(|hits| DenseResult {
                hits,
                embed_tokens: 7,
            })
        }
    }

    #[async_trait]
    impl LexicalSearch for Stub {
        async fn search(&self, _query: &str) -> Result<Vec<Hit>, String> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn search_both_keeps_the_good_retriever_and_names_the_failed_one() {
        let ok = Stub(Ok(vec![hit("A", 0, 0.5)]));
        let bad = Stub(Err("boom".into()));
        let found = search_both(&ok, &bad, "q", Duration::from_secs(1)).await;
        assert_eq!(found.embed_tokens, 7);
        assert!(found.dense.is_some() && found.lexical.is_none());
        assert_eq!(found.errors, ["lexical failed"]);
    }

    #[test]
    fn explicit_numbers_change_the_cap_the_count_and_the_gate() {
        let dense: Vec<Hit> = (0..10).map(|i| hit("A", i, 0.5)).collect();
        let wide = Params {
            max_per_document: 10,
            final_sources: 10,
            ..Params::default()
        };
        assert_eq!(fuse_with(&wide, &dense, &[]).len(), 10);
        assert_eq!(fuse(&dense, &[]).len(), MAX_PER_DOCUMENT);
        let strict = Params {
            gate_max_distance: 0.4,
            ..Params::default()
        };
        assert_eq!(
            decide_with(&strict, Some(dense.clone()), Some(Vec::new())).outcome,
            Outcome::NotCovered
        );
        assert_eq!(
            decide(Some(dense), Some(Vec::new())).outcome,
            Outcome::Found
        );
        // A smaller k makes the first rank count more.
        let sharp = Params {
            rrf_k: 1.0,
            ..Params::default()
        };
        let score = fuse_with(&sharp, &[hit("A", 0, 0.1)], &[])[0].score;
        assert!((score - 0.5).abs() < 1e-12);
    }
}
