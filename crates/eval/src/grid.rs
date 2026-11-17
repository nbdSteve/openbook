//! The tuning grid: every combination of the knobs of `docs/evaluation.md`, offline.
//!
//! The grid searches once for each chunk target, document mode, and candidate count, and fuses many
//! times. It needs the cache to hold the chunk embeddings of every chunk target (`refresh`, or
//! `refresh --missing-only` for a cache that was made before the grid existed).

use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::metrics::{self, Metrics};
use crate::offline::{CHARS_PER_TOKEN, Index, Options, Tuning, judge_all, search_all};
use crate::refresh::{CHUNK_TARGET_TOKENS, PRODUCTION_CHUNK_TOKENS};

pub const RRF_KS: [f64; 3] = [30.0, 60.0, 100.0];
pub const FINAL_SOURCES: [usize; 3] = [6, 8, 10];
/// Candidates from each retriever.
pub const CANDIDATES: [usize; 2] = [20, 40];
/// Most sources per document. 8 is the same as no cap at 6 or 8 final sources.
pub const MAX_PER_DOCUMENT: [usize; 5] = [2, 3, 4, 6, 8];
pub const LEXICAL_WEIGHTS: [f64; 2] = [0.5, 1.0];

/// One setting of the knobs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Knobs {
    pub chunk_tokens: usize,
    pub rrf_k: f64,
    pub final_sources: usize,
    pub candidates: usize,
    pub max_per_document: usize,
    pub lexical_weight: f64,
}

impl Knobs {
    /// The production setting, with the candidate counts of production.
    pub fn production(options: &Options) -> Self {
        Self {
            chunk_tokens: options.chunk_tokens(),
            rrf_k: options.params.rrf_k,
            final_sources: options.params.final_sources,
            candidates: options.dense_top_k,
            max_per_document: options.params.max_per_document,
            lexical_weight: options.params.lexical_weight,
        }
    }
}

/// The result of one setting in one document mode.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeResult {
    pub hybrid: Metrics,
    /// Recall of the whole returned list, whatever its length.
    pub recall_returned: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridRow {
    pub knobs: Knobs,
    /// One document per section.
    pub sectioned: ModeResult,
    /// The whole PDF is one document.
    pub cap_off: ModeResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    pub embedding_model: String,
    pub settings: String,
    pub rows: Vec<GridRow>,
}

/// Runs the whole grid. `tuning` fixes what the grid does not vary (the gate rule above all).
pub async fn run(evals: &Path, cache_dir: &Path, tuning: &Tuning) -> Result<Grid, Error> {
    let loaded = crate::offline::load(evals, cache_dir)?;
    let base = tuning.options(&loaded.cache.meta.embedding_model);
    let mut rows = Vec::new();
    for tokens in CHUNK_TARGET_TOKENS {
        // Two indexes (document modes) for each chunk target.
        let mut indexes = Vec::new();
        for single in [false, true] {
            indexes.push(Index::build(
                &loaded.sections,
                &loaded.cache,
                tokens * CHARS_PER_TOKEN,
                single,
            )?);
        }
        for candidates in CANDIDATES {
            let mut found = Vec::new();
            for index in &mut indexes {
                index.set_candidates(candidates, candidates);
                found.push(search_all(index, &loaded.questions, &loaded.cache).await?);
            }
            for k in RRF_KS {
                for final_sources in FINAL_SOURCES {
                    for max_per_document in MAX_PER_DOCUMENT {
                        for lexical_weight in LEXICAL_WEIGHTS {
                            let mut params = base.params;
                            params.rrf_k = k;
                            params.final_sources = final_sources;
                            params.max_per_document = max_per_document;
                            params.lexical_weight = lexical_weight;
                            let mut modes = Vec::new();
                            for (index, found) in indexes.iter().zip(&found) {
                                let run = judge_all(index, &loaded.questions, found, &params)?;
                                modes.push(ModeResult {
                                    hybrid: metrics::aggregate(&run.hybrid),
                                    recall_returned: metrics::recall_of_returned(&run.hybrid),
                                });
                            }
                            rows.push(GridRow {
                                knobs: Knobs {
                                    chunk_tokens: tokens,
                                    rrf_k: k,
                                    final_sources,
                                    candidates,
                                    max_per_document,
                                    lexical_weight,
                                },
                                sectioned: modes[0],
                                cap_off: modes[1],
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(Grid {
        embedding_model: loaded.cache.meta.embedding_model.clone(),
        settings: base.describe(),
        rows,
    })
}

fn find<'a>(grid: &'a Grid, knobs: &Knobs) -> Option<&'a GridRow> {
    grid.rows.iter().find(|r| r.knobs == *knobs)
}

fn header(out: &mut String, first: &str) {
    let _ = writeln!(
        out,
        "| {first} | R@5 | R@8 | Recall of list | nDCG@8 | MRR | Gate acc. | cap-off R@8 | cap-off nDCG@8 | cap-off MRR |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|");
}

fn line(out: &mut String, label: &str, row: &GridRow) {
    let (s, d) = (&row.sectioned, &row.cap_off);
    let _ = writeln!(
        out,
        "| {label} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} |",
        s.hybrid.recall_at_5,
        s.hybrid.recall_at_8,
        s.recall_returned,
        s.hybrid.ndcg_at_8,
        s.hybrid.mrr,
        s.hybrid.gate_accuracy,
        d.hybrid.recall_at_8,
        d.hybrid.ndcg_at_8,
        d.hybrid.mrr
    );
}

impl Grid {
    /// One table per knob (the others at their production value), then the best of the whole grid.
    /// `*` marks the production setting.
    pub fn to_markdown(&self, production: &Knobs, top: usize) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Retrieval tuning grid");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{} settings, embedding model `{}`. {}.",
            self.rows.len(),
            self.embedding_model,
            self.settings
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Each table changes one knob and keeps the others at production. `*` marks the production value. Columns: hybrid metrics with one document per section, then Recall@8, nDCG@8, and MRR when the whole PDF is one document. Recall of list is the share of questions with a relevant chunk anywhere in the returned list."
        );
        type Set = fn(&mut Knobs, f64);
        let experiments: [(&str, Vec<f64>, f64, Set); 6] = [
            ("RRF k", RRF_KS.to_vec(), production.rrf_k, |k, v| {
                k.rrf_k = v
            }),
            (
                "Final sources",
                FINAL_SOURCES.iter().map(|v| *v as f64).collect(),
                production.final_sources as f64,
                |k, v| k.final_sources = v as usize,
            ),
            (
                "Candidates per retriever",
                CANDIDATES.iter().map(|v| *v as f64).collect(),
                production.candidates as f64,
                |k, v| k.candidates = v as usize,
            ),
            (
                "Max per document",
                MAX_PER_DOCUMENT.iter().map(|v| *v as f64).collect(),
                production.max_per_document as f64,
                |k, v| k.max_per_document = v as usize,
            ),
            (
                "BM25 weight in the fusion",
                LEXICAL_WEIGHTS.to_vec(),
                production.lexical_weight,
                |k, v| k.lexical_weight = v,
            ),
            (
                "Chunk target (tokens)",
                CHUNK_TARGET_TOKENS.iter().map(|v| *v as f64).collect(),
                production.chunk_tokens as f64,
                |k, v| k.chunk_tokens = v as usize,
            ),
        ];
        for (name, values, default, set) in experiments {
            let _ = writeln!(out);
            let _ = writeln!(out, "## {name}");
            let _ = writeln!(out);
            header(&mut out, name);
            for value in values {
                let mut knobs = *production;
                set(&mut knobs, value);
                let mark = if (value - default).abs() < 1e-9 {
                    " *"
                } else {
                    ""
                };
                match find(self, &knobs) {
                    Some(row) => line(&mut out, &format!("{value}{mark}"), row),
                    None => {
                        let _ = writeln!(out, "| {value}{mark} | not in the grid |");
                    }
                }
            }
        }
        let mut best: Vec<&GridRow> = self.rows.iter().collect();
        best.sort_by(|a, b| {
            let key = |r: &GridRow| {
                (
                    r.sectioned.hybrid.gate_accuracy,
                    r.sectioned.hybrid.recall_at_8,
                    r.sectioned.hybrid.ndcg_at_8,
                    r.sectioned.hybrid.mrr,
                )
            };
            let (x, y) = (key(a), key(b));
            y.0.total_cmp(&x.0)
                .then(y.1.total_cmp(&x.1))
                .then(y.2.total_cmp(&x.2))
                .then(y.3.total_cmp(&x.3))
        });
        let _ = writeln!(out);
        let _ = writeln!(out, "## The best {top} settings of the whole grid");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Sorted by gate accuracy, then Recall@8, nDCG@8, and MRR (one document per section)."
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "| chunk | k | sources | candidates | max/doc | BM25 weight | R@5 | R@8 | Recall of list | nDCG@8 | MRR | cap-off R@8 | cap-off nDCG@8 | cap-off MRR |"
        );
        let _ = writeln!(
            out,
            "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
        );
        for row in best.into_iter().take(top) {
            let k = &row.knobs;
            let (s, d) = (&row.sectioned, &row.cap_off);
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} |",
                k.chunk_tokens,
                k.rrf_k,
                k.final_sources,
                k.candidates,
                k.max_per_document,
                k.lexical_weight,
                s.hybrid.recall_at_5,
                s.hybrid.recall_at_8,
                s.recall_returned,
                s.hybrid.ndcg_at_8,
                s.hybrid.mrr,
                d.hybrid.recall_at_8,
                d.hybrid.ndcg_at_8,
                d.hybrid.mrr
            );
        }
        let _ = production_note(&mut out, PRODUCTION_CHUNK_TOKENS);
        out
    }

    pub fn to_json(&self) -> Result<String, Error> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| Error::Run(e.to_string()))?;
        text.push('\n');
        Ok(text)
    }
}

fn production_note(out: &mut String, chunk_tokens: usize) -> std::fmt::Result {
    writeln!(
        out,
        "\nThe production chunk target is {chunk_tokens} tokens. The grid does not change the gate rule: it keeps the one in the settings line."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(recall: f64) -> ModeResult {
        let mut hybrid = Metrics {
            recall_at_5: recall,
            recall_at_8: recall,
            ndcg_at_8: 0.5,
            mrr: 0.5,
            gate_accuracy: 0.9,
            false_gate_rate: 0.0,
            missed_gate_rate: 0.1,
        };
        hybrid.recall_at_5 = recall;
        ModeResult {
            hybrid,
            recall_returned: recall,
        }
    }

    #[test]
    fn the_tables_mark_the_production_value_and_list_every_knob() {
        let production = Knobs::production(&Options::production("amazon.titan-embed-text-v2:0"));
        let mut rows = Vec::new();
        for k in RRF_KS {
            rows.push(GridRow {
                knobs: Knobs {
                    rrf_k: k,
                    ..production
                },
                sectioned: result(0.9),
                cap_off: result(0.8),
            });
        }
        let grid = Grid {
            embedding_model: "m".into(),
            settings: "s".into(),
            rows,
        };
        let text = grid.to_markdown(&production, 5);
        assert!(text.contains("## RRF k"));
        assert!(text.contains("| 60 * | 0.9000 |"), "{text}");
        assert!(text.contains("| 30 | 0.9000 |"));
        // A knob value that the grid lacks is said, not skipped.
        assert!(text.contains("not in the grid"));
        for name in [
            "Final sources",
            "Candidates per retriever",
            "Max per document",
            "BM25 weight in the fusion",
            "Chunk target (tokens)",
        ] {
            assert!(text.contains(name), "{name}");
        }
        let back: Grid = serde_json::from_str(&grid.to_json().unwrap()).unwrap();
        assert_eq!(back, grid);
    }
}
