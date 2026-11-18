use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::data::Kind;
use crate::metrics::round4;

pub const SUMMARY_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub corpus: String,
    pub id: String,
    pub kind: Kind,
    pub small_talk: bool,
    pub held_out: bool,
    pub opens_with_statement: bool,
    pub gated: bool,
    pub sources: usize,
    pub valid_cited: usize,
    pub relevant_cited: usize,
    pub invalid_cited: usize,
    pub relevant_available: bool,
    pub says_not_covered: bool,
    pub mentions_course_materials: bool,
    pub natural_reply: bool,
}

impl Scored {
    pub fn is_covered(&self) -> bool {
        !matches!(self.kind, Kind::NotCovered | Kind::OffTopic)
    }

    pub fn is_uncited(&self) -> bool {
        self.valid_cited == 0
    }

    pub fn grounded(&self) -> bool {
        self.relevant_cited > 0
    }

    pub fn small_talk_correct(&self) -> bool {
        self.natural_reply && !self.mentions_course_materials
    }
}

fn rate(count: usize, total: usize) -> Option<f64> {
    (total > 0).then(|| round4(count as f64 / total as f64))
}

fn count(rows: &[&Scored], test: impl Fn(&Scored) -> bool) -> usize {
    rows.iter().filter(|r| test(r)).count()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoveredMetrics {
    pub questions: usize,
    pub grounded_citation_rate: Option<f64>,
    pub citation_precision: Option<f64>,
    pub uncited_rate: Option<f64>,
    pub nonexistent_citation_rate: Option<f64>,
    pub false_not_covered_rate: Option<f64>,
    pub relevant_source_available_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotCoveredSplit {
    pub questions: usize,
    pub says_not_covered_rate: Option<f64>,
    pub opens_with_statement_rate: Option<f64>,
    pub uncited_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotCoveredMetrics {
    pub all: NotCoveredSplit,
    pub gated: NotCoveredSplit,
    pub not_gated: NotCoveredSplit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffTopicSplit {
    pub questions: usize,
    pub correct_rate: Option<f64>,
    pub opens_with_statement_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffTopicMetrics {
    pub all: OffTopicSplit,
    pub small_talk: OffTopicSplit,
    pub subject: OffTopicSplit,
    pub gated_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KindRow {
    pub kind: String,
    pub questions: usize,
    pub uncited_rate: Option<f64>,
    pub nonexistent_citation_rate: Option<f64>,
    pub says_not_covered_rate: Option<f64>,
    pub grounded_citation_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub questions: usize,
    pub covered: CoveredMetrics,
    pub not_covered: NotCoveredMetrics,
    pub off_topic: OffTopicMetrics,
    pub by_kind: Vec<KindRow>,
}

pub fn covered_metrics(rows: &[&Scored]) -> CoveredMetrics {
    let covered: Vec<&Scored> = rows.iter().copied().filter(|r| r.is_covered()).collect();
    let total = covered.len();
    let citing: Vec<&&Scored> = covered.iter().filter(|r| r.valid_cited > 0).collect();
    let precision = (!citing.is_empty()).then(|| {
        round4(
            citing
                .iter()
                .map(|r| r.relevant_cited as f64 / r.valid_cited as f64)
                .sum::<f64>()
                / citing.len() as f64,
        )
    });
    CoveredMetrics {
        questions: total,
        grounded_citation_rate: rate(count(&covered, Scored::grounded), total),
        citation_precision: precision,
        uncited_rate: rate(count(&covered, Scored::is_uncited), total),
        nonexistent_citation_rate: rate(count(&covered, |r| r.invalid_cited > 0), total),
        false_not_covered_rate: rate(count(&covered, |r| r.says_not_covered), total),
        relevant_source_available_rate: rate(count(&covered, |r| r.relevant_available), total),
    }
}

fn not_covered_split(rows: &[&Scored]) -> NotCoveredSplit {
    NotCoveredSplit {
        questions: rows.len(),
        says_not_covered_rate: rate(count(rows, |r| r.says_not_covered), rows.len()),
        opens_with_statement_rate: rate(count(rows, |r| r.opens_with_statement), rows.len()),
        uncited_rate: rate(count(rows, Scored::is_uncited), rows.len()),
    }
}

pub fn not_covered_metrics(rows: &[&Scored]) -> NotCoveredMetrics {
    let all: Vec<&Scored> = rows
        .iter()
        .copied()
        .filter(|r| r.kind == Kind::NotCovered)
        .collect();
    let gated: Vec<&Scored> = all.iter().copied().filter(|r| r.gated).collect();
    let not_gated: Vec<&Scored> = all.iter().copied().filter(|r| !r.gated).collect();
    NotCoveredMetrics {
        all: not_covered_split(&all),
        gated: not_covered_split(&gated),
        not_gated: not_covered_split(&not_gated),
    }
}

fn off_topic_split(rows: &[&Scored]) -> OffTopicSplit {
    let correct = count(rows, |r| {
        if r.small_talk {
            r.small_talk_correct()
        } else {
            r.says_not_covered
        }
    });
    let subject = rows.iter().all(|r| !r.small_talk);
    OffTopicSplit {
        questions: rows.len(),
        correct_rate: rate(correct, rows.len()),
        opens_with_statement_rate: if subject {
            rate(count(rows, |r| r.opens_with_statement), rows.len())
        } else {
            None
        },
    }
}

pub fn off_topic_metrics(rows: &[&Scored]) -> OffTopicMetrics {
    let all: Vec<&Scored> = rows
        .iter()
        .copied()
        .filter(|r| r.kind == Kind::OffTopic)
        .collect();
    let small: Vec<&Scored> = all.iter().copied().filter(|r| r.small_talk).collect();
    let subject: Vec<&Scored> = all.iter().copied().filter(|r| !r.small_talk).collect();
    OffTopicMetrics {
        gated_rate: rate(count(&all, |r| r.gated), all.len()),
        all: off_topic_split(&all),
        small_talk: off_topic_split(&small),
        subject: off_topic_split(&subject),
    }
}

pub fn scope(rows: &[&Scored]) -> Scope {
    let by_kind = Kind::ALL
        .iter()
        .filter_map(|kind| {
            let of_kind: Vec<&Scored> = rows.iter().copied().filter(|r| r.kind == *kind).collect();
            (!of_kind.is_empty()).then(|| KindRow {
                kind: kind.name().to_owned(),
                questions: of_kind.len(),
                uncited_rate: rate(count(&of_kind, Scored::is_uncited), of_kind.len()),
                nonexistent_citation_rate: rate(
                    count(&of_kind, |r| r.invalid_cited > 0),
                    of_kind.len(),
                ),
                says_not_covered_rate: rate(count(&of_kind, |r| r.says_not_covered), of_kind.len()),
                grounded_citation_rate: rate(count(&of_kind, Scored::grounded), of_kind.len()),
            })
        })
        .collect();
    Scope {
        questions: rows.len(),
        covered: covered_metrics(rows),
        not_covered: not_covered_metrics(rows),
        off_topic: off_topic_metrics(rows),
        by_kind,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub chat_model: String,
    pub judge_model: String,
    pub style: String,
    pub rubric_version: u32,
    pub prompt_fingerprint: String,
    pub selection: String,
    pub turn_note: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerReport {
    pub schema: u32,
    pub settings: Settings,
    pub pooled: Scope,
    pub held_out: Option<Scope>,
    pub corpora: BTreeMap<String, Scope>,
}

impl AnswerReport {
    pub fn build(settings: Settings, scored: &[Scored]) -> AnswerReport {
        let in_sample: Vec<&Scored> = scored.iter().filter(|s| !s.held_out).collect();
        let held_out: Vec<&Scored> = scored.iter().filter(|s| s.held_out).collect();
        let mut corpora = BTreeMap::new();
        for row in &in_sample {
            let rows: Vec<&Scored> = in_sample
                .iter()
                .copied()
                .filter(|s| s.corpus == row.corpus)
                .collect();
            corpora.insert(row.corpus.clone(), scope(&rows));
        }
        AnswerReport {
            schema: SUMMARY_SCHEMA,
            settings,
            pooled: scope(&in_sample),
            held_out: (!held_out.is_empty()).then(|| scope(&held_out)),
            corpora,
        }
    }

    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string_pretty(self)
            .map(|text| text + "\n")
            .map_err(|e| Error::Run(format!("answers report: {e}")))
    }

    pub fn summary(&self) -> AnswerReport {
        AnswerReport {
            schema: self.schema,
            settings: self.settings.clone(),
            pooled: self.pooled.clone(),
            held_out: self.held_out.clone(),
            corpora: BTreeMap::new(),
        }
    }

    pub fn to_markdown(&self) -> String {
        let s = &self.settings;
        let mut out = String::new();
        let _ = writeln!(out, "# Answer evaluation\n");
        let _ = writeln!(
            out,
            "Chat model `{}`, judge `{}` (rubric version {}), tutoring style `{}`, selection: {}. Prompt fingerprint `{}`.\n",
            s.chat_model,
            s.judge_model,
            s.rubric_version,
            s.style,
            s.selection,
            s.prompt_fingerprint
        );
        let _ = writeln!(
            out,
            "Follow-up questions are approximate: the earlier question goes in the same user turn, with no stored tutor reply.\n"
        );
        scope_markdown(
            &mut out,
            "All corpora pooled, in-sample (the questions that the prompt was tuned on)",
            &self.pooled,
        );
        if let Some(held_out) = &self.held_out {
            scope_markdown(
                &mut out,
                "Held-out, out-of-sample (questions that tuning never saw)",
                held_out,
            );
        }
        for (id, scope) in &self.corpora {
            scope_markdown(&mut out, &format!("{id}, in-sample"), scope);
        }
        out
    }
}

fn pct(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |v| format!("{:.1}%", v * 100.0))
}

fn scope_markdown(out: &mut String, title: &str, scope: &Scope) {
    let _ = writeln!(out, "## {title} ({} questions)\n", scope.questions);
    let c = &scope.covered;
    let _ = writeln!(out, "### Covered questions ({})\n", c.questions);
    let _ = writeln!(out, "| Metric | Value |\n|---|---|");
    for (name, value) in [
        ("Grounded citation rate", c.grounded_citation_rate),
        ("Citation precision", c.citation_precision),
        ("Uncited answers", c.uncited_rate),
        (
            "Answers that cite a number that does not exist",
            c.nonexistent_citation_rate,
        ),
        ("False \"not covered\" rate", c.false_not_covered_rate),
        (
            "Relevant source was available",
            c.relevant_source_available_rate,
        ),
    ] {
        let _ = writeln!(out, "| {name} | {} |", pct(value));
    }
    let n = &scope.not_covered;
    let _ = writeln!(out, "\n### not_covered ({})\n", n.all.questions);
    let _ = writeln!(
        out,
        "| Group | Questions | Says \"not covered\" (judge) | Opens with the statement (check) | Uncited |\n|---|---|---|---|---|"
    );
    for (name, split) in [
        ("all", &n.all),
        ("gated (no sources)", &n.gated),
        ("not gated (sources given)", &n.not_gated),
    ] {
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {} | {} |",
            split.questions,
            pct(split.says_not_covered_rate),
            pct(split.opens_with_statement_rate),
            pct(split.uncited_rate)
        );
    }
    let o = &scope.off_topic;
    let _ = writeln!(out, "\n### off_topic ({})\n", o.all.questions);
    let _ = writeln!(
        out,
        "| Group | Questions | Correct handling | Opens with the statement (check) |\n|---|---|---|---|"
    );
    for (name, split) in [
        ("all", &o.all),
        (
            "small talk (natural reply, no talk about materials)",
            &o.small_talk,
        ),
        ("subject questions (says \"not covered\")", &o.subject),
    ] {
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {} |",
            split.questions,
            pct(split.correct_rate),
            pct(split.opens_with_statement_rate)
        );
    }
    let _ = writeln!(
        out,
        "\nThe gate cut off {} of the off_topic questions.\n",
        pct(o.gated_rate)
    );
    let _ = writeln!(out, "### By kind\n");
    let _ = writeln!(
        out,
        "| Kind | Questions | Grounded citation | Uncited | Nonexistent citation | Says \"not covered\" |\n|---|---|---|---|---|---|"
    );
    for row in &scope.by_kind {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} |",
            row.kind,
            row.questions,
            pct(row.grounded_citation_rate),
            pct(row.uncited_rate),
            pct(row.nonexistent_citation_rate),
            pct(row.says_not_covered_rate)
        );
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: Kind) -> Scored {
        Scored {
            corpus: "c".into(),
            id: "q".into(),
            kind,
            small_talk: false,
            held_out: false,
            opens_with_statement: false,
            gated: false,
            sources: 3,
            valid_cited: 0,
            relevant_cited: 0,
            invalid_cited: 0,
            relevant_available: true,
            says_not_covered: false,
            mentions_course_materials: false,
            natural_reply: false,
        }
    }

    #[test]
    fn covered_metrics_count_grounding_precision_and_false_not_covered() {
        let mut good = row(Kind::Lookup);
        good.valid_cited = 2;
        good.relevant_cited = 1;
        let mut wrong = row(Kind::Paraphrase);
        wrong.valid_cited = 1;
        let mut silent = row(Kind::Notation);
        silent.invalid_cited = 1;
        let mut refuses = row(Kind::Followup);
        refuses.says_not_covered = true;
        refuses.relevant_available = false;
        let rows = [&good, &wrong, &silent, &refuses];
        let m = covered_metrics(&rows);
        assert_eq!(m.questions, 4);
        assert_eq!(m.grounded_citation_rate, Some(0.25));
        assert_eq!(m.citation_precision, Some(0.25));
        assert_eq!(m.uncited_rate, Some(0.5));
        assert_eq!(m.nonexistent_citation_rate, Some(0.25));
        assert_eq!(m.false_not_covered_rate, Some(0.25));
        assert_eq!(m.relevant_source_available_rate, Some(0.75));
    }

    #[test]
    fn not_covered_is_split_by_the_gate() {
        let mut gated = row(Kind::NotCovered);
        gated.gated = true;
        gated.sources = 0;
        gated.says_not_covered = true;
        let mut open = row(Kind::NotCovered);
        open.valid_cited = 1;
        let m = not_covered_metrics(&[&gated, &open]);
        assert_eq!(m.all.questions, 2);
        assert_eq!(m.gated.says_not_covered_rate, Some(1.0));
        assert_eq!(m.gated.uncited_rate, Some(1.0));
        assert_eq!(m.not_gated.says_not_covered_rate, Some(0.0));
        assert_eq!(m.not_gated.uncited_rate, Some(0.0));
    }

    #[test]
    fn off_topic_small_talk_and_subject_questions_use_their_own_rule() {
        let mut hello = row(Kind::OffTopic);
        hello.small_talk = true;
        hello.natural_reply = true;
        let mut preachy = row(Kind::OffTopic);
        preachy.small_talk = true;
        preachy.natural_reply = true;
        preachy.mentions_course_materials = true;
        let mut subject = row(Kind::OffTopic);
        subject.says_not_covered = true;
        let silent = row(Kind::OffTopic);
        let m = off_topic_metrics(&[&hello, &preachy, &subject, &silent]);
        assert_eq!(m.small_talk.correct_rate, Some(0.5));
        assert_eq!(m.subject.correct_rate, Some(0.5));
        assert_eq!(m.all.correct_rate, Some(0.5));
    }

    #[test]
    fn an_empty_group_has_no_rate() {
        let m = covered_metrics(&[]);
        assert_eq!(m.questions, 0);
        assert_eq!(m.grounded_citation_rate, None);
        assert_eq!(pct(None), "n/a");
    }

    #[test]
    fn the_report_has_a_scope_for_each_corpus_and_a_small_summary() {
        let mut a = row(Kind::Lookup);
        a.corpus = "a".into();
        let mut b = row(Kind::NotCovered);
        b.corpus = "b".into();
        let settings = Settings {
            chat_model: "m".into(),
            judge_model: "j".into(),
            style: "direct".into(),
            rubric_version: 1,
            prompt_fingerprint: "f".into(),
            selection: "all".into(),
            turn_note: true,
        };
        let report = AnswerReport::build(settings, &[a, b]);
        assert_eq!(report.pooled.questions, 2);
        assert_eq!(report.corpora.len(), 2);
        assert!(report.summary().corpora.is_empty());
        let text = report.to_markdown();
        assert!(text.contains("in-sample"));
        assert!(text.contains("approximate"));
        assert!(text.contains("### not_covered (1)"));
        let json = report.to_json().unwrap();
        assert!(json.contains("\"groundedCitationRate\""));
    }

    #[test]
    fn held_out_questions_are_reported_apart_from_the_in_sample_numbers() {
        let mut tuned = row(Kind::NotCovered);
        tuned.says_not_covered = true;
        let mut held = row(Kind::NotCovered);
        held.held_out = true;
        let settings = Settings {
            chat_model: "m".into(),
            judge_model: "j".into(),
            style: "direct".into(),
            rubric_version: 3,
            prompt_fingerprint: "f".into(),
            selection: "all".into(),
            turn_note: true,
        };
        let report = AnswerReport::build(settings, &[tuned, held]);
        assert_eq!(report.pooled.questions, 1);
        assert_eq!(
            report.pooled.not_covered.all.says_not_covered_rate,
            Some(1.0)
        );
        let held_out = report.held_out.as_ref().unwrap();
        assert_eq!(held_out.questions, 1);
        assert_eq!(held_out.not_covered.all.says_not_covered_rate, Some(0.0));
        assert!(report.to_markdown().contains("out-of-sample"));
    }

    #[test]
    fn the_statement_check_is_reported_next_to_the_judge() {
        let mut said = row(Kind::NotCovered);
        said.says_not_covered = true;
        said.opens_with_statement = true;
        let mut buried = row(Kind::NotCovered);
        buried.says_not_covered = true;
        let m = not_covered_metrics(&[&said, &buried]);
        assert_eq!(m.all.says_not_covered_rate, Some(1.0));
        assert_eq!(m.all.opens_with_statement_rate, Some(0.5));
    }
}
