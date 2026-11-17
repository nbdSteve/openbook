//! The corpora of the evaluation.
//!
//! ```text
//! evals/corpora/<id>/<name>.pdf     the excerpt (exactly one PDF)
//! evals/corpora/<id>/ATTRIBUTION.md  source, licence, and the changes
//! evals/corpora/<id>/sections.json   the sections: one document each for the per-document cap
//! evals/corpora/<id>/golden.jsonl    the golden questions
//! evals/corpora/<id>/cache/          the live cache
//! evals/corpora/<id>/cache-dev/      the development cache
//! ```
//!
//! A corpus is a directory under `evals/corpora` that holds a `sections.json`. The ID is the
//! directory name. Every command that is not told a corpus runs all of them, in the order of
//! their IDs.

use std::path::{Path, PathBuf};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    pub id: String,
    pub dir: PathBuf,
}

impl Corpus {
    pub fn sections_path(&self) -> PathBuf {
        self.dir.join("sections.json")
    }

    pub fn golden_path(&self) -> PathBuf {
        self.dir.join("golden.jsonl")
    }

    pub fn attribution_path(&self) -> PathBuf {
        self.dir.join("ATTRIBUTION.md")
    }

    /// The live cache, or the development cache.
    pub fn cache_dir(&self, dev: bool) -> PathBuf {
        self.dir.join(if dev { "cache-dev" } else { "cache" })
    }

    /// The only PDF of the corpus.
    pub fn pdf_path(&self) -> Result<PathBuf, Error> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&self.dir).map_err(|e| Error::io(&self.dir, &e))? {
            let path = entry.map_err(|e| Error::io(&self.dir, &e))?.path();
            if path.extension().is_some_and(|e| e == "pdf") {
                found.push(path);
            }
        }
        match found.len() {
            1 => Ok(found.remove(0)),
            n => Err(Error::Data(format!(
                "{} must hold exactly one PDF and it holds {n}",
                self.dir.display()
            ))),
        }
    }
}

/// The directory that holds the corpora.
pub fn corpora_dir(evals: &Path) -> PathBuf {
    evals.join("corpora")
}

/// Every corpus of an evals directory, sorted by ID.
pub fn discover(evals: &Path) -> Result<Vec<Corpus>, Error> {
    let root = corpora_dir(evals);
    let entries = std::fs::read_dir(&root).map_err(|e| Error::io(&root, &e))?;
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| Error::io(&root, &e))?.path();
        if path.join("sections.json").is_file()
            && let Some(id) = path.file_name().and_then(|n| n.to_str())
        {
            found.push(Corpus {
                id: id.to_owned(),
                dir: path.clone(),
            });
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    if found.is_empty() {
        return Err(Error::Data(format!(
            "there is no corpus in {}: a corpus is a directory with a sections.json",
            root.display()
        )));
    }
    Ok(found)
}

/// The corpora that a command works on: the one that `--corpus` names, or all of them.
pub fn select(evals: &Path, only: Option<&str>) -> Result<Vec<Corpus>, Error> {
    let all = discover(evals)?;
    match only {
        None => Ok(all),
        Some(id) => {
            let known: Vec<String> = all.iter().map(|c| c.id.clone()).collect();
            all.into_iter()
                .find(|c| c.id == id)
                .map(|c| vec![c])
                .ok_or_else(|| {
                    Error::Run(format!(
                        "there is no corpus {id:?}. The corpora are: {}",
                        known.join(", ")
                    ))
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(root: &Path, id: &str) {
        let dir = root.join("corpora").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sections.json"), "[]").unwrap();
    }

    #[test]
    fn corpora_are_found_and_sorted_by_id() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), "b-history");
        make(dir.path(), "a-algebra");
        // A directory without sections.json is not a corpus.
        std::fs::create_dir_all(dir.path().join("corpora/notes")).unwrap();
        let ids: Vec<String> = discover(dir.path())
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["a-algebra", "b-history"]);
        assert_eq!(select(dir.path(), None).unwrap().len(), 2);
        assert_eq!(
            select(dir.path(), Some("b-history")).unwrap()[0].id,
            "b-history"
        );
        let error = select(dir.path(), Some("nope")).unwrap_err().to_string();
        assert!(error.contains("a-algebra, b-history"), "{error}");
    }

    #[test]
    fn no_corpus_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("corpora")).unwrap();
        assert!(discover(dir.path()).is_err());
    }

    #[test]
    fn the_paths_of_a_corpus_follow_the_layout() {
        let corpus = Corpus {
            id: "x".into(),
            dir: PathBuf::from("/e/corpora/x"),
        };
        assert_eq!(corpus.cache_dir(false), PathBuf::from("/e/corpora/x/cache"));
        assert_eq!(
            corpus.cache_dir(true),
            PathBuf::from("/e/corpora/x/cache-dev")
        );
        assert_eq!(
            corpus.golden_path(),
            PathBuf::from("/e/corpora/x/golden.jsonl")
        );
    }

    #[test]
    fn the_pdf_must_be_exactly_one() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), "c");
        let corpus = discover(dir.path()).unwrap().remove(0);
        assert!(corpus.pdf_path().is_err());
        std::fs::write(corpus.dir.join("one.pdf"), "x").unwrap();
        assert!(corpus.pdf_path().unwrap().ends_with("one.pdf"));
        std::fs::write(corpus.dir.join("two.pdf"), "x").unwrap();
        assert!(corpus.pdf_path().is_err());
    }
}
