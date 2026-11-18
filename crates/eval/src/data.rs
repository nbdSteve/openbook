//! The evaluation data: corpus sections and the golden questions.
//!
//! Page numbers in this crate are pages of the corpus excerpt (1-based), unless a name says "local".

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Error;

/// The least number of golden questions of one corpus, and of each required kind.
pub const MIN_QUESTIONS: usize = 60;
pub const MIN_PER_KIND: usize = 10;

/// A part of the excerpt that the evaluation treats as one document (like one chapter PDF in a
/// course). The per-document cap of retrieval works on these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub first_page: u32,
    pub last_page: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Lookup,
    Paraphrase,
    Notation,
    Followup,
    /// A nearby topic of the same subject that the excerpt does not teach (a hard negative for the
    /// gate).
    NotCovered,
    /// No relation to the course: general knowledge, other subjects, and small talk.
    OffTopic,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Lookup,
        Kind::Paraphrase,
        Kind::Notation,
        Kind::Followup,
        Kind::NotCovered,
        Kind::OffTopic,
    ];

    /// True when a golden set needs at least `MIN_PER_KIND` questions of this kind. A notation
    /// question (math written as symbols) only makes sense for a math corpus.
    pub fn is_required(self) -> bool {
        self != Kind::Notation
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Lookup => "lookup",
            Kind::Paraphrase => "paraphrase",
            Kind::Notation => "notation",
            Kind::Followup => "followup",
            Kind::NotCovered => "not_covered",
            Kind::OffTopic => "off_topic",
        }
    }
}

/// One line of `golden.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub question: String,
    pub relevant_pages: Vec<u32>,
    pub kind: Kind,
    /// For `followup`: the earlier question of the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub small_talk: bool,
}

impl Question {
    /// True when the corpus covers the question.
    pub fn is_covered(&self) -> bool {
        !matches!(self.kind, Kind::NotCovered | Kind::OffTopic)
    }
}

pub fn load_sections(path: &Path) -> Result<Vec<Section>, Error> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, &e))?;
    let sections: Vec<Section> =
        serde_json::from_str(&text).map_err(|e| Error::Data(format!("{}: {e}", path.display())))?;
    validate_sections(&sections)?;
    Ok(sections)
}

/// The sections must be in order, with no gap and no overlap, and start at page 1.
pub fn validate_sections(sections: &[Section]) -> Result<(), Error> {
    let mut next = 1;
    let mut ids = std::collections::HashSet::new();
    for section in sections {
        if section.first_page != next || section.last_page < section.first_page {
            return Err(Error::Data(format!(
                "section {:?} must start at page {next} and end at or after its start",
                section.id
            )));
        }
        if !ids.insert(section.id.as_str()) {
            return Err(Error::Data(format!(
                "section id {:?} is used twice",
                section.id
            )));
        }
        next = section.last_page + 1;
    }
    if sections.is_empty() {
        return Err(Error::Data("there are no sections".into()));
    }
    Ok(())
}

/// The number of pages that the sections cover.
pub fn page_total(sections: &[Section]) -> u32 {
    sections.last().map_or(0, |s| s.last_page)
}

pub fn load_golden(path: &Path, pages: u32) -> Result<Vec<Question>, Error> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, &e))?;
    parse_golden(&text, pages)
}

/// Parses `golden.jsonl` and checks the rules of the set.
pub fn parse_golden(text: &str, pages: u32) -> Result<Vec<Question>, Error> {
    let mut questions = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let question: Question = serde_json::from_str(line)
            .map_err(|e| Error::Data(format!("golden.jsonl line {}: {e}", index + 1)))?;
        questions.push(question);
    }
    validate_golden(&questions, pages)?;
    Ok(questions)
}

pub fn validate_golden(questions: &[Question], pages: u32) -> Result<(), Error> {
    let mut problems = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut per_kind: BTreeMap<Kind, usize> = BTreeMap::new();
    for q in questions {
        if !ids.insert(q.id.as_str()) {
            problems.push(format!("id {:?} is used twice", q.id));
        }
        *per_kind.entry(q.kind).or_default() += 1;
        if q.question.trim().is_empty() {
            problems.push(format!("{}: the question is empty", q.id));
        }
        match q.kind {
            Kind::NotCovered | Kind::OffTopic => {
                if !q.relevant_pages.is_empty() {
                    problems.push(format!(
                        "{}: a {} question has no relevant pages",
                        q.id,
                        q.kind.name()
                    ));
                }
            }
            _ => {
                if q.relevant_pages.is_empty() {
                    problems.push(format!("{}: a covered question needs relevant pages", q.id));
                }
            }
        }
        if q.relevant_pages.iter().any(|p| *p == 0 || *p > pages) {
            problems.push(format!("{}: a relevant page is outside 1..={pages}", q.id));
        }
        match (q.kind, &q.context) {
            (Kind::Followup, None) => {
                problems.push(format!("{}: a followup needs a context", q.id))
            }
            (Kind::Followup, Some(_)) => {}
            (_, Some(_)) => problems.push(format!("{}: only a followup has a context", q.id)),
            (_, None) => {}
        }
        if q.small_talk && q.kind != Kind::OffTopic {
            problems.push(format!(
                "{}: only an off_topic question can be small talk",
                q.id
            ));
        }
    }
    if questions.len() < MIN_QUESTIONS {
        problems.push(format!(
            "there are {} questions and the least is {MIN_QUESTIONS}",
            questions.len()
        ));
    }
    for kind in Kind::ALL.into_iter().filter(|k| k.is_required()) {
        let count = per_kind.get(&kind).copied().unwrap_or(0);
        if count < MIN_PER_KIND {
            problems.push(format!(
                "there are {count} {} questions and the least is {MIN_PER_KIND}",
                kind.name()
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Error::Data(problems.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(id: &str, kind: Kind) -> Question {
        Question {
            id: id.into(),
            question: "What?".into(),
            relevant_pages: if matches!(kind, Kind::NotCovered | Kind::OffTopic) {
                vec![]
            } else {
                vec![1]
            },
            kind,
            context: (kind == Kind::Followup).then(|| "Before.".to_owned()),
            small_talk: false,
        }
    }

    #[test]
    fn only_an_off_topic_question_can_be_small_talk() {
        let mut set = full_set();
        let first_off_topic = set.iter().position(|q| q.kind == Kind::OffTopic).unwrap();
        set[first_off_topic].small_talk = true;
        assert!(validate_golden(&set, 10).is_ok());
        let first_lookup = set.iter().position(|q| q.kind == Kind::Lookup).unwrap();
        set[first_lookup].small_talk = true;
        let problem = validate_golden(&set, 10).unwrap_err().to_string();
        assert!(problem.contains("only an off_topic question can be small talk"));
    }

    fn full_set() -> Vec<Question> {
        let mut all = Vec::new();
        for kind in Kind::ALL {
            for i in 0..MIN_PER_KIND {
                all.push(question(&format!("{}-{i}", kind.name()), kind));
            }
        }
        // 6 kinds x 10 is 60, and the set needs 60 at the least. Add a few more.
        for i in 0..5 {
            all.push(question(&format!("extra-{i}"), Kind::Lookup));
        }
        all
    }

    #[test]
    fn a_good_set_passes() {
        assert!(validate_golden(&full_set(), 43).is_ok());
    }

    #[test]
    fn the_rules_of_the_set_are_checked() {
        let mut few = full_set();
        few.truncate(59);
        assert!(
            validate_golden(&few, 43)
                .unwrap_err()
                .to_string()
                .contains("least is 60")
        );

        let mut missing_kind = full_set();
        missing_kind.retain(|q| q.kind != Kind::Followup);
        for i in 0..40 {
            missing_kind.push(question(&format!("more-{i}"), Kind::Lookup));
        }
        let error = validate_golden(&missing_kind, 43).unwrap_err().to_string();
        assert!(error.contains("0 followup"), "{error}");

        let mut bad = full_set();
        bad[0].relevant_pages = vec![44];
        bad[1].relevant_pages = vec![];
        bad[2].id = bad[3].id.clone();
        let error = validate_golden(&bad, 43).unwrap_err().to_string();
        assert!(error.contains("outside 1..=43"), "{error}");
        assert!(error.contains("needs relevant pages"), "{error}");
        assert!(error.contains("used twice"), "{error}");
    }

    #[test]
    fn a_corpus_may_have_no_notation_questions() {
        let mut set = full_set();
        set.retain(|q| q.kind != Kind::Notation);
        for i in 0..10 {
            set.push(question(&format!("more-{i}"), Kind::Paraphrase));
        }
        assert!(validate_golden(&set, 43).is_ok());
    }

    #[test]
    fn context_belongs_to_followups_only() {
        let mut set = full_set();
        set[0].context = Some("x".into());
        assert!(
            validate_golden(&set, 43)
                .unwrap_err()
                .to_string()
                .contains("only a followup")
        );
        let mut set = full_set();
        let at = set.iter().position(|q| q.kind == Kind::Followup).unwrap();
        set[at].context = None;
        assert!(
            validate_golden(&set, 43)
                .unwrap_err()
                .to_string()
                .contains("needs a context")
        );
    }

    #[test]
    fn sections_need_to_be_in_order_without_gaps() {
        let section = |id: &str, a, b| Section {
            id: id.into(),
            title: id.into(),
            first_page: a,
            last_page: b,
        };
        assert!(validate_sections(&[section("a", 1, 3), section("b", 4, 4)]).is_ok());
        assert!(validate_sections(&[section("a", 1, 3), section("b", 5, 6)]).is_err());
        assert!(validate_sections(&[section("a", 2, 3)]).is_err());
        assert!(validate_sections(&[section("a", 1, 3), section("a", 4, 5)]).is_err());
        assert!(validate_sections(&[]).is_err());
    }

    #[test]
    fn the_committed_golden_sets_are_valid() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals");
        let corpora = crate::corpus::discover(&root).unwrap();
        assert!(
            corpora.len() >= 2,
            "the evaluation needs more than one corpus"
        );
        for corpus in corpora {
            let sections = load_sections(&corpus.sections_path()).unwrap();
            let questions = load_golden(&corpus.golden_path(), page_total(&sections)).unwrap();
            assert!(questions.len() >= MIN_QUESTIONS, "{}", corpus.id);
            assert!(corpus.attribution_path().is_file(), "{}", corpus.id);
        }
    }
}
