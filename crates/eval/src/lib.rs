//! Openbook retrieval evaluation (`docs/evaluation.md`).
//!
//! The offline run chunks the cached pages with the real chunker, builds the real BM25 index and an
//! in-memory dense index, and runs the shared retrieval core of `openbook-search` for every golden
//! question. The refresh builds the cache that makes the offline run deterministic.

pub mod cache;
pub mod data;
pub mod grid;
pub mod metrics;
pub mod offline;
pub mod refresh;
pub mod report;

use std::path::{Path, PathBuf};

/// The `evals` directory of this repository.
pub fn default_evals_dir() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals");
    std::fs::canonicalize(&path).unwrap_or(path)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(String),
    /// The corpus, sections, or golden set break a rule.
    #[error("data problem: {0}")]
    Data(String),
    /// The cache is missing, broken, or does not hold something that the run needs.
    #[error("cache problem: {0}")]
    Cache(String),
    #[error("{0}")]
    Run(String),
    #[error("{0}")]
    Aws(String),
}

impl Error {
    pub fn io(path: &Path, error: &std::io::Error) -> Self {
        Error::Io(format!("{}: {error}", path.display()))
    }
}
