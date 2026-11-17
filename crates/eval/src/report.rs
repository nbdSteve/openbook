//! The report (a Markdown table and a JSON file), the baseline, and the regression check.

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
    pub single_document: Option<ModeMetrics>,
    pub misses: Vec<Miss>,
    pub latency: Option<Latency>,
    /// What the gate read for every question.
    pub diagnostics: Vec<QuestionDiag>,
    /// Every candidate gate rule applied to the diagnostics.
    pub gate_sweep: Vec<SweepRow>,
}

impl Report {
    pub fn to_json(&self) -> Result<String, Error> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| Error::Run(e.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Retrieval evaluation");
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
        let rows: [(&str, Getter); 7] = [
            ("Recall@5", |m| m.recall_at_5),
            ("Recall@8", |m| m.recall_at_8),
            ("nDCG@10", |m| m.ndcg_at_10),
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
        if let Some(single) = &self.single_document {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Single-document mode (the whole PDF is one document, so the per-document cap sees one):"
            );
            let _ = writeln!(out);
            let _ = writeln!(out, "| Metric | Dense only | BM25 only | Hybrid |");
            let _ = writeln!(out, "|---|---|---|---|");
            let rows: [(&str, Getter); 4] = [
                ("Recall@5", |m| m.recall_at_5),
                ("Recall@8", |m| m.recall_at_8),
                ("nDCG@10", |m| m.ndcg_at_10),
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
        if !self.gate_sweep.is_empty() {
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

/// What `evals/baseline.json` holds: the hybrid metrics of the committed code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Baseline {
    pub cache_kind: CacheKind,
    pub embedding_model: String,
    pub hybrid: Metrics,
}

impl Baseline {
    pub fn from_report(report: &Report) -> Self {
        Self {
            cache_kind: report.cache_kind,
            embedding_model: report.embedding_model.clone(),
            hybrid: report.hybrid,
        }
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
    pub metric: &'static str,
    pub baseline: f64,
    pub current: f64,
    /// The lowest value that the check accepts.
    pub lowest_allowed: f64,
}

impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {:.4}, the baseline is {:.4} and the lowest allowed is {:.4}",
            self.metric, self.current, self.baseline, self.lowest_allowed
        )
    }
}

/// Compares the hybrid metrics with the baseline. A ranking metric may fall by `TOLERANCE`. Gate
/// accuracy may not fall at all.
pub fn check(baseline: &Metrics, current: &Metrics) -> Vec<Regression> {
    let ranking: [(&'static str, f64, f64); 4] = [
        ("Recall@5", baseline.recall_at_5, current.recall_at_5),
        ("Recall@8", baseline.recall_at_8, current.recall_at_8),
        ("nDCG@10", baseline.ndcg_at_10, current.ndcg_at_10),
        ("MRR", baseline.mrr, current.mrr),
    ];
    let mut found = Vec::new();
    for (metric, base, now) in ranking {
        let lowest = base - TOLERANCE;
        if now < lowest - EPSILON {
            found.push(Regression {
                metric,
                baseline: base,
                current: now,
                lowest_allowed: lowest,
            });
        }
    }
    if current.gate_accuracy < baseline.gate_accuracy - EPSILON {
        found.push(Regression {
            metric: "Gate accuracy",
            baseline: baseline.gate_accuracy,
            current: current.gate_accuracy,
            lowest_allowed: baseline.gate_accuracy,
        });
    }
    found
}

/// True when a metric rose by more than the tolerance, so the baseline should be updated.
pub fn improved(baseline: &Metrics, current: &Metrics) -> bool {
    current.recall_at_5 > baseline.recall_at_5 + TOLERANCE
        || current.recall_at_8 > baseline.recall_at_8 + TOLERANCE
        || current.ndcg_at_10 > baseline.ndcg_at_10 + TOLERANCE
        || current.mrr > baseline.mrr + TOLERANCE
        || current.gate_accuracy > baseline.gate_accuracy + EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics {
            recall_at_5: 0.80,
            recall_at_8: 0.90,
            ndcg_at_10: 0.70,
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
        now.recall_at_5 -= 0.05;
        now.ndcg_at_10 -= 0.05;
        now.mrr -= 0.05;
        let names: Vec<_> = check(&base, &now).iter().map(|r| r.metric).collect();
        assert_eq!(names, ["Recall@5", "nDCG@10", "MRR"]);
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
        let baseline = Baseline {
            cache_kind: CacheKind::Live,
            embedding_model: "amazon.titan-embed-text-v2:0".into(),
            hybrid: metrics(),
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
            single_document: Some(ModeMetrics {
                dense: metrics(),
                bm25: metrics(),
                hybrid: metrics(),
            }),
            latency: None,
            diagnostics: vec![diag("lookup-01", "lookup", 0.5, 3, 3)],
            gate_sweep: vec![],
        };
        let text = report.to_markdown();
        assert!(text.contains("| Metric | Dense only | BM25 only | Hybrid |"));
        assert!(text.contains("| Recall@8 | 0.9000 | 0.9000 | 0.9000 |"));
        assert!(text.contains("DEV CACHE"));
        assert!(text.contains("| covered (false-gate rate) | 2 | 0.0000 | 0.5000 | 0.0000 |"));
        assert!(text.contains("Single-document mode"));
        assert!(text.contains("`lookup-01`"));
        assert!(text.contains("Returned pages: 1, 4-5."));
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
}
