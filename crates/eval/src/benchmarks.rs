//! `evals/benchmarks.json`: a small public summary of the evaluation, for the README and the
//! landing page.
//!
//! `offline --write-baseline` writes it together with the baseline, and `offline --check` fails when
//! the file is stale (it is not what the code gives now). The schema is stable: a reader can rely on
//! `schema: 1`, and a change that breaks a reader raises the number.
//!
//! ```text
//! {
//!   "schema": 1,
//!   "embeddingModel": "amazon.titan-embed-text-v2:0",
//!   "questions": 180,
//!   "corpora": [ { "id", "title", "licence", "source", "pages", "questions" } ],
//!   "retrieval": {
//!     "combined": { "hybrid": R, "dense": R, "bm25": R },       // R: recallAt5, recallAt8, ndcgAt8, mrr
//!     "corpora": { "<id>": { "hybrid": R, "dense": R, "bm25": R } }
//!   },
//!   "gate": {
//!     "combined": G,                                              // G: accuracy, falseGateRate,
//!     "corpora": { "<id>": G }                                    //    offTopicCaught, notCoveredCaught
//!   }
//! }
//! ```
//!
//! The numbers have four decimals. The questions are the ones that the retrieval was tuned on, so
//! they are in-sample. The pooled numbers weigh every question the same, so a corpus with more
//! questions weighs more. `offTopicCaught` and `notCoveredCaught` are the shares of those questions
//! that the gate cut off (no sources), from 0 to 1, for the hybrid setup.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::corpus::Corpus;
use crate::metrics::Metrics;
use crate::report::{Report, SuiteReport};

pub const SCHEMA: u32 = 1;
pub const START_MARKER: &str = "<!-- benchmarks:start -->";
pub const END_MARKER: &str = "<!-- benchmarks:end -->";

/// What a corpus says about itself, in `corpus.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorpusMeta {
    pub title: String,
    pub licence: String,
    /// The page of the book on openstax.org.
    pub source: String,
}

impl CorpusMeta {
    pub fn load(corpus: &Corpus) -> Result<Self, Error> {
        let path = corpus.dir.join("corpus.json");
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
        serde_json::from_str(&text).map_err(|e| Error::Data(format!("{}: {e}", path.display())))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorpusInfo {
    pub id: String,
    pub title: String,
    pub licence: String,
    pub source: String,
    pub pages: u32,
    pub questions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retrieval {
    pub recall_at_5: f64,
    pub recall_at_8: f64,
    pub ndcg_at_8: f64,
    pub mrr: f64,
}

impl From<&Metrics> for Retrieval {
    fn from(m: &Metrics) -> Self {
        Self {
            recall_at_5: m.recall_at_5,
            recall_at_8: m.recall_at_8,
            ndcg_at_8: m.ndcg_at_8,
            mrr: m.mrr,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetrievalSet {
    pub hybrid: Retrieval,
    pub dense: Retrieval,
    pub bm25: Retrieval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Gate {
    pub accuracy: f64,
    pub false_gate_rate: f64,
    pub off_topic_caught: f64,
    pub not_covered_caught: f64,
}

/// A value for all corpora pooled, and one for each corpus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scoped<T> {
    pub combined: T,
    pub corpora: BTreeMap<String, T>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Benchmarks {
    pub schema: u32,
    pub embedding_model: String,
    pub questions: usize,
    pub corpora: Vec<CorpusInfo>,
    pub retrieval: Scoped<RetrievalSet>,
    pub gate: Scoped<Gate>,
}

fn retrieval_set(report: &Report) -> RetrievalSet {
    RetrievalSet {
        hybrid: (&report.hybrid).into(),
        dense: (&report.dense).into(),
        bm25: (&report.bm25).into(),
    }
}

fn gate(report: &Report) -> Gate {
    let caught = |kind: &str| {
        report
            .hybrid_by_kind
            .get(kind)
            .map_or(0.0, |k| k.gated_rate)
    };
    Gate {
        accuracy: report.hybrid.gate_accuracy,
        false_gate_rate: report.hybrid.false_gate_rate,
        off_topic_caught: caught("off_topic"),
        not_covered_caught: caught("not_covered"),
    }
}

impl Benchmarks {
    /// The summary of a run over all corpora. `corpora` is in the order of the suite.
    pub fn from_suite(suite: &SuiteReport, corpora: &[Corpus]) -> Result<Self, Error> {
        let mut info = Vec::new();
        for (corpus, reported) in corpora.iter().zip(&suite.corpora) {
            let meta = CorpusMeta::load(corpus)?;
            let sections = crate::data::load_sections(&corpus.sections_path())?;
            info.push(CorpusInfo {
                id: corpus.id.clone(),
                title: meta.title,
                licence: meta.licence,
                source: meta.source,
                pages: crate::data::page_total(&sections),
                questions: reported.report.questions,
            });
        }
        Ok(Self {
            schema: SCHEMA,
            embedding_model: suite.combined.embedding_model.clone(),
            questions: suite.combined.questions,
            corpora: info,
            retrieval: Scoped {
                combined: retrieval_set(&suite.combined),
                corpora: suite
                    .corpora
                    .iter()
                    .map(|c| (c.id.clone(), retrieval_set(&c.report)))
                    .collect(),
            },
            gate: Scoped {
                combined: gate(&suite.combined),
                corpora: suite
                    .corpora
                    .iter()
                    .map(|c| (c.id.clone(), gate(&c.report)))
                    .collect(),
            },
        })
    }

    pub fn to_json(&self) -> Result<String, Error> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| Error::Run(e.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Run(format!(
                    "there is no {}. Run `openbook-eval offline --write-baseline`.",
                    path.display()
                ))
            } else {
                Error::io(path, &e)
            }
        })?;
        serde_json::from_str(&text).map_err(|e| Error::Run(format!("{}: {e}", path.display())))
    }

    /// The Markdown that goes between the markers of the README: the retrieval table, then the gate
    /// table. Every number has four decimals, like the file.
    pub fn to_markdown(&self) -> String {
        let questions = |id: &str| {
            self.corpora
                .iter()
                .find(|c| c.id == id)
                .map_or(0, |c| c.questions)
        };
        let title = |id: &str| {
            self.corpora
                .iter()
                .find(|c| c.id == id)
                .map_or_else(|| id.to_owned(), |c| c.title.clone())
        };
        let mut out = String::new();
        out.push_str("| Corpus | Questions | Setup | Recall@5 | Recall@8 | nDCG@8 | MRR |\n|---|---|---|---|---|---|---|\n");
        let mut scopes: Vec<(String, usize, &RetrievalSet)> = vec![(
            "All corpora".to_owned(),
            self.questions,
            &self.retrieval.combined,
        )];
        for c in &self.corpora {
            if let Some(set) = self.retrieval.corpora.get(&c.id) {
                scopes.push((title(&c.id), questions(&c.id), set));
            }
        }
        for (name, count, set) in scopes {
            for (setup, r) in [
                ("Hybrid", &set.hybrid),
                ("Dense only", &set.dense),
                ("BM25 only", &set.bm25),
            ] {
                out.push_str(&format!(
                    "| {name} | {count} | {setup} | {:.4} | {:.4} | {:.4} | {:.4} |\n",
                    r.recall_at_5, r.recall_at_8, r.ndcg_at_8, r.mrr
                ));
            }
        }
        out.push_str("\n| Corpus | Gate accuracy | False-gate rate | Off-topic questions caught | Not-covered questions caught |\n|---|---|---|---|---|\n");
        let mut gates: Vec<(String, &Gate)> = vec![("All corpora".to_owned(), &self.gate.combined)];
        for c in &self.corpora {
            if let Some(g) = self.gate.corpora.get(&c.id) {
                gates.push((title(&c.id), g));
            }
        }
        for (name, g) in gates {
            out.push_str(&format!(
                "| {name} | {:.4} | {:.4} | {:.4} | {:.4} |\n",
                g.accuracy, g.false_gate_rate, g.off_topic_caught, g.not_covered_caught
            ));
        }
        out
    }
}

/// The README with the block between the markers replaced by `block`.
pub fn replace_block(readme: &str, block: &str) -> Result<String, Error> {
    let start = readme
        .find(START_MARKER)
        .ok_or_else(|| Error::Run(format!("the README has no {START_MARKER} marker")))?;
    let end = readme
        .find(END_MARKER)
        .ok_or_else(|| Error::Run(format!("the README has no {END_MARKER} marker")))?;
    if end < start {
        return Err(Error::Run(
            "the benchmark markers are in the wrong order".into(),
        ));
    }
    Ok(format!(
        "{}{START_MARKER}\n{block}{}",
        &readme[..start],
        &readme[end..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Benchmarks {
        let r = |x: f64| Retrieval {
            recall_at_5: x,
            recall_at_8: x + 0.01,
            ndcg_at_8: x - 0.2,
            mrr: x - 0.1,
        };
        let set = |x: f64| RetrievalSet {
            hybrid: r(x),
            dense: r(x - 0.01),
            bm25: r(x - 0.02),
        };
        let g = |x: f64| Gate {
            accuracy: x,
            false_gate_rate: 0.0,
            off_topic_caught: 1.0,
            not_covered_caught: 0.5,
        };
        Benchmarks {
            schema: SCHEMA,
            embedding_model: "m".into(),
            questions: 20,
            corpora: vec![CorpusInfo {
                id: "algebra".into(),
                title: "College Algebra".into(),
                licence: "CC BY 4.0".into(),
                source: "https://openstax.org/details/books/college-algebra".into(),
                pages: 43,
                questions: 20,
            }],
            retrieval: Scoped {
                combined: set(0.9),
                corpora: BTreeMap::from([("algebra".to_owned(), set(0.9))]),
            },
            gate: Scoped {
                combined: g(0.9),
                corpora: BTreeMap::from([("algebra".to_owned(), g(0.9))]),
            },
        }
    }

    #[test]
    fn the_schema_is_stable() {
        let json: serde_json::Value = serde_json::from_str(&sample().to_json().unwrap()).unwrap();
        assert_eq!(json["schema"], 1);
        for path in [
            "/embeddingModel",
            "/questions",
            "/corpora/0/id",
            "/corpora/0/title",
            "/corpora/0/licence",
            "/corpora/0/source",
            "/corpora/0/pages",
            "/corpora/0/questions",
            "/retrieval/combined/hybrid/recallAt5",
            "/retrieval/combined/hybrid/recallAt8",
            "/retrieval/combined/dense/ndcgAt8",
            "/retrieval/combined/bm25/mrr",
            "/retrieval/corpora/algebra/hybrid/mrr",
            "/gate/combined/accuracy",
            "/gate/combined/falseGateRate",
            "/gate/combined/offTopicCaught",
            "/gate/combined/notCoveredCaught",
            "/gate/corpora/algebra/notCoveredCaught",
        ] {
            assert!(json.pointer(path).is_some(), "{path}");
        }
        let back: Benchmarks = serde_json::from_value(json).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn the_markdown_has_a_row_for_each_scope_and_setup() {
        let text = sample().to_markdown();
        assert!(text.contains("| All corpora | 20 | Hybrid | 0.9000 | 0.9100 | 0.7000 | 0.8000 |"));
        assert!(text.contains("| College Algebra | 20 | BM25 only |"));
        assert!(text.contains("| All corpora | 0.9000 | 0.0000 | 1.0000 | 0.5000 |"));
    }

    #[test]
    fn the_block_between_the_markers_is_replaced_and_the_rest_stays() {
        let readme = format!("# A\n\n{START_MARKER}\nold\n{END_MARKER}\n\nafter\n");
        let new = replace_block(&readme, "new line\n").unwrap();
        assert_eq!(
            new,
            format!("# A\n\n{START_MARKER}\nnew line\n{END_MARKER}\n\nafter\n")
        );
        // A second run changes nothing.
        assert_eq!(replace_block(&new, "new line\n").unwrap(), new);
        assert!(replace_block("no markers", "x").is_err());
    }
}
