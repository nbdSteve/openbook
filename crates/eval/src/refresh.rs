//! The cache refresh.
//!
//! - `refresh --dev-cache` needs no AWS. The pages are the PDF text layer, the embeddings are fake,
//!   and a follow-up is rewritten by joining it to its earlier question. It writes the
//!   `cache-dev/` of each corpus. It shows that the harness works. It never gives a baseline.
//! - `refresh` (live) runs the real ingestion code on the corpus PDF on this machine: pdfium, the
//!   real Bedrock page transcription for pages with a poor text layer, the real chunker, Titan
//!   embeddings, and the real follow-up rewrite with the fast model. It writes the `cache/` of each
//!   corpus. It reads the normal AWS environment and prints what it will call before it calls
//!   anything.
//! - `refresh --missing-only` fills what the caches lack, for all corpora in one plan: a corpus
//!   without a cache gets the full live refresh (page transcription included), and a corpus with
//!   a cache gets only the embeddings and rewrites that it lacks.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use openbook_api::retrieval::rewrite_query;
use openbook_core::catalog::{ModelEntry, ProviderKind, Residency};
use openbook_core::config::DEFAULT_EMBEDDING_MODEL_ID;
use openbook_core::domain::MessageRole;
use openbook_core::model::BedrockChat;
use openbook_ingest::pdf::{self, PageData};
use openbook_ingest::transcribe::{BedrockTranscriber, TranscribeError, Transcriber};
use openbook_search::chunker::TARGET_CHARS;
use openbook_search::embed::{
    DIMENSIONS, EmbedError, Embedder, FakeEmbedder, TitanEmbedder, embed_all,
};
use openbook_search::retrieval::{MAX_QUERY_CHARS, clean_query, clip};

use crate::Error;
use crate::cache::{Cache, CacheKind, EmbeddingStore, FORMAT, Latency, Meta, rewrite_key};
use crate::corpus::Corpus;
use crate::data::{self, Kind, Question, Section};
use crate::offline::{CHARS_PER_TOKEN, build_chunks_with};
use openbook_search::headers::FirstOccurrence;

/// The model for page transcription and for the follow-up rewrite, unless `OPENBOOK_EVAL_MODEL_ID`
/// says another one. It is the fast model of the stack.
pub const DEFAULT_MODEL_ID: &str = "au.anthropic.claude-haiku-4-5-20251001-v1:0";
/// The Region when the environment has none. It is the Region of the stack.
pub const DEFAULT_REGION: &str = "ap-southeast-2";
pub const DEV_EMBEDDING_MODEL: &str = "fake";
pub const DEV_REWRITE_MODEL: &str = "dev-concat";

// Rough prices in US dollars, for the estimate that refresh prints. They are list prices of
// Claude Haiku 4.5 ($1 per million input tokens, $5 per million output tokens) and Titan Text
// Embeddings v2 ($0.02 per million tokens) when this was written. Check the current price list.
const TRANSCRIBE_USD_PER_PAGE: f64 = 0.0095;
const EMBED_USD_PER_CALL: f64 = 0.000_01;
const REWRITE_USD_PER_CALL: f64 = 0.0005;

/// The chunk targets of the grid, in tokens. The cache holds the chunk embeddings of all of them, so
/// the grid runs offline. The production target is the middle one.
pub const CHUNK_TARGET_TOKENS: [usize; 3] = [300, 400, 600];

/// The production chunk target, in tokens.
pub const PRODUCTION_CHUNK_TOKENS: usize = TARGET_CHARS / CHARS_PER_TOKEN;

/// Reads the pages of the corpus PDF with pdfium, like the ingestion worker does.
pub async fn read_pages(pdf_path: &Path) -> Result<Vec<PageData>, Error> {
    let library = std::env::var_os("OPENBOOK_PDFIUM_PATH").map(PathBuf::from);
    let dir = pdf::library_dir(library.as_deref());
    let path = pdf_path.to_path_buf();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<PageData>(8);
    let reader =
        tokio::task::spawn_blocking(move || pdf::extract_pages(&dir, &path, &HashSet::new(), &tx));
    let mut pages = Vec::new();
    while let Some(page) = rx.recv().await {
        pages.push(page);
    }
    reader
        .await
        .map_err(|e| Error::Run(e.to_string()))?
        .map_err(|e| {
            Error::Run(format!(
                "{e}. Set OPENBOOK_PDFIUM_PATH to the directory that scripts/fetch-pdfium.sh prints."
            ))
        })?;
    pages.sort_by_key(|p| p.number);
    Ok(pages)
}

/// What the query of a question is, once the rewrite is known.
fn plain_query(question: &Question) -> String {
    clip(question.question.trim(), MAX_QUERY_CHARS)
}

/// The text of every chunk (for each chunk target in `targets`, in tokens) and every query, in a
/// fixed order, without repeats.
pub fn texts_to_embed(
    sections: &[Section],
    pages: &[(u32, String)],
    queries: &[String],
    targets: &[usize],
) -> Result<Vec<String>, Error> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tokens in targets {
        // Both variants of the removal of running headers, so that the grid runs offline.
        for variant in FirstOccurrence::ALL {
            for chunk in build_chunks_with(sections, pages, tokens * CHARS_PER_TOKEN, variant)? {
                if seen.insert(chunk.embed_text.clone()) {
                    out.push(chunk.embed_text);
                }
            }
        }
    }
    for query in queries {
        if seen.insert(query.clone()) {
            out.push(query.clone());
        }
    }
    Ok(out)
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

// ---- Dev cache

/// Builds the development cache: no AWS.
pub async fn build_dev(corpus: &Corpus) -> Result<Cache, Error> {
    let sections = data::load_sections(&corpus.sections_path())?;
    let questions = data::load_golden(&corpus.golden_path(), data::page_total(&sections))?;
    let pdf_path = corpus.pdf_path()?;
    let page_data = read_pages(&pdf_path).await?;
    let total = data::page_total(&sections);
    if page_data.len() != total as usize {
        return Err(Error::Data(format!(
            "the PDF has {} pages and the sections cover {total}",
            page_data.len()
        )));
    }
    let pages: Vec<(u32, String)> = page_data
        .iter()
        .map(|p| (p.number, p.text.clone()))
        .collect();

    let mut rewrites = std::collections::BTreeMap::new();
    let mut queries = Vec::new();
    for q in &questions {
        match (&q.kind, &q.context) {
            (Kind::Followup, Some(context)) => {
                let joined = clean_query(&format!("{context} {}", q.question))
                    .unwrap_or_else(|| plain_query(q));
                rewrites.insert(
                    rewrite_key(DEV_REWRITE_MODEL, context, &q.question),
                    joined.clone(),
                );
                queries.push(joined);
            }
            _ => queries.push(plain_query(q)),
        }
    }
    let mut embeddings = EmbeddingStore::new(DEV_EMBEDDING_MODEL, DIMENSIONS);
    for text in texts_to_embed(&sections, &pages, &queries, &CHUNK_TARGET_TOKENS)? {
        embeddings.insert(&text, &FakeEmbedder::vector(&text))?;
    }
    Ok(Cache {
        meta: Meta {
            format: FORMAT,
            kind: CacheKind::Dev,
            corpus: file_name(&pdf_path),
            corpus_sha256: crate::cache::corpus_sha256(&pdf_path)?,
            embedding_model: DEV_EMBEDDING_MODEL.into(),
            transcribe_model: None,
            rewrite_model: DEV_REWRITE_MODEL.into(),
            pages: total,
            transcribed_pages: 0,
            fallback_pages: page_data
                .iter()
                .filter(|p| p.needs_transcription)
                .map(|p| p.number)
                .collect(),
            latency: None,
        },
        pages,
        embeddings,
        rewrites,
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---- Live refresh

/// The models of a cache. A corpus that is added later uses the models of the corpora that are
/// there, so that every cache has the same embedding model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Models {
    pub chat: String,
    pub embedding: String,
}

/// An upper bound of the cost of one transcribed page: 4096 output tokens at $5 per million, and at
/// most 6000 input tokens at $1 per million.
const TRANSCRIBE_UPPER_USD_PER_PAGE: f64 = 4096.0 * 5.0 / 1_000_000.0 + 6000.0 / 1_000_000.0;

/// What a live refresh will call.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub model_id: String,
    pub embedding_model: String,
    pub region: String,
    pub profile: String,
    /// Pages whose text layer is poor: each is one transcription call.
    pub transcribe_pages: Vec<u32>,
    pub embed_calls: usize,
    /// The characters of the texts to embed, for the bound of the cost. Pages that a model
    /// transcribes can give other chunks than the text layer, so this is an estimate.
    pub embed_chars: usize,
    pub rewrite_calls: usize,
}

impl Plan {
    /// An upper bound of the cost in US dollars: every page at the most that a call can cost, and
    /// twice the embedding text of the text layer.
    pub fn cost_upper_bound_usd(&self) -> f64 {
        let embed_tokens = self.embed_chars as f64 / 2.0 * 2.0;
        self.transcribe_pages.len() as f64 * TRANSCRIBE_UPPER_USD_PER_PAGE
            + embed_tokens * EMBED_USD_PER_TOKEN
            + self.rewrite_calls as f64 * REWRITE_UPPER_USD_PER_CALL
    }

    pub fn cost_usd(&self) -> f64 {
        self.transcribe_pages.len() as f64 * TRANSCRIBE_USD_PER_PAGE
            + self.embed_calls as f64 * EMBED_USD_PER_CALL
            + self.rewrite_calls as f64 * REWRITE_USD_PER_CALL
    }

    pub fn describe(&self) -> String {
        let mut out = String::new();
        out.push_str("openbook-eval refresh will call REAL AWS services with the credentials of this environment.\n");
        out.push_str(&format!(
            "  AWS profile: {}\n  AWS Region: {}\n\n",
            self.profile, self.region
        ));
        out.push_str(&format!(
            "  1. Bedrock Converse (page transcription), model {}: {} calls, one per page with a poor text layer (pages {:?}). About ${:.2}.\n",
            self.model_id,
            self.transcribe_pages.len(),
            self.transcribe_pages,
            self.transcribe_pages.len() as f64 * TRANSCRIBE_USD_PER_PAGE
        ));
        out.push_str(&format!(
            "  2. Bedrock InvokeModel (embeddings), model {}: about {} calls, one per chunk and one per question. About ${:.2}.\n",
            self.embedding_model,
            self.embed_calls,
            self.embed_calls as f64 * EMBED_USD_PER_CALL
        ));
        out.push_str(&format!(
            "  3. Bedrock ConverseStream (rewrite of follow-up questions), model {}: {} calls. About ${:.2}.\n\n",
            self.model_id,
            self.rewrite_calls,
            self.rewrite_calls as f64 * REWRITE_USD_PER_CALL
        ));
        out.push_str(&format!(
            "  Total: about ${:.2} at list prices (a rough estimate).\n",
            self.cost_usd()
        ));
        out.push_str("  It does not touch S3, DynamoDB, SQS, or S3 Vectors. It writes only to evals/cache/.\n");
        out
    }
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// The plan for a live refresh. It reads the PDF locally and calls nothing.
pub async fn plan_live(
    corpus: &Corpus,
    models: Option<&Models>,
) -> Result<(Plan, Vec<PageData>), Error> {
    let sections = data::load_sections(&corpus.sections_path())?;
    let questions = data::load_golden(&corpus.golden_path(), data::page_total(&sections))?;
    let pages = read_pages(&corpus.pdf_path()?).await?;
    let total = data::page_total(&sections);
    if pages.len() != total as usize {
        return Err(Error::Data(format!(
            "the PDF has {} pages and the sections cover {total}",
            pages.len()
        )));
    }
    let text: Vec<(u32, String)> = pages.iter().map(|p| (p.number, p.text.clone())).collect();
    let queries: Vec<String> = questions.iter().map(plain_query).collect();
    let to_embed = texts_to_embed(&sections, &text, &queries, &CHUNK_TARGET_TOKENS)?;
    let embed_calls = to_embed.len();
    let embed_chars: usize = to_embed.iter().map(|t| t.chars().count()).sum();
    let transcribe_pages: Vec<u32> = pages
        .iter()
        .filter(|p| p.needs_transcription && p.image.is_some())
        .map(|p| p.number)
        .collect();
    let plan = Plan {
        model_id: models.map_or_else(
            || env_or("OPENBOOK_EVAL_MODEL_ID", DEFAULT_MODEL_ID),
            |m| m.chat.clone(),
        ),
        embedding_model: models.map_or_else(
            || env_or("OPENBOOK_EMBEDDING_MODEL_ID", DEFAULT_EMBEDDING_MODEL_ID),
            |m| m.embedding.clone(),
        ),
        region: std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| format!("{DEFAULT_REGION} (default)")),
        profile: std::env::var("AWS_PROFILE")
            .unwrap_or_else(|_| "(none: the default credential chain)".into()),
        transcribe_pages,
        embed_calls,
        embed_chars,
        rewrite_calls: questions
            .iter()
            .filter(|q| q.kind == Kind::Followup)
            .count(),
    };
    Ok((plan, pages))
}

async fn retry<T, E, F, Fut>(
    what: &str,
    mut call: F,
    transient: impl Fn(&E) -> bool,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut wait = Duration::from_secs(1);
    let mut attempt = 1;
    loop {
        match call().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt < 5 && transient(&error) => {
                eprintln!("  {what}: attempt {attempt} failed ({error}); retrying in {wait:?}");
                tokio::time::sleep(wait).await;
                wait *= 2;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// The error for a rewrite that gave no query (the model failed, timed out, or said nothing).
fn rewrite_failed(id: &str) -> Error {
    Error::Aws(format!(
        "the rewrite of follow-up {id} gave no query. Nothing was stored for it: run the refresh again"
    ))
}

fn eval_entry(model_id: &str) -> ModelEntry {
    ModelEntry {
        id: "eval".into(),
        label: "Evaluation model".into(),
        vendor: "anthropic".into(),
        provider: ProviderKind::Bedrock,
        model_id: model_id.to_owned(),
        residency: Residency::Au,
        max_output_tokens: Some(4096),
        temperature: None,
        vision: true,
    }
}

/// Runs the live refresh. The caller has shown the plan and has the go-ahead.
pub async fn build_live(
    corpus: &Corpus,
    plan: &Plan,
    pages: Vec<PageData>,
) -> Result<Cache, Error> {
    let sections = data::load_sections(&corpus.sections_path())?;
    let questions = data::load_golden(&corpus.golden_path(), data::page_total(&sections))?;
    let pdf_path = corpus.pdf_path()?;

    let sdk = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(
            aws_config::meta::region::RegionProviderChain::default_provider()
                .or_else(DEFAULT_REGION),
        )
        .load()
        .await;
    let client = aws_sdk_bedrockruntime::Client::new(&sdk);
    let entry = eval_entry(&plan.model_id);
    let transcriber =
        BedrockTranscriber::new(client.clone(), plan.model_id.clone(), entry.max_tokens());
    let embedder = TitanEmbedder::new(client.clone(), plan.embedding_model.clone());
    let chat = BedrockChat::new(client, &entry);

    // 1. The page Markdown, like the ingestion worker makes it.
    eprintln!("Transcribing {} pages ...", plan.transcribe_pages.len());
    let results: Vec<(u32, String, bool, bool)> = futures::stream::iter(pages)
        .map(|page| {
            let transcriber = &transcriber;
            async move {
                let wants = page.needs_transcription;
                let Some(image) = page.image.as_ref().filter(|_| wants) else {
                    // A page that needs transcription but could not be rendered keeps its text layer.
                    return Ok::<_, Error>((page.number, page.text.clone(), false, wants));
                };
                let outcome = retry(
                    &format!("page {}", page.number),
                    || transcriber.transcribe(page.number, image, &page.text),
                    |e| matches!(e, TranscribeError::Transient(_)),
                )
                .await;
                match outcome {
                    Ok(markdown) => Ok((page.number, markdown, true, false)),
                    Err(TranscribeError::Permanent(error)) => {
                        eprintln!(
                            "  page {}: transcription failed ({error}); using the text layer",
                            page.number
                        );
                        Ok((page.number, page.text.clone(), false, true))
                    }
                    Err(error) => Err(Error::Aws(format!("page {}: {error}", page.number))),
                }
            }
        })
        .buffered(4)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()?;
    let transcribed_pages = results.iter().filter(|r| r.2).count() as u32;
    let fallback_pages: Vec<u32> = results.iter().filter(|r| r.3).map(|r| r.0).collect();
    let page_text: Vec<(u32, String)> = results.into_iter().map(|r| (r.0, r.1)).collect();

    // 2. The search query of every question. A follow-up goes through the real rewrite.
    eprintln!("Rewriting {} follow-up questions ...", plan.rewrite_calls);
    let mut rewrites = std::collections::BTreeMap::new();
    let mut queries = Vec::new();
    for q in &questions {
        match (&q.kind, &q.context) {
            (Kind::Followup, Some(context)) => {
                let earlier = [(MessageRole::User, context.clone())];
                let (better, _usage) = retry(
                    &format!("rewrite {}", q.id),
                    || async { Ok::<_, String>(rewrite_query(&chat, &earlier, &q.question).await) },
                    |_| false,
                )
                .await
                .map_err(Error::Aws)?;
                // The API uses the raw message when the rewrite fails, but a cache that holds the raw
                // question as the "rewrite" would hide the failure and measure the wrong thing.
                let query = better.ok_or_else(|| rewrite_failed(&q.id))?;
                rewrites.insert(
                    rewrite_key(&plan.model_id, context, &q.question),
                    query.clone(),
                );
                queries.push(query);
            }
            _ => queries.push(plain_query(q)),
        }
    }

    // 3. Embeddings: chunks in a batch, then each query alone so that its time is the time of one call.
    let texts = texts_to_embed(&sections, &page_text, &queries, &CHUNK_TARGET_TOKENS)?;
    let query_set: HashSet<&String> = queries.iter().collect();
    let (query_texts, chunk_texts): (Vec<String>, Vec<String>) =
        texts.into_iter().partition(|t| query_set.contains(t));
    eprintln!("Embedding {} chunks ...", chunk_texts.len());
    let mut store = EmbeddingStore::new(&plan.embedding_model, DIMENSIONS);
    let vectors = retry(
        "chunk embeddings",
        || embed_all(&embedder, &chunk_texts),
        |e| matches!(e, EmbedError::Transient(_)),
    )
    .await
    .map_err(|e| Error::Aws(e.to_string()))?;
    for (text, vector) in chunk_texts.iter().zip(&vectors) {
        store.insert(text, vector)?;
    }
    eprintln!("Embedding {} questions ...", query_texts.len());
    let mut millis = Vec::new();
    for text in &query_texts {
        let started = Instant::now();
        let vector = retry(
            "query embedding",
            || embedder.embed(text),
            |e| matches!(e, EmbedError::Transient(_)),
        )
        .await
        .map_err(|e| Error::Aws(e.to_string()))?;
        millis.push(started.elapsed().as_millis() as u64);
        store.insert(text, &vector)?;
    }
    millis.sort_unstable();
    let latency = Latency {
        query_embedding_ms_p50: percentile(&millis, 0.50),
        query_embedding_ms_p95: percentile(&millis, 0.95),
        samples: millis.len(),
    };

    Ok(Cache {
        meta: Meta {
            format: FORMAT,
            kind: CacheKind::Live,
            corpus: file_name(&pdf_path),
            corpus_sha256: crate::cache::corpus_sha256(&pdf_path)?,
            embedding_model: plan.embedding_model.clone(),
            transcribe_model: Some(plan.model_id.clone()),
            rewrite_model: plan.model_id.clone(),
            pages: data::page_total(&sections),
            transcribed_pages,
            fallback_pages,
            latency: Some(latency),
        },
        pages: page_text,
        embeddings: store,
        rewrites,
    })
}

// ---- Incremental live refresh

/// Rough upper bounds for the estimate of `refresh --missing-only`. A text of `n` characters has
/// at most `n / 2` tokens (math text tokenizes worse than the 4 characters per token of prose).
/// Titan Text Embeddings v2 costs $0.02 per million tokens. A rewrite sends at most 1000 input
/// tokens ($1 per million) and gets at most 100 output tokens ($5 per million).
const EMBED_USD_PER_TOKEN: f64 = 0.02 / 1_000_000.0;
const REWRITE_UPPER_USD_PER_CALL: f64 = 1000.0 * 1.0 / 1_000_000.0 + 100.0 * 5.0 / 1_000_000.0;

/// What `refresh --missing-only` will call: only what the cache does not have.
#[derive(Debug, Clone, PartialEq)]
pub struct MissingPlan {
    pub model_id: String,
    pub embedding_model: String,
    pub region: String,
    pub profile: String,
    /// The texts that have no embedding in the cache (chunks of every chunk target, and queries).
    pub embed_texts: Vec<String>,
    /// Follow-up questions that have no cached rewrite. Each is one rewrite call and one more
    /// embedding call, because the rewritten query is a new text.
    pub rewrite_ids: Vec<String>,
}

impl MissingPlan {
    pub fn embed_calls(&self) -> usize {
        self.embed_texts.len() + self.rewrite_ids.len()
    }

    /// An upper bound of the cost in US dollars.
    pub fn cost_upper_bound_usd(&self) -> f64 {
        let tokens: f64 = self
            .embed_texts
            .iter()
            .map(|t| t.chars().count() as f64 / 2.0)
            .sum::<f64>()
            // A rewritten query is at most 500 characters.
            + self.rewrite_ids.len() as f64 * MAX_QUERY_CHARS as f64 / 2.0;
        tokens * EMBED_USD_PER_TOKEN + self.rewrite_ids.len() as f64 * REWRITE_UPPER_USD_PER_CALL
    }

    pub fn is_empty(&self) -> bool {
        self.embed_texts.is_empty() && self.rewrite_ids.is_empty()
    }

    pub fn describe(&self) -> String {
        let mut out = String::new();
        out.push_str("openbook-eval refresh --missing-only will call REAL AWS services with the credentials of this environment.\n");
        out.push_str(&format!(
            "  AWS profile: {}\n  AWS Region: {}\n\n",
            self.profile, self.region
        ));
        out.push_str(
            "  It reuses evals/cache/pages. It does NO page transcription and no other call.\n",
        );
        out.push_str(&format!(
            "  1. Bedrock InvokeModel (embeddings), model {}: {} calls for the texts that the cache does not have, and up to {} more for the rewritten follow-up queries.\n",
            self.embedding_model,
            self.embed_texts.len(),
            self.rewrite_ids.len()
        ));
        out.push_str(&format!(
            "  2. Bedrock ConverseStream (rewrite of follow-up questions), model {}: {} calls{}.\n\n",
            self.model_id,
            self.rewrite_ids.len(),
            if self.rewrite_ids.is_empty() {
                String::new()
            } else {
                format!(" ({})", self.rewrite_ids.join(", "))
            }
        ));
        out.push_str(&format!(
            "  Total: at most ${:.4} at list prices (an upper bound).\n",
            self.cost_upper_bound_usd()
        ));
        out.push_str("  It does not touch S3, DynamoDB, SQS, or S3 Vectors. It writes only to evals/cache/.\n");
        out
    }
}

/// The search queries of the questions that are known now, and the follow-ups that have no cached
/// rewrite.
fn known_queries<'a>(questions: &'a [Question], cache: &Cache) -> (Vec<String>, Vec<&'a Question>) {
    let mut queries = Vec::new();
    let mut missing = Vec::new();
    for q in questions {
        match (&q.kind, &q.context) {
            (Kind::Followup, Some(context)) => {
                let key = rewrite_key(&cache.meta.rewrite_model, context, &q.question);
                match cache.rewrites.get(&key) {
                    Some(query) => queries.push(query.clone()),
                    None => missing.push(q),
                }
            }
            _ => queries.push(plain_query(q)),
        }
    }
    (queries, missing)
}

/// Reads the live cache and works out what it lacks. It calls nothing.
pub fn plan_missing(corpus: &Corpus, cache_dir: &Path) -> Result<(MissingPlan, Cache), Error> {
    let sections = data::load_sections(&corpus.sections_path())?;
    let questions = data::load_golden(&corpus.golden_path(), data::page_total(&sections))?;
    let cache = Cache::load(cache_dir)?;
    if cache.meta.kind != CacheKind::Live {
        return Err(Error::Run(
            "--missing-only works on a live cache. A dev cache has no Bedrock calls to add.".into(),
        ));
    }
    let sha = crate::cache::corpus_sha256(&corpus.pdf_path()?)?;
    if sha != cache.meta.corpus_sha256 {
        return Err(Error::Cache(
            "the cache was made for another corpus file. Run a full `refresh`.".into(),
        ));
    }
    let (queries, missing) = known_queries(&questions, &cache);
    let embed_texts: Vec<String> =
        texts_to_embed(&sections, &cache.pages, &queries, &CHUNK_TARGET_TOKENS)?
            .into_iter()
            .filter(|t| !cache.embeddings.contains(t))
            .collect();
    let plan = MissingPlan {
        // The models of the cache decide, so that the cache keys stay valid.
        model_id: cache.meta.rewrite_model.clone(),
        embedding_model: cache.meta.embedding_model.clone(),
        region: std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| format!("{DEFAULT_REGION} (default)")),
        profile: std::env::var("AWS_PROFILE")
            .unwrap_or_else(|_| "(none: the default credential chain)".into()),
        embed_texts,
        rewrite_ids: missing.iter().map(|q| q.id.clone()).collect(),
    };
    Ok((plan, cache))
}

/// Makes the calls of the plan and returns the completed cache. The pages stay as they are.
pub async fn run_missing(
    corpus: &Corpus,
    plan: &MissingPlan,
    mut cache: Cache,
) -> Result<Cache, Error> {
    let sections = data::load_sections(&corpus.sections_path())?;
    let questions = data::load_golden(&corpus.golden_path(), data::page_total(&sections))?;
    let sdk = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(
            aws_config::meta::region::RegionProviderChain::default_provider()
                .or_else(DEFAULT_REGION),
        )
        .load()
        .await;
    let client = aws_sdk_bedrockruntime::Client::new(&sdk);
    let entry = eval_entry(&plan.model_id);
    let embedder = TitanEmbedder::new(client.clone(), plan.embedding_model.clone());
    let chat = BedrockChat::new(client, &entry);

    // 1. The rewrites that the cache does not have.
    let (_, missing) = known_queries(&questions, &cache);
    eprintln!("Rewriting {} follow-up questions ...", missing.len());
    for q in missing {
        let Some(context) = &q.context else { continue };
        let earlier = [(MessageRole::User, context.clone())];
        let (better, _usage) = retry(
            &format!("rewrite {}", q.id),
            || async { Ok::<_, String>(rewrite_query(&chat, &earlier, &q.question).await) },
            |_| false,
        )
        .await
        .map_err(Error::Aws)?;
        let query = better.ok_or_else(|| rewrite_failed(&q.id))?;
        cache.rewrites.insert(
            rewrite_key(&cache.meta.rewrite_model, context, &q.question),
            query,
        );
    }

    // 2. The embeddings that the cache does not have (the rewritten queries are known now).
    let (queries, _) = known_queries(&questions, &cache);
    let needed = texts_to_embed(&sections, &cache.pages, &queries, &CHUNK_TARGET_TOKENS)?;
    let to_embed: Vec<String> = needed
        .iter()
        .filter(|t| !cache.embeddings.contains(t))
        .cloned()
        .collect();
    eprintln!("Embedding {} texts ...", to_embed.len());
    let vectors = retry(
        "embeddings",
        || embed_all(&embedder, &to_embed),
        |e| matches!(e, EmbedError::Transient(_)),
    )
    .await
    .map_err(|e| Error::Aws(e.to_string()))?;
    for (text, vector) in to_embed.iter().zip(&vectors) {
        cache.embeddings.insert(text, vector)?;
    }

    // 3. Keep only what the run needs, so that the cache is what a full refresh would hold.
    let mut kept = EmbeddingStore::new(cache.embeddings.model(), cache.embeddings.dimensions());
    for text in &needed {
        kept.insert(text, &cache.embeddings.get(text)?)?;
    }
    cache.embeddings = kept;
    Ok(cache)
}

// ---- One plan for all corpora

/// What the refresh does for one corpus.
pub enum Action {
    /// The corpus has no cache: the full live refresh, with the page transcription.
    Full { plan: Plan, pages: Vec<PageData> },
    /// The corpus has a live cache: only the calls for what the cache lacks.
    Missing {
        plan: MissingPlan,
        cache: Box<Cache>,
    },
}

pub struct Step {
    pub corpus: Corpus,
    pub action: Action,
}

/// The plan of one `refresh` command over one or more corpora.
pub struct Refresh {
    pub region: String,
    pub profile: String,
    pub steps: Vec<Step>,
}

fn current_region() -> String {
    std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| format!("{DEFAULT_REGION} (default)"))
}

fn current_profile() -> String {
    std::env::var("AWS_PROFILE").unwrap_or_else(|_| "(none: the default credential chain)".into())
}

/// Makes the plan. It reads the PDFs of the corpora that have no cache and calls nothing.
///
/// With `missing_only`, a corpus that has a live cache gets only the calls that its cache lacks,
/// and a corpus that has none gets the full live refresh with the models of the other caches (so
/// that all caches have one embedding model). Without it, every corpus gets the full refresh.
pub async fn plan_refresh(corpora: &[Corpus], missing_only: bool) -> Result<Refresh, Error> {
    let mut planned: Vec<Option<Action>> = Vec::new();
    let mut models: Option<Models> = None;
    for corpus in corpora {
        let dir = corpus.cache_dir(false);
        if missing_only && dir.join("meta.json").exists() {
            let (plan, cache) = plan_missing(corpus, &dir)?;
            models.get_or_insert_with(|| Models {
                chat: plan.model_id.clone(),
                embedding: plan.embedding_model.clone(),
            });
            planned.push(Some(Action::Missing {
                plan,
                cache: Box::new(cache),
            }));
        } else {
            planned.push(None);
        }
    }
    let mut steps = Vec::new();
    for (corpus, action) in corpora.iter().zip(planned) {
        let action = match action {
            Some(action) => action,
            None => {
                let (plan, pages) = plan_live(corpus, models.as_ref()).await?;
                Action::Full { plan, pages }
            }
        };
        steps.push(Step {
            corpus: corpus.clone(),
            action,
        });
    }
    Ok(Refresh {
        region: current_region(),
        profile: current_profile(),
        steps,
    })
}

impl Refresh {
    /// True when no step has a call to make.
    pub fn is_empty(&self) -> bool {
        self.steps.iter().all(|s| match &s.action {
            Action::Full { .. } => false,
            Action::Missing { plan, .. } => plan.is_empty(),
        })
    }

    /// An upper bound of the cost in US dollars.
    pub fn cost_upper_bound_usd(&self) -> f64 {
        self.steps
            .iter()
            .map(|s| match &s.action {
                Action::Full { plan, .. } => plan.cost_upper_bound_usd(),
                Action::Missing { plan, .. } => plan.cost_upper_bound_usd(),
            })
            .sum()
    }

    pub fn describe(&self) -> String {
        let mut out = String::new();
        out.push_str("openbook-eval refresh will call REAL AWS services with the credentials of this environment.\n");
        out.push_str(&format!(
            "  AWS profile: {}\n  AWS Region: {}\n",
            self.profile, self.region
        ));
        for step in &self.steps {
            out.push('\n');
            match &step.action {
                Action::Full { plan, .. } => {
                    out.push_str(&format!(
                        "Corpus {}: no cache yet, so this is the full live refresh. It reads {}.\n",
                        step.corpus.id,
                        step.corpus
                            .pdf_path()
                            .map(|p| file_name(&p))
                            .unwrap_or_default()
                    ));
                    out.push_str(&format!(
                        "  1. Bedrock Converse (page transcription), model {}: {} calls, one per page with a poor text layer (pages {:?}).\n",
                        plan.model_id,
                        plan.transcribe_pages.len(),
                        plan.transcribe_pages
                    ));
                    out.push_str(&format!(
                        "  2. Bedrock InvokeModel (embeddings), model {}: about {} calls, one per chunk (every chunk target of the grid) and one per question.\n",
                        plan.embedding_model, plan.embed_calls
                    ));
                    out.push_str(&format!(
                        "  3. Bedrock ConverseStream (rewrite of follow-up questions), model {}: {} calls.\n",
                        plan.model_id, plan.rewrite_calls
                    ));
                    out.push_str(&format!(
                        "  Bound for this corpus: ${:.4}.\n",
                        plan.cost_upper_bound_usd()
                    ));
                }
                Action::Missing { plan, .. } => {
                    out.push_str(&format!(
                        "Corpus {}: the cache has the pages. No page transcription.\n",
                        step.corpus.id
                    ));
                    out.push_str(&format!(
                        "  1. Bedrock InvokeModel (embeddings), model {}: {} calls for the texts that the cache does not have, and up to {} more for the rewritten follow-up queries.\n",
                        plan.embedding_model,
                        plan.embed_texts.len(),
                        plan.rewrite_ids.len()
                    ));
                    out.push_str(&format!(
                        "  2. Bedrock ConverseStream (rewrite of follow-up questions), model {}: {} calls{}.\n",
                        plan.model_id,
                        plan.rewrite_ids.len(),
                        if plan.rewrite_ids.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", plan.rewrite_ids.join(", "))
                        }
                    ));
                    out.push_str(&format!(
                        "  Bound for this corpus: ${:.4}.\n",
                        plan.cost_upper_bound_usd()
                    ));
                }
            }
        }
        out.push_str(&format!(
            "\nTotal: at most ${:.4} at list prices (an upper bound).\n",
            self.cost_upper_bound_usd()
        ));
        out.push_str("It does not touch S3, DynamoDB, SQS, or S3 Vectors. It writes only to the cache directories of the corpora in evals/corpora/.\n");
        out
    }
}

/// Makes the calls of the plan. It returns the finished cache of each corpus, in order. The caller
/// saves them.
pub async fn run_refresh(refresh: Refresh) -> Result<Vec<(Corpus, Cache)>, Error> {
    let mut done = Vec::new();
    for step in refresh.steps {
        eprintln!("== Corpus {}", step.corpus.id);
        let cache = match step.action {
            Action::Full { plan, pages } => build_live(&step.corpus, &plan, pages).await?,
            Action::Missing { plan, cache } => run_missing(&step.corpus, &plan, *cache).await?,
        };
        done.push((step.corpus, cache));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbook_search::chunker::MAX_CHARS;

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&sorted, 0.50), 50);
        assert_eq!(percentile(&sorted, 0.95), 95);
        assert_eq!(percentile(&[7], 0.95), 7);
        assert_eq!(percentile(&[], 0.5), 0);
    }

    #[test]
    fn the_plan_names_every_call_and_the_cost() {
        let plan = Plan {
            model_id: "m".into(),
            embedding_model: "e".into(),
            region: "r".into(),
            profile: "p".into(),
            transcribe_pages: vec![3, 4],
            embed_calls: 100,
            embed_chars: 100_000,
            rewrite_calls: 18,
        };
        let text = plan.describe();
        assert!(text.contains("REAL AWS"));
        assert!(text.contains("Bedrock Converse (page transcription), model m: 2 calls"));
        assert!(text.contains("Bedrock InvokeModel (embeddings), model e: about 100 calls"));
        assert!(
            text.contains("ConverseStream (rewrite of follow-up questions), model m: 18 calls")
        );
        assert!(text.contains("Total: about $"));
        assert!(text.contains("evals/cache/"));
        let expected = 2.0 * TRANSCRIBE_USD_PER_PAGE
            + 100.0 * EMBED_USD_PER_CALL
            + 18.0 * REWRITE_USD_PER_CALL;
        assert!((plan.cost_usd() - expected).abs() < 1e-12);
    }

    #[test]
    fn the_texts_to_embed_are_the_chunks_then_the_queries_without_repeats() {
        let sections = vec![Section {
            id: "s".into(),
            title: "Title".into(),
            first_page: 1,
            last_page: 1,
        }];
        let pages = vec![(1, "# Heading\n\nSome text about quadratics.".to_owned())];
        let queries = vec!["q one".to_owned(), "q one".to_owned(), "q two".to_owned()];
        let texts = texts_to_embed(&sections, &pages, &queries, &[400]).unwrap();
        assert_eq!(texts.len(), 3);
        assert!(texts[0].starts_with("Title > Heading\n"), "{}", texts[0]);
        assert_eq!(&texts[1..], ["q one", "q two"]);
        assert!(texts.iter().all(|t| t.chars().count() <= MAX_CHARS + 200));
    }

    #[tokio::test]
    async fn a_transient_error_is_retried_and_a_permanent_one_is_not() {
        let calls = std::sync::atomic::AtomicU32::new(0);
        tokio::time::pause();
        let result: Result<u32, String> = retry(
            "t",
            || async {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < 2 { Err("busy".to_owned()) } else { Ok(n) }
            },
            |e: &String| e == "busy",
        )
        .await;
        assert_eq!(result, Ok(2));
        let calls = std::sync::atomic::AtomicU32::new(0);
        let result: Result<u32, String> = retry(
            "t",
            || async {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err("bad".to_owned())
            },
            |e: &String| e == "busy",
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn the_missing_plan_names_the_calls_and_bounds_the_cost() {
        let plan = MissingPlan {
            model_id: "m".into(),
            embedding_model: "e".into(),
            region: "r".into(),
            profile: "p".into(),
            embed_texts: vec!["a".repeat(2000), "b".repeat(100)],
            rewrite_ids: vec!["followup-11".into()],
        };
        assert_eq!(plan.embed_calls(), 3);
        let text = plan.describe();
        assert!(text.contains("REAL AWS"));
        assert!(text.contains("NO page transcription"));
        assert!(text.contains("model e: 2 calls"));
        assert!(text.contains("model m: 1 calls (followup-11)"));
        assert!(text.contains("at most $"));
        assert!(!text.contains("page transcription), model"));
        assert!(plan.cost_upper_bound_usd() > 0.0);
        // Bigger work gives a bigger bound, and the bound is far below a dollar for this work.
        let more = MissingPlan {
            rewrite_ids: vec![],
            embed_texts: vec!["a".repeat(2000)],
            ..plan.clone()
        };
        assert!(more.cost_upper_bound_usd() < plan.cost_upper_bound_usd());
        assert!(plan.cost_upper_bound_usd() < 0.01);
        assert!(!plan.is_empty());
    }

    fn cache_stub() -> Cache {
        Cache {
            meta: Meta {
                format: FORMAT,
                kind: CacheKind::Live,
                corpus: "c.pdf".into(),
                corpus_sha256: "00".into(),
                embedding_model: "e".into(),
                transcribe_model: Some("m".into()),
                rewrite_model: "m".into(),
                pages: 1,
                transcribed_pages: 0,
                fallback_pages: vec![],
                latency: None,
            },
            pages: vec![(1, "Text".into())],
            embeddings: EmbeddingStore::new("e", 8),
            rewrites: Default::default(),
        }
    }

    fn corpus(id: &str) -> Corpus {
        Corpus {
            id: id.into(),
            dir: PathBuf::from("/nowhere").join(id),
        }
    }

    #[test]
    fn one_plan_covers_a_new_corpus_and_a_corpus_with_a_cache() {
        let full = Plan {
            model_id: "m".into(),
            embedding_model: "e".into(),
            region: "r".into(),
            profile: "p".into(),
            transcribe_pages: vec![5, 6],
            embed_calls: 300,
            embed_chars: 300_000,
            rewrite_calls: 11,
        };
        let missing = MissingPlan {
            model_id: "m".into(),
            embedding_model: "e".into(),
            region: "r".into(),
            profile: "p".into(),
            embed_texts: vec!["a".repeat(1000)],
            rewrite_ids: vec![],
        };
        let refresh = Refresh {
            region: "ap-southeast-2".into(),
            profile: "p".into(),
            steps: vec![
                Step {
                    corpus: corpus("history"),
                    action: Action::Full {
                        plan: full.clone(),
                        pages: vec![],
                    },
                },
                Step {
                    corpus: corpus("algebra"),
                    action: Action::Missing {
                        plan: missing.clone(),
                        cache: Box::new(cache_stub()),
                    },
                },
            ],
        };
        let text = refresh.describe();
        assert!(text.contains("AWS Region: ap-southeast-2"));
        assert!(text.contains("Corpus history: no cache yet, so this is the full live refresh"));
        assert!(text.contains("page transcription), model m: 2 calls"));
        assert!(text.contains("Corpus algebra: the cache has the pages. No page transcription."));
        assert!(text.contains("Total: at most $"));
        assert!(!refresh.is_empty());
        let total = full.cost_upper_bound_usd() + missing.cost_upper_bound_usd();
        assert!((refresh.cost_upper_bound_usd() - total).abs() < 1e-12);
        // Every page that is transcribed costs at most a few cents, and the whole plan stays small.
        assert!(full.cost_upper_bound_usd() > 2.0 * 0.02);
        assert!(refresh.cost_upper_bound_usd() < 1.0);

        // A plan with only complete caches has nothing to call.
        let done = Refresh {
            region: "r".into(),
            profile: "p".into(),
            steps: vec![Step {
                corpus: corpus("algebra"),
                action: Action::Missing {
                    plan: MissingPlan {
                        embed_texts: vec![],
                        ..missing
                    },
                    cache: Box::new(cache_stub()),
                },
            }],
        };
        assert!(done.is_empty());
    }

    #[test]
    fn the_grid_has_the_chunk_target_of_production() {
        // The cache must hold the chunk embeddings of the production target, or the normal run fails.
        assert!(CHUNK_TARGET_TOKENS.contains(&PRODUCTION_CHUNK_TOKENS));
        assert_eq!(PRODUCTION_CHUNK_TOKENS, 400);
    }

    #[test]
    fn a_failed_rewrite_is_an_error_that_names_the_question() {
        let error = rewrite_failed("followup-11").to_string();
        assert!(
            error.contains("followup-11") && error.contains("gave no query"),
            "{error}"
        );
    }
}
