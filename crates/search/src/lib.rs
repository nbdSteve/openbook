//! Openbook search: the Markdown chunker, embeddings, the S3 Vectors store, the page-quality
//! heuristic, and the course BM25 index.

pub mod chunker;
pub mod embed;
pub mod quality;
pub mod vectors;

#[cfg(feature = "bm25")]
pub mod bm25;
