//! The offline run: no network, no AWS, the same retrieval code as the API.
//!
//! It chunks the cached pages with the real chunker, writes a real Tantivy index, keeps the cached
//! embeddings in memory, and runs `search_both` and the fusion and gate code of `openbook-search`
//! for every golden question.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openbook_search::bm25::{Bm25Chunk, Bm25Reader, Bm25Writer};
use openbook_search::chunker::{TARGET_CHARS, chunk_pages_with, chunk_stripped_pages, embed_text};
use openbook_search::headers::{phrase_key, strip_running_lines};
use openbook_search::retrieval::{
    DENSE_TOP_K, DenseResult, DenseSearch, Fused, GateRule, Hit, LEXICAL_TOP_K, LexicalResult,
    LexicalSearch, MAX_QUERY_CHARS, Outcome, Params, Passage, clip, decide_with, fuse_with,
    gate_inputs, search_both,
};
use openbook_search::vectors::vector_key;

use crate::Error;
use crate::cache::{Cache, CacheKind, EmbeddingStore, Latency, rewrite_key};
use crate::corpus::Corpus;
use crate::data::{Kind, Question, Section};
use crate::metrics::{self, Outcome as RunOutcome, RECALL_LARGE, Span};
use crate::report::{
    CorpusReport, GateRow, HeaderLine, HeaderStats, KindMetrics, Miss, ModeMetrics, QuestionDiag,
    Report, SuiteReport, gate_sweep,
};

/// The document ID that every chunk gets in cap-off mode.
pub const CAP_OFF_DOCUMENT_ID: &str = "course";

/// One chunk is about this many characters per token.
pub const CHARS_PER_TOKEN: usize = 4;

/// Everything that one offline run can change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub params: Params,
    /// The chunk target in characters (the chunker's `TARGET_CHARS` in production).
    pub chunk_chars: usize,
    /// Treat the whole PDF as one document: the per-document cap then sees one document. The chunks
    /// and their embeddings stay the same, so this isolates the effect of the cap.
    pub cap_off: bool,
    pub dense_top_k: usize,
    pub lexical_top_k: usize,
}

impl Options {
    /// The production numbers for an embedding model.
    pub fn production(embedding_model: &str) -> Self {
        Self {
            params: Params::for_embedding_model(embedding_model),
            chunk_chars: TARGET_CHARS,
            cap_off: false,
            dense_top_k: DENSE_TOP_K as usize,
            lexical_top_k: LEXICAL_TOP_K,
        }
    }

    /// The chunk target in tokens.
    pub fn chunk_tokens(&self) -> usize {
        self.chunk_chars / CHARS_PER_TOKEN
    }

    /// The same options in the other document mode.
    pub fn with_cap_off(mut self, single: bool) -> Self {
        self.cap_off = single;
        self
    }

    /// The settings in words, for the report.
    pub fn describe(&self) -> String {
        format!(
            "{}; RRF k {}, BM25 weight {}; {} sources, at most {} per document; {} dense and {} BM25 candidates; chunk target {} tokens; {}",
            describe_rule(&self.params.gate),
            self.params.rrf_k,
            self.params.lexical_weight,
            self.params.final_sources,
            self.params.max_per_document,
            self.dense_top_k,
            self.lexical_top_k,
            self.chunk_tokens(),
            if self.cap_off {
                "cap off"
            } else {
                "one document per section"
            }
        )
    }
}

/// A gate rule in words.
pub fn describe_rule(rule: &GateRule) -> String {
    match rule {
        GateRule::AnyLexicalHit { max_distance } => {
            format!("gate: dense distance at most {max_distance} or any BM25 hit")
        }
        GateRule::Dense { max_distance } => {
            format!("gate: dense distance at most {max_distance}")
        }
        GateRule::DenseOrLexical {
            max_distance,
            min_coverage,
            min_matched,
        } => format!(
            "gate: dense distance at most {max_distance}, or BM25 best hit with coverage at least {min_coverage} and at least {min_matched} matched terms"
        ),
    }
}

/// Which gate rule a flag asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateKind {
    Any,
    Dense,
    Combined,
}

impl GateKind {
    pub fn parse(text: &str) -> Option<GateKind> {
        match text {
            "any" => Some(GateKind::Any),
            "dense" => Some(GateKind::Dense),
            "combined" => Some(GateKind::Combined),
            _ => None,
        }
    }
}

/// The changes that the command line asks for. `Default` changes nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tuning {
    pub rrf_k: Option<f64>,
    pub lexical_weight: Option<f64>,
    pub final_sources: Option<usize>,
    pub max_per_document: Option<usize>,
    pub gate_kind: Option<GateKind>,
    pub gate_distance: Option<f32>,
    pub gate_coverage: Option<f64>,
    pub gate_matched: Option<usize>,
    pub chunk_tokens: Option<usize>,
    pub dense_top_k: Option<usize>,
    pub lexical_top_k: Option<usize>,
    pub cap_off: bool,
}

impl Tuning {
    /// True when any number differs from production. `--check` and `--write-baseline` refuse it.
    pub fn is_tuned(&self) -> bool {
        *self != Tuning::default()
    }

    /// The options for an embedding model.
    pub fn options(&self, embedding_model: &str) -> Options {
        let mut o = Options::production(embedding_model);
        let base = o.params.gate;
        let distance = self.gate_distance.unwrap_or(base.max_distance());
        o.params.gate = match self.gate_kind {
            None => base.with_max_distance(distance),
            Some(GateKind::Any) => GateRule::AnyLexicalHit {
                max_distance: distance,
            },
            Some(GateKind::Dense) => GateRule::Dense {
                max_distance: distance,
            },
            Some(GateKind::Combined) => GateRule::DenseOrLexical {
                max_distance: distance,
                min_coverage: self.gate_coverage.unwrap_or(0.75),
                min_matched: self.gate_matched.unwrap_or(3),
            },
        };
        if let (
            None,
            GateRule::DenseOrLexical {
                max_distance,
                min_coverage,
                min_matched,
            },
        ) = (self.gate_kind, o.params.gate)
        {
            o.params.gate = GateRule::DenseOrLexical {
                max_distance,
                min_coverage: self.gate_coverage.unwrap_or(min_coverage),
                min_matched: self.gate_matched.unwrap_or(min_matched),
            };
        }
        o.params.rrf_k = self.rrf_k.unwrap_or(o.params.rrf_k);
        o.params.lexical_weight = self.lexical_weight.unwrap_or(o.params.lexical_weight);
        o.params.final_sources = self.final_sources.unwrap_or(o.params.final_sources);
        o.params.max_per_document = self.max_per_document.unwrap_or(o.params.max_per_document);
        if let Some(tokens) = self.chunk_tokens {
            o.chunk_chars = tokens * CHARS_PER_TOKEN;
        }
        o.dense_top_k = self.dense_top_k.unwrap_or(o.dense_top_k);
        o.lexical_top_k = self.lexical_top_k.unwrap_or(o.lexical_top_k);
        o.cap_off = self.cap_off;
        o
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

/// The pages of one section, as the pages of its own document: they count from 1.
fn section_pages(
    section: &Section,
    by_number: &HashMap<u32, &str>,
) -> Result<Vec<(u32, String)>, Error> {
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
    Ok(local)
}

/// Chunks every section like ingestion does for one document: the section is its own document,
/// its pages count from 1, and the title is the section title. The running headers and footers of
/// the book are removed on the way, by the same code as in ingestion (`chunk_pages_with`).
/// `target_chars` is the chunk target (`TARGET_CHARS` in production).
pub fn build_chunks(
    sections: &[Section],
    pages: &[(u32, String)],
    target_chars: usize,
) -> Result<Vec<EvalChunk>, Error> {
    let by_number: HashMap<u32, &str> = pages.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let mut out = Vec::new();
    for section in sections {
        let offset = section.first_page - 1;
        let local = section_pages(section, &by_number)?;
        for chunk in chunk_pages_with(&local, target_chars) {
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

fn passage(chunk: &EvalChunk, cap_off: bool) -> Passage {
    Passage {
        key: chunk.key.clone(),
        document_id: if cap_off {
            CAP_OFF_DOCUMENT_ID.to_owned()
        } else {
            chunk.document_id.clone()
        },
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
    cap_off: bool,
}

#[async_trait]
impl LexicalSearch for TantivyLexical {
    async fn search(&self, query: &str) -> Result<LexicalResult, String> {
        let (hits, terms) = self
            .reader
            .search_with_stats(query, self.top_k)
            .map_err(|e| e.to_string())?;
        let hits = hits
            .into_iter()
            .map(|h| Hit {
                value: h.score,
                passage: Passage {
                    key: h.chunk_id,
                    document_id: if self.cap_off {
                        CAP_OFF_DOCUMENT_ID.to_owned()
                    } else {
                        h.document_id
                    },
                    page: u32::try_from(h.page).unwrap_or(0),
                    heading: h.heading,
                    text: h.text,
                },
            })
            .collect();
        Ok(LexicalResult { hits, terms })
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
    /// Builds the indexes for one chunk target and one document mode.
    pub fn build(
        sections: &[Section],
        cache: &Cache,
        chunk_chars: usize,
        cap_off: bool,
    ) -> Result<Index, Error> {
        let chunks = build_chunks(sections, &cache.pages, chunk_chars)?;
        if chunks.is_empty() {
            return Err(Error::Cache("the cached pages give no chunks".into()));
        }
        let mut vectors = Vec::with_capacity(chunks.len());
        for chunk in &chunks {
            let vector = cache.embeddings.get(&chunk.embed_text).map_err(|e| {
                Error::Cache(format!(
                    "{e} (a chunk of the {} token target: refresh the cache with `refresh --missing-only`)",
                    chunk_chars / CHARS_PER_TOKEN
                ))
            })?;
            vectors.push((passage(chunk, cap_off), vector));
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
                top_k: DENSE_TOP_K as usize,
            },
            lexical: TantivyLexical {
                reader,
                top_k: LEXICAL_TOP_K,
                cap_off,
            },
            chunks,
            spans,
            _dir: dir,
        })
    }

    /// How many candidates each retriever returns.
    pub fn set_candidates(&mut self, dense: usize, lexical: usize) {
        self.dense.top_k = dense;
        self.lexical.top_k = lexical;
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
    diag: QuestionDiag,
}

fn round5(value: f32) -> f64 {
    (f64::from(value) * 100_000.0).round() / 100_000.0
}

/// What the two retrievers found for one question. It does not depend on the fusion numbers, so the
/// grid searches once and fuses many times.
#[derive(Debug, Clone)]
pub struct Candidates {
    pub dense: Vec<Hit>,
    pub lexical: Vec<Hit>,
    pub terms: openbook_search::retrieval::TermStats,
}

/// Searches both indexes for the query of a question.
pub async fn candidates(
    index: &Index,
    question: &Question,
    query: &str,
) -> Result<Candidates, Error> {
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
    Ok(Candidates {
        dense: searched.dense.unwrap_or_default(),
        lexical: searched.lexical.unwrap_or_default(),
        terms: searched.terms,
    })
}

/// Gives the three setups their sources for one question.
fn answer(
    index: &Index,
    question: &Question,
    found: &Candidates,
    params: &Params,
) -> Result<Answers, Error> {
    let (dense, lexical) = (&found.dense, &found.lexical);
    let terms = found.terms;

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
    let gains = |fused: &[Fused]| -> Result<Vec<u8>, Error> {
        fused
            .iter()
            .map(|f| {
                index
                    .span(&f.passage.key)
                    .map(|span| metrics::gain(span, &question.relevant_pages))
            })
            .collect()
    };
    let outcome = |pass: bool, list: &[Fused]| -> Result<RunOutcome, Error> {
        let returned = if pass { gains(list)? } else { Vec::new() };
        Ok(RunOutcome {
            covered,
            gated: !pass,
            returned,
            all: all.clone(),
        })
    };

    // Hybrid: exactly what the API does after the two searches.
    let decision = decide_with(params, Some(dense.clone()), Some(lexical.clone()), terms);
    let hybrid_pass = decision.outcome != Outcome::NotCovered;
    let hybrid = outcome(hybrid_pass, &decision.fused)?;

    // Dense only and BM25 only: the dense or the lexical half of the gate and the same fusion code
    // on one list, so the cap and the depth are the same as for the hybrid run.
    let dense_inputs = gate_inputs(dense, &[], Default::default());
    let dense_pass = params.gate.passes_dense_only(&dense_inputs);
    let dense_returned = if dense_pass {
        fuse_with(params, dense, &[])
    } else {
        Vec::new()
    };
    let bm25_inputs = gate_inputs(&[], lexical, terms);
    let bm25_pass = params.gate.passes_lexical_only(&bm25_inputs);
    let bm25_returned = if bm25_pass {
        fuse_with(params, &[], lexical)
    } else {
        Vec::new()
    };

    let gate = decision.gate;
    let diag = QuestionDiag {
        id: question.id.clone(),
        kind: question.kind.name().to_owned(),
        best_dense_distance: gate.best_dense_distance.map(round5),
        top_bm25_score: gate.top_bm25_score.map(round5),
        term_coverage: metrics::round4(gate.term_coverage()),
        query_terms: gate.terms.query_terms,
        indexed_terms: gate.terms.indexed_terms,
        matched_terms: gate.terms.matched_terms,
        lexical_hits: gate.lexical_hits,
        gated: !hybrid_pass,
    };
    Ok(Answers {
        dense: outcome(dense_pass, &dense_returned)?,
        bm25: outcome(bm25_pass, &bm25_returned)?,
        hybrid,
        hybrid_returned: decision.fused,
        diag,
    })
}

fn span_text(span: Span) -> String {
    if span.0 == span.1 {
        span.0.to_string()
    } else {
        format!("{}-{}", span.0, span.1)
    }
}

/// What a run over all questions found.
pub struct Run {
    pub dense: Vec<RunOutcome>,
    pub bm25: Vec<RunOutcome>,
    pub hybrid: Vec<RunOutcome>,
    /// The kind of each question, in the order of the outcomes.
    pub kinds: Vec<Kind>,
    pub diagnostics: Vec<QuestionDiag>,
    pub misses: Vec<Miss>,
}

impl Run {
    /// The runs of several corpora as one run. Question IDs get the corpus as prefix.
    pub fn merged(parts: &[(&str, &Run)]) -> Run {
        let mut all = Run {
            dense: Vec::new(),
            bm25: Vec::new(),
            hybrid: Vec::new(),
            kinds: Vec::new(),
            diagnostics: Vec::new(),
            misses: Vec::new(),
        };
        for (corpus, run) in parts {
            all.dense.extend(run.dense.iter().cloned());
            all.bm25.extend(run.bm25.iter().cloned());
            all.hybrid.extend(run.hybrid.iter().cloned());
            all.kinds.extend(run.kinds.iter().copied());
            for d in &run.diagnostics {
                let mut d = d.clone();
                d.id = format!("{corpus}/{}", d.id);
                all.diagnostics.push(d);
            }
            for m in &run.misses {
                let mut m = m.clone();
                m.id = format!("{corpus}/{}", m.id);
                all.misses.push(m);
            }
        }
        all
    }

    pub fn mode_metrics(&self) -> ModeMetrics {
        ModeMetrics {
            dense: metrics::aggregate(&self.dense),
            bm25: metrics::aggregate(&self.bm25),
            hybrid: metrics::aggregate(&self.hybrid),
        }
    }
}

/// Searches for every question. The result does not depend on the fusion numbers.
pub async fn search_all(
    index: &Index,
    questions: &[Question],
    cache: &Cache,
) -> Result<Vec<Candidates>, Error> {
    let mut all = Vec::with_capacity(questions.len());
    for question in questions {
        let query = query_for(question, cache)?;
        all.push(candidates(index, question, &query).await?);
    }
    Ok(all)
}

/// Fuses and judges every question from its candidates.
pub fn judge_all(
    index: &Index,
    questions: &[Question],
    found: &[Candidates],
    params: &Params,
) -> Result<Run, Error> {
    let mut run = Run {
        dense: Vec::new(),
        bm25: Vec::new(),
        hybrid: Vec::new(),
        kinds: Vec::new(),
        diagnostics: Vec::new(),
        misses: Vec::new(),
    };
    for (question, candidates) in questions.iter().zip(found) {
        let answers = answer(index, question, candidates, params)?;
        if question.is_covered() && !metrics::hit_at(&answers.hybrid.returned, RECALL_LARGE) {
            let mut returned = Vec::new();
            for f in answers.hybrid_returned.iter().take(RECALL_LARGE) {
                returned.push(span_text(index.span(&f.passage.key)?));
            }
            run.misses.push(Miss {
                id: question.id.clone(),
                kind: question.kind.name().to_owned(),
                question: question.question.clone(),
                relevant_pages: question.relevant_pages.clone(),
                gated: answers.hybrid.gated,
                returned,
            });
        }
        run.kinds.push(question.kind);
        run.diagnostics.push(answers.diag);
        run.dense.push(answers.dense);
        run.bm25.push(answers.bm25);
        run.hybrid.push(answers.hybrid);
    }
    Ok(run)
}

/// Runs every question over a built index.
pub async fn run_questions(
    index: &Index,
    questions: &[Question],
    cache: &Cache,
    params: &Params,
) -> Result<Run, Error> {
    let found = search_all(index, questions, cache).await?;
    judge_all(index, questions, &found, params)
}

/// How often the gate was wrong, by group of questions. For covered questions: the false-gate
/// rate. For the others: the missed-gate rate.
fn gate_rows(run: &Run) -> Vec<GateRow> {
    type Member = fn(Kind) -> bool;
    let groups: [(&str, Member); 4] = [
        ("covered (false-gate rate)", |k| {
            !matches!(k, Kind::NotCovered | Kind::OffTopic)
        }),
        ("not_covered (missed-gate rate)", |k| k == Kind::NotCovered),
        ("off_topic (missed-gate rate)", |k| k == Kind::OffTopic),
        ("not_covered and off_topic (missed-gate rate)", |k| {
            matches!(k, Kind::NotCovered | Kind::OffTopic)
        }),
    ];
    let wrong = |outcomes: &[RunOutcome], member: Member| -> (usize, usize) {
        let mut total = 0;
        let mut bad = 0;
        for (kind, o) in run.kinds.iter().zip(outcomes) {
            if member(*kind) {
                total += 1;
                // A gated covered question and an ungated uncovered one are both wrong.
                bad += usize::from(o.gated == o.covered);
            }
        }
        (total, bad)
    };
    let rate = |bad: usize, total: usize| {
        if total == 0 {
            0.0
        } else {
            metrics::round4(bad as f64 / total as f64)
        }
    };
    groups
        .iter()
        .map(|(name, member)| {
            let (total, d) = wrong(&run.dense, *member);
            let (_, b) = wrong(&run.bm25, *member);
            let (_, h) = wrong(&run.hybrid, *member);
            GateRow {
                group: (*name).to_owned(),
                questions: total,
                dense: rate(d, total),
                bm25: rate(b, total),
                hybrid: rate(h, total),
            }
        })
        .collect()
}

/// What one run over one corpus found: the report, and the runs behind it (to pool corpora).
pub struct Evaluated {
    pub report: Report,
    pub main: Run,
    /// The run with the per-document cap off. `None` when this run is in that mode itself.
    pub single: Option<Run>,
}

/// What a report says about its corpus, besides the numbers of the runs.
pub struct ReportInfo {
    pub cache_kind: CacheKind,
    pub embedding_model: String,
    pub corpus_sha256: String,
    pub chunks: usize,
    pub settings: String,
    pub latency: Option<Latency>,
    pub headers: HeaderStats,
}

/// The report of a run (or of runs that were pooled).
pub fn report_from_runs(info: ReportInfo, main: &Run, single: Option<&Run>) -> Report {
    let questions = main.kinds.len();
    let covered = main.hybrid.iter().filter(|o| o.covered).count();
    let mut by_kind: BTreeMap<Kind, Vec<RunOutcome>> = BTreeMap::new();
    for (kind, outcome) in main.kinds.iter().zip(&main.hybrid) {
        by_kind.entry(*kind).or_default().push(outcome.clone());
    }
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
    let modes = main.mode_metrics();
    Report {
        cache_kind: info.cache_kind,
        embedding_model: info.embedding_model,
        corpus_sha256: info.corpus_sha256,
        questions,
        covered,
        not_covered: questions - covered,
        chunks: info.chunks,
        dense: modes.dense,
        bm25: modes.bm25,
        hybrid: modes.hybrid,
        hybrid_by_kind,
        gate_by_group: gate_rows(main),
        settings: info.settings,
        cap_off: single.map(Run::mode_metrics),
        misses: main.misses.clone(),
        latency: info.latency,
        gate_sweep: gate_sweep(&main.diagnostics),
        diagnostics: main.diagnostics.clone(),
        headers: Some(info.headers),
    }
}

/// Runs every question of one corpus. A run that is not in cap-off mode also runs the cap-off mode,
/// to report both.
pub async fn evaluate(
    sections: &[Section],
    questions: &[Question],
    cache: &Cache,
    opts: &Options,
) -> Result<Evaluated, Error> {
    let mut index = Index::build(sections, cache, opts.chunk_chars, opts.cap_off)?;
    index.set_candidates(opts.dense_top_k, opts.lexical_top_k);
    let main = run_questions(&index, questions, cache, &opts.params).await?;
    let single = if opts.cap_off {
        None
    } else {
        let mut single = Index::build(sections, cache, opts.chunk_chars, true)?;
        single.set_candidates(opts.dense_top_k, opts.lexical_top_k);
        Some(run_questions(&single, questions, cache, &opts.params).await?)
    };
    let info = ReportInfo {
        cache_kind: cache.meta.kind,
        embedding_model: cache.meta.embedding_model.clone(),
        corpus_sha256: cache.meta.corpus_sha256.clone(),
        chunks: index.chunks.len(),
        settings: opts.describe(),
        latency: cache.meta.latency.clone(),
        headers: header_stats(sections, &cache.pages, opts.chunk_chars)?,
    };
    let report = report_from_runs(info, &main, single.as_ref());
    Ok(Evaluated {
        report,
        main,
        single,
    })
}

/// Runs every question and builds the report of one corpus.
pub async fn run(
    sections: &[Section],
    questions: &[Question],
    cache: &Cache,
    opts: &Options,
) -> Result<Report, Error> {
    Ok(evaluate(sections, questions, cache, opts).await?.report)
}

/// What the removal of running headers and footers did to the chunks of a corpus: the chunks that
/// have a running header in their heading path, before and after.
pub fn header_stats(
    sections: &[Section],
    pages: &[(u32, String)],
    chunk_chars: usize,
) -> Result<HeaderStats, Error> {
    let by_number: HashMap<u32, &str> = pages.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let mut stats = HeaderStats::default();
    for section in sections {
        let local = section_pages(section, &by_number)?;
        let stripped = strip_running_lines(&local);
        let keys: HashSet<&str> = stripped.lines.iter().map(|l| l.key.as_str()).collect();
        let carries = |chunk: &openbook_search::chunker::Chunk| {
            chunk
                .heading_path
                .split(" > ")
                .any(|part| keys.contains(phrase_key(part).as_str()))
        };
        let before = chunk_stripped_pages(&local, chunk_chars);
        let after = chunk_pages_with(&local, chunk_chars);
        stats.running_lines += stripped.lines.len();
        stats.removed_lines += stripped
            .lines
            .iter()
            .map(|l| l.removed_from.len())
            .sum::<usize>();
        stats.chunks_before += before.len();
        stats.chunks_with_header_before += before.iter().filter(|c| carries(c)).count();
        stats.chunks += after.len();
        stats.chunks_with_header_after += after.iter().filter(|c| carries(c)).count();
        for line in &stripped.lines {
            stats.lines.push(HeaderLine {
                section: section.id.clone(),
                text: line.first_text.clone(),
                first_page: line.first_page + section.first_page - 1,
                removed_pages: line.removed_from.len(),
            });
        }
    }
    Ok(stats)
}

/// The data and the cache of one corpus.
pub struct Loaded {
    pub corpus: Corpus,
    pub sections: Vec<Section>,
    pub questions: Vec<Question>,
    pub cache: Cache,
}

/// Reads the data and the cache of a corpus, and checks that they belong together.
pub fn load(corpus: &Corpus, cache_dir: &Path) -> Result<Loaded, Error> {
    let sections = crate::data::load_sections(&corpus.sections_path())?;
    let questions =
        crate::data::load_golden(&corpus.golden_path(), crate::data::page_total(&sections))?;
    let cache = Cache::load(cache_dir).map_err(|e| match e {
        Error::Cache(text) => Error::Cache(format!("corpus {}: {text}", corpus.id)),
        other => other,
    })?;
    let sha = crate::cache::corpus_sha256(&corpus.pdf_path()?)?;
    if sha != cache.meta.corpus_sha256 {
        return Err(Error::Cache(format!(
            "the cache of corpus {} was made for another corpus file. Refresh the cache.",
            corpus.id
        )));
    }
    if cache.meta.pages != crate::data::page_total(&sections) {
        return Err(Error::Cache(format!(
            "the cache of corpus {} has {} pages and the sections cover {}",
            corpus.id,
            cache.meta.pages,
            crate::data::page_total(&sections)
        )));
    }
    Ok(Loaded {
        corpus: corpus.clone(),
        sections,
        questions,
        cache,
    })
}

/// Where the cache of a corpus is: the one that the caller names (only for one corpus), or the
/// live or development cache of the corpus.
pub fn cache_dir_of(corpus: &Corpus, dev: bool, named: Option<&Path>) -> PathBuf {
    named.map_or_else(|| corpus.cache_dir(dev), Path::to_path_buf)
}

/// Pools the runs of several corpora into one report. The ID of a question gets the corpus as its
/// prefix, so two corpora can use the same question IDs.
pub fn combine(parts: &[(&Corpus, &Evaluated)]) -> Result<Report, Error> {
    let first = &parts
        .first()
        .ok_or_else(|| Error::Run("there is no corpus to combine".into()))?
        .1
        .report;
    for (corpus, evaluated) in parts {
        let r = &evaluated.report;
        if r.cache_kind != first.cache_kind || r.embedding_model != first.embedding_model {
            return Err(Error::Cache(format!(
                "corpus {} has another kind of cache or another embedding model than the first corpus. Refresh the caches with one model.",
                corpus.id
            )));
        }
    }
    let main_parts: Vec<(&str, &Run)> = parts
        .iter()
        .map(|(c, e)| (c.id.as_str(), &e.main))
        .collect();
    let main = Run::merged(&main_parts);
    let single = if parts.iter().all(|(_, e)| e.single.is_some()) {
        let singles: Vec<(&str, &Run)> = parts
            .iter()
            .filter_map(|(c, e)| e.single.as_ref().map(|s| (c.id.as_str(), s)))
            .collect();
        Some(Run::merged(&singles))
    } else {
        None
    };
    let mut headers = HeaderStats::default();
    for (_, e) in parts {
        if let Some(h) = &e.report.headers {
            headers.add(h);
        }
    }
    let info = ReportInfo {
        cache_kind: first.cache_kind,
        embedding_model: first.embedding_model.clone(),
        corpus_sha256: parts
            .iter()
            .map(|(_, e)| e.report.corpus_sha256.as_str())
            .collect::<Vec<_>>()
            .join("+"),
        chunks: parts.iter().map(|(_, e)| e.report.chunks).sum(),
        settings: first.settings.clone(),
        latency: None,
        headers,
    };
    Ok(report_from_runs(info, &main, single.as_ref()))
}

/// Runs the evaluation for corpora, from their caches.
pub async fn run_suite(
    corpora: &[Corpus],
    dev: bool,
    named_cache: Option<&Path>,
    tuning: &Tuning,
) -> Result<SuiteReport, Error> {
    let mut done: Vec<(&Corpus, Evaluated)> = Vec::new();
    for corpus in corpora {
        let loaded = load(corpus, &cache_dir_of(corpus, dev, named_cache))?;
        let opts = tuning.options(&loaded.cache.meta.embedding_model);
        let evaluated = evaluate(&loaded.sections, &loaded.questions, &loaded.cache, &opts).await?;
        done.push((corpus, evaluated));
    }
    let parts: Vec<(&Corpus, &Evaluated)> = done.iter().map(|(c, e)| (*c, e)).collect();
    let combined = combine(&parts)?;
    Ok(SuiteReport {
        corpora: done
            .into_iter()
            .map(|(c, e)| CorpusReport {
                id: c.id.clone(),
                report: e.report,
            })
            .collect(),
        combined,
    })
}
