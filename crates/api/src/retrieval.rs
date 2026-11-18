//! Hybrid retrieval for a chat turn (`docs/retrieval.md`, "Retrieval for a chat turn").
//!
//! Dense search (S3 Vectors) and BM25 (Tantivy) run in parallel. Reciprocal rank fusion merges them,
//! a relevance gate decides whether there are sources, and a trace records what happened.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use openbook_core::domain::{CourseRecord, Document, DocumentStatus, MessageRole, Source};
use openbook_core::model::{ChatModel, ChatRequest, ChatTurn, ModelEvent, TokenUsage};
use openbook_core::prompt::{REWRITE_SYSTEM_PROMPT, SourceContext, SourceMode, rewrite_user_text};
use openbook_search::bm25::Bm25ReaderCache;
use openbook_search::embed::Embedder;
pub use openbook_search::retrieval::{
    DENSE_TOP_K, FINAL_SOURCES, Fused, GateInputs, GateRule, Hit, LEXICAL_TOP_K, MAX_PER_DOCUMENT,
    MAX_QUERY_CHARS, Outcome, Params, Passage, RETRIEVER_TIMEOUT, RRF_K, clean_query, clip, fuse,
};
use openbook_search::retrieval::{
    DenseResult, DenseSearch, LexicalResult, LexicalSearch, decide_with, search_both,
};
use openbook_search::vectors::{VectorStore, index_name};
use serde::Serialize;

/// The longest snippet in a `sources` event, in characters. It is the whole chunk: the chunker's
/// limit is `MAX_CHARS` (2400), and a table or a formula that cannot be split is cut here. The model
/// gets the same clipped text, so it sees exactly what the student can read.
pub const SNIPPET_CHARS: usize = openbook_search::chunker::MAX_CHARS;
/// Candidates per retriever in the trace.
pub const TRACE_CANDIDATES: usize = 10;
/// The trace stays under this many bytes.
pub const TRACE_MAX_BYTES: usize = 32 * 1024;
/// The rewrite of a follow-up question: time, output size, and earlier messages that it sees.
pub const REWRITE_TIMEOUT: Duration = Duration::from_secs(5);
pub const REWRITE_MAX_TOKENS: i32 = 100;
pub const REWRITE_HISTORY: usize = 6;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceHit {
    pub rank: usize,
    pub key: String,
    pub document_id: String,
    pub page: u32,
    /// Cosine distance for the dense list, BM25 score for the lexical list.
    pub value: f32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceFused {
    pub rank: usize,
    pub key: String,
    pub document_id: String,
    pub page: u32,
    pub score: f64,
    pub dense_rank: Option<usize>,
    pub lexical_rank: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Timings {
    pub rewrite_ms: Option<u64>,
    pub dense_ms: Option<u64>,
    pub lexical_ms: Option<u64>,
    pub retrieve_ms: u64,
    pub fuse_ms: u64,
}

/// What retrieval did for one turn, for evaluation and teacher insights.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Trace {
    /// The query that was searched.
    pub query: String,
    /// True when the fast model rewrote the message.
    pub rewritten: bool,
    /// True when the turn needed a rewrite (a follow-up) and it failed, so the raw message was
    /// searched and the fallback gate rule was used.
    pub rewrite_failed: bool,
    pub dense: Vec<TraceHit>,
    pub lexical: Vec<TraceHit>,
    pub fused: Vec<TraceFused>,
    /// True when the relevance gate found nothing.
    pub gated: bool,
    /// What the relevance gate read (`None` when retrieval did not get that far).
    pub gate: Option<TraceGate>,
    pub timings: Timings,
    /// Retriever failures (short, no details).
    pub errors: Vec<String>,
}

impl Trace {
    /// The trace as JSON text under `TRACE_MAX_BYTES`. When it is too big, it drops the candidates
    /// that matter least: the tail of the lexical list, then the dense list, then the fused list.
    pub fn to_json_capped(&self) -> String {
        let mut trace = self.clone();
        trace.query = clip(&trace.query, MAX_QUERY_CHARS);
        trace.errors.truncate(10);
        trace.errors = trace.errors.iter().map(|e| clip(e, 200)).collect();
        trace.dense.truncate(TRACE_CANDIDATES);
        trace.lexical.truncate(TRACE_CANDIDATES);
        loop {
            let json = serde_json::to_string(&trace).unwrap_or_else(|_| "{}".to_owned());
            if json.len() <= TRACE_MAX_BYTES {
                return json;
            }
            if !trace.lexical.is_empty() {
                trace.lexical.pop();
            } else if !trace.dense.is_empty() {
                trace.dense.pop();
            } else if !trace.fused.is_empty() {
                trace.fused.pop();
            } else if !trace.errors.is_empty() {
                trace.errors.pop();
            } else {
                trace.query = clip(&trace.query, 100);
                let json = serde_json::to_string(&trace).unwrap_or_else(|_| "{}".to_owned());
                return clip(&json, TRACE_MAX_BYTES);
            }
        }
    }
}

/// The inputs of the relevance gate, for tuning it from real traces.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceGate {
    /// The name of the gate rule that this turn used.
    pub rule: String,
    pub best_dense_distance: Option<f32>,
    pub top_bm25_score: Option<f32>,
    pub term_coverage: f64,
    pub query_terms: usize,
    pub indexed_terms: usize,
    pub matched_terms: usize,
}

impl TraceGate {
    fn new(rule: &GateRule, g: &GateInputs) -> Self {
        Self {
            rule: rule.name().to_owned(),
            best_dense_distance: g.best_dense_distance,
            top_bm25_score: g.top_bm25_score,
            term_coverage: (g.term_coverage() * 1000.0).round() / 1000.0,
            query_terms: g.terms.query_terms,
            indexed_terms: g.terms.indexed_terms,
            matched_terms: g.terms.matched_terms,
        }
    }
}

fn trace_hits(hits: &[Hit]) -> Vec<TraceHit> {
    hits.iter()
        .take(TRACE_CANDIDATES)
        .enumerate()
        .map(|(i, h)| TraceHit {
            rank: i + 1,
            key: h.passage.key.clone(),
            document_id: h.passage.document_id.clone(),
            page: h.passage.page,
            value: h.value,
        })
        .collect()
}

/// The result for one turn.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub outcome: Outcome,
    /// The sources for the SSE event and the saved message.
    pub sources: Vec<Source>,
    /// The same passages, for the prompt.
    pub contexts: Vec<SourceContext>,
    /// The trace as JSON text. `None` when retrieval did not run.
    pub trace: Option<String>,
    /// Tokens that retrieval spent: the rewrite (fast model) and the query embedding.
    pub usage: TokenUsage,
}

impl TurnResult {
    fn not_used() -> Self {
        Self {
            outcome: Outcome::NotUsed,
            sources: Vec::new(),
            contexts: Vec::new(),
            trace: None,
            usage: TokenUsage::default(),
        }
    }

    pub fn mode(&self) -> SourceMode<'_> {
        match self.outcome {
            Outcome::NotUsed => SourceMode::NotUsed,
            Outcome::Found => SourceMode::Found(&self.contexts),
            Outcome::NotCovered => SourceMode::NotCovered,
        }
    }
}

/// The two retrievers.
pub struct Retrieval {
    pub embedder: Arc<dyn Embedder>,
    pub vectors: VectorStore,
    pub bm25: Bm25ReaderCache,
}

/// The dense backend for one course: Titan embeddings and S3 Vectors.
struct CourseDense<'a> {
    retrieval: &'a Retrieval,
    course_id: &'a str,
}

#[async_trait]
impl DenseSearch for CourseDense<'_> {
    async fn search(&self, query: &str) -> Result<DenseResult, String> {
        let (embedding, tokens) = self
            .retrieval
            .embedder
            .embed_counted(query)
            .await
            .map_err(|e| format!("embedding: {e}"))?;
        let hits = self
            .retrieval
            .vectors
            .query_existing(&index_name(self.course_id), &embedding, DENSE_TOP_K, None)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "the vector index was not found".to_owned())?;
        let hits = hits
            .into_iter()
            .map(|h| Hit {
                value: h.distance,
                passage: Passage {
                    key: h.key,
                    document_id: h.document_id,
                    page: h.page,
                    heading: h.heading,
                    text: h.text,
                },
            })
            .collect();
        Ok(DenseResult {
            hits,
            embed_tokens: tokens,
        })
    }
}

/// The lexical backend for one course: the BM25 index from S3.
struct CourseLexical<'a> {
    retrieval: &'a Retrieval,
    course: &'a CourseRecord,
}

#[async_trait]
impl LexicalSearch for CourseLexical<'_> {
    async fn search(&self, query: &str) -> Result<LexicalResult, String> {
        let Some(key) = &self.course.bm25_key else {
            return Ok(LexicalResult::default());
        };
        let reader = self
            .retrieval
            .bm25
            .reader(&self.course.id, key)
            .await
            .map_err(|e| e.to_string())?;
        let query = query.to_owned();
        let (hits, terms) =
            tokio::task::spawn_blocking(move || reader.search_with_stats(&query, LEXICAL_TOP_K))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        let hits = hits
            .into_iter()
            .map(|h| Hit {
                value: h.score,
                passage: Passage {
                    key: h.chunk_id,
                    document_id: h.document_id,
                    page: u32::try_from(h.page).unwrap_or(0),
                    heading: h.heading,
                    text: h.text,
                },
            })
            .collect();
        Ok(LexicalResult { hits, terms })
    }
}

impl Retrieval {
    async fn search(
        &self,
        course: &CourseRecord,
        query: &str,
    ) -> openbook_search::retrieval::Searched {
        let dense = CourseDense {
            retrieval: self,
            course_id: &course.id,
        };
        let lexical = CourseLexical {
            retrieval: self,
            course,
        };
        search_both(&dense, &lexical, query, RETRIEVER_TIMEOUT).await
    }
}

/// Turns the last student message and the earlier ones into one search query with the fast model.
/// It returns `None` for the query on any failure, so the caller uses the raw message. The usage is
/// what the fast model reported (zero when it failed before it said).
pub async fn rewrite_query(
    model: &dyn ChatModel,
    earlier: &[(MessageRole, String)],
    message: &str,
) -> (Option<String>, TokenUsage) {
    let start = earlier.len().saturating_sub(REWRITE_HISTORY);
    let request = ChatRequest {
        system: REWRITE_SYSTEM_PROMPT.to_owned(),
        turns: vec![ChatTurn {
            role: MessageRole::User,
            text: rewrite_user_text(&earlier[start..], message),
        }],
        max_tokens: Some(REWRITE_MAX_TOKENS),
        temperature: Some(0.0),
    };
    let usage = std::sync::Mutex::new(TokenUsage::default());
    let work = async {
        let mut stream = model.stream(request).await.ok()?;
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(ModelEvent::Delta(delta)) => text.push_str(&delta),
                Ok(ModelEvent::Finish { usage: used, .. }) => {
                    if let Ok(mut slot) = usage.lock() {
                        *slot = used;
                    }
                }
                Err(_) => return None,
            }
        }
        Some(text)
    };
    let text = tokio::time::timeout(REWRITE_TIMEOUT, work)
        .await
        .ok()
        .flatten();
    let used = usage.lock().map(|u| *u).unwrap_or_default();
    (text.and_then(|t| clean_query(&t)), used)
}

/// The numbers of one turn. The gate limit belongs to the embedding model. A follow-up whose rewrite
/// failed searches the raw message ("can you explain the second step again?"), which is far from every
/// chunk, so the turn must not end as "not covered" because of a distance: it uses the first rule
/// (any BM25 hit passes).
pub fn turn_params(embedding_model: &str, rewrite_failed: bool) -> Params {
    let mut params = Params::for_embedding_model(embedding_model);
    if rewrite_failed {
        params.gate = GateRule::fallback();
    }
    params
}

pub fn source_lists<'a>(
    fused: &[Fused],
    document_name: impl Fn(&str) -> &'a str,
) -> (Vec<Source>, Vec<SourceContext>) {
    let mut sources = Vec::with_capacity(fused.len());
    let mut contexts = Vec::with_capacity(fused.len());
    for (i, f) in fused.iter().enumerate() {
        let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let name = document_name(f.passage.document_id.as_str());
        let page = (f.passage.page > 0).then_some(f.passage.page);
        sources.push(Source {
            n,
            document_id: f.passage.document_id.clone(),
            document_name: name.to_owned(),
            page,
            snippet: clip(&f.passage.text, SNIPPET_CHARS),
        });
        contexts.push(SourceContext {
            n,
            document_name: name.to_owned(),
            page,
            text: clip(&f.passage.text, SNIPPET_CHARS),
        });
    }
    (sources, contexts)
}

/// Runs retrieval for one turn. It never fails: a problem gives `NotUsed` or a smaller result.
///
/// `earlier` is the conversation before this message. `documents` is every document of the course.
pub async fn run_turn(
    retrieval: &Retrieval,
    rewriter: Option<&dyn ChatModel>,
    course: &CourseRecord,
    documents: &[Document],
    earlier: &[(MessageRole, String)],
    message: &str,
) -> TurnResult {
    let ready: HashMap<&str, &str> = documents
        .iter()
        .filter(|d| d.status == DocumentStatus::Ready)
        .map(|d| (d.id.as_str(), d.filename.as_str()))
        .collect();
    if ready.is_empty() {
        return TurnResult::not_used();
    }

    let started = Instant::now();
    let mut timings = Timings::default();
    let mut usage = TokenUsage::default();
    let mut query = clip(message.trim(), MAX_QUERY_CHARS);
    let mut rewritten = false;
    // A follow-up needs a rewrite. Without one the raw message is searched.
    let mut rewrite_failed = !earlier.is_empty();
    if !earlier.is_empty()
        && let Some(model) = rewriter
    {
        let at = Instant::now();
        let (better, used) = rewrite_query(model, earlier, message).await;
        usage.input_tokens += used.input_tokens;
        usage.output_tokens += used.output_tokens;
        if let Some(better) = better {
            rewrite_failed = false;
            rewritten = better != query;
            query = better;
        }
        timings.rewrite_ms = Some(at.elapsed().as_millis() as u64);
    }

    let at = Instant::now();
    let searched = retrieval.search(course, &query).await;
    timings.retrieve_ms = at.elapsed().as_millis() as u64;
    usage.input_tokens += searched.embed_tokens;
    timings.dense_ms = searched.dense_ms;
    timings.lexical_ms = searched.lexical_ms;
    let mut trace = Trace {
        query,
        rewritten,
        rewrite_failed,
        errors: searched.errors.clone(),
        ..Trace::default()
    };

    // Only passages of ready documents count. A vector of a document that is being deleted is skipped.
    let only_ready = |hits: Option<Vec<Hit>>| {
        hits.map(|h| {
            h.into_iter()
                .filter(|hit| ready.contains_key(hit.passage.document_id.as_str()))
                .collect::<Vec<_>>()
        })
    };
    let (dense, lexical) = (only_ready(searched.dense), only_ready(searched.lexical));
    let fuse_at = Instant::now();
    // The distance limit of the gate belongs to the embedding model.
    let params = turn_params(retrieval.embedder.model_id(), rewrite_failed);
    let decision = decide_with(&params, dense, lexical, searched.terms);
    timings.fuse_ms = fuse_at.elapsed().as_millis() as u64;
    if decision.outcome == Outcome::NotUsed {
        // Dense search failed and BM25 found nothing (or failed too). The relevance gate cannot judge
        // without the dense result, so this is not "the materials do not cover it": answer without
        // sources and without the notice, and keep the trace.
        trace.timings = timings;
        return TurnResult {
            trace: Some(trace.to_json_capped()),
            usage,
            ..TurnResult::not_used()
        };
    }
    trace.dense = trace_hits(&decision.dense);
    trace.lexical = trace_hits(&decision.lexical);
    let fused = decision.fused;
    trace.gated = fused.is_empty();
    trace.gate = Some(TraceGate::new(&params.gate, &decision.gate));
    trace.fused = fused
        .iter()
        .enumerate()
        .map(|(i, f)| TraceFused {
            rank: i + 1,
            key: f.passage.key.clone(),
            document_id: f.passage.document_id.clone(),
            page: f.passage.page,
            score: f.score,
            dense_rank: f.dense_rank,
            lexical_rank: f.lexical_rank,
        })
        .collect();
    trace.timings = timings;
    tracing::info!(
        sources = fused.len(),
        rewritten,
        retrieve_ms = trace.timings.retrieve_ms,
        total_ms = started.elapsed().as_millis() as u64,
        "retrieval done"
    );

    if fused.is_empty() {
        return TurnResult {
            outcome: Outcome::NotCovered,
            sources: Vec::new(),
            contexts: Vec::new(),
            trace: Some(trace.to_json_capped()),
            usage,
        };
    }
    let (sources, contexts) =
        source_lists(&fused, |id| ready.get(id).copied().unwrap_or("document"));
    TurnResult {
        outcome: Outcome::Found,
        sources,
        contexts,
        trace: Some(trace.to_json_capped()),
        usage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_numbers_are_the_documented_ones() {
        assert_eq!(
            (
                DENSE_TOP_K,
                LEXICAL_TOP_K,
                FINAL_SOURCES,
                MAX_PER_DOCUMENT,
                SNIPPET_CHARS
            ),
            (40, 40, 8, 6, 2400)
        );
        assert_eq!(RRF_K, 60.0);
        assert_eq!(
            (REWRITE_MAX_TOKENS, REWRITE_TIMEOUT),
            (100, Duration::from_secs(5))
        );
    }

    fn big_trace() -> Trace {
        let many = |n: usize| -> Vec<TraceHit> {
            (0..n)
                .map(|i| TraceHit {
                    rank: i + 1,
                    key: format!("{}:{i}", "D".repeat(26)),
                    document_id: "D".repeat(26),
                    page: 1,
                    value: 0.5,
                })
                .collect()
        };
        Trace {
            query: "q".repeat(100_000),
            dense: many(1000),
            lexical: many(1000),
            fused: (0..8)
                .map(|i| TraceFused {
                    rank: i + 1,
                    key: format!("K{i}"),
                    document_id: "D".into(),
                    page: 1,
                    score: 0.03,
                    dense_rank: Some(1),
                    lexical_rank: None,
                })
                .collect(),
            errors: vec!["e".repeat(500); 100],
            ..Trace::default()
        }
    }

    #[test]
    fn the_trace_stays_under_32_kb_and_keeps_ten_candidates_per_retriever() {
        let json = big_trace().to_json_capped();
        assert!(json.len() <= TRACE_MAX_BYTES, "{} bytes", json.len());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["dense"].as_array().unwrap().len(), TRACE_CANDIDATES);
        assert_eq!(value["lexical"].as_array().unwrap().len(), TRACE_CANDIDATES);
        assert_eq!(value["fused"].as_array().unwrap().len(), 8);
        assert!(value["query"].as_str().unwrap().len() <= MAX_QUERY_CHARS);
    }

    #[test]
    fn a_trace_that_is_still_too_big_loses_its_lists_before_it_is_cut() {
        let mut trace = big_trace();
        trace.errors = vec!["e".repeat(40_000)];
        let json = trace.to_json_capped();
        assert!(json.len() <= TRACE_MAX_BYTES);
        let value: serde_json::Value = serde_json::from_str(&json).expect("still valid JSON");
        assert!(value["fused"].as_array().is_some());
    }

    #[test]
    fn the_trace_json_has_the_documented_fields() {
        let trace = Trace {
            query: "pythagorean theorem".into(),
            rewritten: true,
            dense: vec![TraceHit {
                rank: 1,
                key: "D:0".into(),
                document_id: "D".into(),
                page: 2,
                value: 0.1,
            }],
            ..Trace::default()
        };
        let value: serde_json::Value = serde_json::from_str(&trace.to_json_capped()).unwrap();
        for field in [
            "query",
            "rewritten",
            "rewriteFailed",
            "dense",
            "lexical",
            "fused",
            "gated",
            "gate",
            "timings",
            "errors",
        ] {
            assert!(value.get(field).is_some(), "{field}");
        }
        assert_eq!(value["dense"][0]["rank"], 1);
        assert!(value["timings"].get("retrieveMs").is_some());
    }

    struct FailingRewriter;

    #[async_trait]
    impl ChatModel for FailingRewriter {
        async fn stream(
            &self,
            _request: openbook_core::model::ChatRequest,
        ) -> Result<openbook_core::model::ModelStream, openbook_core::model::ModelError> {
            Err(openbook_core::model::ModelError("down".into()))
        }
    }

    #[tokio::test]
    async fn a_failed_rewrite_gives_no_query_and_the_turn_uses_the_fallback_gate() {
        let earlier = [(MessageRole::User, "What is a parabola?".to_owned())];
        let (query, _) = rewrite_query(
            &FailingRewriter,
            &earlier,
            "can you explain the second step again?",
        )
        .await;
        assert_eq!(query, None);
        // Titan v2: a normal turn uses the dense rule, and a turn with a failed rewrite does not.
        let titan = "amazon.titan-embed-text-v2:0";
        assert_eq!(turn_params(titan, false).gate.name(), "dense");
        assert_eq!(turn_params(titan, true).gate.name(), "any_lexical_hit");
        // The raw message is far from every chunk (distance 0.9) but has BM25 hits: it keeps sources.
        let far = |gate: &Params| {
            let dense = vec![Hit {
                value: 0.9,
                passage: Passage {
                    key: "D:0".into(),
                    document_id: "D".into(),
                    page: 1,
                    heading: String::new(),
                    text: "text".into(),
                },
            }];
            let lexical = dense.clone();
            decide_with(gate, Some(dense), Some(lexical), Default::default()).outcome
        };
        assert_eq!(far(&turn_params(titan, false)), Outcome::NotCovered);
        assert_eq!(far(&turn_params(titan, true)), Outcome::Found);
    }

    #[test]
    fn a_snippet_holds_the_whole_chunk() {
        // The chunker never makes a chunk longer than this (except one atomic table or formula).
        const { assert!(SNIPPET_CHARS >= openbook_search::chunker::MAX_CHARS) };
    }
    #[test]
    fn the_prompt_passages_and_the_sources_event_come_from_one_list() {
        let fused: Vec<Fused> = [(1, "A"), (0, "B")]
            .iter()
            .map(|(page, document)| Fused {
                passage: Passage {
                    key: format!("{document}:0"),
                    document_id: (*document).to_owned(),
                    page: *page,
                    heading: String::new(),
                    text: "x".repeat(SNIPPET_CHARS + 10),
                },
                score: 1.0,
                dense_rank: Some(1),
                lexical_rank: None,
            })
            .collect();
        let (sources, contexts) =
            source_lists(&fused, |id| if id == "A" { "a.pdf" } else { "b.pdf" });
        assert_eq!(sources.len(), 2);
        assert_eq!(contexts.len(), 2);
        assert_eq!((sources[1].n, contexts[1].n), (2, 2));
        assert_eq!((sources[0].page, sources[1].page), (Some(1), None));
        assert_eq!(contexts[0].document_name, "a.pdf");
        assert_eq!(sources[0].snippet, contexts[0].text);
        assert_eq!(contexts[0].text.chars().count(), SNIPPET_CHARS);
    }
}
