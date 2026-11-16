//! The offline run: no network, no AWS, the same retrieval code as the API.
//!
//! It chunks the cached pages with the real chunker, writes a real Tantivy index, keeps the cached
//! embeddings in memory, and runs `search_both` and the fusion and gate code of `openbook-search`
//! for every golden question.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openbook_search::bm25::{Bm25Chunk, Bm25Reader, Bm25Writer};
use openbook_search::chunker::{chunk_pages, embed_text};
use openbook_search::retrieval::{
    DenseResult, DenseSearch, Fused, Hit, LEXICAL_TOP_K, LexicalSearch, MAX_QUERY_CHARS, Outcome,
    Params, Passage, clip, decide_with, fuse_with, passes_gate_with, search_both,
};
use openbook_search::vectors::vector_key;

use crate::Error;
use crate::cache::{Cache, EmbeddingStore, rewrite_key};
use crate::data::{Kind, Question, Section};
use crate::metrics::{self, NDCG_DEPTH, Outcome as RunOutcome, RECALL_LARGE, Span};
use crate::report::{KindMetrics, Miss, Report};

/// The ranked lists have this many results, so nDCG@10 has ten places. The API returns the first
/// `Params::final_sources` of them, and the fusion order is the same.
pub fn ranking_params(params: &Params) -> Params {
    Params {
        final_sources: params.final_sources.max(NDCG_DEPTH),
        ..*params
    }
}

/// A chunk of the corpus, with the pages it covers in the excerpt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalChunk {
    pub key: String,
    pub document_id: String,
    /// First and last page of the excerpt.
    pub span: Span,
    pub heading: String,
    pub text: String,
    /// The text that is embedded: title, heading path, and chunk text.
    pub embed_text: String,
}

/// Chunks every section like ingestion does for one document: the section is its own document,
/// its pages count from 1, and the title is the section title.
pub fn build_chunks(
    sections: &[Section],
    pages: &[(u32, String)],
) -> Result<Vec<EvalChunk>, Error> {
    let by_number: HashMap<u32, &str> = pages.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let mut out = Vec::new();
    for section in sections {
        let offset = section.first_page - 1;
        let mut local = Vec::new();
        for number in section.first_page..=section.last_page {
            let text = by_number.get(&number).ok_or_else(|| {
                Error::Cache(format!(
                    "the cache has no page {number} (section {})",
                    section.id
                ))
            })?;
            local.push((number - offset, (*text).to_owned()));
        }
        for chunk in chunk_pages(&local) {
            out.push(EvalChunk {
                key: vector_key(&section.id, chunk.index),
                document_id: section.id.clone(),
                span: (chunk.first_page + offset, chunk.last_page + offset),
                heading: chunk.heading_path.clone(),
                text: chunk.text.clone(),
                embed_text: embed_text(&section.title, &chunk),
            });
        }
    }
    Ok(out)
}

fn passage(chunk: &EvalChunk) -> Passage {
    Passage {
        key: chunk.key.clone(),
        document_id: chunk.document_id.clone(),
        // The vector store and the index keep the first page of the chunk.
        page: chunk.span.0,
        heading: chunk.heading.clone(),
        text: chunk.text.clone(),
    }
}

/// Cosine distance, like S3 Vectors reports it.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 1.0;
    }
    1.0 - dot / (na * nb)
}

/// The dense backend: a brute-force search over the cached chunk embeddings.
pub struct MemoryDense {
    chunks: Vec<(Passage, Vec<f32>)>,
    queries: EmbeddingStore,
    top_k: usize,
}

#[async_trait]
impl DenseSearch for MemoryDense {
    async fn search(&self, query: &str) -> Result<DenseResult, String> {
        let embedding = self.queries.get(query).map_err(|e| e.to_string())?;
        let mut scored: Vec<(f32, &Passage)> = self
            .chunks
            .iter()
            .map(|(p, v)| (cosine_distance(&embedding, v), p))
            .collect();
        // Closest first. The key breaks a tie, so the order never changes between runs.
        scored.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.key.cmp(&b.1.key)));
        let hits = scored
            .into_iter()
            .take(self.top_k)
            .map(|(distance, p)| Hit {
                passage: p.clone(),
                value: distance,
            })
            .collect();
        Ok(DenseResult {
            hits,
            embed_tokens: 0,
        })
    }
}

/// The lexical backend: the real BM25 index in a temporary directory.
pub struct TantivyLexical {
    reader: Arc<Bm25Reader>,
    top_k: usize,
}

#[async_trait]
impl LexicalSearch for TantivyLexical {
    async fn search(&self, query: &str) -> Result<Vec<Hit>, String> {
        let hits = self
            .reader
            .search(query, self.top_k)
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
}

/// Both indexes over the corpus.
pub struct Index {
    pub chunks: Vec<EvalChunk>,
    dense: MemoryDense,
    lexical: TantivyLexical,
    spans: HashMap<String, Span>,
    // Keeps the index files until the run ends.
    _dir: tempfile::TempDir,
}

impl Index {
    pub fn build(sections: &[Section], cache: &Cache) -> Result<Index, Error> {
        let chunks = build_chunks(sections, &cache.pages)?;
        if chunks.is_empty() {
            return Err(Error::Cache("the cached pages give no chunks".into()));
        }
        let mut vectors = Vec::with_capacity(chunks.len());
        for chunk in &chunks {
            vectors.push((passage(chunk), cache.embeddings.get(&chunk.embed_text)?));
        }
        let dir =
            tempfile::tempdir().map_err(|e| Error::Run(format!("temporary directory: {e}")))?;
        let index_dir = dir.path().join("bm25");
        std::fs::create_dir(&index_dir).map_err(|e| Error::io(&index_dir, &e))?;
        let bm25 = |e: openbook_search::bm25::Bm25Error| Error::Run(format!("BM25 index: {e}"));
        let mut writer = Bm25Writer::create(&index_dir).map_err(bm25)?;
        for chunk in &chunks {
            writer
                .add_chunk(&Bm25Chunk {
                    chunk_id: chunk.key.clone(),
                    document_id: chunk.document_id.clone(),
                    page: u64::from(chunk.span.0),
                    heading: chunk.heading.clone(),
                    text: chunk.text.clone(),
                })
                .map_err(bm25)?;
        }
        writer.commit().map_err(bm25)?;
        let reader = Arc::new(Bm25Reader::open(&index_dir).map_err(bm25)?);
        let spans = chunks.iter().map(|c| (c.key.clone(), c.span)).collect();
        Ok(Index {
            dense: MemoryDense {
                chunks: vectors,
                queries: cache.embeddings.clone(),
                top_k: openbook_search::retrieval::DENSE_TOP_K as usize,
            },
            lexical: TantivyLexical {
                reader,
                top_k: LEXICAL_TOP_K,
            },
            chunks,
            spans,
            _dir: dir,
        })
    }

    fn span(&self, key: &str) -> Result<Span, Error> {
        self.spans
            .get(key)
            .copied()
            .ok_or_else(|| Error::Run(format!("a result has an unknown chunk key {key:?}")))
    }
}

/// The search query of a question: the cleaned and clipped question, or for a follow-up the cached
/// rewrite.
pub fn query_for(question: &Question, cache: &Cache) -> Result<String, Error> {
    match (&question.kind, &question.context) {
        (Kind::Followup, Some(context)) => {
            let key = rewrite_key(&cache.meta.rewrite_model, context, &question.question);
            cache.rewrites.get(&key).cloned().ok_or_else(|| {
                Error::Cache(format!(
                    "no rewrite for follow-up {} (model {}, key {}). Refresh the cache.",
                    question.id,
                    cache.meta.rewrite_model,
                    &key[..12]
                ))
            })
        }
        _ => Ok(clip(question.question.trim(), MAX_QUERY_CHARS)),
    }
}

/// What the three setups returned for one question.
struct Answers {
    dense: RunOutcome,
    bm25: RunOutcome,
    hybrid: RunOutcome,
    hybrid_returned: Vec<Fused>,
}

async fn answer(
    index: &Index,
    question: &Question,
    query: &str,
    params: &Params,
) -> Result<Answers, Error> {
    // A missing embedding is a cache problem with a clear message, not "dense failed".
    index
        .dense
        .queries
        .get(query)
        .map_err(|e| Error::Cache(format!("question {}: {e}", question.id)))?;
    let searched = search_both(&index.dense, &index.lexical, query, Duration::from_secs(60)).await;
    if !searched.errors.is_empty() {
        return Err(Error::Run(format!(
            "question {}: {}",
            question.id,
            searched.errors.join(", ")
        )));
    }
    let (dense, lexical) = (
        searched.dense.unwrap_or_default(),
        searched.lexical.unwrap_or_default(),
    );

    let covered = question.is_covered();
    let all: Vec<u8> = index
        .chunks
        .iter()
        .map(|c| metrics::gain(c.span, &question.relevant_pages))
        .collect();
    if covered && !all.contains(&2) {
        return Err(Error::Data(format!(
            "question {}: no chunk holds the labeled pages {:?}",
            question.id, question.relevant_pages
        )));
    }
    let outcome = |fused: &[Fused], gated: bool| -> Result<RunOutcome, Error> {
        let ranked = fused
            .iter()
            .map(|f| {
                index
                    .span(&f.passage.key)
                    .map(|span| metrics::gain(span, &question.relevant_pages))
            })
            .collect::<Result<Vec<u8>, Error>>()?;
        Ok(RunOutcome {
            covered,
            gated,
            ranked,
            all: all.clone(),
        })
    };

    // Hybrid: exactly what the API does after the two searches.
    let decision = decide_with(params, Some(dense.clone()), Some(lexical.clone()));
    let hybrid = outcome(&decision.fused, decision.outcome == Outcome::NotCovered)?;

    // Dense only and BM25 only: the same gate rule and the same fusion code on one list, so the
    // cap and the depth are the same as for the hybrid run.
    let best_dense = dense.first().map(|h| h.value);
    let dense_pass = passes_gate_with(params, best_dense, 0);
    let dense_fused = if dense_pass {
        fuse_with(params, &dense, &[])
    } else {
        Vec::new()
    };
    let bm25_pass = passes_gate_with(params, None, lexical.len());
    let bm25_fused = if bm25_pass {
        fuse_with(params, &[], &lexical)
    } else {
        Vec::new()
    };
    Ok(Answers {
        dense: outcome(&dense_fused, !dense_pass)?,
        bm25: outcome(&bm25_fused, !bm25_pass)?,
        hybrid,
        hybrid_returned: decision.fused,
    })
}

fn span_text(span: Span) -> String {
    if span.0 == span.1 {
        span.0.to_string()
    } else {
        format!("{}-{}", span.0, span.1)
    }
}

/// Runs every question and builds the report.
pub async fn run(
    sections: &[Section],
    questions: &[Question],
    cache: &Cache,
    params: &Params,
) -> Result<Report, Error> {
    let index = Index::build(sections, cache)?;
    let ranking = ranking_params(params);
    let mut dense = Vec::new();
    let mut bm25 = Vec::new();
    let mut hybrid = Vec::new();
    let mut misses = Vec::new();
    let mut by_kind: BTreeMap<Kind, Vec<RunOutcome>> = BTreeMap::new();
    for question in questions {
        let query = query_for(question, cache)?;
        let answers = answer(&index, question, &query, &ranking).await?;
        if question.is_covered() && !metrics::hit_at(&answers.hybrid.ranked, RECALL_LARGE) {
            let mut returned = Vec::new();
            for f in answers.hybrid_returned.iter().take(RECALL_LARGE) {
                returned.push(span_text(index.span(&f.passage.key)?));
            }
            misses.push(Miss {
                id: question.id.clone(),
                kind: question.kind.name().to_owned(),
                question: question.question.clone(),
                relevant_pages: question.relevant_pages.clone(),
                gated: answers.hybrid.gated,
                returned,
            });
        }
        by_kind
            .entry(question.kind)
            .or_default()
            .push(answers.hybrid.clone());
        dense.push(answers.dense);
        bm25.push(answers.bm25);
        hybrid.push(answers.hybrid);
    }
    let covered = questions.iter().filter(|q| q.is_covered()).count();
    let hybrid_by_kind = by_kind
        .iter()
        .map(|(kind, outcomes)| {
            let m = metrics::aggregate(outcomes);
            let gated = outcomes.iter().filter(|o| o.gated).count();
            (
                kind.name().to_owned(),
                KindMetrics {
                    questions: outcomes.len(),
                    recall_at_8: m.recall_at_8,
                    mrr: m.mrr,
                    gated_rate: metrics::round4(gated as f64 / outcomes.len() as f64),
                },
            )
        })
        .collect();
    Ok(Report {
        cache_kind: cache.meta.kind,
        embedding_model: cache.meta.embedding_model.clone(),
        corpus_sha256: cache.meta.corpus_sha256.clone(),
        questions: questions.len(),
        covered,
        not_covered: questions.len() - covered,
        chunks: index.chunks.len(),
        dense: metrics::aggregate(&dense),
        bm25: metrics::aggregate(&bm25),
        hybrid: metrics::aggregate(&hybrid),
        hybrid_by_kind,
        misses,
        latency: cache.meta.latency.clone(),
    })
}

/// Reads the corpus data and the cache, and runs the evaluation.
pub async fn run_dir(evals: &Path, cache_dir: &Path, params: &Params) -> Result<Report, Error> {
    let sections = crate::data::load_sections(&evals.join("corpus/sections.json"))?;
    let questions = crate::data::load_golden(
        &evals.join("golden.jsonl"),
        crate::data::page_total(&sections),
    )?;
    let cache = Cache::load(cache_dir)?;
    let corpus = crate::refresh::corpus_path(evals)?;
    let sha = crate::cache::corpus_sha256(&corpus)?;
    if sha != cache.meta.corpus_sha256 {
        return Err(Error::Cache(
            "the cache was made for another corpus file. Refresh the cache.".into(),
        ));
    }
    if cache.meta.pages != crate::data::page_total(&sections) {
        return Err(Error::Cache(format!(
            "the cache has {} pages and the sections cover {}",
            cache.meta.pages,
            crate::data::page_total(&sections)
        )));
    }
    run(&sections, &questions, &cache, params).await
}
