use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openbook_core::model::ChatRequest;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Error;
use crate::cache::hex;

pub const ANSWERS_DIR: &str = "answers";

pub trait Keyed {
    fn key(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnswerEntry {
    pub key: String,
    pub question_id: String,
    pub style: String,
    pub prompt_sha256: String,
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub stop_reason: String,
}

impl Keyed for AnswerEntry {
    fn key(&self) -> &str {
        &self.key
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Judgment {
    pub key: String,
    pub question_id: String,
    pub answer_sha256: String,
    pub says_not_covered: bool,
    pub mentions_course_materials: bool,
    pub natural_reply: bool,
    pub reason: String,
}

impl Keyed for Judgment {
    fn key(&self) -> &str {
        &self.key
    }
}

pub fn slug(model_id: &str) -> String {
    model_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

pub fn sha256_parts(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hex(&hasher.finalize())
}

pub fn request_sha256(request: &ChatRequest, max_tokens: i32, temperature: Option<f32>) -> String {
    let mut hasher = Sha256::new();
    let mut put = |part: &str| {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    };
    put(&request.system);
    for turn in &request.turns {
        put(&format!("{:?}", turn.role));
        put(&turn.text);
    }
    put(&max_tokens.to_string());
    put(&temperature.map_or_else(|| "none".to_owned(), |t| t.to_string()));
    hex(&hasher.finalize())
}

pub fn answer_key(model_id: &str, prompt_sha256: &str, question: &str) -> String {
    sha256_parts(&["answer", model_id, prompt_sha256, question])
}

pub fn judgment_key(
    judge_model_id: &str,
    rubric_version: u32,
    answer_sha256: &str,
    question: &str,
) -> String {
    sha256_parts(&[
        "judgment",
        judge_model_id,
        &rubric_version.to_string(),
        answer_sha256,
        question,
    ])
}

#[derive(Debug, Clone)]
pub struct Store<T> {
    path: PathBuf,
    entries: BTreeMap<String, T>,
}

impl<T: Keyed + Serialize + DeserializeOwned> Store<T> {
    pub fn load(path: PathBuf) -> Result<Self, Error> {
        let mut entries = BTreeMap::new();
        if path.exists() {
            let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
            for (index, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let entry: T = serde_json::from_str(line).map_err(|e| {
                    Error::Cache(format!("{} line {}: {e}", path.display(), index + 1))
                })?;
                entries.insert(entry.key().to_owned(), entry);
            }
        }
        Ok(Self { path, entries })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, key: &str) -> Option<&T> {
        self.entries.get(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, entry: T) {
        self.entries.insert(entry.key().to_owned(), entry);
    }

    pub fn retain(&mut self, keep: impl Fn(&T) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, entry| keep(entry));
        before - self.entries.len()
    }

    pub fn save(&self) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, &e))?;
        }
        if self.entries.is_empty() {
            if self.path.exists() {
                std::fs::remove_file(&self.path).map_err(|e| Error::io(&self.path, &e))?;
            }
            return Ok(());
        }
        let mut text = String::new();
        for entry in self.entries.values() {
            text.push_str(
                &serde_json::to_string(entry).map_err(|e| Error::Run(format!("answers: {e}")))?,
            );
            text.push('\n');
        }
        let temporary = self.path.with_extension("jsonl.tmp");
        std::fs::write(&temporary, text).map_err(|e| Error::io(&temporary, &e))?;
        std::fs::rename(&temporary, &self.path).map_err(|e| Error::io(&self.path, &e))
    }
}

pub fn answers_path(corpus_dir: &Path, model_id: &str) -> PathBuf {
    corpus_dir
        .join(ANSWERS_DIR)
        .join(format!("{}.answers.jsonl", slug(model_id)))
}

pub fn judgments_path(corpus_dir: &Path, judge_model_id: &str) -> PathBuf {
    corpus_dir
        .join(ANSWERS_DIR)
        .join(format!("{}.judgments.jsonl", slug(judge_model_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbook_core::domain::MessageRole;
    use openbook_core::model::ChatTurn;

    fn request(system: &str, text: &str) -> ChatRequest {
        ChatRequest {
            system: system.into(),
            turns: vec![ChatTurn {
                role: MessageRole::User,
                text: text.into(),
            }],
            ..ChatRequest::default()
        }
    }

    #[test]
    fn the_request_hash_covers_every_part_of_the_request() {
        let base = request_sha256(&request("s", "q"), 2000, None);
        assert_eq!(base, request_sha256(&request("s", "q"), 2000, None));
        assert_ne!(base, request_sha256(&request("s2", "q"), 2000, None));
        assert_ne!(base, request_sha256(&request("s", "q2"), 2000, None));
        assert_ne!(base, request_sha256(&request("s", "q"), 1000, None));
        assert_ne!(base, request_sha256(&request("s", "q"), 2000, Some(0.2)));
        assert_ne!(
            request_sha256(&request("ab", "c"), 1, None),
            request_sha256(&request("a", "bc"), 1, None)
        );
    }

    #[test]
    fn the_keys_change_with_the_model_the_prompt_and_the_question() {
        let key = answer_key("m", "p", "q");
        assert_eq!(key, answer_key("m", "p", "q"));
        assert_ne!(key, answer_key("m2", "p", "q"));
        assert_ne!(key, answer_key("m", "p2", "q"));
        assert_ne!(key, answer_key("m", "p", "q2"));
        let judged = judgment_key("j", 1, "a", "q");
        assert_ne!(judged, judgment_key("j", 2, "a", "q"));
        assert_ne!(judged, judgment_key("j2", 1, "a", "q"));
        assert_ne!(judged, judgment_key("j", 1, "a2", "q"));
    }

    #[test]
    fn a_store_saves_sorted_lines_and_loads_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = answers_path(dir.path(), "au.anthropic.claude-sonnet-4-6");
        assert!(path.ends_with("answers/au.anthropic.claude-sonnet-4-6.answers.jsonl"));
        let mut store: Store<AnswerEntry> = Store::load(path.clone()).unwrap();
        assert!(store.is_empty());
        for key in ["b", "a"] {
            store.insert(AnswerEntry {
                key: key.into(),
                question_id: "q".into(),
                style: "direct".into(),
                prompt_sha256: "p".into(),
                text: format!("answer {key}"),
                input_tokens: 1,
                output_tokens: 2,
                stop_reason: "end_turn".into(),
            });
        }
        store.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().next().unwrap().contains("\"key\":\"a\""));
        let back: Store<AnswerEntry> = Store::load(path).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back.get("b").unwrap().text, "answer b");
    }

    #[test]
    fn retain_drops_entries_and_an_empty_store_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = judgments_path(dir.path(), "j");
        let mut store: Store<Judgment> = Store::load(path.clone()).unwrap();
        for key in ["a", "b"] {
            store.insert(Judgment {
                key: key.into(),
                question_id: "q".into(),
                answer_sha256: "x".into(),
                says_not_covered: false,
                mentions_course_materials: false,
                natural_reply: true,
                reason: "r".into(),
            });
        }
        store.save().unwrap();
        assert_eq!(store.retain(|entry| entry.key == "a"), 1);
        assert!(store.contains("a") && !store.contains("b"));
        store.retain(|_| false);
        store.save().unwrap();
        assert!(!path.exists());
    }
}
