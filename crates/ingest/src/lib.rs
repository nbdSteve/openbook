//! Openbook ingest worker: PDF pages to Markdown, chunks, embeddings, vectors, and the BM25 index.
//!
//! The same handler runs in Lambda (an SQS event) and locally (`openbook-ingest --poll`).

pub mod pdf;
pub mod pipeline;
pub mod transcribe;
pub mod worker;
