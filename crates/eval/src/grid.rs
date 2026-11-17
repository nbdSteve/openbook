//! The tuning grid: every combination of the knobs of `docs/evaluation.md`, offline.
//!
//! The grid searches once for each chunk target, document mode, candidate count, and corpus, and
//! fuses many times. It needs the cache of every corpus to hold the chunk embeddings of every chunk
//! target (`refresh`, or `refresh --missing-only` for a cache that was made before the grid
//! existed). Each setting is scored on every corpus alone, and on all corpora pooled.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::corpus::Corpus;
use crate::metrics::{self, Metrics};
use openbook_search::headers::FirstOccurrence;

use crate::offline::{
    CHARS_PER_TOKEN, Candidates, Index, Options, Run, Tuning, cache_dir_of, judge_all, load,
    search_all,
};
use crate::refresh::{CHUNK_TARGET_TOKENS, PRODUCTION_CHUNK_TOKENS};

pub const RRF_KS: [f64; 3] = [30.0, 60.0, 100.0];
pub const FINAL_SOURCES: [usize; 3] = [6, 8, 10];
/// Candidates from each retriever.
pub const CANDIDATES: [usize; 2] = [20, 40];
/// Most sources per document. 8 is the same as no cap at 6 or 8 final sources.
pub const MAX_PER_DOCUMENT: [usize; 5] = [2, 3, 4, 6, 8];
/// The weight of the BM25 list in the fusion. A weight below about 0.9 means that a chunk that only
/// BM25 finds can never reach the sources at k 60 and 40 dense candidates (see `docs/retrieval.md`).
pub const LEXICAL_WEIGHTS: [f64; 4] = [0.5, 0.75, 1.0, 1.5];

/// The name of the pooled scope in the tables.
pub const COMBINED: &str = "combined";

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
    /// True when the removal of running headers drops the first occurrence too (`drop-all`).
    pub drop_all_headers: bool,
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
            drop_all_headers: options.header_variant == FirstOccurrence::Drop,
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

impl ModeResult {
    fn of(run: &Run) -> Self {
        Self {
            hybrid: metrics::aggregate(&run.hybrid),
            recall_returned: metrics::recall_of_returned(&run.hybrid),
        }
    }
}

/// The result of one setting for one scope (one corpus, or all pooled), in both document modes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    /// One document per section.
    pub sectioned: ModeResult,
    /// The whole PDF is one document.
    pub cap_off: ModeResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridRow {
    pub knobs: Knobs,
    /// All corpora pooled.
    pub combined: Pair,
    /// Each corpus alone, by ID.
    pub corpora: BTreeMap<String, Pair>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    pub embedding_model: String,
    pub settings: String,
    pub rows: Vec<GridRow>,
}

/// The values of each knob that a grid runs.
#[derive(Debug, Clone)]
pub struct Axes {
    pub chunk_tokens: Vec<usize>,
    pub rrf_k: Vec<f64>,
    pub final_sources: Vec<usize>,
    pub candidates: Vec<usize>,
    pub max_per_document: Vec<usize>,
    pub lexical_weight: Vec<f64>,
    pub drop_all_headers: Vec<bool>,
}

impl Axes {
    /// The whole grid: `make eval-grid`.
    pub fn full() -> Self {
        Self {
            chunk_tokens: CHUNK_TARGET_TOKENS.to_vec(),
            rrf_k: RRF_KS.to_vec(),
            final_sources: FINAL_SOURCES.to_vec(),
            candidates: CANDIDATES.to_vec(),
            max_per_document: MAX_PER_DOCUMENT.to_vec(),
            lexical_weight: LEXICAL_WEIGHTS.to_vec(),
            drop_all_headers: vec![false, true],
        }
    }

    /// Two rows (the production setting with both header variants). The tests use it to show that
    /// the grid code works. It is fast.
    pub fn tiny() -> Self {
        Self {
            chunk_tokens: vec![PRODUCTION_CHUNK_TOKENS],
            rrf_k: vec![60.0],
            final_sources: vec![8],
            candidates: vec![40],
            max_per_document: vec![6],
            lexical_weight: vec![1.0],
            drop_all_headers: vec![false, true],
        }
    }
}

/// Runs the grid. `tuning` fixes what the grid does not vary (the gate rule above all).
pub async fn run(
    corpora: &[Corpus],
    dev: bool,
    named_cache: Option<&Path>,
    tuning: &Tuning,
    axes: &Axes,
) -> Result<Grid, Error> {
    let mut loaded = Vec::new();
    for corpus in corpora {
        loaded.push(load(corpus, &cache_dir_of(corpus, dev, named_cache))?);
    }
    let model = loaded[0].cache.meta.embedding_model.clone();
    if loaded.iter().any(|l| l.cache.meta.embedding_model != model) {
        return Err(Error::Cache(
            "the corpora have caches from different embedding models".into(),
        ));
    }
    let base = tuning.options(&model);
    let mut rows = Vec::new();
    for &tokens in &axes.chunk_tokens {
        for &drop_all in &axes.drop_all_headers {
            let variant = if drop_all {
                FirstOccurrence::Drop
            } else {
                FirstOccurrence::Keep
            };
            // Two indexes (document modes) for each corpus, chunk target, and header variant.
            let mut indexes: Vec<[Index; 2]> = Vec::new();
            for l in &loaded {
                let build = |single| {
                    Index::build(
                        &l.sections,
                        &l.cache,
                        tokens * CHARS_PER_TOKEN,
                        single,
                        variant,
                    )
                };
                indexes.push([build(false)?, build(true)?]);
            }
            for &candidates in &axes.candidates {
                // found[corpus][mode]
                let mut found: Vec<[Vec<Candidates>; 2]> = Vec::new();
                for (l, pair) in loaded.iter().zip(&mut indexes) {
                    let mut both = [Vec::new(), Vec::new()];
                    for (mode, index) in pair.iter_mut().enumerate() {
                        index.set_candidates(candidates, candidates);
                        both[mode] = search_all(index, &l.questions, &l.cache).await?;
                    }
                    found.push(both);
                }
                for &k in &axes.rrf_k {
                    for &final_sources in &axes.final_sources {
                        for &max_per_document in &axes.max_per_document {
                            for &lexical_weight in &axes.lexical_weight {
                                let mut params = base.params;
                                params.rrf_k = k;
                                params.final_sources = final_sources;
                                params.max_per_document = max_per_document;
                                params.lexical_weight = lexical_weight;
                                // runs[corpus][mode]
                                let mut runs: Vec<[Run; 2]> = Vec::new();
                                for ((l, pair), seen) in loaded.iter().zip(&indexes).zip(&found) {
                                    runs.push([
                                        judge_all(&pair[0], &l.questions, &seen[0], &params)?,
                                        judge_all(&pair[1], &l.questions, &seen[1], &params)?,
                                    ]);
                                }
                                let mut per_corpus = BTreeMap::new();
                                for (l, run) in loaded.iter().zip(&runs) {
                                    per_corpus.insert(
                                        l.corpus.id.clone(),
                                        Pair {
                                            sectioned: ModeResult::of(&run[0]),
                                            cap_off: ModeResult::of(&run[1]),
                                        },
                                    );
                                }
                                let pooled = |mode: usize| {
                                    let parts: Vec<(&str, &Run)> = loaded
                                        .iter()
                                        .zip(&runs)
                                        .map(|(l, r)| (l.corpus.id.as_str(), &r[mode]))
                                        .collect();
                                    ModeResult::of(&Run::merged(&parts))
                                };
                                rows.push(GridRow {
                                    knobs: Knobs {
                                        chunk_tokens: tokens,
                                        rrf_k: k,
                                        final_sources,
                                        candidates,
                                        max_per_document,
                                        lexical_weight,
                                        drop_all_headers: drop_all,
                                    },
                                    combined: Pair {
                                        sectioned: pooled(0),
                                        cap_off: pooled(1),
                                    },
                                    corpora: per_corpus,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(Grid {
        embedding_model: model,
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
        "| {first} | R@5 | R@8 | Recall of list | nDCG@8 | MRR | Gate acc. | False gates | cap-off R@8 | cap-off nDCG@8 | cap-off MRR |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|");
}

fn line(out: &mut String, label: &str, pair: &Pair) {
    let (s, d) = (&pair.sectioned, &pair.cap_off);
    let _ = writeln!(
        out,
        "| {label} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} |",
        s.hybrid.recall_at_5,
        s.hybrid.recall_at_8,
        s.recall_returned,
        s.hybrid.ndcg_at_8,
        s.hybrid.mrr,
        s.hybrid.gate_accuracy,
        s.hybrid.false_gate_rate,
        d.hybrid.recall_at_8,
        d.hybrid.ndcg_at_8,
        d.hybrid.mrr
    );
}

type Pick = for<'a> fn(&'a GridRow, &str) -> Option<&'a Pair>;

fn pick_combined<'a>(row: &'a GridRow, _scope: &str) -> Option<&'a Pair> {
    Some(&row.combined)
}

fn pick_corpus<'a>(row: &'a GridRow, scope: &str) -> Option<&'a Pair> {
    row.corpora.get(scope)
}

impl Grid {
    /// The IDs of the corpora of the grid.
    pub fn corpus_ids(&self) -> Vec<&str> {
        self.rows
            .first()
            .map(|r| r.corpora.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// One table per knob (the others at their production value) for all corpora pooled, then the
    /// best settings of the whole grid, then the same tables for each corpus alone. `*` marks the
    /// production setting.
    pub fn to_markdown(&self, production: &Knobs, top: usize) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Retrieval tuning grid");
        let _ = writeln!(out);
        let ids = self.corpus_ids();
        let _ = writeln!(
            out,
            "{} settings, embedding model `{}`, corpora: {}. {}.",
            self.rows.len(),
            self.embedding_model,
            ids.join(", "),
            self.settings
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Each table changes one knob and keeps the others at production. `*` marks the production value. Columns: hybrid metrics with one document per section, then Recall@8, nDCG@8, and MRR when the whole PDF is one document. Recall of list is the share of questions with a relevant chunk anywhere in the returned list. False gates is the false-gate rate: the share of covered questions that the gate cut off."
        );
        if ids.len() > 1 {
            self.scope_tables(
                &mut out,
                "Combined (all corpora pooled)",
                COMBINED,
                pick_combined,
                production,
            );
            self.best_table(&mut out, top);
            for id in &ids {
                self.scope_tables(
                    &mut out,
                    &format!("Corpus: {id}"),
                    id,
                    pick_corpus,
                    production,
                );
            }
        } else if let Some(id) = ids.first() {
            self.scope_tables(
                &mut out,
                &format!("Corpus: {id}"),
                id,
                pick_corpus,
                production,
            );
            self.best_table(&mut out, top);
        }
        let _ = production_note(&mut out, PRODUCTION_CHUNK_TOKENS);
        out
    }

    fn scope_tables(
        &self,
        out: &mut String,
        title: &str,
        scope: &str,
        pick: Pick,
        production: &Knobs,
    ) {
        type Set = fn(&mut Knobs, f64);
        let experiments: [(&str, Vec<f64>, f64, Set); 7] = [
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
            (
                "Running headers (0 = keep-first, 1 = drop-all)",
                vec![0.0, 1.0],
                f64::from(u8::from(production.drop_all_headers)),
                |k, v| k.drop_all_headers = v > 0.5,
            ),
        ];
        let _ = writeln!(out);
        let _ = writeln!(out, "## {title}");
        for (name, values, default, set) in experiments {
            let _ = writeln!(out);
            let _ = writeln!(out, "### {name}");
            let _ = writeln!(out);
            header(out, name);
            for value in values {
                let mut knobs = *production;
                set(&mut knobs, value);
                let mark = if (value - default).abs() < 1e-9 {
                    " *"
                } else {
                    ""
                };
                match find(self, &knobs).and_then(|row| pick(row, scope)) {
                    Some(pair) => line(out, &format!("{value}{mark}"), pair),
                    None => {
                        let _ = writeln!(out, "| {value}{mark} | not in the grid |");
                    }
                }
            }
        }
    }

    fn best_table(&self, out: &mut String, top: usize) {
        let mut best: Vec<&GridRow> = self.rows.iter().collect();
        best.sort_by(|a, b| {
            let key = |r: &GridRow| {
                (
                    r.combined.sectioned.hybrid.gate_accuracy,
                    r.combined.sectioned.hybrid.recall_at_8,
                    r.combined.sectioned.hybrid.ndcg_at_8,
                    r.combined.sectioned.hybrid.mrr,
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
            "All corpora pooled. Sorted by gate accuracy, then Recall@8, nDCG@8, and MRR (one document per section)."
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
            let (s, d) = (&row.combined.sectioned, &row.combined.cap_off);
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

    fn pair(a: f64, b: f64) -> Pair {
        Pair {
            sectioned: result(a),
            cap_off: result(b),
        }
    }

    fn grid_of(production: &Knobs, ids: &[&str]) -> Grid {
        let mut rows = Vec::new();
        for k in RRF_KS {
            rows.push(GridRow {
                knobs: Knobs {
                    rrf_k: k,
                    ..*production
                },
                combined: pair(0.9, 0.8),
                corpora: ids
                    .iter()
                    .map(|id| ((*id).to_owned(), pair(0.7, 0.6)))
                    .collect(),
            });
        }
        Grid {
            embedding_model: "m".into(),
            settings: "s".into(),
            rows,
        }
    }

    #[test]
    fn the_tables_mark_the_production_value_and_list_every_knob() {
        let production = Knobs::production(&Options::production("amazon.titan-embed-text-v2:0"));
        let grid = grid_of(&production, &["algebra", "history"]);
        let text = grid.to_markdown(&production, 5);
        assert!(text.contains("## Combined (all corpora pooled)"));
        assert!(text.contains("## Corpus: algebra"));
        assert!(text.contains("## Corpus: history"));
        assert!(text.contains("### RRF k"));
        // The pooled table has the pooled numbers and the corpus tables have their own.
        assert!(text.contains("| 60 * | 0.9000 |"), "{text}");
        assert!(text.contains("| 60 * | 0.7000 |"), "{text}");
        assert!(text.contains("| 30 | 0.9000 |"));
        // A knob value that the grid lacks is said, not skipped.
        assert!(text.contains("not in the grid"));
        for name in [
            "Final sources",
            "Candidates per retriever",
            "Max per document",
            "BM25 weight in the fusion",
            "Chunk target (tokens)",
            "Running headers",
        ] {
            assert!(text.contains(name), "{name}");
        }
        let back: Grid = serde_json::from_str(&grid.to_json().unwrap()).unwrap();
        assert_eq!(back, grid);
    }

    #[test]
    fn one_corpus_has_no_pooled_section() {
        let production = Knobs::production(&Options::production("amazon.titan-embed-text-v2:0"));
        let grid = grid_of(&production, &["algebra"]);
        let text = grid.to_markdown(&production, 5);
        assert!(!text.contains("Combined"));
        assert!(text.contains("## Corpus: algebra"));
        assert!(text.contains("The best 5 settings"));
    }
}
