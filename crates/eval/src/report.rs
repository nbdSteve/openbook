//! The report (a Markdown table and a JSON file), the baseline, and the regression check.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::cache::{CacheKind, Latency};
use crate::metrics::Metrics;

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
    pub misses: Vec<Miss>,
    pub latency: Option<Latency>,
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
        if let Some(latency) = &self.latency {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Query embedding latency at the last live refresh: p50 {} ms, p95 {} ms ({} calls).",
                latency.query_embedding_ms_p50, latency.query_embedding_ms_p95, latency.samples
            );
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
            latency: None,
        };
        let text = report.to_markdown();
        assert!(text.contains("| Metric | Dense only | BM25 only | Hybrid |"));
        assert!(text.contains("| Recall@8 | 0.9000 | 0.9000 | 0.9000 |"));
        assert!(text.contains("DEV CACHE"));
        assert!(text.contains("`lookup-01`"));
        assert!(text.contains("Returned pages: 1, 4-5."));
        let json = report.to_json().unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
        assert!(json.contains("\"recallAt8\""));
    }
}
