//! The course BM25 index (Tantivy).
//!
//! One index per course. It lives in S3 as one `tar.zst` blob per version,
//! `courses/<courseId>/bm25/v<version>.tar.zst`. The course `META` item holds `bm25Version`.
//! The ingest worker is the only writer. The API (phase 3) reads a given version through `Bm25Cache`.

use std::path::{Path, PathBuf};

use aws_sdk_s3::Client as S3Client;
use aws_smithy_types::error::display::DisplayErrorContext;
use openbook_core::store::Store;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{
    Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, TextAnalyzer,
};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, Term, doc};

const TOKENIZER: &str = "en_stem";
const WRITER_MEMORY: usize = 30_000_000;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Bm25Error {
    /// A problem that a retry may fix: S3 or DynamoDB failed, or another writer moved the version.
    #[error("BM25 index error (transient): {0}")]
    Transient(String),
    /// The index files are broken or cannot be written.
    #[error("BM25 index error: {0}")]
    Index(String),
}

fn index_err(error: impl std::fmt::Display) -> Bm25Error {
    Bm25Error::Index(error.to_string())
}

fn s3_err(error: &dyn std::error::Error) -> Bm25Error {
    Bm25Error::Transient(DisplayErrorContext(error).to_string())
}

/// The S3 key of one version.
pub fn blob_key(course_id: &str, version: u64) -> String {
    format!("courses/{course_id}/bm25/v{version}.tar.zst")
}

/// The version in a blob key, or `None` when the key is not a blob.
pub fn version_of_key(key: &str) -> Option<u64> {
    key.rsplit('/')
        .next()?
        .strip_prefix('v')?
        .strip_suffix(".tar.zst")?
        .parse()
        .ok()
}

/// One chunk in the index. `chunk_id` equals the vector key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bm25Chunk {
    pub chunk_id: String,
    pub document_id: String,
    pub page: u64,
    pub heading: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bm25Hit {
    pub chunk_id: String,
    pub document_id: String,
    pub page: u64,
    pub heading: String,
    pub text: String,
    pub score: f32,
}

#[derive(Clone, Copy)]
struct Fields {
    chunk_id: Field,
    document_id: Field,
    page: Field,
    heading: Field,
    text: Field,
}

fn schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let analyzed = || {
        TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored()
    };
    let fields = Fields {
        chunk_id: builder.add_text_field("chunkId", STRING | STORED),
        document_id: builder.add_text_field("documentId", STRING | STORED),
        page: builder.add_u64_field("page", STORED),
        heading: builder.add_text_field("heading", analyzed()),
        text: builder.add_text_field("text", analyzed()),
    };
    (builder.build(), fields)
}

fn register_tokenizer(index: &Index) {
    let analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(40))
        .filter(LowerCaser)
        .filter(Stemmer::new(Language::English))
        .build();
    index.tokenizers().register(TOKENIZER, analyzer);
}

fn fields_of(index: &Index) -> Result<Fields, Bm25Error> {
    let schema = index.schema();
    let get = |name: &str| schema.get_field(name).map_err(index_err);
    Ok(Fields {
        chunk_id: get("chunkId")?,
        document_id: get("documentId")?,
        page: get("page")?,
        heading: get("heading")?,
        text: get("text")?,
    })
}

/// Writes to a local index directory.
pub struct Bm25Writer {
    writer: IndexWriter,
    fields: Fields,
}

impl Bm25Writer {
    /// Creates an empty index in `dir` (the directory must exist and be empty).
    pub fn create(dir: &Path) -> Result<Self, Bm25Error> {
        let (schema, _) = schema();
        let index = Index::create_in_dir(dir, schema).map_err(index_err)?;
        Self::from_index(index)
    }

    /// Opens the index in `dir`.
    pub fn open(dir: &Path) -> Result<Self, Bm25Error> {
        let index = Index::open_in_dir(dir).map_err(index_err)?;
        Self::from_index(index)
    }

    fn from_index(index: Index) -> Result<Self, Bm25Error> {
        register_tokenizer(&index);
        let fields = fields_of(&index)?;
        let writer = index
            .writer_with_num_threads(1, WRITER_MEMORY)
            .map_err(index_err)?;
        Ok(Self { writer, fields })
    }

    /// Removes every chunk of the document (a re-ingest or a delete). Applies at the next commit.
    pub fn delete_document(&mut self, document_id: &str) {
        self.writer
            .delete_term(Term::from_field_text(self.fields.document_id, document_id));
    }

    pub fn add_chunk(&mut self, chunk: &Bm25Chunk) -> Result<(), Bm25Error> {
        let f = self.fields;
        self.writer
            .add_document(doc!(
                f.chunk_id => chunk.chunk_id.as_str(),
                f.document_id => chunk.document_id.as_str(),
                f.page => chunk.page,
                f.heading => chunk.heading.as_str(),
                f.text => chunk.text.as_str(),
            ))
            .map_err(index_err)?;
        Ok(())
    }

    /// Commits and waits for the merge threads, so the directory can be packed.
    pub fn commit(mut self) -> Result<(), Bm25Error> {
        self.writer.commit().map_err(index_err)?;
        self.writer.wait_merging_threads().map_err(index_err)?;
        Ok(())
    }
}

/// Reads a local index directory.
pub struct Bm25Reader {
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

impl Bm25Reader {
    pub fn open(dir: &Path) -> Result<Self, Bm25Error> {
        let index = Index::open_in_dir(dir).map_err(index_err)?;
        register_tokenizer(&index);
        let fields = fields_of(&index)?;
        let reader = index.reader().map_err(index_err)?;
        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    /// The number of chunks in the index.
    pub fn chunk_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// The best `limit` chunks for a query, best first. The query is free text: bad syntax is handled.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Bm25Hit>, Bm25Error> {
        let parser =
            QueryParser::for_index(&self.index, vec![self.fields.text, self.fields.heading]);
        let (parsed, _errors) = parser.parse_query_lenient(query);
        let searcher = self.reader.searcher();
        let top = searcher
            .search(&parsed, &TopDocs::with_limit(limit.max(1)).order_by_score())
            .map_err(index_err)?;
        let mut hits = Vec::with_capacity(top.len());
        for (score, address) in top {
            let document: TantivyDocument = searcher.doc(address).map_err(index_err)?;
            let text_of = |field: Field| -> String {
                document
                    .get_first(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            hits.push(Bm25Hit {
                chunk_id: text_of(self.fields.chunk_id),
                document_id: text_of(self.fields.document_id),
                page: document
                    .get_first(self.fields.page)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                heading: text_of(self.fields.heading),
                text: text_of(self.fields.text),
                score,
            });
        }
        Ok(hits)
    }
}

/// Packs an index directory into one `tar.zst` blob.
pub fn pack(dir: &Path) -> Result<Vec<u8>, Bm25Error> {
    let encoder = zstd::Encoder::new(Vec::new(), 3).map_err(index_err)?;
    let mut archive = tar::Builder::new(encoder);
    for entry in std::fs::read_dir(dir).map_err(index_err)? {
        let entry = entry.map_err(index_err)?;
        let name = entry.file_name();
        // Lock files are local state.
        if name.to_string_lossy().ends_with(".lock") {
            continue;
        }
        if entry.file_type().map_err(index_err)?.is_file() {
            archive
                .append_path_with_name(entry.path(), &name)
                .map_err(index_err)?;
        }
    }
    let encoder = archive.into_inner().map_err(index_err)?;
    encoder.finish().map_err(index_err)
}

/// Unpacks a blob into `dir` (created if needed).
pub fn unpack(blob: &[u8], dir: &Path) -> Result<(), Bm25Error> {
    std::fs::create_dir_all(dir).map_err(index_err)?;
    let decoder = zstd::Decoder::new(blob).map_err(index_err)?;
    tar::Archive::new(decoder).unpack(dir).map_err(index_err)
}

/// The writer's view of the S3 blobs and the version in DynamoDB.
pub struct Bm25Store {
    s3: S3Client,
    bucket: String,
    store: Store,
}

impl Bm25Store {
    pub fn new(s3: S3Client, bucket: String, store: Store) -> Self {
        Self { s3, bucket, store }
    }

    async fn download(&self, key: &str) -> Result<Vec<u8>, Bm25Error> {
        let out = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| s3_err(&e))?;
        let bytes = out.body.collect().await.map_err(|e| s3_err(&e))?;
        Ok(bytes.into_bytes().to_vec())
    }

    /// Removes the given documents from the course index and adds the given chunks.
    ///
    /// It downloads the current version into `work_dir`, changes it, uploads the next version,
    /// moves `bm25Version` forward with a conditional update on the old value, and deletes blobs
    /// older than the previous version. Returns the new version.
    /// `work_dir` should be empty or missing: it is removed first.
    pub async fn update(
        &self,
        course_id: &str,
        work_dir: &Path,
        remove_documents: &[String],
        add: Vec<Bm25Chunk>,
    ) -> Result<u64, Bm25Error> {
        let course = self
            .store
            .get_course(course_id)
            .await
            .map_err(|e| Bm25Error::Transient(e.to_string()))?
            .ok_or_else(|| Bm25Error::Index("the course no longer exists".into()))?;
        let old = course.bm25_version;
        let dir = work_dir.join("index");
        let _ = std::fs::remove_dir_all(work_dir);
        std::fs::create_dir_all(&dir).map_err(index_err)?;

        let existing = match old {
            Some(version) => Some(self.download(&blob_key(course_id, version)).await?),
            None => None,
        };
        let remove = remove_documents.to_vec();
        let work = dir.clone();
        let blob = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, Bm25Error> {
            let mut writer = match existing {
                Some(blob) => {
                    unpack(&blob, &work)?;
                    Bm25Writer::open(&work)?
                }
                None => Bm25Writer::create(&work)?,
            };
            for document_id in &remove {
                writer.delete_document(document_id);
            }
            for chunk in &add {
                writer.add_chunk(chunk)?;
            }
            writer.commit()?;
            pack(&work)
        })
        .await
        .map_err(index_err)??;

        let new = old.unwrap_or(0) + 1;
        self.s3
            .put_object()
            .bucket(&self.bucket)
            .key(blob_key(course_id, new))
            .body(blob.into())
            .send()
            .await
            .map_err(|e| s3_err(&e))?;
        let moved = self
            .store
            .set_bm25_version(course_id, old, new)
            .await
            .map_err(|e| Bm25Error::Transient(e.to_string()))?;
        let _ = std::fs::remove_dir_all(work_dir);
        if !moved {
            // Another writer moved the version first (it cannot happen with one writer per course).
            return Err(Bm25Error::Transient(
                "the BM25 version changed during the update".into(),
            ));
        }
        self.delete_old_blobs(course_id, old).await;
        Ok(new)
    }

    /// Deletes blobs older than the previous version. Best effort.
    async fn delete_old_blobs(&self, course_id: &str, previous: Option<u64>) {
        let Some(previous) = previous else { return };
        let prefix = format!("courses/{course_id}/bm25/");
        let listed = match self
            .s3
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&prefix)
            .send()
            .await
        {
            Ok(listed) => listed,
            Err(error) => {
                tracing::warn!(error = %DisplayErrorContext(&error), "could not list old BM25 blobs");
                return;
            }
        };
        for object in listed.contents() {
            let Some(key) = object.key() else { continue };
            if !version_of_key(key).is_some_and(|v| v < previous) {
                continue;
            }
            if let Err(error) = self
                .s3
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
            {
                tracing::warn!(error = %DisplayErrorContext(&error), key, "could not delete an old BM25 blob");
            }
        }
    }
}

/// Opens a given version of a course index with a local cache (for warm Lambda invocations).
pub struct Bm25Cache {
    s3: S3Client,
    bucket: String,
    root: PathBuf,
}

impl Bm25Cache {
    pub fn new(s3: S3Client, bucket: String, root: PathBuf) -> Self {
        Self { s3, bucket, root }
    }

    /// The reader for `version` of the course index. It downloads and unpacks on a cache miss, and
    /// removes the older cached versions of the course.
    pub async fn open(&self, course_id: &str, version: u64) -> Result<Bm25Reader, Bm25Error> {
        let course_dir = self.root.join(course_id);
        let dir = course_dir.join(format!("v{version}"));
        let marker = dir.join(".complete");
        if !marker.exists() {
            let out = self
                .s3
                .get_object()
                .bucket(&self.bucket)
                .key(blob_key(course_id, version))
                .send()
                .await
                .map_err(|e| s3_err(&e))?;
            let bytes = out
                .body
                .collect()
                .await
                .map_err(|e| s3_err(&e))?
                .into_bytes()
                .to_vec();
            let staging = course_dir.join(format!("v{version}.tmp"));
            let _ = std::fs::remove_dir_all(&staging);
            let final_dir = dir.clone();
            tokio::task::spawn_blocking(move || -> Result<(), Bm25Error> {
                unpack(&bytes, &staging)?;
                let _ = std::fs::remove_dir_all(&final_dir);
                std::fs::rename(&staging, &final_dir).map_err(index_err)?;
                std::fs::write(final_dir.join(".complete"), b"ok").map_err(index_err)
            })
            .await
            .map_err(index_err)??;
        }
        if let Ok(entries) = std::fs::read_dir(&course_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name != format!("v{version}") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let open_dir = dir.clone();
        tokio::task::spawn_blocking(move || Bm25Reader::open(&open_dir))
            .await
            .map_err(index_err)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(doc: &str, n: u32, page: u64, heading: &str, text: &str) -> Bm25Chunk {
        Bm25Chunk {
            chunk_id: format!("{doc}:{n}"),
            document_id: doc.into(),
            page,
            heading: heading.into(),
            text: text.into(),
        }
    }

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("openbook-bm25-{}", openbook_core::ids::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build(dir: &Path, chunks: &[Bm25Chunk]) {
        let mut writer = Bm25Writer::create(dir).unwrap();
        for c in chunks {
            writer.add_chunk(c).unwrap();
        }
        writer.commit().unwrap();
    }

    #[test]
    fn keys_and_versions_round_trip() {
        assert_eq!(blob_key("C1", 3), "courses/C1/bm25/v3.tar.zst");
        assert_eq!(version_of_key("courses/C1/bm25/v12.tar.zst"), Some(12));
        assert_eq!(version_of_key("courses/C1/bm25/readme.txt"), None);
        assert_eq!(version_of_key("courses/C1/bm25/vx.tar.zst"), None);
    }

    #[test]
    fn search_finds_the_matching_chunk_first() {
        let dir = temp_dir();
        build(
            &dir,
            &[
                chunk(
                    "D1",
                    0,
                    1,
                    "Geometry",
                    "The Pythagorean theorem relates the sides of a right triangle.",
                ),
                chunk(
                    "D1",
                    1,
                    2,
                    "Biology",
                    "Photosynthesis converts light into chemical energy.",
                ),
                chunk("D2", 0, 7, "Algebra", "Quadratic equations have two roots."),
            ],
        );
        let reader = Bm25Reader::open(&dir).unwrap();
        assert_eq!(reader.chunk_count(), 3);
        let hits = reader.search("pythagorean triangle", 5).unwrap();
        assert_eq!(hits[0].chunk_id, "D1:0");
        assert_eq!(hits[0].document_id, "D1");
        assert_eq!(hits[0].page, 1);
        assert_eq!(hits[0].heading, "Geometry");
        assert!(hits[0].text.contains("Pythagorean"));
        assert!(hits[0].score > 0.0);
        assert_eq!(
            reader.search("photosynthesis", 5).unwrap()[0].chunk_id,
            "D1:1"
        );
        assert_eq!(reader.search("quadratic", 1).unwrap()[0].page, 7);
        assert!(reader.search("zebra", 5).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_english_stemmer_matches_word_forms() {
        let dir = temp_dir();
        build(
            &dir,
            &[chunk(
                "D",
                0,
                1,
                "",
                "Solving equations by factoring polynomials",
            )],
        );
        let reader = Bm25Reader::open(&dir).unwrap();
        for query in ["solve equation", "factor polynomial", "SOLVING"] {
            assert_eq!(reader.search(query, 3).unwrap().len(), 1, "{query}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn headings_are_searched_too() {
        let dir = temp_dir();
        build(
            &dir,
            &[chunk(
                "D",
                0,
                1,
                "Thermodynamics",
                "Heat flows from hot to cold.",
            )],
        );
        let reader = Bm25Reader::open(&dir).unwrap();
        assert_eq!(reader.search("thermodynamics", 3).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bad_query_syntax_does_not_fail() {
        let dir = temp_dir();
        build(&dir, &[chunk("D", 0, 1, "", "plain text about limits")]);
        let reader = Bm25Reader::open(&dir).unwrap();
        for query in ["limits AND (", "\"unclosed", "text:::", "", "   ", "+-*"] {
            assert!(reader.search(query, 3).is_ok(), "{query:?}");
        }
        assert_eq!(reader.search("limits AND (", 3).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_a_document_removes_its_terms_and_re_ingest_replaces_them() {
        let dir = temp_dir();
        build(
            &dir,
            &[
                chunk("D1", 0, 1, "", "alpha beta gamma"),
                chunk("D2", 0, 1, "", "alpha delta"),
            ],
        );
        let mut writer = Bm25Writer::open(&dir).unwrap();
        writer.delete_document("D1");
        writer
            .add_chunk(&chunk("D1", 0, 3, "", "epsilon zeta"))
            .unwrap();
        writer.commit().unwrap();
        let reader = Bm25Reader::open(&dir).unwrap();
        assert!(
            reader.search("beta", 5).unwrap().is_empty(),
            "old text is gone"
        );
        assert_eq!(reader.search("epsilon", 5).unwrap()[0].page, 3);
        assert_eq!(
            reader.search("alpha", 5).unwrap().len(),
            1,
            "only D2 still has alpha"
        );
        assert_eq!(reader.chunk_count(), 2);

        let mut writer = Bm25Writer::open(&dir).unwrap();
        writer.delete_document("D2");
        writer.commit().unwrap();
        let reader = Bm25Reader::open(&dir).unwrap();
        assert!(reader.search("delta", 5).unwrap().is_empty());
        assert_eq!(reader.chunk_count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_packed_blob_round_trips() {
        let dir = temp_dir();
        build(
            &dir,
            &[chunk("D", 0, 4, "H", "round trip through a tar zst blob")],
        );
        let blob = pack(&dir).unwrap();
        assert!(blob.len() > 100);
        // The blob has the zstd magic number.
        assert_eq!(&blob[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        let target = temp_dir().join("unpacked");
        unpack(&blob, &target).unwrap();
        let reader = Bm25Reader::open(&target).unwrap();
        let hits = reader.search("tar blob", 3).unwrap();
        assert_eq!((hits[0].chunk_id.as_str(), hits[0].page), ("D:0", 4));
        // Unpacked indexes can be written to and packed again.
        let mut writer = Bm25Writer::open(&target).unwrap();
        writer
            .add_chunk(&chunk("E", 0, 1, "", "second generation"))
            .unwrap();
        writer.commit().unwrap();
        let again = pack(&target).unwrap();
        let third = temp_dir().join("third");
        unpack(&again, &third).unwrap();
        assert_eq!(Bm25Reader::open(&third).unwrap().chunk_count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn packing_skips_lock_files() {
        let dir = temp_dir();
        build(&dir, &[chunk("D", 0, 1, "", "text")]);
        std::fs::write(dir.join("somewhere.lock"), b"x").unwrap();
        let target = temp_dir().join("out");
        unpack(&pack(&dir).unwrap(), &target).unwrap();
        assert!(!target.join("somewhere.lock").exists());
        assert!(target.join("meta.json").exists());
    }
}
