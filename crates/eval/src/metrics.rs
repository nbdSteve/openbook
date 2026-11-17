//! The metrics of `docs/evaluation.md`: Recall@5, Recall@8, nDCG@10, MRR, and gate accuracy.
//!
//! The functions here are pure. A ranked list is a list of gains, best rank first: 2 for a chunk
//! that holds a labeled page, 1 for a chunk that holds only a page next to a labeled page, 0 for
//! any other chunk.

use serde::{Deserialize, Serialize};

pub const RECALL_SMALL: usize = 5;
pub const RECALL_LARGE: usize = 8;
pub const NDCG_DEPTH: usize = 10;

/// The pages (first and last, 1-based, inclusive) that a chunk covers.
pub type Span = (u32, u32);

fn holds(span: Span, page: u32) -> bool {
    span.0 <= page && page <= span.1
}

/// A chunk is relevant when its page range includes a labeled page.
pub fn is_relevant(span: Span, labeled: &[u32]) -> bool {
    labeled.iter().any(|p| holds(span, *p))
}

/// The gain of a chunk: 2 when it is relevant, 1 when it includes a page next to a labeled page,
/// and 0 otherwise.
pub fn gain(span: Span, labeled: &[u32]) -> u8 {
    if is_relevant(span, labeled) {
        2
    } else if labeled
        .iter()
        .any(|p| (*p > 1 && holds(span, p - 1)) || holds(span, p + 1))
    {
        1
    } else {
        0
    }
}

/// True when one of the first `k` results is relevant.
pub fn hit_at(ranked: &[u8], k: usize) -> bool {
    ranked.iter().take(k).any(|g| *g == 2)
}

/// 1 / rank of the first relevant result, or 0.
pub fn reciprocal_rank(ranked: &[u8]) -> f64 {
    ranked
        .iter()
        .position(|g| *g == 2)
        .map_or(0.0, |i| 1.0 / (i as f64 + 1.0))
}

fn dcg(gains: impl Iterator<Item = u8>) -> f64 {
    gains
        .enumerate()
        .map(|(i, g)| f64::from(g) / (i as f64 + 2.0).log2())
        .sum()
}

/// nDCG at `k`. `all` holds the gain of every chunk of the corpus (in any order): it gives the
/// ideal ranking. The result is 0 when no chunk has a gain.
pub fn ndcg_at(ranked: &[u8], all: &[u8], k: usize) -> f64 {
    let mut ideal = all.to_vec();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    let best = dcg(ideal.into_iter().take(k));
    if best == 0.0 {
        return 0.0;
    }
    dcg(ranked.iter().copied().take(k)) / best
}

/// What one retriever setup did for one question.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub covered: bool,
    /// True when the gate gave no sources.
    pub gated: bool,
    /// The gains of a ten-deep ranking, best rank first. nDCG@10 reads it, so it has ten places.
    pub ranked: Vec<u8>,
    /// The gains of the list that the API returns (`final_sources` long). Recall and MRR read it.
    pub returned: Vec<u8>,
    /// The gain of every chunk of the corpus for this question.
    pub all: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metrics {
    pub recall_at_5: f64,
    pub recall_at_8: f64,
    pub ndcg_at_10: f64,
    pub mrr: f64,
    /// The share of all questions where the gate was right: a covered question has sources and a
    /// question that is not covered has none.
    pub gate_accuracy: f64,
    /// The share of covered questions that the gate cut off.
    pub false_gate_rate: f64,
    /// The share of not covered questions that got sources.
    pub missed_gate_rate: f64,
}

/// Four decimals, so a report has no float noise.
pub fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn share(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    }
}

/// The metrics of a set of outcomes. The ranking metrics use the covered questions only.
pub fn aggregate(outcomes: &[Outcome]) -> Metrics {
    let covered: Vec<&Outcome> = outcomes.iter().filter(|o| o.covered).collect();
    let not_covered = outcomes.len() - covered.len();
    let mean = |f: &dyn Fn(&Outcome) -> f64| -> f64 {
        if covered.is_empty() {
            0.0
        } else {
            covered.iter().map(|o| f(o)).sum::<f64>() / covered.len() as f64
        }
    };
    let false_gated = covered.iter().filter(|o| o.gated).count();
    let missed = outcomes.iter().filter(|o| !o.covered && !o.gated).count();
    let right = outcomes.len() - false_gated - missed;
    Metrics {
        recall_at_5: round4(mean(&|o| {
            f64::from(u8::from(hit_at(&o.returned, RECALL_SMALL)))
        })),
        recall_at_8: round4(mean(&|o| {
            f64::from(u8::from(hit_at(&o.returned, RECALL_LARGE)))
        })),
        ndcg_at_10: round4(mean(&|o| ndcg_at(&o.ranked, &o.all, NDCG_DEPTH))),
        mrr: round4(mean(&|o| reciprocal_rank(&o.returned))),
        gate_accuracy: round4(share(right, outcomes.len())),
        false_gate_rate: round4(share(false_gated, covered.len())),
        missed_gate_rate: round4(share(missed, not_covered)),
    }
}

/// The share of covered questions that have a relevant chunk anywhere in the returned list (the
/// recall of the whole list that the model gets, whatever its length).
pub fn recall_of_returned(outcomes: &[Outcome]) -> f64 {
    let covered: Vec<&Outcome> = outcomes.iter().filter(|o| o.covered).collect();
    let hits = covered.iter().filter(|o| o.returned.contains(&2)).count();
    round4(share(hits, covered.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_chunk_is_relevant_when_its_pages_include_a_labeled_page() {
        assert!(is_relevant((3, 3), &[3]));
        assert!(is_relevant((2, 4), &[4]));
        assert!(is_relevant((4, 6), &[1, 4]));
        assert!(!is_relevant((4, 6), &[3, 7]));
        assert!(!is_relevant((1, 2), &[]));
    }

    #[test]
    fn the_gain_is_two_for_the_page_one_for_a_neighbor_and_zero_otherwise() {
        assert_eq!(gain((5, 5), &[5]), 2);
        assert_eq!(
            gain((5, 6), &[6]),
            2,
            "a chunk across pages counts for both"
        );
        assert_eq!(gain((4, 4), &[5]), 1);
        assert_eq!(gain((6, 6), &[5]), 1);
        assert_eq!(gain((3, 4), &[5]), 1, "a range that ends next to the page");
        assert_eq!(gain((7, 7), &[5]), 0);
        assert_eq!(gain((1, 1), &[1]), 2);
        assert_eq!(gain((2, 2), &[1]), 1);
        // Page 1 has no page 0 next to it.
        assert_eq!(gain((9, 9), &[1]), 0);
        // The relevant page wins over a neighbor.
        assert_eq!(gain((4, 5), &[4, 5]), 2);
    }

    #[test]
    fn recall_looks_at_the_first_k_results() {
        let ranked = [0, 1, 0, 0, 0, 2, 0, 0];
        assert!(!hit_at(&ranked, 5));
        assert!(hit_at(&ranked, 6));
        assert!(hit_at(&ranked, 8));
        assert!(!hit_at(&[], 8));
        assert!(!hit_at(&[1, 1, 1], 8), "a neighbor is not a hit");
    }

    #[test]
    fn mrr_is_one_over_the_rank_of_the_first_relevant_result() {
        assert!(close(reciprocal_rank(&[2, 0]), 1.0));
        assert!(close(reciprocal_rank(&[0, 2, 2]), 0.5));
        assert!(close(reciprocal_rank(&[1, 0, 2]), 1.0 / 3.0));
        assert!(close(reciprocal_rank(&[1, 0]), 0.0));
        assert!(close(reciprocal_rank(&[]), 0.0));
    }

    #[test]
    fn ndcg_is_one_for_the_ideal_ranking_and_less_for_a_worse_one() {
        // Two chunks have gains 2 and 1, the rest 0.
        let all = [0, 2, 0, 1, 0];
        assert!(close(ndcg_at(&[2, 1, 0], &all, 10), 1.0));
        // DCG of [1, 2] is 1/log2(2) + 2/log2(3); the ideal DCG is 2/log2(2) + 1/log2(3).
        let worse = 1.0 + 2.0 / 3f64.log2();
        let ideal = 2.0 + 1.0 / 3f64.log2();
        assert!(close(ndcg_at(&[1, 2], &all, 10), worse / ideal));
        // The depth cuts the list: with k = 1 only the first result counts.
        assert!(close(ndcg_at(&[0, 2], &all, 1), 0.0));
        assert!(close(ndcg_at(&[2, 0], &all, 1), 1.0));
        assert!(close(ndcg_at(&[], &all, 10), 0.0));
        // No chunk has a gain: nothing to rank.
        assert!(close(ndcg_at(&[0, 0], &[0, 0], 10), 0.0));
    }

    fn outcome(covered: bool, gated: bool, ranked: &[u8]) -> Outcome {
        Outcome {
            covered,
            gated,
            ranked: ranked.to_vec(),
            returned: ranked.to_vec(),
            all: vec![2, 1, 0, 0],
        }
    }

    #[test]
    fn the_aggregate_averages_the_covered_questions() {
        let outcomes = [
            outcome(true, false, &[2, 0]),             // rank 1
            outcome(true, false, &[0, 0, 0, 2]),       // rank 4
            outcome(true, false, &[0; 6]),             // no hit
            outcome(true, false, &[0, 0, 0, 0, 0, 2]), // rank 6: in the top 8, not in the top 5
        ];
        let m = aggregate(&outcomes);
        assert!(close(m.recall_at_5, 0.5));
        assert!(close(m.recall_at_8, 0.75));
        assert!(close(m.mrr, round4((1.0 + 0.25 + 0.0 + 1.0 / 6.0) / 4.0)));
        assert!(m.ndcg_at_10 > 0.0 && m.ndcg_at_10 < 1.0);
        assert!(close(m.gate_accuracy, 1.0));
    }

    #[test]
    fn the_gate_counts_false_gates_and_missed_gates_apart() {
        let outcomes = [
            outcome(true, false, &[2]),  // right
            outcome(true, true, &[]),    // false gate
            outcome(false, true, &[]),   // right
            outcome(false, false, &[0]), // missed gate
            outcome(false, false, &[0]), // missed gate
        ];
        let m = aggregate(&outcomes);
        assert!(close(m.false_gate_rate, 0.5));
        assert!(close(m.missed_gate_rate, round4(2.0 / 3.0)));
        assert!(close(m.gate_accuracy, 0.4));
        // A gated covered question has no results, so it counts as a miss for recall.
        assert!(close(m.recall_at_8, 0.5));
    }

    #[test]
    fn an_empty_set_gives_zeros_and_not_a_division_error() {
        let m = aggregate(&[]);
        assert_eq!(m.recall_at_8, 0.0);
        assert_eq!(m.gate_accuracy, 0.0);
        let only_uncovered = aggregate(&[outcome(false, true, &[])]);
        assert_eq!(only_uncovered.recall_at_8, 0.0);
        assert_eq!(only_uncovered.gate_accuracy, 1.0);
        assert_eq!(only_uncovered.false_gate_rate, 0.0);
    }

    #[test]
    fn the_recall_of_the_returned_list_ignores_its_length() {
        let outcomes = [
            outcome(true, false, &[0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            outcome(true, false, &[0, 0]),
            outcome(false, false, &[2]),
        ];
        assert_eq!(recall_of_returned(&outcomes), 0.5);
        assert_eq!(recall_of_returned(&[]), 0.0);
    }

    #[test]
    fn rounding_keeps_four_decimals() {
        assert_eq!(round4(2.0 / 3.0), 0.6667);
        assert_eq!(round4(0.5), 0.5);
    }
}
