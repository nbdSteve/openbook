//! The retrieval core that the API and the evaluation share (`docs/evaluation.md`).
//!
//! It holds the pure parts: query cleaning, reciprocal rank fusion, the per-document cap, and the
//! relevance gate. Two small traits stand for the dense and the lexical backend, so the API (S3
//! Vectors and Tantivy from S3) and the evaluation (an in-memory index) run exactly this code.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;

// ---- The numbers. They are tuned against the evaluation set (`docs/evaluation.md`). Keep them here.
/// Candidates from the dense search.
pub const DENSE_TOP_K: u32 = 40;
/// Candidates from BM25.
pub const LEXICAL_TOP_K: usize = 40;
/// Reciprocal rank fusion: `score = sum over lists of weight / (RRF_K + rank)`, with rank from 1.
pub const RRF_K: f64 = 60.0;
/// The weight of the BM25 list in the fusion. The dense list has weight 1.
///
/// Keep it at 1.0 or more. A chunk that only BM25 finds has the best score `weight / (RRF_K + 1)`, and
/// the worst chunk that only dense search finds has `1 / (RRF_K + DENSE_TOP_K)`. With weight 0.5 that
/// is 0.0082 against 0.0100, so BM25 could only re-order dense results and never add a source. That
/// would remove the reason for hybrid search: an exact match (notation, a keyword) that dense ranks
/// low. A test pins this. Weight 0.5 looked better on the golden set, and it was rejected for that
/// property. Round 2 (`evals/results/2026-11-18.md`) tested it again with the running headers gone and
/// a prose corpus: MRR 0.8806 to 0.8980 pooled (round 1: 0.852 to 0.901, so a good part of that gain came
/// from the headers in the BM25 heading field), Recall@8 the same, and a lower MRR on the prose corpus
/// (0.8936 to 0.8830). That is not strong evidence for giving up the property.
pub const LEXICAL_WEIGHT: f64 = 1.0;
/// Sources that go to the model and the student.
pub const FINAL_SOURCES: usize = 8;
/// Most sources from one document while other candidates are left. The cap is soft: when fewer than
/// `FINAL_SOURCES` candidates pass it, the free places go to the skipped candidates, in fused order.
/// On the two corpora pooled (round 2), 6 gives Recall@8 0.9756 and nDCG@8 0.6129, against 0.9431 and
/// 0.5555 for 4. The cap costs 0.027 of nDCG@8 against no cap (0.6394): it protects the source
/// diversity of a course with several documents, which the golden sets do not measure.
pub const MAX_PER_DOCUMENT: usize = 6;

/// The embedding model that the stack uses by default. One definition, in `openbook-core`. The CDK
/// default (`infra/lib/config.ts`) is tied to it by an infra test.
pub const TITAN_V2_MODEL_ID: &str = openbook_core::config::DEFAULT_EMBEDDING_MODEL_ID;
/// The hashed bag-of-words embedder of local runs and tests.
pub const FAKE_MODEL_ID: &str = openbook_core::config::FAKE_EMBEDDING_MODEL_ID;
/// The gate distance for Titan v2 (see `GateRule`): 0.79 since round 2 (it was 0.78).
///
/// The rule that chose it: no false gate on the golden questions, and a margin of at least 0.02
/// between the limit and the worst covered question; among the limits that pass, the highest gate
/// accuracy. The margin is a safety condition and it comes first: a false gate (a student with a
/// real course question gets no sources) costs more than a missed gate (the question gets sources
/// that the answer may not cite, and the UI shows only the cited ones).
///
/// The numbers (`evals/results/2026-11-18.md`), over both corpora and 123 covered questions: the
/// worst covered question is 0.764 (a college-algebra paraphrase), so 0.78 has a margin of 0.016 and
/// fails, and 0.79 has 0.026 and passes. 0.79 costs gate accuracy (0.9000 to 0.8722: five more
/// `not_covered` questions get through, because the adjacent topics of a course look like the course).
/// All 24 `off_topic` questions (distances 0.82 and farther) are still gated. The false-gate rate is
/// 0 on both corpora.
///
/// "No false gate" is in-sample: the limit was chosen on the golden questions that the retrieval was
/// tuned on. A real course can have a covered question that is farther, and then the student gets no
/// sources. The trace of every turn records the real distances (`trace.gate`), so the next round can
/// re-measure. The distance is only valid for Titan v2.
pub const GATE_TITAN_V2_MAX_DISTANCE: f32 = 0.79;

/// What the relevance gate reads about the words of a query (see `Bm25Reader::search_with_stats`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TermStats {
    /// Distinct analyzed terms of the query.
    pub query_terms: usize,
    /// Of those, the terms that occur anywhere in the index.
    pub indexed_terms: usize,
    /// Of those, the terms that the best BM25 hit holds.
    pub matched_terms: usize,
}

/// What the relevance gate reads for one query.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GateInputs {
    /// The cosine distance of the closest dense result. `None` when dense search found nothing.
    pub best_dense_distance: Option<f32>,
    /// The number of BM25 hits.
    pub lexical_hits: usize,
    /// The score of the best BM25 hit.
    pub top_bm25_score: Option<f32>,
    /// The terms of the query and how many of them the best BM25 hit holds.
    pub terms: TermStats,
}

impl GateInputs {
    /// The share of the distinct query terms that the best BM25 hit holds. 0 when there are no terms.
    pub fn term_coverage(&self) -> f64 {
        if self.terms.query_terms == 0 {
            0.0
        } else {
            self.terms.matched_terms as f64 / self.terms.query_terms as f64
        }
    }
}

/// The relevance gate: the rule that decides whether a query has sources at all.
///
/// A distance threshold belongs to one embedding model: the distances of Titan v2 and of the fake
/// embedder have different scales. `GateRule::for_embedding_model` gives the rule of a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GateRule {
    /// Passes when the closest dense result is within `max_distance`, or BM25 found any hit. This
    /// was the first rule. BM25 is an OR of terms, so nearly every query has a hit: the rule almost
    /// never gates. The fake embedder uses it, because its distances are not calibrated.
    AnyLexicalHit { max_distance: f32 },
    /// Passes when the closest dense result is within `max_distance`.
    Dense { max_distance: f32 },
    /// Passes when the closest dense result is within `max_distance`, or the best BM25 hit has a
    /// strong lexical match: it holds at least `min_coverage` of the query terms and at least
    /// `min_matched` of them. This catches queries that embed badly, such as notation.
    DenseOrLexical {
        max_distance: f32,
        min_coverage: f64,
        min_matched: usize,
    },
}

impl GateRule {
    /// The rule for an embedding model. A model that has not been measured gets the permissive first
    /// rule: run the evaluation with that model before you trust its distances.
    pub fn for_embedding_model(model_id: &str) -> GateRule {
        match model_id {
            TITAN_V2_MODEL_ID => GateRule::Dense {
                max_distance: GATE_TITAN_V2_MAX_DISTANCE,
            },
            _ => GateRule::fallback(),
        }
    }

    /// The first rule: the closest distance within 0.85, or any BM25 hit. A turn uses it when the
    /// search query is the raw message of a follow-up whose rewrite failed, because the raw message
    /// ("can you explain the second step again?") is far from every chunk.
    pub fn fallback() -> GateRule {
        GateRule::AnyLexicalHit { max_distance: 0.85 }
    }

    /// True when the evaluation measured the distance limit for this embedding model. The fake
    /// embedder is not measured on purpose: its rule only has to keep local runs sensible.
    pub fn is_calibrated(model_id: &str) -> bool {
        model_id == TITAN_V2_MODEL_ID
    }

    /// The name of the rule, for traces and logs.
    pub fn name(&self) -> &'static str {
        match self {
            GateRule::AnyLexicalHit { .. } => "any_lexical_hit",
            GateRule::Dense { .. } => "dense",
            GateRule::DenseOrLexical { .. } => "dense_or_lexical",
        }
    }

    /// The dense distance limit of the rule.
    pub fn max_distance(&self) -> f32 {
        match self {
            GateRule::AnyLexicalHit { max_distance }
            | GateRule::Dense { max_distance }
            | GateRule::DenseOrLexical { max_distance, .. } => *max_distance,
        }
    }

    /// The same rule with another distance limit.
    pub fn with_max_distance(self, max_distance: f32) -> GateRule {
        match self {
            GateRule::AnyLexicalHit { .. } => GateRule::AnyLexicalHit { max_distance },
            GateRule::Dense { .. } => GateRule::Dense { max_distance },
            GateRule::DenseOrLexical {
                min_coverage,
                min_matched,
                ..
            } => GateRule::DenseOrLexical {
                max_distance,
                min_coverage,
                min_matched,
            },
        }
    }

    fn dense_is_close(&self, inputs: &GateInputs) -> bool {
        inputs
            .best_dense_distance
            .is_some_and(|d| d <= self.max_distance())
    }

    fn lexical_passes(&self, inputs: &GateInputs) -> bool {
        match self {
            GateRule::AnyLexicalHit { .. } | GateRule::Dense { .. } => inputs.lexical_hits > 0,
            GateRule::DenseOrLexical {
                min_coverage,
                min_matched,
                ..
            } => {
                inputs.lexical_hits > 0
                    && inputs.term_coverage() >= *min_coverage
                    && inputs.terms.matched_terms >= *min_matched
            }
        }
    }

    /// True when the query has sources.
    pub fn passes(&self, inputs: &GateInputs) -> bool {
        match self {
            GateRule::AnyLexicalHit { .. } => {
                self.dense_is_close(inputs) || inputs.lexical_hits > 0
            }
            GateRule::Dense { .. } => self.dense_is_close(inputs),
            GateRule::DenseOrLexical { .. } => {
                self.dense_is_close(inputs) || self.lexical_passes(inputs)
            }
        }
    }

    /// The dense half of the rule alone (the dense-only run of the evaluation).
    pub fn passes_dense_only(&self, inputs: &GateInputs) -> bool {
        self.dense_is_close(inputs)
    }

    /// The lexical half of the rule alone (the BM25-only run of the evaluation). For a rule without
    /// a lexical part, any BM25 hit passes.
    pub fn passes_lexical_only(&self, inputs: &GateInputs) -> bool {
        self.lexical_passes(inputs)
    }
}

/// The tunable numbers of fusion and the gate. `Default` is what production uses with the default
/// embedding model. The evaluation changes them to compare settings; the API never reads flags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub rrf_k: f64,
    pub lexical_weight: f64,
    pub final_sources: usize,
    pub max_per_document: usize,
    pub gate: GateRule,
}

impl Params {
    /// The production numbers for an embedding model.
    pub fn for_embedding_model(model_id: &str) -> Self {
        Self {
            rrf_k: RRF_K,
            lexical_weight: LEXICAL_WEIGHT,
            final_sources: FINAL_SOURCES,
            max_per_document: MAX_PER_DOCUMENT,
            gate: GateRule::for_embedding_model(model_id),
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Self::for_embedding_model(TITAN_V2_MODEL_ID)
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
/// is in fused order, at most `FINAL_SOURCES` long, with the soft per-document cap of `fuse_with`.
pub fn fuse(dense: &[Hit], lexical: &[Hit]) -> Vec<Fused> {
    fuse_with(&Params::default(), dense, lexical)
}

/// `fuse` with explicit numbers.
///
/// The per-document cap is soft. A first pass takes candidates in fused order and skips a document
/// that already has `max_per_document` places. When that leaves free places, they go to the skipped
/// candidates in fused order. The final list is in fused order. So a course with one document still
/// gets `final_sources` sources, and a course with several documents still gets a mix.
pub fn fuse_with(params: &Params, dense: &[Hit], lexical: &[Hit]) -> Vec<Fused> {
    let mut by_key: HashMap<&str, Fused> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (list, is_dense) in [(dense, true), (lexical, false)] {
        let weight = if is_dense { 1.0 } else { params.lexical_weight };
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
                entry.score += weight / (params.rrf_k + rank as f64);
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
    // First pass: the capped list. Candidates are numbered by fused order.
    let mut per_document: HashMap<String, usize> = HashMap::new();
    let mut kept: Vec<(usize, Fused)> = Vec::new();
    let mut skipped: Vec<(usize, Fused)> = Vec::new();
    for (position, candidate) in all.into_iter().enumerate() {
        if kept.len() == params.final_sources {
            break;
        }
        let count = per_document
            .entry(candidate.passage.document_id.clone())
            .or_insert(0);
        if *count >= params.max_per_document {
            skipped.push((position, candidate));
        } else {
            *count += 1;
            kept.push((position, candidate));
        }
    }
    // Second pass: free places go to the skipped candidates, best first.
    let free = params.final_sources.saturating_sub(kept.len());
    kept.extend(skipped.into_iter().take(free));
    kept.sort_by_key(|(position, _)| *position);
    kept.into_iter().map(|(_, f)| f).collect()
}

fn best_rank(f: &Fused) -> usize {
    [f.dense_rank, f.lexical_rank]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(usize::MAX)
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
    async fn search(&self, query: &str) -> Result<LexicalResult, String>;
}

/// The result of a BM25 search.
#[derive(Debug, Clone, Default)]
pub struct LexicalResult {
    /// Candidates, best first.
    pub hits: Vec<Hit>,
    /// The term statistics that the relevance gate reads.
    pub terms: TermStats,
}

/// What the two retrievers returned. A retriever that failed has `None` and an entry in `errors`.
#[derive(Debug, Clone, Default)]
pub struct Searched {
    pub embed_tokens: u64,
    pub dense: Option<Vec<Hit>>,
    pub lexical: Option<Vec<Hit>>,
    /// The term statistics of the BM25 search (zero when it failed).
    pub terms: TermStats,
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
    let mut terms = TermStats::default();
    let lexical = keep(
        "lexical",
        lexical.map(|found| {
            terms = found.terms;
            found.hits
        }),
    );
    Searched {
        embed_tokens,
        dense,
        lexical,
        terms,
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
    /// What the gate read.
    pub gate: GateInputs,
    /// The fused sources. Empty unless the outcome is `Found`.
    pub fused: Vec<Fused>,
}

/// The inputs of the relevance gate for two candidate lists.
pub fn gate_inputs(dense: &[Hit], lexical: &[Hit], terms: TermStats) -> GateInputs {
    GateInputs {
        best_dense_distance: dense.first().map(|h| h.value),
        lexical_hits: lexical.len(),
        top_bm25_score: lexical.first().map(|h| h.value),
        terms,
    }
}

/// Applies the relevance gate and the fusion to the two candidate lists. `None` means the retriever
/// failed. An empty dense list counts as a failed dense search: the caller drops the dense hits of
/// documents that are not ready, so an empty list can mean "the documents are still processing", and
/// the gate cannot judge without a distance. When dense search failed and BM25 found nothing, the
/// outcome is `NotUsed` and not `NotCovered`. When dense search failed and BM25 found something, the
/// gate passes: the lexical hits are all that there is.
pub fn decide_with(
    params: &Params,
    dense: Option<Vec<Hit>>,
    lexical: Option<Vec<Hit>>,
    terms: TermStats,
) -> Decision {
    let dense = dense.filter(|hits| !hits.is_empty());
    if dense.is_none() && lexical.as_ref().is_none_or(Vec::is_empty) {
        return Decision {
            outcome: Outcome::NotUsed,
            dense: Vec::new(),
            lexical: Vec::new(),
            gate: GateInputs::default(),
            fused: Vec::new(),
        };
    }
    let dense_failed = dense.is_none();
    let (dense, lexical) = (dense.unwrap_or_default(), lexical.unwrap_or_default());
    let gate = gate_inputs(&dense, &lexical, terms);
    let fused = if dense_failed || params.gate.passes(&gate) {
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
        gate,
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

    /// The two lists count the same, so the tests of the fusion read simply.
    fn equal_weights() -> Params {
        Params {
            lexical_weight: 1.0,
            ..Params::default()
        }
    }

    #[test]
    fn rrf_adds_one_over_sixty_plus_rank_for_each_list() {
        let dense = [hit("A", 0, 0.1), hit("B", 0, 0.2)];
        let lexical = [hit("B", 0, 9.0), hit("C", 0, 5.0)];
        let fused = fuse_with(&equal_weights(), &dense, &lexical);
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
        let fused = fuse_with(&equal_weights(), &[hit("A", 0, 0.1)], &[hit("B", 0, 9.0)]);
        assert_eq!(keys(&fused), ["A:0", "B:0"]);
        // The same score again, with the keys the other way round: the key still decides.
        let fused = fuse_with(
            &equal_weights(),
            &[hit("Z", 0, 0.1)],
            &[hit("Y", 0, 9.0), hit("X", 0, 8.0)],
        );
        assert_eq!(keys(&fused), ["Y:0", "Z:0", "X:0"]);
        // The result does not depend on the order of the inputs inside a tie.
        let again = fuse_with(
            &equal_weights(),
            &[hit("Z", 0, 0.1)],
            &[hit("Y", 0, 9.0), hit("X", 0, 8.0)],
        );
        assert_eq!(keys(&fused), keys(&again));
    }

    fn per_document(fused: &[Fused]) -> HashMap<&str, usize> {
        let mut per: HashMap<&str, usize> = HashMap::new();
        for f in fused {
            *per.entry(f.passage.document_id.as_str()).or_default() += 1;
        }
        per
    }

    #[test]
    fn a_course_with_one_document_still_gets_eight_sources() {
        // The cap is soft: the free places go to the skipped candidates of the same document.
        let dense: Vec<Hit> = (0..40)
            .map(|i| hit("A", i, 0.1 + i as f32 * 0.001))
            .collect();
        let lexical: Vec<Hit> = (0..40)
            .map(|i| hit("A", i, 9.0 - i as f32 * 0.01))
            .collect();
        let fused = fuse(&dense, &lexical);
        assert_eq!(fused.len(), FINAL_SOURCES);
        assert!(fused.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn the_cap_keeps_a_mix_when_other_documents_have_candidates() {
        let dense: Vec<Hit> = (0..40)
            .map(|i| hit("A", i, 0.1 + i as f32 * 0.001))
            .collect();
        let more: Vec<Hit> = (0..10)
            .map(|i| hit(&format!("D{}", i % 5), i, 0.5))
            .collect();
        let lexical: Vec<Hit> = (0..40)
            .map(|i| hit("A", i, 9.0 - i as f32 * 0.01))
            .collect();
        let mixed = fuse(&[dense, more].concat(), &lexical);
        assert_eq!(mixed.len(), FINAL_SOURCES);
        let per = per_document(&mixed);
        assert!(per.values().all(|n| *n <= MAX_PER_DOCUMENT), "{per:?}");
        assert!(mixed.iter().any(|f| f.passage.document_id != "A"));
        assert!(mixed.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn the_cap_skips_a_busy_document_first_and_fills_the_free_places_in_fused_order() {
        let dense = [
            hit("A", 0, 0.1),
            hit("A", 1, 0.1),
            hit("A", 2, 0.1),
            hit("A", 3, 0.1),
            hit("A", 4, 0.1),
            hit("B", 0, 0.1),
        ];
        let params = Params {
            final_sources: 5,
            max_per_document: 4,
            ..Params::default()
        };
        // The first pass takes A:0 to A:3 and B:0 (A:4 is over the cap). That fills the five places.
        assert_eq!(
            keys(&fuse_with(&params, &dense, &[])),
            ["A:0", "A:1", "A:2", "A:3", "B:0"]
        );
        // With six places, A:4 comes back into the free place, and the list stays in fused order.
        let six = Params {
            max_per_document: 4,
            ..Params::default()
        };
        let fused = fuse_with(&six, &dense, &[]);
        assert_eq!(keys(&fused), ["A:0", "A:1", "A:2", "A:3", "A:4", "B:0"]);
    }

    #[test]
    fn the_free_places_go_to_the_best_skipped_candidates_even_across_documents() {
        // A has 6 candidates, B has 2, and there are 7 places and a cap of 3.
        let dense = [
            hit("A", 0, 0.1),
            hit("A", 1, 0.1),
            hit("A", 2, 0.1),
            hit("A", 3, 0.1),
            hit("B", 0, 0.1),
            hit("A", 4, 0.1),
            hit("B", 1, 0.1),
            hit("B", 2, 0.1),
            hit("B", 3, 0.1),
            hit("A", 5, 0.1),
        ];
        let params = Params {
            final_sources: 7,
            max_per_document: 3,
            ..Params::default()
        };
        let fused = fuse_with(&params, &dense, &[]);
        // The first pass keeps A:0 A:1 A:2 B:0 B:1 B:2. A:3, A:4, B:3, and A:5 are over the cap.
        // One place is free, and it goes to A:3, the best skipped candidate.
        assert_eq!(
            keys(&fused),
            ["A:0", "A:1", "A:2", "A:3", "B:0", "B:1", "B:2"]
        );
    }

    #[test]
    fn the_lexical_weight_changes_the_fusion() {
        let dense = [hit("A", 0, 0.1)];
        let lexical = [hit("B", 0, 9.0)];
        let heavy = Params {
            lexical_weight: 2.0,
            ..Params::default()
        };
        assert_eq!(keys(&fuse_with(&heavy, &dense, &lexical)), ["B:0", "A:0"]);
        let light = Params {
            lexical_weight: 0.5,
            ..Params::default()
        };
        let fused = fuse_with(&light, &dense, &lexical);
        assert_eq!(keys(&fused), ["A:0", "B:0"]);
        assert!((fused[1].score - 0.5 / 61.0).abs() < 1e-12);
    }

    #[test]
    fn repeated_keys_in_one_list_count_once_and_empty_lists_work() {
        let fused = fuse(&[hit("A", 0, 0.1), hit("A", 0, 0.2)], &[]);
        assert_eq!(fused.len(), 1);
        assert!((fused[0].score - 1.0 / 61.0).abs() < 1e-12);
        assert!(fuse(&[], &[]).is_empty());
    }

    fn inputs(
        distance: Option<f32>,
        hits: usize,
        query_terms: usize,
        matched: usize,
    ) -> GateInputs {
        GateInputs {
            best_dense_distance: distance,
            lexical_hits: hits,
            top_bm25_score: (hits > 0).then_some(3.0),
            terms: TermStats {
                query_terms,
                indexed_terms: query_terms,
                matched_terms: matched,
            },
        }
    }

    #[test]
    fn the_first_rule_passes_on_a_close_dense_result_or_any_bm25_hit() {
        let rule = GateRule::AnyLexicalHit { max_distance: 0.85 };
        assert!(rule.passes(&inputs(Some(0.2), 0, 3, 0)));
        assert!(
            rule.passes(&inputs(Some(0.85), 0, 3, 0)),
            "the limit passes"
        );
        assert!(!rule.passes(&inputs(Some(0.86), 0, 3, 0)));
        assert!(
            rule.passes(&inputs(Some(0.9), 1, 3, 1)),
            "one hit is enough"
        );
        assert!(!rule.passes(&inputs(None, 0, 0, 0)));
    }

    #[test]
    fn the_dense_rule_ignores_bm25() {
        let rule = GateRule::Dense { max_distance: 0.7 };
        assert!(rule.passes(&inputs(Some(0.7), 0, 3, 0)));
        assert!(
            !rule.passes(&inputs(Some(0.71), 40, 3, 3)),
            "BM25 cannot save it"
        );
        assert!(!rule.passes(&inputs(None, 5, 3, 3)));
        assert!(rule.passes_dense_only(&inputs(Some(0.5), 0, 3, 0)));
    }

    #[test]
    fn the_combined_rule_lets_a_strong_lexical_match_through() {
        let rule = GateRule::DenseOrLexical {
            max_distance: 0.7,
            min_coverage: 0.75,
            min_matched: 3,
        };
        assert!(
            rule.passes(&inputs(Some(0.6), 0, 4, 0)),
            "close dense passes"
        );
        // Dense is far, but the best hit holds 3 of 4 terms.
        assert!(rule.passes(&inputs(Some(0.9), 10, 4, 3)));
        // Not enough of the terms.
        assert!(!rule.passes(&inputs(Some(0.9), 10, 4, 2)));
        // A short query: full coverage but fewer than three matched terms.
        assert!(!rule.passes(&inputs(Some(0.9), 10, 2, 2)));
        assert!(!rule.passes(&inputs(Some(0.9), 0, 0, 0)));
        assert!(rule.passes_lexical_only(&inputs(None, 10, 4, 4)));
        assert!(!rule.passes_dense_only(&inputs(Some(0.9), 10, 4, 4)));
    }

    #[test]
    fn the_distance_belongs_to_the_embedding_model() {
        // Titan v2 has a calibrated dense-only rule. The fake embedder and unknown models keep the
        // permissive first rule, so a Titan distance never gates their different scale.
        assert!(matches!(
            GateRule::for_embedding_model(TITAN_V2_MODEL_ID),
            GateRule::Dense { .. }
        ));
        for model in [FAKE_MODEL_ID, "some.other-model"] {
            let rule = GateRule::for_embedding_model(model);
            assert!(matches!(rule, GateRule::AnyLexicalHit { .. }), "{model}");
            // A far dense result with a BM25 hit still has sources.
            assert!(rule.passes(&inputs(Some(0.99), 3, 5, 1)));
        }
        assert_eq!(
            Params::default(),
            Params::for_embedding_model(TITAN_V2_MODEL_ID)
        );
        assert_eq!(
            Params::for_embedding_model(FAKE_MODEL_ID).gate,
            GateRule::for_embedding_model(FAKE_MODEL_ID)
        );
    }

    #[test]
    fn the_inputs_of_the_gate_come_from_the_two_lists() {
        let terms = TermStats {
            query_terms: 4,
            indexed_terms: 3,
            matched_terms: 2,
        };
        let g = gate_inputs(
            &[hit("A", 0, 0.4), hit("A", 1, 0.6)],
            &[hit("B", 0, 7.5)],
            terms,
        );
        assert_eq!(g.best_dense_distance, Some(0.4));
        assert_eq!((g.lexical_hits, g.top_bm25_score), (1, Some(7.5)));
        assert!((g.term_coverage() - 0.5).abs() < 1e-12);
        let none = gate_inputs(&[], &[], TermStats::default());
        assert_eq!(
            (none.best_dense_distance, none.top_bm25_score),
            (None, None)
        );
        assert_eq!(none.term_coverage(), 0.0);
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
        let params = Params {
            gate: GateRule::Dense { max_distance: 0.85 },
            ..Params::default()
        };
        let no_terms = TermStats::default();
        let near = vec![hit("A", 0, 0.5)];
        let far = vec![hit("A", 0, 0.95)];
        let found = decide_with(&params, Some(near), Some(Vec::new()), no_terms);
        assert_eq!(found.outcome, Outcome::Found);
        assert_eq!(keys(&found.fused), ["A:0"]);
        // A far dense result: the gate says the course does not cover it, even with a BM25 hit.
        let gap = decide_with(
            &params,
            Some(far.clone()),
            Some(vec![hit("B", 1, 3.0)]),
            no_terms,
        );
        assert_eq!(gap.outcome, Outcome::NotCovered);
        assert!(gap.fused.is_empty());
        assert_eq!(gap.gate.best_dense_distance, Some(0.95));
        assert_eq!(gap.gate.lexical_hits, 1);
        // The first rule lets a BM25 hit keep the sources.
        let first = Params {
            gate: GateRule::AnyLexicalHit { max_distance: 0.85 },
            ..Params::default()
        };
        let kept = decide_with(&first, Some(far), Some(vec![hit("B", 1, 3.0)]), no_terms);
        assert_eq!(kept.outcome, Outcome::Found);
        // Dense failed and BM25 found nothing: the gate cannot judge.
        let failed = |dense, lexical| decide_with(&params, dense, lexical, no_terms).outcome;
        assert_eq!(failed(None, Some(Vec::new())), Outcome::NotUsed);
        assert_eq!(failed(None, None), Outcome::NotUsed);
        // Dense failed but BM25 found something: the gate passes, whatever the rule.
        assert_eq!(failed(None, Some(vec![hit("B", 1, 3.0)])), Outcome::Found);
        // An empty dense list (for example every dense hit belonged to a document that is still
        // processing) is a failed dense search: it does not gate a question that BM25 can answer.
        assert_eq!(
            failed(Some(Vec::new()), Some(vec![hit("B", 1, 3.0)])),
            Outcome::Found
        );
        assert_eq!(failed(Some(Vec::new()), Some(Vec::new())), Outcome::NotUsed);
    }

    #[test]
    fn with_the_production_numbers_a_chunk_that_only_bm25_finds_reaches_the_sources() {
        // The point of hybrid search: an exact match that dense ranks low (or not at all). The best
        // BM25-only score must beat the worst dense-only score, at the production k and candidates.
        let best_lexical_only = LEXICAL_WEIGHT / (RRF_K + 1.0);
        let worst_dense_only = 1.0 / (RRF_K + f64::from(DENSE_TOP_K));
        assert!(
            best_lexical_only > worst_dense_only,
            "BM25 could not add a source"
        );
        // And it does: 40 dense hits from three documents, and a BM25 rank 1 that dense lacks.
        let dense: Vec<Hit> = (0..40)
            .map(|i| hit(["A", "B", "C"][i as usize % 3], i, 0.2 + i as f32 * 0.01))
            .collect();
        let lexical = [hit("D", 99, 14.0)];
        let fused = fuse(&dense, &lexical);
        assert_eq!(fused.len(), FINAL_SOURCES);
        assert!(keys(&fused).contains(&"D:99"), "{:?}", keys(&fused));
        let found = fused.iter().find(|f| f.passage.key == "D:99").unwrap();
        assert_eq!((found.dense_rank, found.lexical_rank), (None, Some(1)));
    }

    #[test]
    fn a_follow_up_with_a_failed_rewrite_uses_the_fallback_rule() {
        let titan = GateRule::for_embedding_model(TITAN_V2_MODEL_ID);
        assert_eq!(titan.name(), "dense");
        let fallback = GateRule::fallback();
        assert_eq!(fallback.name(), "any_lexical_hit");
        // The raw message of a follow-up is far from every chunk (distance 0.9), but it shares words
        // with the course: the Titan rule gates it and the fallback keeps the sources.
        let raw = inputs(Some(0.9), 12, 4, 3);
        assert!(!titan.passes(&raw));
        assert!(fallback.passes(&raw));
        assert!(GateRule::is_calibrated(TITAN_V2_MODEL_ID));
        assert!(!GateRule::is_calibrated(FAKE_MODEL_ID));
        assert!(!GateRule::is_calibrated("amazon.titan-embed-text-v3:0"));
        assert_eq!(
            GateRule::for_embedding_model("amazon.titan-embed-text-v3:0"),
            fallback
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
        async fn search(&self, _query: &str) -> Result<LexicalResult, String> {
            self.0.clone().map(|hits| LexicalResult {
                hits,
                terms: TermStats {
                    query_terms: 3,
                    indexed_terms: 2,
                    matched_terms: 1,
                },
            })
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
        assert_eq!(found.terms, TermStats::default());
        let both = search_both(&ok, &ok, "q", Duration::from_secs(1)).await;
        assert_eq!(both.terms.matched_terms, 1);
    }

    #[test]
    fn explicit_numbers_change_the_cap_the_count_and_the_gate() {
        let dense: Vec<Hit> = (0..10).map(|i| hit("A", i, 0.5)).collect();
        let wide = Params {
            final_sources: 10,
            ..Params::default()
        };
        assert_eq!(fuse_with(&wide, &dense, &[]).len(), 10);
        let strict = Params {
            gate: GateRule::Dense { max_distance: 0.4 },
            ..Params::default()
        };
        let no_terms = TermStats::default();
        assert_eq!(
            decide_with(&strict, Some(dense.clone()), Some(Vec::new()), no_terms).outcome,
            Outcome::NotCovered
        );
        assert_eq!(
            decide_with(&Params::default(), Some(dense), Some(Vec::new()), no_terms).outcome,
            Outcome::Found
        );
        // A smaller k makes the first rank count more.
        let sharp = Params {
            rrf_k: 1.0,
            ..Params::default()
        };
        let score = fuse_with(&sharp, &[hit("A", 0, 0.1)], &[])[0].score;
        assert!((score - 0.5).abs() < 1e-12);
        // The gate keeps its kind when only the distance changes.
        let rule = GateRule::DenseOrLexical {
            max_distance: 0.7,
            min_coverage: 0.5,
            min_matched: 2,
        };
        assert_eq!(rule.with_max_distance(0.6).max_distance(), 0.6);
        assert!(matches!(
            rule.with_max_distance(0.6),
            GateRule::DenseOrLexical { min_matched: 2, .. }
        ));
    }
}
