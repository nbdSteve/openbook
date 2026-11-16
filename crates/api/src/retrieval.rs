//! Hybrid retrieval for a chat turn (`docs/retrieval.md`, "Retrieval for a chat turn").
//!
//! Dense search (S3 Vectors) and BM25 (Tantivy) run in parallel. Reciprocal rank fusion merges them,
//! a relevance gate decides whether there are sources, and a trace records what happened.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use openbook_core::domain::{CourseRecord, Document, DocumentStatus, MessageRole, Source};
use openbook_core::model::{ChatModel, ChatRequest, ChatTurn, ModelEvent, TokenUsage};
use openbook_core::prompt::{REWRITE_SYSTEM_PROMPT, SourceContext, SourceMode, rewrite_user_text};
use openbook_search::bm25::Bm25ReaderCache;
use openbook_search::embed::Embedder;
use openbook_search::vectors::{VectorStore, index_name};
use serde::Serialize;

// ---- The numbers. They are tuned in phase 4. Keep them here.
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
                entry.score += 1.0 / (RRF_K + rank as f64);
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
        if *count >= MAX_PER_DOCUMENT {
            continue;
        }
        *count += 1;
        kept.push(candidate);
        if kept.len() == FINAL_SOURCES {
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
    let dense_is_close = best_dense_distance.is_some_and(|d| d <= GATE_MAX_DISTANCE);
    dense_is_close || lexical_hits > 0
}

/// Cuts text to `max` characters (never inside a character).
pub fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

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
    pub dense: Vec<TraceHit>,
    pub lexical: Vec<TraceHit>,
    pub fused: Vec<TraceFused>,
    /// True when the relevance gate found nothing.
    pub gated: bool,
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

/// How retrieval ended for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Retrieval did not run (no ready documents) or both retrievers failed.
    NotUsed,
    Found,
    /// The gate found nothing.
    NotCovered,
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

/// A retriever result: `None` when it failed.
struct Searched {
    /// Input tokens of the query embedding.
    embed_tokens: u64,
    dense: Option<Vec<Hit>>,
    lexical: Option<Vec<Hit>>,
    dense_ms: Option<u64>,
    lexical_ms: Option<u64>,
    errors: Vec<String>,
}

impl Retrieval {
    async fn dense(&self, course_id: &str, query: &str) -> Result<(Vec<Hit>, u64), String> {
        let (embedding, tokens) = self
            .embedder
            .embed_counted(query)
            .await
            .map_err(|e| format!("embedding: {e}"))?;
        let hits = self
            .vectors
            .query_existing(&index_name(course_id), &embedding, DENSE_TOP_K, None)
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
        Ok((hits, tokens))
    }

    async fn lexical(&self, course: &CourseRecord, query: &str) -> Result<Vec<Hit>, String> {
        let Some(key) = &course.bm25_key else {
            return Ok(Vec::new());
        };
        let reader = self
            .bm25
            .reader(&course.id, key)
            .await
            .map_err(|e| e.to_string())?;
        let query = query.to_owned();
        let hits = tokio::task::spawn_blocking(move || reader.search(&query, LEXICAL_TOP_K))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        Ok(hits
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
            .collect())
    }

    async fn search(&self, course: &CourseRecord, query: &str) -> Searched {
        async fn timed<T>(
            future: impl std::future::Future<Output = Result<T, String>>,
        ) -> (Result<T, String>, u64) {
            let started = Instant::now();
            let result = match tokio::time::timeout(RETRIEVER_TIMEOUT, future).await {
                Ok(result) => result,
                Err(_) => Err("timed out".to_owned()),
            };
            (result, started.elapsed().as_millis() as u64)
        }
        let ((dense, dense_ms), (lexical, lexical_ms)) = tokio::join!(
            timed(self.dense(&course.id, query)),
            timed(self.lexical(course, query))
        );
        let mut errors = Vec::new();
        let mut embed_tokens = 0;
        let dense = dense.map(|(hits, tokens)| {
            embed_tokens = tokens;
            hits
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

/// One line, no quotes, one space between words, at most `MAX_QUERY_CHARS`. `None` when empty.
pub fn clean_query(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let unquoted = line.trim_matches(|c| matches!(c, '"' | '\'' | '`' | '“' | '”'));
    let words = unquoted.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped = clip(&words, MAX_QUERY_CHARS);
    (!clipped.is_empty()).then_some(clipped)
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
    if !earlier.is_empty()
        && let Some(model) = rewriter
    {
        let at = Instant::now();
        let (better, used) = rewrite_query(model, earlier, message).await;
        usage.input_tokens += used.input_tokens;
        usage.output_tokens += used.output_tokens;
        if let Some(better) = better {
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
    if dense.is_none() && lexical.as_ref().is_none_or(Vec::is_empty) {
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
    let (dense, lexical) = (dense.unwrap_or_default(), lexical.unwrap_or_default());
    trace.dense = trace_hits(&dense);
    trace.lexical = trace_hits(&lexical);

    let fuse_at = Instant::now();
    let best_dense = dense.first().map(|h| h.value);
    let gate = passes_gate(best_dense, lexical.len());
    let fused = if gate {
        fuse(&dense, &lexical)
    } else {
        Vec::new()
    };
    timings.fuse_ms = fuse_at.elapsed().as_millis() as u64;
    trace.gated = fused.is_empty();
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
    let mut sources = Vec::with_capacity(fused.len());
    let mut contexts = Vec::with_capacity(fused.len());
    for (i, f) in fused.iter().enumerate() {
        let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let name = ready
            .get(f.passage.document_id.as_str())
            .copied()
            .unwrap_or("document");
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
            // The same text as the snippet: the model sees what the student can read.
            text: clip(&f.passage.text, SNIPPET_CHARS),
        });
    }
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
    fn the_numbers_are_the_documented_ones() {
        assert_eq!(
            (
                DENSE_TOP_K,
                LEXICAL_TOP_K,
                FINAL_SOURCES,
                MAX_PER_DOCUMENT,
                SNIPPET_CHARS
            ),
            (40, 40, 8, 4, 2400)
        );
        assert_eq!((RRF_K, GATE_MAX_DISTANCE), (60.0, 0.85));
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
            "dense",
            "lexical",
            "fused",
            "gated",
            "timings",
            "errors",
        ] {
            assert!(value.get(field).is_some(), "{field}");
        }
        assert_eq!(value["dense"][0]["rank"], 1);
        assert!(value["timings"].get("retrieveMs").is_some());
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
    fn a_snippet_holds_the_whole_chunk() {
        // The chunker never makes a chunk longer than this (except one atomic table or formula).
        const { assert!(SNIPPET_CHARS >= openbook_search::chunker::MAX_CHARS) };
    }
}
