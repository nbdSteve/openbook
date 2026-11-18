use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use openbook_api::retrieval::source_lists;
use openbook_core::catalog::{Catalog, ModelEntry};
use openbook_core::domain::{MessageRole, TutoringStyle};
use openbook_core::model::{ChatModel, ChatRequest};
use openbook_core::prompt::{SourceMode, build_system_prompt_for, normalize_turns};
use openbook_search::retrieval::Outcome;

use crate::Error;
use crate::answer_cache::{
    AnswerEntry, Judgment, Store, answer_key, answers_path, judgment_key, judgments_path,
    request_sha256, sha256_parts,
};
use crate::answer_report::Scored;
use crate::benchmarks::CorpusMeta;
use crate::cache::{Cache, CacheKind, sha256_hex};
use crate::citations;
use crate::corpus::Corpus;
use crate::data::{self, Kind, Question};
use crate::judge::{self, JUDGE_MAX_TOKENS, RUBRIC, RUBRIC_VERSION};
use crate::metrics::is_relevant;
use crate::offline::{self, Index, Options};
use crate::refresh::retry;

pub const UPPER_BOUND_CHARS_PER_TOKEN: u64 = 2;
pub const CONCURRENCY: usize = 4;
pub const DEFAULT_SEED: u64 = 1;

pub fn parse_style(text: &str) -> Result<TutoringStyle, Error> {
    TutoringStyle::ALL
        .into_iter()
        .find(|s| s.id() == text)
        .ok_or_else(|| {
            let ids: Vec<&str> = TutoringStyle::ALL.iter().map(|s| s.id()).collect();
            Error::Run(format!(
                "unknown tutoring style {text:?}. The styles are: {}",
                ids.join(", ")
            ))
        })
}

pub fn parse_kinds(text: &str) -> Result<Vec<Kind>, Error> {
    let mut kinds = Vec::new();
    for name in text.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        match name {
            "covered" => kinds.extend([
                Kind::Lookup,
                Kind::Paraphrase,
                Kind::Notation,
                Kind::Followup,
            ]),
            other => kinds.push(
                Kind::ALL
                    .into_iter()
                    .find(|k| k.name() == other)
                    .ok_or_else(|| {
                        Error::Run(format!(
                            "unknown kind {other:?}. The kinds are: covered, lookup, paraphrase, notation, followup, not_covered, off_topic"
                        ))
                    })?,
            ),
        }
    }
    Ok(kinds)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub kinds: Vec<Kind>,
    pub sample_covered: Option<usize>,
    pub seed: Option<u64>,
}

impl Selection {
    pub fn is_full(&self) -> bool {
        self.kinds.is_empty() && self.sample_covered.is_none()
    }

    pub fn describe(&self) -> String {
        if self.is_full() {
            return "all questions".to_owned();
        }
        let kinds = if self.kinds.is_empty() {
            "all kinds".to_owned()
        } else {
            self.kinds
                .iter()
                .map(|k| k.name())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self.sample_covered {
            Some(n) => format!(
                "{kinds}; a seeded sample of {n} covered questions (seed {})",
                self.seed.unwrap_or(DEFAULT_SEED)
            ),
            None => kinds,
        }
    }

    pub fn pick(&self, pool: Vec<(String, Question)>) -> Vec<(String, Question)> {
        let wanted = |q: &Question| self.kinds.is_empty() || self.kinds.contains(&q.kind);
        let pool: Vec<(String, Question)> = pool.into_iter().filter(|(_, q)| wanted(q)).collect();
        let Some(limit) = self.sample_covered else {
            return pool;
        };
        let seed = self.seed.unwrap_or(DEFAULT_SEED).to_string();
        let mut covered: Vec<(String, usize)> = pool
            .iter()
            .enumerate()
            .filter(|(_, (_, q))| q.is_covered())
            .map(|(i, (corpus, q))| (sha256_parts(&[&seed, corpus, &q.id]), i))
            .collect();
        covered.sort();
        let keep: HashSet<usize> = covered.into_iter().take(limit).map(|(_, i)| i).collect();
        pool.into_iter()
            .enumerate()
            .filter(|(i, (_, q))| !q.is_covered() || keep.contains(i))
            .map(|(_, item)| item)
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub corpus: String,
    pub question: Question,
    pub gated: bool,
    pub sources: usize,
    pub relevant_sources: Vec<bool>,
    pub relevant_available: bool,
    pub request: ChatRequest,
    pub prompt_sha256: String,
    pub key: String,
}

pub fn load_catalog(path: &Path) -> Result<Catalog, Error> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, &e))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| Error::Data(format!("{}: {e}", path.display())))?;
    let context = json
        .get("context")
        .ok_or_else(|| Error::Data(format!("{} has no context", path.display())))?;
    let field = |name: &str| {
        context
            .get(name)
            .ok_or_else(|| Error::Data(format!("{}: context.{name} is missing", path.display())))
    };
    let text_of = |name: &str| {
        field(name)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Data(format!("context.{name} must be a string")))
    };
    Catalog::parse(
        &field("models")?.to_string(),
        &text_of("defaultChatModel")?,
        &text_of("fastModel")?,
    )
    .map_err(|problems| Error::Data(problems.join("; ")))
}

pub fn default_catalog_path(evals: &Path) -> std::path::PathBuf {
    evals.parent().map_or_else(
        || std::path::PathBuf::from("infra/cdk.json"),
        |root| root.join("infra/cdk.json"),
    )
}

pub struct ModelChoice {
    pub chat: ModelEntry,
    pub judge: ModelEntry,
}

pub fn choose_models(
    catalog: &Catalog,
    chat: Option<&str>,
    judge: Option<&str>,
) -> Result<ModelChoice, Error> {
    let find = |id: &str| {
        catalog
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Run(format!("model {id:?} is not in the catalog")))
    };
    Ok(ModelChoice {
        chat: find(chat.unwrap_or_else(|| catalog.default_chat_id()))?,
        judge: find(judge.unwrap_or_else(|| catalog.fast_id()))?,
    })
}

pub fn history_for(question: &Question) -> Vec<(MessageRole, String)> {
    let mut history = Vec::new();
    if let Some(context) = &question.context {
        history.push((MessageRole::User, context.clone()));
    }
    history.push((MessageRole::User, question.question.clone()));
    history
}

pub fn build_request(
    course_name: &str,
    style: TutoringStyle,
    mode: SourceMode<'_>,
    question: &Question,
) -> ChatRequest {
    ChatRequest {
        system: build_system_prompt_for(course_name, "", style, mode),
        turns: normalize_turns(&history_for(question)),
        ..ChatRequest::default()
    }
}

pub async fn prepare_corpus(
    corpus: &Corpus,
    questions: &[Question],
    style: TutoringStyle,
    chat: &ModelEntry,
) -> Result<Vec<Prepared>, Error> {
    let sections = data::load_sections(&corpus.sections_path())?;
    let cache = Cache::load(&corpus.cache_dir(false))?;
    if cache.meta.kind != CacheKind::Live {
        return Err(Error::Run(format!(
            "the cache of {} is not a live cache: the answer evaluation needs the live retrieval",
            corpus.id
        )));
    }
    let course_name = CorpusMeta::load(corpus)?.title;
    let options = Options::production(&cache.meta.embedding_model);
    let mut index = Index::build(
        &sections,
        &cache,
        options.chunk_chars,
        options.cap_off,
        options.header_variant,
    )?;
    index.set_candidates(options.dense_top_k, options.lexical_top_k);
    let mut out = Vec::with_capacity(questions.len());
    for question in questions {
        let retrieved =
            offline::retrieve(&index, &sections, question, &cache, &options.params).await?;
        let fused = retrieved.fused();
        let titles: std::collections::HashMap<&str, &str> = retrieved
            .chunks
            .iter()
            .map(|c| {
                (
                    c.fused.passage.document_id.as_str(),
                    c.document_title.as_str(),
                )
            })
            .collect();
        let (_, contexts) =
            source_lists(&fused, |id| titles.get(id).copied().unwrap_or("document"));
        let mode = match retrieved.outcome {
            Outcome::Found => SourceMode::Found(&contexts),
            Outcome::NotCovered => SourceMode::NotCovered,
            Outcome::NotUsed => SourceMode::NotUsed,
        };
        let request = build_request(&course_name, style, mode, question);
        let relevant_sources: Vec<bool> = retrieved
            .chunks
            .iter()
            .map(|c| is_relevant(c.span, &question.relevant_pages))
            .collect();
        let prompt_sha256 = request_sha256(&request, chat.max_tokens(), chat.temperature);
        let key = answer_key(&chat.model_id, &prompt_sha256, &question.question);
        out.push(Prepared {
            corpus: corpus.id.clone(),
            question: question.clone(),
            gated: retrieved.outcome == Outcome::NotCovered,
            sources: retrieved.chunks.len(),
            relevant_available: relevant_sources.iter().any(|r| *r),
            relevant_sources,
            request,
            prompt_sha256,
            key,
        });
    }
    Ok(out)
}

pub struct CorpusRun {
    pub corpus: Corpus,
    pub prepared: Vec<Prepared>,
    pub answers: Store<AnswerEntry>,
    pub judgments: Store<Judgment>,
}

impl CorpusRun {
    pub fn judgment_key_for(&self, judge_model_id: &str, item: &Prepared) -> Option<String> {
        let answer = self.answers.get(&item.key)?;
        Some(judgment_key(
            judge_model_id,
            RUBRIC_VERSION,
            &sha256_hex(answer.text.as_bytes()),
            &item.question.question,
        ))
    }
}

pub fn open_stores(
    corpus: &Corpus,
    chat: &ModelEntry,
    judge: &ModelEntry,
) -> Result<(Store<AnswerEntry>, Store<Judgment>), Error> {
    Ok((
        Store::load(answers_path(&corpus.dir, &chat.model_id))?,
        Store::load(judgments_path(&corpus.dir, &judge.model_id))?,
    ))
}

pub fn prompt_fingerprint(runs: &[CorpusRun]) -> String {
    let hashes: Vec<&str> = runs
        .iter()
        .flat_map(|r| r.prepared.iter().map(|p| p.prompt_sha256.as_str()))
        .collect();
    sha256_parts(&hashes)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub region: String,
    pub profile: String,
    pub style: TutoringStyle,
    pub chat: ModelEntry,
    pub judge: ModelEntry,
    pub selection: String,
    pub questions: usize,
    pub cached_answers: usize,
    pub answer_calls: usize,
    pub judge_calls: usize,
    pub cost_upper_bound_usd: f64,
    pub by_corpus: Vec<(String, usize, usize)>,
}

fn price(entry: &ModelEntry, input_tokens: u64, output_tokens: u64) -> Result<u64, Error> {
    entry
        .cost_micro_usd(input_tokens, output_tokens)
        .ok_or_else(|| {
            Error::Run(format!(
                "the catalog has no prices for {:?}, so the cost cannot be bounded",
                entry.id
            ))
        })
}

fn bound_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(UPPER_BOUND_CHARS_PER_TOKEN)
}

pub fn make_plan(
    runs: &[CorpusRun],
    choice: &ModelChoice,
    style: TutoringStyle,
    selection: &Selection,
    region: String,
    profile: String,
) -> Result<Plan, Error> {
    let mut micro: u64 = 0;
    let (mut questions, mut cached, mut answer_calls, mut judge_calls) = (0, 0, 0, 0);
    let mut by_corpus = Vec::new();
    let max_output_chars = usize::try_from(choice.chat.max_tokens())
        .unwrap_or(0)
        .saturating_mul(UPPER_BOUND_CHARS_PER_TOKEN as usize);
    for run in runs {
        let (mut corpus_answers, mut corpus_judgments) = (0, 0);
        for item in &run.prepared {
            questions += 1;
            let answer_chars = match run.answers.get(&item.key) {
                Some(answer) => {
                    cached += 1;
                    answer.text.chars().count()
                }
                None => {
                    answer_calls += 1;
                    corpus_answers += 1;
                    let chars = item.request.system.chars().count()
                        + item
                            .request
                            .turns
                            .iter()
                            .map(|t| t.text.chars().count())
                            .sum::<usize>();
                    micro += price(
                        &choice.chat,
                        bound_tokens(chars),
                        choice.chat.max_tokens() as u64,
                    )?;
                    max_output_chars
                }
            };
            let judged = run
                .judgment_key_for(&choice.judge.model_id, item)
                .is_some_and(|key| run.judgments.contains(&key));
            if !judged {
                judge_calls += 1;
                corpus_judgments += 1;
                let chars =
                    RUBRIC.chars().count() + item.question.question.chars().count() + answer_chars;
                micro += price(&choice.judge, bound_tokens(chars), JUDGE_MAX_TOKENS as u64)?;
            }
        }
        by_corpus.push((run.corpus.id.clone(), corpus_answers, corpus_judgments));
    }
    Ok(Plan {
        region,
        profile,
        style,
        chat: choice.chat.clone(),
        judge: choice.judge.clone(),
        selection: selection.describe(),
        questions,
        cached_answers: cached,
        answer_calls,
        judge_calls,
        cost_upper_bound_usd: micro as f64 / 1_000_000.0,
        by_corpus,
    })
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.answer_calls == 0 && self.judge_calls == 0
    }

    pub fn describe(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "Answer evaluation plan");
        let _ = writeln!(out, "  AWS profile: {}", self.profile);
        let _ = writeln!(out, "  AWS Region: {}", self.region);
        let _ = writeln!(out, "  Tutoring style: {}", self.style.id());
        let _ = writeln!(out, "  Selection: {}", self.selection);
        let _ = writeln!(out, "  Questions: {}", self.questions);
        let _ = writeln!(
            out,
            "  Chat model: {} ({}), answers in the cache: {}",
            self.chat.id, self.chat.model_id, self.cached_answers
        );
        let _ = writeln!(
            out,
            "  Judge model: {} ({}), rubric version {}",
            self.judge.id, self.judge.model_id, RUBRIC_VERSION
        );
        for (corpus, answers, judgments) in &self.by_corpus {
            let _ = writeln!(
                out,
                "  {corpus}: {answers} answer calls, {judgments} judge calls"
            );
        }
        let _ = writeln!(
            out,
            "  Calls: {} answer calls to Bedrock ConverseStream, {} judge calls (at most: a changed answer needs a new judgment)",
            self.answer_calls, self.judge_calls
        );
        let _ = writeln!(
            out,
            "  Cost: at most ${:.2} (list prices of the catalog, {UPPER_BOUND_CHARS_PER_TOKEN} characters per input token, the full output limit on every call)",
            self.cost_upper_bound_usd
        );
        out
    }
}

pub fn score(item: &Prepared, answer: &str, judgment: &Judgment) -> Scored {
    let markers = citations::markers(answer);
    let mut valid: Vec<u32> = markers
        .iter()
        .copied()
        .filter(|n| (1..=item.sources as u32).contains(n))
        .collect();
    valid.sort_unstable();
    valid.dedup();
    let mut invalid: Vec<u32> = markers
        .iter()
        .copied()
        .filter(|n| !(1..=item.sources as u32).contains(n))
        .collect();
    invalid.sort_unstable();
    invalid.dedup();
    let relevant_cited = valid
        .iter()
        .filter(|n| item.relevant_sources[**n as usize - 1])
        .count();
    Scored {
        corpus: item.corpus.clone(),
        id: item.question.id.clone(),
        kind: item.question.kind,
        small_talk: item.question.small_talk,
        gated: item.gated,
        sources: item.sources,
        valid_cited: valid.len(),
        relevant_cited,
        invalid_cited: invalid.len(),
        relevant_available: item.relevant_available,
        says_not_covered: judgment.says_not_covered,
        mentions_course_materials: judgment.mentions_course_materials,
        natural_reply: judgment.natural_reply,
    }
}

pub fn score_all(runs: &[CorpusRun], judge_model_id: &str) -> Result<Vec<Scored>, Error> {
    let mut scored = Vec::new();
    for run in runs {
        for item in &run.prepared {
            let answer = run.answers.get(&item.key).ok_or_else(|| {
                Error::Cache(format!(
                    "{}/{}: the answer is not in the cache",
                    item.corpus, item.question.id
                ))
            })?;
            let judgment = run
                .judgment_key_for(judge_model_id, item)
                .and_then(|key| run.judgments.get(&key))
                .ok_or_else(|| {
                    Error::Cache(format!(
                        "{}/{}: the judgment is not in the cache",
                        item.corpus, item.question.id
                    ))
                })?;
            scored.push(score(item, &answer.text, judgment));
        }
    }
    Ok(scored)
}

pub struct Models {
    pub chat: Arc<dyn ChatModel>,
    pub judge: Arc<dyn ChatModel>,
}

pub async fn run_calls(
    runs: &mut [CorpusRun],
    models: &Models,
    choice: &ModelChoice,
    style: TutoringStyle,
) -> Result<(), Error> {
    let missing: Vec<(usize, usize)> = runs
        .iter()
        .enumerate()
        .flat_map(|(r, run)| {
            run.prepared
                .iter()
                .enumerate()
                .filter(|(_, item)| !run.answers.contains(&item.key))
                .map(move |(i, _)| (r, i))
        })
        .collect();
    eprintln!(
        "Calling {} for {} answers ...",
        choice.chat.id,
        missing.len()
    );
    let jobs: Vec<(usize, Prepared)> = missing
        .iter()
        .map(|(r, i)| (*r, runs[*r].prepared[*i].clone()))
        .collect();
    let mut stream = futures::stream::iter(jobs)
        .map(|(r, item)| {
            let model = models.chat.clone();
            async move {
                let label = format!("{}/{}", item.corpus, item.question.id);
                let done = retry(
                    &label,
                    || judge::complete(model.as_ref(), item.request.clone()),
                    |_| true,
                )
                .await;
                (r, item, done)
            }
        })
        .buffer_unordered(CONCURRENCY);
    while let Some((r, item, done)) = stream.next().await {
        let completion = done.map_err(|e| {
            Error::Aws(format!(
                "{}/{}: the answer call failed: {e}. The finished answers are saved: run the command again",
                item.corpus, item.question.id
            ))
        })?;
        let run = &mut runs[r];
        run.answers.insert(AnswerEntry {
            key: item.key.clone(),
            question_id: item.question.id.clone(),
            style: style.id().to_owned(),
            prompt_sha256: item.prompt_sha256.clone(),
            text: completion.text,
            input_tokens: completion.usage.input_tokens,
            output_tokens: completion.usage.output_tokens,
            stop_reason: completion.stop_reason,
        });
        run.answers.save()?;
    }
    drop(stream);

    let mut jobs = Vec::new();
    for (r, run) in runs.iter().enumerate() {
        for item in &run.prepared {
            let Some(key) = run.judgment_key_for(&choice.judge.model_id, item) else {
                continue;
            };
            if run.judgments.contains(&key) {
                continue;
            }
            let answer = run.answers.get(&item.key).map(|a| a.text.clone());
            if let Some(answer) = answer {
                jobs.push((r, item.corpus.clone(), item.question.clone(), key, answer));
            }
        }
    }
    eprintln!(
        "Calling {} for {} judgments ...",
        choice.judge.id,
        jobs.len()
    );
    let mut stream = futures::stream::iter(jobs)
        .map(|(r, corpus, question, key, answer)| {
            let model = models.judge.clone();
            async move {
                let label = format!("{corpus}/{} judge", question.id);
                let verdict = retry(
                    &label,
                    || judge::judge(model.as_ref(), &question.question, &answer),
                    |_| true,
                )
                .await;
                (r, corpus, question, key, answer, verdict)
            }
        })
        .buffer_unordered(CONCURRENCY);
    while let Some((r, corpus, question, key, answer, verdict)) = stream.next().await {
        let verdict = verdict.map_err(|e| {
            Error::Aws(format!(
                "{corpus}/{}: the judge call failed: {e}. The finished judgments are saved: run the command again",
                question.id
            ))
        })?;
        let run = &mut runs[r];
        run.judgments.insert(Judgment {
            key,
            question_id: question.id.clone(),
            answer_sha256: sha256_hex(answer.as_bytes()),
            says_not_covered: verdict.says_not_covered,
            mentions_course_materials: verdict.mentions_course_materials,
            natural_reply: verdict.natural_reply,
            reason: verdict.reason,
        });
        run.judgments.save()?;
    }
    Ok(())
}

pub fn prune(runs: &mut [CorpusRun], style: TutoringStyle) -> Result<usize, Error> {
    let mut removed = 0;
    for run in runs {
        let wanted: HashSet<String> = run.prepared.iter().map(|p| p.key.clone()).collect();
        let selected: HashSet<String> =
            run.prepared.iter().map(|p| p.question.id.clone()).collect();
        removed += run.answers.retain(|entry| {
            entry.style != style.id()
                || !selected.contains(&entry.question_id)
                || wanted.contains(&entry.key)
        });
        let judged_answers: HashSet<String> = run
            .answers
            .values()
            .map(|a| sha256_hex(a.text.as_bytes()))
            .collect();
        removed += run.judgments.retain(|j| {
            judged_answers.contains(&j.answer_sha256) || !selected.contains(&j.question_id)
        });
        run.answers.save()?;
        run.judgments.save()?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Kind;

    fn question(id: &str, kind: Kind) -> Question {
        Question {
            id: id.into(),
            question: format!("question {id}"),
            relevant_pages: if matches!(kind, Kind::NotCovered | Kind::OffTopic) {
                vec![]
            } else {
                vec![1]
            },
            kind,
            context: (kind == Kind::Followup).then(|| "earlier".to_owned()),
            small_talk: false,
        }
    }

    fn pool() -> Vec<(String, Question)> {
        let mut pool = Vec::new();
        for corpus in ["a", "b"] {
            for i in 0..10 {
                pool.push((corpus.to_owned(), question(&format!("l{i}"), Kind::Lookup)));
            }
            for i in 0..3 {
                pool.push((
                    corpus.to_owned(),
                    question(&format!("n{i}"), Kind::NotCovered),
                ));
                pool.push((
                    corpus.to_owned(),
                    question(&format!("o{i}"), Kind::OffTopic),
                ));
            }
        }
        pool
    }

    #[test]
    fn kinds_and_the_covered_group_are_parsed() {
        assert_eq!(
            parse_kinds("not_covered,off_topic").unwrap(),
            [Kind::NotCovered, Kind::OffTopic]
        );
        assert_eq!(parse_kinds("covered").unwrap().len(), 4);
        assert!(parse_kinds("nope").is_err());
        assert!(parse_kinds("").unwrap().is_empty());
    }

    #[test]
    fn a_selection_by_kind_keeps_only_those_kinds() {
        let picked = Selection {
            kinds: vec![Kind::NotCovered],
            ..Selection::default()
        }
        .pick(pool());
        assert_eq!(picked.len(), 6);
        assert!(picked.iter().all(|(_, q)| q.kind == Kind::NotCovered));
    }

    #[test]
    fn a_covered_sample_is_seeded_stable_and_keeps_the_other_kinds() {
        let selection = Selection {
            kinds: vec![],
            sample_covered: Some(5),
            seed: Some(7),
        };
        let first = selection.pick(pool());
        let again = selection.pick(pool());
        assert_eq!(first, again);
        let covered = first.iter().filter(|(_, q)| q.is_covered()).count();
        assert_eq!(covered, 5);
        assert_eq!(first.len(), 5 + 12);
        let other = Selection {
            seed: Some(8),
            ..selection.clone()
        }
        .pick(pool());
        assert_ne!(first, other);
        let bigger = Selection {
            sample_covered: Some(8),
            ..selection
        }
        .pick(pool());
        for item in first.iter().filter(|(_, q)| q.is_covered()) {
            assert!(
                bigger.contains(item),
                "a larger sample keeps the smaller one"
            );
        }
    }

    #[test]
    fn only_the_default_selection_is_full() {
        assert!(Selection::default().is_full());
        assert!(
            !Selection {
                sample_covered: Some(1),
                ..Selection::default()
            }
            .is_full()
        );
        assert_eq!(Selection::default().describe(), "all questions");
    }

    #[test]
    fn the_request_is_the_production_prompt_for_the_course_and_the_turns() {
        let q = question("f1", Kind::Followup);
        let request = build_request("Algebra", TutoringStyle::Direct, SourceMode::NotCovered, &q);
        assert!(request.system.contains("\"Algebra\""));
        assert!(
            request
                .system
                .contains(openbook_core::prompt::NOT_COVERED_NOTICE)
        );
        assert_eq!(request.turns.len(), 1);
        assert!(request.turns[0].text.contains("earlier"));
        assert!(request.turns[0].text.contains("question f1"));
    }

    fn prepared(sources: usize, relevant: &[bool]) -> Prepared {
        Prepared {
            corpus: "c".into(),
            question: question("q", Kind::Lookup),
            gated: false,
            sources,
            relevant_sources: relevant.to_vec(),
            relevant_available: relevant.iter().any(|r| *r),
            request: ChatRequest::default(),
            prompt_sha256: "p".into(),
            key: "k".into(),
        }
    }

    fn verdict(says: bool) -> Judgment {
        Judgment {
            key: "j".into(),
            question_id: "q".into(),
            answer_sha256: "a".into(),
            says_not_covered: says,
            mentions_course_materials: says,
            natural_reply: false,
            reason: String::new(),
        }
    }

    #[test]
    fn scoring_reads_citations_against_the_sources_of_the_prompt() {
        let item = prepared(3, &[false, true, false]);
        let scored = score(
            &item,
            "Fact [2]. Same fact [2]. Weak [1]. Ghost [7].",
            &verdict(false),
        );
        assert_eq!(scored.valid_cited, 2);
        assert_eq!(scored.relevant_cited, 1);
        assert_eq!(scored.invalid_cited, 1);
        assert!(scored.grounded() && !scored.is_uncited());
        let bare = score(&item, "No sources here.", &verdict(true));
        assert!(bare.is_uncited() && !bare.grounded() && bare.says_not_covered);
    }

    #[test]
    fn a_gated_answer_has_no_valid_citation() {
        let mut item = prepared(0, &[]);
        item.gated = true;
        let scored = score(&item, "Wrongly cited [1].", &verdict(false));
        assert_eq!((scored.valid_cited, scored.invalid_cited), (0, 1));
    }

    #[test]
    fn the_cost_bound_needs_prices() {
        let entry = ModelEntry {
            id: "m".into(),
            label: "m".into(),
            vendor: "anthropic".into(),
            provider: openbook_core::catalog::ProviderKind::Bedrock,
            model_id: "au.anthropic.x".into(),
            residency: openbook_core::catalog::Residency::Au,
            max_output_tokens: None,
            temperature: None,
            vision: false,
            input_usd_per_million: None,
            output_usd_per_million: None,
        };
        assert!(price(&entry, 1, 1).is_err());
        let priced = ModelEntry {
            input_usd_per_million: Some(2.0),
            output_usd_per_million: Some(10.0),
            ..entry
        };
        assert_eq!(price(&priced, 1000, 100).unwrap(), 3000);
        assert_eq!(bound_tokens(5), 3);
    }
}
