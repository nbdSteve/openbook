//! The report (a Markdown table and a JSON file), the baseline, and the regression check. A run
//! over several corpora gives one report for each corpus and one for all of them pooled.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use openbook_search::retrieval::{GateInputs, GateRule, TermStats};

use crate::Error;
use crate::cache::{CacheKind, Latency};
use crate::data::Kind;
use crate::metrics::{Metrics, round4};

type Getter = fn(&Metrics) -> f64;

/// A hybrid ranking metric may fall this far (absolute) below the baseline.
pub const TOLERANCE: f64 = 0.01;
/// Float noise that is not a change. Metrics have four decimals.
const EPSILON: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KindMetrics {
    pub questions: usize,
    /// For a covered kind: the hybrid Recall@8 and MRR. For `not_covered`: both are 0.
    pub recall_at_8: f64,
    pub mrr: f64,
    /// The share of this kind that the gate cut off.
    pub gated_rate: f64,
}

/// A covered question where the hybrid run has no relevant chunk in the top 8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Miss {
    pub id: String,
    pub kind: String,
    pub question: String,
    pub relevant_pages: Vec<u32>,
    pub gated: bool,
    /// The pages of the returned chunks, best first, for example `4-5`.
    pub returned: Vec<String>,
}

/// What the relevance gate read for one question, and what the hybrid run did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionDiag {
    pub id: String,
    pub kind: String,
    /// The cosine distance of the closest dense result.
    pub best_dense_distance: Option<f64>,
    /// The score of the best BM25 hit.
    pub top_bm25_score: Option<f64>,
    /// The share of the distinct analyzed query terms that the best BM25 hit holds.
    pub term_coverage: f64,
    /// Distinct analyzed query terms, those that exist in the index, and those in the best hit.
    pub query_terms: usize,
    pub indexed_terms: usize,
    pub matched_terms: usize,
    pub lexical_hits: usize,
    /// True when the production gate cut the question off.
    pub gated: bool,
}

impl QuestionDiag {
    pub fn is_covered(&self) -> bool {
        self.kind != Kind::NotCovered.name() && self.kind != Kind::OffTopic.name()
    }

    /// The inputs of the gate, as the run saw them.
    pub fn gate_inputs(&self) -> GateInputs {
        GateInputs {
            best_dense_distance: self.best_dense_distance.map(|d| d as f32),
            lexical_hits: self.lexical_hits,
            top_bm25_score: self.top_bm25_score.map(|d| d as f32),
            terms: TermStats {
                query_terms: self.query_terms,
                indexed_terms: self.indexed_terms,
                matched_terms: self.matched_terms,
            },
        }
    }
}

/// How often the gate was wrong for a group of questions, for each setup. For covered questions it
/// is the false-gate rate. For `not_covered`, `off_topic`, and both together it is the missed-gate
/// rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateRow {
    pub group: String,
    pub questions: usize,
    pub dense: f64,
    pub bm25: f64,
    pub hybrid: f64,
}

/// The metrics of the three setups in one mode.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeMetrics {
    pub dense: Metrics,
    pub bm25: Metrics,
    pub hybrid: Metrics,
}

/// One candidate gate rule applied to the diagnostics of all questions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepRow {
    pub rule: String,
    pub max_distance: f64,
    pub min_coverage: Option<f64>,
    pub min_matched: Option<usize>,
    /// Covered questions that the rule cuts off, and their share.
    pub false_gates: usize,
    pub false_gate_rate: f64,
    pub missed_not_covered: f64,
    pub missed_off_topic: f64,
    pub missed_combined: f64,
    pub gate_accuracy: f64,
    /// The distance limit minus the worst best-dense distance of the covered questions that the
    /// rule does not rescue with its lexical part. Negative or small means a false gate is near.
    pub margin: Option<f64>,
    /// True when the rule has no false gate and a margin of at least `REQUIRED_MARGIN`.
    pub passes: bool,
}

/// The least distance between the gate limit and the worst covered question.
pub const REQUIRED_MARGIN: f64 = 0.02;

fn share(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        round4(count as f64 / total as f64)
    }
}

/// Applies a rule to the diagnostics.
pub fn sweep_row(label: &str, rule: &GateRule, diags: &[QuestionDiag]) -> SweepRow {
    let mut false_gates = 0;
    let (mut covered, mut not_covered, mut off_topic) = (0, 0, 0);
    let (mut missed_nc, mut missed_ot) = (0, 0);
    let mut worst: Option<f64> = None;
    for d in diags {
        let inputs = d.gate_inputs();
        let passes = rule.passes(&inputs);
        match d.kind.as_str() {
            "not_covered" => {
                not_covered += 1;
                missed_nc += usize::from(passes);
            }
            "off_topic" => {
                off_topic += 1;
                missed_ot += usize::from(passes);
            }
            _ => {
                covered += 1;
                false_gates += usize::from(!passes);
                // The distance of a covered question counts for the margin unless the lexical part
                // of the rule rescues it.
                let rescued = rule.passes_lexical_only(&inputs)
                    && !matches!(
                        rule,
                        GateRule::AnyLexicalHit { .. } | GateRule::Dense { .. }
                    );
                if !rescued && let Some(distance) = d.best_dense_distance {
                    worst = Some(worst.map_or(distance, |w: f64| w.max(distance)));
                }
            }
        }
    }
    let (min_coverage, min_matched) = match rule {
        GateRule::DenseOrLexical {
            min_coverage,
            min_matched,
            ..
        } => (Some(*min_coverage), Some(*min_matched)),
        _ => (None, None),
    };
    let max_distance = f64::from(rule.max_distance());
    let margin = worst.map(|w| round4(max_distance - w));
    let uncovered = not_covered + off_topic;
    let wrong = false_gates + missed_nc + missed_ot;
    SweepRow {
        rule: label.to_owned(),
        max_distance: round4(max_distance),
        min_coverage,
        min_matched,
        false_gates,
        false_gate_rate: share(false_gates, covered),
        missed_not_covered: share(missed_nc, not_covered),
        missed_off_topic: share(missed_ot, off_topic),
        missed_combined: share(missed_nc + missed_ot, uncovered),
        gate_accuracy: round4(1.0 - wrong as f64 / diags.len().max(1) as f64),
        margin,
        passes: false_gates == 0 && margin.is_none_or(|m| m >= REQUIRED_MARGIN - 1e-9),
    }
}

/// `base + step * i`, rounded to three decimals, as a distance.
fn grid_value(base: f64, step: f64, i: u32) -> f32 {
    (((base + step * f64::from(i)) * 1000.0).round() / 1000.0) as f32
}

/// The candidate rules: the first rule, then (a) the dense distance only, then (b) the dense
/// distance or a strong lexical match.
pub fn gate_sweep(diags: &[QuestionDiag]) -> Vec<SweepRow> {
    let mut rows = Vec::new();
    rows.push(sweep_row(
        "any BM25 hit (first rule)",
        &GateRule::AnyLexicalHit { max_distance: 0.85 },
        diags,
    ));
    for step in 0..=30 {
        let d = grid_value(0.60, 0.01, step);
        rows.push(sweep_row(
            "(a) dense",
            &GateRule::Dense { max_distance: d },
            diags,
        ));
    }
    for min_matched in [2usize, 3, 4] {
        for min_coverage in [0.6, 0.75, 0.9] {
            for step in 0..=10 {
                let d = grid_value(0.60, 0.03, step);
                rows.push(sweep_row(
                    "(b) dense or lexical",
                    &GateRule::DenseOrLexical {
                        max_distance: d,
                        min_coverage,
                        min_matched,
                    },
                    diags,
                ));
            }
        }
    }
    rows
}

/// One running header or footer that the chunker no longer sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderLine {
    pub section: String,
    /// The first occurrence as written (it stays in the text).
    pub text: String,
    /// The page of the excerpt of the first occurrence.
    pub first_page: u32,
    /// On how many later pages the line was removed.
    pub removed_pages: usize,
}

/// What the removal of running headers and footers did to the chunks of the production chunk
/// target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderStats {
    /// Distinct running lines, summed over the sections (one document each).
    pub running_lines: usize,
    /// Lines that were removed from a page.
    pub removed_lines: usize,
    /// Chunks with the removal, and without it.
    pub chunks: usize,
    pub chunks_before: usize,
    /// Chunks with a running header in the heading path, without the removal, and with it. What is
    /// left after the removal is the first occurrence, which stays on purpose (it is the real
    /// heading of the section).
    pub chunks_with_header_before: usize,
    pub chunks_with_header_after: usize,
    /// Chunks with a heading that is a part of a running phrase (a fragment: something is left when
    /// a rule removes too little), before and after. After the removal it must be 0.
    pub chunks_with_fragment_before: usize,
    pub chunks_with_fragment_after: usize,
    pub lines: Vec<HeaderLine>,
}

impl HeaderStats {
    /// Adds the counts of another corpus.
    pub fn add(&mut self, other: &HeaderStats) {
        self.running_lines += other.running_lines;
        self.removed_lines += other.removed_lines;
        self.chunks += other.chunks;
        self.chunks_before += other.chunks_before;
        self.chunks_with_header_before += other.chunks_with_header_before;
        self.chunks_with_header_after += other.chunks_with_header_after;
        self.chunks_with_fragment_before += other.chunks_with_fragment_before;
        self.chunks_with_fragment_after += other.chunks_with_fragment_after;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub cache_kind: CacheKind,
    pub embedding_model: String,
    pub corpus_sha256: String,
    pub questions: usize,
    pub covered: usize,
    pub not_covered: usize,
    pub chunks: usize,
    pub dense: Metrics,
    pub bm25: Metrics,
    pub hybrid: Metrics,
    pub hybrid_by_kind: BTreeMap<String, KindMetrics>,
    /// The gate split by group of questions.
    pub gate_by_group: Vec<GateRow>,
    /// The settings of this run, in words (gate rule, fusion, chunk size, mode).
    pub settings: String,
    /// The same three setups when the whole PDF is one document (`None` in that mode itself).
    pub cap_off: Option<ModeMetrics>,
    pub misses: Vec<Miss>,
    pub latency: Option<Latency>,
    /// What the gate read for every question.
    pub diagnostics: Vec<QuestionDiag>,
    /// Every candidate gate rule applied to the diagnostics.
    pub gate_sweep: Vec<SweepRow>,
    /// What the removal of running headers and footers changed.
    pub headers: Option<HeaderStats>,
}

/// The report of one corpus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusReport {
    pub id: String,
    pub report: Report,
}

/// The reports of all corpora, and one report with their questions pooled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuiteReport {
    pub corpora: Vec<CorpusReport>,
    pub combined: Report,
}

impl SuiteReport {
    pub fn to_json(&self) -> Result<String, Error> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| Error::Run(e.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    /// The combined report first (the gate sweep is only in this one), then each corpus.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Retrieval evaluation\n\n");
        let ids: Vec<&str> = self.corpora.iter().map(|c| c.id.as_str()).collect();
        let _ = writeln!(
            out,
            "{} corpus{}: {}.\n",
            ids.len(),
            if ids.len() == 1 { "" } else { "es" },
            ids.join(", ")
        );
        if self.corpora.len() > 1 {
            out.push_str(
                &self
                    .combined
                    .markdown_titled("## Combined (all corpora pooled)", true),
            );
            out.push('\n');
        }
        for corpus in &self.corpora {
            out.push_str(&corpus.report.markdown_titled(
                &format!("## Corpus: {}", corpus.id),
                self.corpora.len() == 1,
            ));
            out.push('\n');
        }
        out
    }
}

impl Report {
    /// One line about where the gate limit sits: the farthest covered question, and the nearest and
    /// farthest questions that the course does not cover.
    fn write_gate_distances(&self, out: &mut String) {
        let range = |keep: &dyn Fn(&QuestionDiag) -> bool| {
            let mut found: Vec<(f64, &str)> = self
                .diagnostics
                .iter()
                .filter(|d| keep(d))
                .filter_map(|d| d.best_dense_distance.map(|v| (v, d.id.as_str())))
                .collect();
            found.sort_by(|a, b| a.0.total_cmp(&b.0));
            found
        };
        let covered = range(&|d| d.is_covered());
        let not_covered = range(&|d| d.kind == Kind::NotCovered.name());
        let off_topic = range(&|d| d.kind == Kind::OffTopic.name());
        let (Some(worst), Some(nc_near), Some(ot_near)) =
            (covered.last(), not_covered.first(), off_topic.first())
        else {
            return;
        };
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Best dense distance: the farthest covered question is {:.4} (`{}`). The nearest not_covered question is {:.4} (`{}`), the nearest off_topic question is {:.4} (`{}`).",
            worst.0, worst.1, nc_near.0, nc_near.1, ot_near.0, ot_near.1
        );
    }

    pub fn to_json(&self) -> Result<String, Error> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| Error::Run(e.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    pub fn to_markdown(&self) -> String {
        self.markdown_titled("# Retrieval evaluation", true)
    }

    /// The report as Markdown, under a title line. `sweep` adds the gate sweep table.
    pub fn markdown_titled(&self, title: &str, sweep: bool) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{title}");
        let _ = writeln!(out);
        if self.cache_kind == CacheKind::Dev {
            let _ = writeln!(
                out,
                "**DEV CACHE: the pages are the PDF text layer and the embeddings are fake. These numbers only prove that the harness runs. Never use them as a baseline.**"
            );
            let _ = writeln!(out);
        }
        let _ = writeln!(
            out,
            "{} questions ({} covered, {} not covered), {} chunks, embedding model `{}`.",
            self.questions, self.covered, self.not_covered, self.chunks, self.embedding_model
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "| Metric | Dense only | BM25 only | Hybrid |");
        let _ = writeln!(out, "|---|---|---|---|");
        let rows: [(&str, Getter); 8] = [
            ("Recall@1", |m| m.recall_at_1),
            ("Recall@5", |m| m.recall_at_5),
            ("Recall@8", |m| m.recall_at_8),
            ("nDCG@8", |m| m.ndcg_at_8),
            ("MRR", |m| m.mrr),
            ("Gate accuracy", |m| m.gate_accuracy),
            ("False-gate rate", |m| m.false_gate_rate),
            ("Missed-gate rate", |m| m.missed_gate_rate),
        ];
        for (name, get) in rows {
            let _ = writeln!(
                out,
                "| {name} | {:.4} | {:.4} | {:.4} |",
                get(&self.dense),
                get(&self.bm25),
                get(&self.hybrid)
            );
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "Hybrid by kind:");
        let _ = writeln!(out);
        let _ = writeln!(out, "| Kind | Questions | Recall@8 | MRR | Gated |");
        let _ = writeln!(out, "|---|---|---|---|---|");
        for (kind, m) in &self.hybrid_by_kind {
            let _ = writeln!(
                out,
                "| {kind} | {} | {:.4} | {:.4} | {:.4} |",
                m.questions, m.recall_at_8, m.mrr, m.gated_rate
            );
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "Settings: {}.", self.settings);
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Gate by group (covered: false-gate rate; the others: missed-gate rate; lower is better):"
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "| Group | Questions | Dense only | BM25 only | Hybrid |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|");
        for row in &self.gate_by_group {
            let _ = writeln!(
                out,
                "| {} | {} | {:.4} | {:.4} | {:.4} |",
                row.group, row.questions, row.dense, row.bm25, row.hybrid
            );
        }
        if let Some(single) = &self.cap_off {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Cap-off mode (every chunk has the same document ID, so the per-document cap never skips a chunk):"
            );
            let _ = writeln!(out);
            let _ = writeln!(out, "| Metric | Dense only | BM25 only | Hybrid |");
            let _ = writeln!(out, "|---|---|---|---|");
            let rows: [(&str, Getter); 5] = [
                ("Recall@1", |m| m.recall_at_1),
                ("Recall@5", |m| m.recall_at_5),
                ("Recall@8", |m| m.recall_at_8),
                ("nDCG@8", |m| m.ndcg_at_8),
                ("MRR", |m| m.mrr),
            ];
            for (name, get) in rows {
                let _ = writeln!(
                    out,
                    "| {name} | {:.4} | {:.4} | {:.4} |",
                    get(&single.dense),
                    get(&single.bm25),
                    get(&single.hybrid)
                );
            }
        }
        if let Some(latency) = &self.latency {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Query embedding latency at the last live refresh: p50 {} ms, p95 {} ms ({} calls).",
                latency.query_embedding_ms_p50, latency.query_embedding_ms_p95, latency.samples
            );
        }
        self.write_gate_distances(&mut out);
        if let Some(h) = &self.headers {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Running headers and footers: {} lines found, removed {} times. Chunks with a running header in the heading path: {} of {} before the removal, {} of {} after it (what is left is the first occurrence of each line, which stays). Chunks with a fragment of a running phrase in the heading path: {} before, {} after.",
                h.running_lines,
                h.removed_lines,
                h.chunks_with_header_before,
                h.chunks_before,
                h.chunks_with_header_after,
                h.chunks,
                h.chunks_with_fragment_before,
                h.chunks_with_fragment_after
            );
        }
        if sweep && !self.gate_sweep.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Gate sweep. False-gate rate is on covered questions. Missed-gate rates are on not_covered, off_topic, and both. A rule passes when it has no false gate and a margin of at least {REQUIRED_MARGIN} in distance (margin: the limit minus the worst covered distance that the lexical part does not rescue)."
            );
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "| Rule | Max distance | Min coverage | Min matched | False gates | False-gate rate | Missed not_covered | Missed off_topic | Missed both | Gate accuracy | Margin | Passes |"
            );
            let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|");
            for row in &self.gate_sweep {
                let _ = writeln!(
                    out,
                    "| {} | {:.2} | {} | {} | {} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {} | {} |",
                    row.rule,
                    row.max_distance,
                    row.min_coverage
                        .map_or("-".to_owned(), |c| format!("{c:.2}")),
                    row.min_matched.map_or("-".to_owned(), |m| m.to_string()),
                    row.false_gates,
                    row.false_gate_rate,
                    row.missed_not_covered,
                    row.missed_off_topic,
                    row.missed_combined,
                    row.gate_accuracy,
                    row.margin.map_or("-".to_owned(), |m| format!("{m:.4}")),
                    if row.passes { "yes" } else { "no" }
                );
            }
        }
        if !self.misses.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "Hybrid misses (no relevant chunk in the top 8):");
            let _ = writeln!(out);
            for miss in &self.misses {
                let _ = writeln!(
                    out,
                    "- `{}` ({}): {} Labeled pages {:?}. {}",
                    miss.id,
                    miss.kind,
                    miss.question,
                    miss.relevant_pages,
                    if miss.gated {
                        "Cut off by the gate.".to_owned()
                    } else {
                        format!("Returned pages: {}.", miss.returned.join(", "))
                    }
                );
            }
        }
        out
    }
}

/// The hybrid metrics of one corpus (or of all pooled) in the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeBaseline {
    pub hybrid: Metrics,
    /// The hybrid metrics when the per-document cap is off (the whole PDF is one document).
    pub cap_off: Metrics,
}

impl ScopeBaseline {
    /// The baseline of a report. It needs the cap-off run, so the report must not be a cap-off run.
    pub fn from_report(report: &Report) -> Result<Self, Error> {
        let cap_off = report.cap_off.as_ref().ok_or_else(|| {
            Error::Run(
                "the baseline needs the cap-off metrics: do not run it with --cap-off".into(),
            )
        })?;
        Ok(Self {
            hybrid: report.hybrid,
            cap_off: cap_off.hybrid,
        })
    }
}

/// What `evals/baseline.json` holds: the hybrid metrics of the committed code, for each corpus and
/// for all corpora pooled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Baseline {
    pub cache_kind: CacheKind,
    pub embedding_model: String,
    pub corpora: BTreeMap<String, ScopeBaseline>,
    pub combined: ScopeBaseline,
}

impl Baseline {
    /// The baseline of a run over all corpora.
    pub fn from_suite(suite: &SuiteReport) -> Result<Self, Error> {
        let mut corpora = BTreeMap::new();
        for c in &suite.corpora {
            corpora.insert(c.id.clone(), ScopeBaseline::from_report(&c.report)?);
        }
        Ok(Self {
            cache_kind: suite.combined.cache_kind,
            embedding_model: suite.combined.embedding_model.clone(),
            corpora,
            combined: ScopeBaseline::from_report(&suite.combined)?,
        })
    }

    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Run(format!(
                    "there is no baseline at {}. Run `openbook-eval offline --write-baseline` on a live cache.",
                    path.display()
                ))
            } else {
                Error::io(path, &e)
            }
        })?;
        serde_json::from_str(&text).map_err(|e| Error::Run(format!("{}: {e}", path.display())))
    }

    pub fn to_json(&self) -> Result<String, Error> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| Error::Run(e.to_string()))?;
        text.push('\n');
        Ok(text)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    /// The corpus of the metric, or "combined". Empty when one report is checked alone.
    pub scope: String,
    /// The mode of the metric: empty for the normal run, "cap off" for the cap-off run.
    pub mode: &'static str,
    pub metric: &'static str,
    pub baseline: f64,
    pub current: f64,
    /// The lowest value that the check accepts.
    pub lowest_allowed: f64,
}

impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scope = if self.scope.is_empty() {
            String::new()
        } else {
            format!("{}, ", self.scope)
        };
        let mode = if self.mode.is_empty() {
            String::new()
        } else {
            format!("{}, ", self.mode)
        };
        write!(
            f,
            "{scope}{mode}{}: {:.4}, the baseline is {:.4} and the limit is {:.4}",
            self.metric, self.current, self.baseline, self.lowest_allowed
        )
    }
}

/// Compares the hybrid metrics with the baseline. A ranking metric may fall by `TOLERANCE`. Gate
/// accuracy may not fall at all, and the false-gate rate may not rise at all.
pub fn check(baseline: &Metrics, current: &Metrics) -> Vec<Regression> {
    let ranking: [(&'static str, f64, f64); 5] = [
        ("Recall@1", baseline.recall_at_1, current.recall_at_1),
        ("Recall@5", baseline.recall_at_5, current.recall_at_5),
        ("Recall@8", baseline.recall_at_8, current.recall_at_8),
        ("nDCG@8", baseline.ndcg_at_8, current.ndcg_at_8),
        ("MRR", baseline.mrr, current.mrr),
    ];
    let mut found = Vec::new();
    for (metric, base, now) in ranking {
        let lowest = base - TOLERANCE;
        if now < lowest - EPSILON {
            found.push(Regression {
                scope: String::new(),
                mode: "",
                metric,
                baseline: base,
                current: now,
                lowest_allowed: lowest,
            });
        }
    }
    if current.gate_accuracy < baseline.gate_accuracy - EPSILON {
        found.push(Regression {
            scope: String::new(),
            mode: "",
            metric: "Gate accuracy",
            baseline: baseline.gate_accuracy,
            current: current.gate_accuracy,
            lowest_allowed: baseline.gate_accuracy,
        });
    }
    // A covered question that the gate cuts off is the worst error for a student: no tolerance.
    if current.false_gate_rate > baseline.false_gate_rate + EPSILON {
        found.push(Regression {
            scope: String::new(),
            mode: "",
            metric: "False-gate rate",
            baseline: baseline.false_gate_rate,
            current: current.false_gate_rate,
            lowest_allowed: baseline.false_gate_rate,
        });
    }
    found
}

/// Checks both runs of a report against the baseline of its scope: the normal run and the cap-off
/// run, with the same tolerance.
pub fn check_report(scope: &str, baseline: &ScopeBaseline, report: &Report) -> Vec<Regression> {
    let mut found = check(&baseline.hybrid, &report.hybrid);
    if let Some(cap_off) = &report.cap_off {
        for mut regression in check(&baseline.cap_off, &cap_off.hybrid) {
            regression.mode = "cap off";
            found.push(regression);
        }
    }
    for regression in &mut found {
        regression.scope = scope.to_owned();
    }
    found
}

/// Checks every corpus and the pooled report against the baseline. The baseline and the run must
/// have the same corpora.
pub fn check_suite(baseline: &Baseline, suite: &SuiteReport) -> Result<Vec<Regression>, Error> {
    let run_ids: Vec<&str> = suite.corpora.iter().map(|c| c.id.as_str()).collect();
    let base_ids: Vec<&str> = baseline.corpora.keys().map(String::as_str).collect();
    if run_ids != base_ids {
        return Err(Error::Run(format!(
            "the baseline has the corpora [{}] and the run has [{}]. Run `openbook-eval offline --write-baseline` in the commit that adds or removes a corpus.",
            base_ids.join(", "),
            run_ids.join(", ")
        )));
    }
    let mut found = Vec::new();
    for corpus in &suite.corpora {
        found.extend(check_report(
            &corpus.id,
            &baseline.corpora[&corpus.id],
            &corpus.report,
        ));
    }
    found.extend(check_report(
        "combined",
        &baseline.combined,
        &suite.combined,
    ));
    Ok(found)
}

/// True when a metric of any scope rose by more than the tolerance, so the baseline should be
/// updated.
pub fn suite_improved(baseline: &Baseline, suite: &SuiteReport) -> bool {
    let scope = |b: &ScopeBaseline, r: &Report| {
        improved(&b.hybrid, &r.hybrid)
            || r.cap_off
                .as_ref()
                .is_some_and(|c| improved(&b.cap_off, &c.hybrid))
    };
    suite.corpora.iter().any(|c| {
        baseline
            .corpora
            .get(&c.id)
            .is_some_and(|b| scope(b, &c.report))
    }) || scope(&baseline.combined, &suite.combined)
}

/// True when a metric rose by more than the tolerance, so the baseline should be updated.
pub fn improved(baseline: &Metrics, current: &Metrics) -> bool {
    current.recall_at_1 > baseline.recall_at_1 + TOLERANCE
        || current.recall_at_5 > baseline.recall_at_5 + TOLERANCE
        || current.recall_at_8 > baseline.recall_at_8 + TOLERANCE
        || current.ndcg_at_8 > baseline.ndcg_at_8 + TOLERANCE
        || current.mrr > baseline.mrr + TOLERANCE
        || current.gate_accuracy > baseline.gate_accuracy + EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics {
            recall_at_1: 0.60,
            recall_at_5: 0.80,
            recall_at_8: 0.90,
            ndcg_at_8: 0.70,
            mrr: 0.60,
            gate_accuracy: 0.95,
            false_gate_rate: 0.02,
            missed_gate_rate: 0.10,
        }
    }

    #[test]
    fn the_same_numbers_pass() {
        assert!(check(&metrics(), &metrics()).is_empty());
    }

    #[test]
    fn a_drop_of_exactly_the_tolerance_passes_and_a_bit_more_fails() {
        let base = metrics();
        let mut now = base;
        now.recall_at_8 = 0.89;
        assert!(check(&base, &now).is_empty(), "0.01 is allowed");
        now.recall_at_8 = 0.8899;
        let found = check(&base, &now);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].metric, "Recall@8");
        assert!(found[0].to_string().contains("0.8899"));
    }

    #[test]
    fn every_ranking_metric_is_checked() {
        let base = metrics();
        let mut now = base;
        now.recall_at_1 -= 0.05;
        now.recall_at_5 -= 0.05;
        now.ndcg_at_8 -= 0.05;
        now.mrr -= 0.05;
        let names: Vec<_> = check(&base, &now).iter().map(|r| r.metric).collect();
        assert_eq!(names, ["Recall@1", "Recall@5", "nDCG@8", "MRR"]);
    }

    #[test]
    fn gate_accuracy_has_no_tolerance() {
        let base = metrics();
        let mut now = base;
        now.gate_accuracy = 0.9444;
        let found = check(&base, &now);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].metric, "Gate accuracy");
        assert_eq!(found[0].lowest_allowed, 0.95);
        now.gate_accuracy = 0.95;
        assert!(check(&base, &now).is_empty());
    }

    #[test]
    fn a_better_result_passes_and_is_reported_as_an_improvement() {
        let base = metrics();
        let mut now = base;
        now.recall_at_8 = 0.95;
        assert!(check(&base, &now).is_empty());
        assert!(improved(&base, &now));
        assert!(!improved(&base, &base));
        let mut small = base;
        small.recall_at_8 += 0.005;
        assert!(!improved(&base, &small), "within the tolerance");
    }

    #[test]
    fn the_baseline_reads_what_it_writes_and_a_missing_one_says_what_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("baseline.json");
        let error = Baseline::load(&path).unwrap_err().to_string();
        assert!(error.contains("--write-baseline"), "{error}");
        let scope = ScopeBaseline {
            hybrid: metrics(),
            cap_off: metrics(),
        };
        let baseline = Baseline {
            cache_kind: CacheKind::Live,
            embedding_model: "amazon.titan-embed-text-v2:0".into(),
            corpora: BTreeMap::from([("algebra".to_owned(), scope)]),
            combined: scope,
        };
        std::fs::write(&path, baseline.to_json().unwrap()).unwrap();
        assert_eq!(Baseline::load(&path).unwrap(), baseline);
    }

    #[test]
    fn the_markdown_has_the_three_columns_and_warns_about_a_dev_cache() {
        let report = Report {
            cache_kind: CacheKind::Dev,
            embedding_model: "fake".into(),
            corpus_sha256: "00".into(),
            questions: 3,
            covered: 2,
            not_covered: 1,
            chunks: 10,
            dense: metrics(),
            bm25: metrics(),
            hybrid: metrics(),
            hybrid_by_kind: BTreeMap::from([(
                "lookup".to_owned(),
                KindMetrics {
                    questions: 2,
                    recall_at_8: 1.0,
                    mrr: 0.5,
                    gated_rate: 0.0,
                },
            )]),
            misses: vec![Miss {
                id: "lookup-01".into(),
                kind: "lookup".into(),
                question: "What?".into(),
                relevant_pages: vec![3],
                gated: false,
                returned: vec!["1".into(), "4-5".into()],
            }],
            gate_by_group: vec![GateRow {
                group: "covered (false-gate rate)".into(),
                questions: 2,
                dense: 0.0,
                bm25: 0.5,
                hybrid: 0.0,
            }],
            settings: "gate: dense distance at most 0.85".into(),
            cap_off: Some(ModeMetrics {
                dense: metrics(),
                bm25: metrics(),
                hybrid: metrics(),
            }),
            latency: None,
            diagnostics: vec![
                diag("lookup-01", "lookup", 0.5, 3, 3),
                diag("not-covered-01", "not_covered", 0.8, 3, 1),
                diag("off-topic-01", "off_topic", 0.9, 1, 0),
            ],
            gate_sweep: vec![],
            headers: Some(HeaderStats {
                running_lines: 2,
                removed_lines: 6,
                chunks: 10,
                chunks_before: 12,
                chunks_with_header_before: 7,
                chunks_with_header_after: 3,
                chunks_with_fragment_before: 2,
                chunks_with_fragment_after: 0,
                lines: vec![],
            }),
        };
        let text = report.to_markdown();
        assert!(text.contains("| Metric | Dense only | BM25 only | Hybrid |"));
        assert!(text.contains("| Recall@8 | 0.9000 | 0.9000 | 0.9000 |"));
        assert!(text.contains("DEV CACHE"));
        assert!(text.contains("| covered (false-gate rate) | 2 | 0.0000 | 0.5000 | 0.0000 |"));
        assert!(text.contains("Cap-off mode"));
        assert!(text.contains("`lookup-01`"));
        assert!(text.contains("Returned pages: 1, 4-5."));
        assert!(text.contains("the farthest covered question is 0.5000 (`lookup-01`)"));
        assert!(text.contains("7 of 12 before the removal, 3 of 10 after it"));
        let json = report.to_json().unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
        assert!(json.contains("\"recallAt8\""));
    }

    fn diag(id: &str, kind: &str, distance: f64, terms: usize, matched: usize) -> QuestionDiag {
        QuestionDiag {
            id: id.into(),
            kind: kind.into(),
            best_dense_distance: Some(distance),
            top_bm25_score: Some(4.0),
            term_coverage: matched as f64 / terms as f64,
            query_terms: terms,
            indexed_terms: terms,
            matched_terms: matched,
            lexical_hits: 5,
            gated: false,
        }
    }

    #[test]
    fn a_sweep_row_counts_false_gates_and_misses_and_the_margin() {
        let diags = vec![
            diag("a", "lookup", 0.55, 4, 3),
            diag("b", "paraphrase", 0.65, 4, 1),
            diag("c", "not_covered", 0.70, 4, 2),
            diag("d", "not_covered", 0.90, 4, 1),
            diag("e", "off_topic", 0.95, 1, 1),
        ];
        let dense = |d| sweep_row("(a)", &GateRule::Dense { max_distance: d }, &diags);
        // 0.64 cuts off b (0.65): one false gate.
        let tight = dense(0.64);
        assert_eq!((tight.false_gates, tight.passes), (1, false));
        // 0.66 keeps both covered questions, but the margin is only 0.01.
        let near = dense(0.66);
        assert_eq!(near.false_gates, 0);
        assert!(!near.passes, "margin {:?}", near.margin);
        assert_eq!(near.margin, Some(0.01));
        // 0.68 has the margin of 0.03 and gates c, d, and e.
        let good = dense(0.68);
        assert!(good.passes);
        assert_eq!(good.missed_combined, 0.0);
        assert_eq!(good.gate_accuracy, 1.0);
        // 0.75 lets c through (0.70): one of the two not_covered questions is missed.
        let loose = dense(0.75);
        assert_eq!(loose.missed_not_covered, 0.5);
        assert_eq!(loose.missed_off_topic, 0.0);
        assert_eq!(loose.gate_accuracy, 0.8);
        // The first rule lets everything through that has a BM25 hit.
        let first = sweep_row(
            "first",
            &GateRule::AnyLexicalHit { max_distance: 0.85 },
            &diags,
        );
        assert_eq!(first.missed_combined, 1.0);
        // The combined rule rescues covered question a by lexical match, so b sets the margin.
        let combined = sweep_row(
            "(b)",
            &GateRule::DenseOrLexical {
                max_distance: 0.66,
                min_coverage: 0.75,
                min_matched: 3,
            },
            &diags,
        );
        assert_eq!(combined.margin, Some(0.01));
        // The sweep has the first rule, then (a), then (b), and every row is in range.
        let sweep = gate_sweep(&diags);
        assert!(sweep[0].rule.starts_with("any BM25"));
        assert!(sweep.iter().any(|r| r.rule.starts_with("(a)")));
        assert!(sweep.iter().any(|r| r.rule.starts_with("(b)")));
        assert!(sweep.iter().all(|r| (0.0..=1.0).contains(&r.gate_accuracy)));
    }

    #[test]
    fn a_rise_of_the_false_gate_rate_fails_by_any_amount() {
        let base = metrics();
        let mut now = base;
        now.false_gate_rate += 0.0132;
        let found = check(&base, &now);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].metric, "False-gate rate");
        now.false_gate_rate = base.false_gate_rate;
        assert!(check(&base, &now).is_empty());
    }

    fn scope(metrics: Metrics) -> ScopeBaseline {
        ScopeBaseline {
            hybrid: metrics,
            cap_off: metrics,
        }
    }

    fn report_with(hybrid: Metrics, cap_off: Metrics) -> Report {
        Report {
            cache_kind: CacheKind::Live,
            embedding_model: "m".into(),
            corpus_sha256: "0".into(),
            questions: 1,
            covered: 1,
            not_covered: 0,
            chunks: 1,
            dense: metrics(),
            bm25: metrics(),
            hybrid,
            hybrid_by_kind: BTreeMap::new(),
            gate_by_group: vec![],
            settings: String::new(),
            cap_off: Some(ModeMetrics {
                dense: metrics(),
                bm25: metrics(),
                hybrid: cap_off,
            }),
            misses: vec![],
            latency: None,
            diagnostics: vec![],
            gate_sweep: vec![],
            headers: None,
        }
    }

    #[test]
    fn the_cap_off_run_is_checked_with_the_same_tolerance() {
        let baseline = scope(metrics());
        let mut worse = metrics();
        worse.recall_at_8 -= 0.05;
        assert!(check_report("algebra", &baseline, &report_with(metrics(), metrics())).is_empty());
        let found = check_report("algebra", &baseline, &report_with(metrics(), worse));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mode, "cap off");
        assert!(
            found[0]
                .to_string()
                .starts_with("algebra, cap off, Recall@8"),
            "{}",
            found[0]
        );
    }

    fn suite(corpora: &[(&str, Metrics)], combined: Metrics) -> SuiteReport {
        SuiteReport {
            corpora: corpora
                .iter()
                .map(|(id, m)| CorpusReport {
                    id: (*id).to_owned(),
                    report: report_with(*m, *m),
                })
                .collect(),
            combined: report_with(combined, combined),
        }
    }

    #[test]
    fn every_corpus_and_the_pooled_report_are_checked() {
        let mut worse = metrics();
        worse.ndcg_at_8 -= 0.05;
        let base = suite(&[("algebra", metrics()), ("history", metrics())], metrics());
        let baseline = Baseline::from_suite(&base).unwrap();
        assert_eq!(baseline.corpora.len(), 2);
        assert!(check_suite(&baseline, &base).unwrap().is_empty());
        assert!(!suite_improved(&baseline, &base));

        // One corpus gets worse, and the pooled numbers hold: the check still fails.
        let one = suite(&[("algebra", metrics()), ("history", worse)], metrics());
        let found = check_suite(&baseline, &one).unwrap();
        assert!(found.iter().all(|r| r.scope == "history"), "{found:?}");
        assert_eq!(found.len(), 2, "the normal run and the cap-off run");
        // The pooled report gets worse alone.
        let pooled = suite(&[("algebra", metrics()), ("history", metrics())], worse);
        let found = check_suite(&baseline, &pooled).unwrap();
        assert!(found.iter().all(|r| r.scope == "combined"));
        assert!(found[0].to_string().starts_with("combined, nDCG@8"));
    }

    #[test]
    fn a_baseline_for_other_corpora_is_an_error_that_says_what_to_do() {
        let base = suite(&[("algebra", metrics())], metrics());
        let baseline = Baseline::from_suite(&base).unwrap();
        let more = suite(&[("algebra", metrics()), ("history", metrics())], metrics());
        let error = check_suite(&baseline, &more).unwrap_err().to_string();
        assert!(
            error.contains("[algebra]") && error.contains("[algebra, history]"),
            "{error}"
        );
        assert!(error.contains("--write-baseline"), "{error}");
    }

    #[test]
    fn a_cap_off_run_cannot_give_a_baseline() {
        let mut one = suite(&[("algebra", metrics())], metrics());
        one.corpora[0].report.cap_off = None;
        assert!(Baseline::from_suite(&one).is_err());
    }

    #[test]
    fn the_suite_markdown_shows_the_pooled_report_first_and_each_corpus() {
        let two = suite(&[("algebra", metrics()), ("history", metrics())], metrics());
        let text = two.to_markdown();
        let combined = text.find("## Combined (all corpora pooled)").unwrap();
        let algebra = text.find("## Corpus: algebra").unwrap();
        let history = text.find("## Corpus: history").unwrap();
        assert!(combined < algebra && algebra < history, "{text}");
        let one = suite(&[("algebra", metrics())], metrics());
        assert!(!one.to_markdown().contains("Combined"));
        let back: SuiteReport = serde_json::from_str(&two.to_json().unwrap()).unwrap();
        assert_eq!(back, two);
    }
}
