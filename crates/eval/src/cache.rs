//! The cache that makes the offline run deterministic.
//!
//! ```text
//! <cache>/meta.json            what made the cache (kind, corpus hash, models)
//! <cache>/pages/001.md ...     the page Markdown that ingestion produced for the corpus
//! <cache>/embeddings.bin.zst   the embeddings of every chunk and every query
//! <cache>/rewrites.json        the search query of every follow-up question
//! ```
//!
//! An embedding is keyed by the embedding model and the SHA-256 of the exact text that was
//! embedded. It is stored as a 16-bit float, and the file is compressed with zstd. The entries are
//! sorted, so the same cache always gives the same bytes and a refresh gives a reviewable diff.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Error;

pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 8] = b"OBEMB1\n\0";

/// Who made the cache. A `dev` cache comes from the PDF text layer and fake embeddings. It proves
/// that the harness works, and it never gives a baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheKind {
    Live,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Latency {
    /// Time of one Titan query embedding call at refresh time.
    pub query_embedding_ms_p50: u64,
    pub query_embedding_ms_p95: u64,
    pub samples: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Meta {
    pub format: u32,
    pub kind: CacheKind,
    /// The corpus file name and the SHA-256 of its bytes.
    pub corpus: String,
    pub corpus_sha256: String,
    pub embedding_model: String,
    /// The model that transcribed the pages. `None` for a dev cache.
    pub transcribe_model: Option<String>,
    /// The model that rewrote the follow-up questions, or `dev-concat` for a dev cache.
    pub rewrite_model: String,
    pub pages: u32,
    /// Pages that a model transcribed.
    pub transcribed_pages: u32,
    /// Pages that needed transcription and fell back to the text layer.
    pub fallback_pages: Vec<u32>,
    pub latency: Option<Latency>,
}

pub fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&sha256_bytes(data))
}

pub fn corpus_sha256(path: &Path) -> Result<String, Error> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, &e))?;
    Ok(sha256_hex(&bytes))
}

// ---- 16-bit floats

pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x007f_ffff;
    if exp == 0xff {
        // Infinity or not a number.
        return sign | 0x7c00 | if mant != 0 { 0x0200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x0080_0000;
        let shift = (14 - e) as u32;
        let mut half = m >> shift;
        let rest = m & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        if rest > halfway || (rest == halfway && half & 1 == 1) {
            half += 1;
        }
        return sign | half as u16;
    }
    let mut half = ((e as u32) << 10) | (mant >> 13);
    let rest = mant & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && half & 1 == 1) {
        // A carry into the exponent is right: it rounds up to the next power of two.
        half += 1;
    }
    sign | half as u16
}

pub fn f16_to_f32(half: u16) -> f32 {
    let sign = u32::from(half & 0x8000) << 16;
    let exp = u32::from((half >> 10) & 0x1f);
    let frac = u32::from(half & 0x3ff);
    let bits = if exp == 0 {
        if frac == 0 {
            sign
        } else {
            let mut e = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((f & 0x3ff) << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (frac << 13)
    } else {
        sign | ((exp + 112) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

// ---- Embeddings

/// Embeddings by text, for one embedding model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingStore {
    model: String,
    dimensions: usize,
    entries: BTreeMap<[u8; 32], Vec<u16>>,
}

impl EmbeddingStore {
    pub fn new(model: &str, dimensions: usize) -> Self {
        Self {
            model: model.to_owned(),
            dimensions,
            entries: BTreeMap::new(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The key of a text: the SHA-256 of the model, a zero byte, and the exact text.
    pub fn key(&self, text: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.model.as_bytes());
        hasher.update([0u8]);
        hasher.update(text.as_bytes());
        hasher.finalize().into()
    }

    pub fn contains(&self, text: &str) -> bool {
        self.entries.contains_key(&self.key(text))
    }

    pub fn insert(&mut self, text: &str, vector: &[f32]) -> Result<(), Error> {
        if vector.len() != self.dimensions {
            return Err(Error::Cache(format!(
                "an embedding has {} values and the cache holds {}",
                vector.len(),
                self.dimensions
            )));
        }
        let key = self.key(text);
        self.entries
            .insert(key, vector.iter().map(|v| f32_to_f16(*v)).collect());
        Ok(())
    }

    /// The embedding of a text. A missing entry is an error: the run never skips a question.
    pub fn get(&self, text: &str) -> Result<Vec<f32>, Error> {
        let key = self.key(text);
        let Some(stored) = self.entries.get(&key) else {
            let shown: String = text.chars().take(60).collect();
            return Err(Error::Cache(format!(
                "no embedding for {shown:?} (model {}, key {}). Refresh the cache.",
                self.model,
                &hex(&key)[..12]
            )));
        };
        Ok(stored.iter().map(|h| f16_to_f32(*h)).collect())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut raw = Vec::with_capacity(32 + self.entries.len() * (32 + self.dimensions * 2));
        raw.extend_from_slice(MAGIC);
        let model = self.model.as_bytes();
        let model_len = u16::try_from(model.len())
            .map_err(|_| Error::Cache("the model name is too long".into()))?;
        raw.extend_from_slice(&model_len.to_le_bytes());
        raw.extend_from_slice(model);
        raw.extend_from_slice(&(self.dimensions as u32).to_le_bytes());
        raw.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (key, vector) in &self.entries {
            raw.extend_from_slice(key);
            for value in vector {
                raw.extend_from_slice(&value.to_le_bytes());
            }
        }
        zstd::encode_all(raw.as_slice(), 19).map_err(|e| Error::Cache(e.to_string()))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let raw = zstd::decode_all(bytes).map_err(|e| Error::Cache(format!("embeddings: {e}")))?;
        let mut reader = Reader { data: &raw, at: 0 };
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(Error::Cache(
                "embeddings: the file has the wrong header".into(),
            ));
        }
        let model_len = usize::from(u16::from_le_bytes(reader.array()?));
        let model = String::from_utf8(reader.take(model_len)?.to_vec())
            .map_err(|_| Error::Cache("embeddings: the model name is not text".into()))?;
        let dimensions = u32::from_le_bytes(reader.array()?) as usize;
        let count = u32::from_le_bytes(reader.array()?) as usize;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let key: [u8; 32] = reader.array()?;
            let body = reader.take(dimensions * 2)?;
            let vector = body
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            entries.insert(key, vector);
        }
        if reader.at != raw.len() {
            return Err(Error::Cache("embeddings: the file has extra bytes".into()));
        }
        Ok(Self {
            model,
            dimensions,
            entries,
        })
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| Error::Cache("embeddings: the file is cut short".into()))?;
        let part = &self.data[self.at..end];
        self.at = end;
        Ok(part)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let part = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(part);
        Ok(out)
    }
}

// ---- Rewrites

/// The key of a follow-up rewrite: the rewrite model, the earlier question, and the question.
pub fn rewrite_key(model: &str, context: &str, question: &str) -> String {
    let mut hasher = Sha256::new();
    for part in [model, context, question] {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hex(&hasher.finalize())
}

// ---- The whole cache

#[derive(Debug, Clone)]
pub struct Cache {
    pub meta: Meta,
    /// Page Markdown by excerpt page number (1-based), in order.
    pub pages: Vec<(u32, String)>,
    pub embeddings: EmbeddingStore,
    /// Search queries of follow-up questions, by `rewrite_key`.
    pub rewrites: BTreeMap<String, String>,
}

fn page_file(dir: &Path, number: u32) -> PathBuf {
    dir.join("pages").join(format!("{number:03}.md"))
}

impl Cache {
    pub fn load(dir: &Path) -> Result<Cache, Error> {
        let meta_path = dir.join("meta.json");
        if !meta_path.exists() {
            return Err(Error::Cache(format!(
                "there is no cache at {}. Run `openbook-eval refresh` (live) or `openbook-eval refresh --dev-cache`.",
                dir.display()
            )));
        }
        let text = std::fs::read_to_string(&meta_path).map_err(|e| Error::io(&meta_path, &e))?;
        let meta: Meta = serde_json::from_str(&text)
            .map_err(|e| Error::Cache(format!("{}: {e}", meta_path.display())))?;
        if meta.format != FORMAT {
            return Err(Error::Cache(format!(
                "the cache has format {} and this build reads format {FORMAT}",
                meta.format
            )));
        }
        let mut pages = Vec::new();
        for number in 1..=meta.pages {
            let path = page_file(dir, number);
            let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
            pages.push((number, text));
        }
        let path = dir.join("embeddings.bin.zst");
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, &e))?;
        let embeddings = EmbeddingStore::from_bytes(&bytes)?;
        if embeddings.model() != meta.embedding_model {
            return Err(Error::Cache(format!(
                "the embeddings are from {:?} and meta.json says {:?}",
                embeddings.model(),
                meta.embedding_model
            )));
        }
        let path = dir.join("rewrites.json");
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
        let rewrites: BTreeMap<String, String> = serde_json::from_str(&text)
            .map_err(|e| Error::Cache(format!("{}: {e}", path.display())))?;
        Ok(Cache {
            meta,
            pages,
            embeddings,
            rewrites,
        })
    }

    pub fn save(&self, dir: &Path) -> Result<(), Error> {
        let pages_dir = dir.join("pages");
        if pages_dir.exists() {
            std::fs::remove_dir_all(&pages_dir).map_err(|e| Error::io(&pages_dir, &e))?;
        }
        std::fs::create_dir_all(&pages_dir).map_err(|e| Error::io(&pages_dir, &e))?;
        for (number, text) in &self.pages {
            let path = page_file(dir, *number);
            std::fs::write(&path, text).map_err(|e| Error::io(&path, &e))?;
        }
        let write = |name: &str, bytes: Vec<u8>| -> Result<(), Error> {
            let path = dir.join(name);
            std::fs::write(&path, bytes).map_err(|e| Error::io(&path, &e))
        };
        write("meta.json", pretty(&self.meta)?)?;
        write("rewrites.json", pretty(&self.rewrites)?)?;
        write("embeddings.bin.zst", self.embeddings.to_bytes()?)
    }

    /// The Markdown of a page, by excerpt page number.
    pub fn page(&self, number: u32) -> Option<&str> {
        self.pages
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, text)| text.as_str())
    }
}

/// Pretty JSON with a final newline.
fn pretty<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| Error::Cache(e.to_string()))?;
    text.push('\n');
    Ok(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(seed: f32) -> Vec<f32> {
        (0..8).map(|i| (seed + i as f32) / 10.0).collect()
    }

    #[test]
    fn sixteen_bit_floats_round_trip_every_value() {
        for bits in 0..=u16::MAX {
            let value = f16_to_f32(bits);
            if value.is_nan() {
                assert!(f16_to_f32(f32_to_f16(value)).is_nan());
            } else {
                assert_eq!(f32_to_f16(value), bits, "bits {bits:#06x} value {value}");
            }
        }
    }

    #[test]
    fn sixteen_bit_floats_keep_three_digits() {
        for value in [
            0.0f32, 1.0, -1.0, 0.5, 0.123_456, -0.987_654, 0.031_25, 1e-6, 65_504.0,
        ] {
            let back = f16_to_f32(f32_to_f16(value));
            assert!(
                (back - value).abs() <= value.abs() * 1e-3 + 1e-7,
                "{value} came back as {back}"
            );
        }
        assert_eq!(f16_to_f32(f32_to_f16(1e9)), f32::INFINITY);
        assert_eq!(f16_to_f32(f32_to_f16(-1e9)), f32::NEG_INFINITY);
    }

    #[test]
    fn an_embedding_is_found_by_its_exact_text_and_model() {
        let mut store = EmbeddingStore::new("model-a", 8);
        store.insert("What is a vertex?", &vector(1.0)).unwrap();
        let back = store.get("What is a vertex?").unwrap();
        assert_eq!(back.len(), 8);
        assert!((back[3] - 0.4).abs() < 1e-3);
        // One changed character is another key.
        assert!(store.get("What is a vertex? ").is_err());
        assert!(store.get("what is a vertex?").is_err());
        assert!(!store.contains("What is a vertex"));
        // The model is part of the key.
        let other = EmbeddingStore::new("model-b", 8);
        assert_ne!(store.key("x"), other.key("x"));
        assert_eq!(store.key("x"), EmbeddingStore::new("model-a", 8).key("x"));
    }

    #[test]
    fn a_missing_entry_is_an_error_that_names_the_text() {
        let store = EmbeddingStore::new("m", 8);
        let error = store.get("quadratic formula").unwrap_err().to_string();
        assert!(error.contains("quadratic formula"), "{error}");
        assert!(error.contains("Refresh the cache"), "{error}");
    }

    #[test]
    fn a_vector_of_the_wrong_size_is_refused() {
        let mut store = EmbeddingStore::new("m", 8);
        assert!(store.insert("x", &[0.0; 7]).is_err());
    }

    #[test]
    fn the_file_is_the_same_every_time_and_reads_back() {
        let mut a = EmbeddingStore::new("model-a", 8);
        let mut b = EmbeddingStore::new("model-a", 8);
        // The insert order does not matter.
        for i in 0..5 {
            a.insert(&format!("text {i}"), &vector(i as f32)).unwrap();
        }
        for i in (0..5).rev() {
            b.insert(&format!("text {i}"), &vector(i as f32)).unwrap();
        }
        assert_eq!(a.to_bytes().unwrap(), b.to_bytes().unwrap());
        let back = EmbeddingStore::from_bytes(&a.to_bytes().unwrap()).unwrap();
        assert_eq!(back, a);
        assert_eq!(back.model(), "model-a");
        assert_eq!(back.len(), 5);
    }

    #[test]
    fn a_broken_file_is_an_error_and_not_a_panic() {
        assert!(EmbeddingStore::from_bytes(b"not zstd").is_err());
        let good = {
            let mut s = EmbeddingStore::new("m", 8);
            s.insert("x", &vector(1.0)).unwrap();
            zstd::decode_all(s.to_bytes().unwrap().as_slice()).unwrap()
        };
        for cut in [3, 10, good.len() - 1] {
            let short = zstd::encode_all(&good[..cut], 3).unwrap();
            assert!(EmbeddingStore::from_bytes(&short).is_err(), "cut at {cut}");
        }
        let mut extra = good.clone();
        extra.push(0);
        assert!(
            EmbeddingStore::from_bytes(&zstd::encode_all(extra.as_slice(), 3).unwrap()).is_err()
        );
        let mut wrong = good;
        wrong[0] = b'X';
        assert!(
            EmbeddingStore::from_bytes(&zstd::encode_all(wrong.as_slice(), 3).unwrap()).is_err()
        );
    }

    #[test]
    fn the_rewrite_key_depends_on_the_model_the_context_and_the_question() {
        let base = rewrite_key("haiku", "What is a vertex?", "And the range?");
        assert_eq!(
            base,
            rewrite_key("haiku", "What is a vertex?", "And the range?")
        );
        assert_ne!(
            base,
            rewrite_key("other", "What is a vertex?", "And the range?")
        );
        assert_ne!(
            base,
            rewrite_key("haiku", "What is a root?", "And the range?")
        );
        assert_ne!(
            base,
            rewrite_key("haiku", "What is a vertex?", "And the domain?")
        );
        // The parts cannot slide into each other.
        assert_ne!(rewrite_key("a", "bc", "d"), rewrite_key("ab", "c", "d"));
    }

    #[test]
    fn a_cache_saves_and_loads_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let mut embeddings = EmbeddingStore::new("fake", 8);
        embeddings.insert("hello", &vector(2.0)).unwrap();
        let cache = Cache {
            meta: Meta {
                format: FORMAT,
                kind: CacheKind::Dev,
                corpus: "c.pdf".into(),
                corpus_sha256: "00".into(),
                embedding_model: "fake".into(),
                transcribe_model: None,
                rewrite_model: "dev-concat".into(),
                pages: 2,
                transcribed_pages: 0,
                fallback_pages: vec![],
                latency: None,
            },
            pages: vec![(1, "# One\n".into()), (2, "Two\n".into())],
            embeddings,
            rewrites: BTreeMap::from([("k".to_owned(), "v".to_owned())]),
        };
        cache.save(dir.path()).unwrap();
        let back = Cache::load(dir.path()).unwrap();
        assert_eq!(back.meta, cache.meta);
        assert_eq!(back.pages, cache.pages);
        assert_eq!(back.embeddings, cache.embeddings);
        assert_eq!(back.rewrites, cache.rewrites);
        assert_eq!(back.page(2), Some("Two\n"));
        // A second save gives the same files.
        let again = tempfile::tempdir().unwrap();
        cache.save(again.path()).unwrap();
        for name in [
            "meta.json",
            "rewrites.json",
            "embeddings.bin.zst",
            "pages/001.md",
        ] {
            assert_eq!(
                std::fs::read(dir.path().join(name)).unwrap(),
                std::fs::read(again.path().join(name)).unwrap(),
                "{name}"
            );
        }
    }

    #[test]
    fn a_missing_cache_tells_how_to_make_one() {
        let dir = tempfile::tempdir().unwrap();
        let error = Cache::load(&dir.path().join("none"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("openbook-eval refresh"), "{error}");
    }
}
