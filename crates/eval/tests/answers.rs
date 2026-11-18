use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use openbook_core::catalog::Catalog;
use openbook_core::domain::TutoringStyle;
use openbook_core::model::{
    ChatModel, ChatRequest, ModelError, ModelEvent, ModelStream, TokenUsage,
};
use openbook_core::prompt::{GATED_TURN_NOTE, NOT_COVERED_NOTICE, SOURCES_TURN_NOTE};
use openbook_eval::answer_cache::Store;
use openbook_eval::answer_report::{AnswerReport, Settings};
use openbook_eval::answers::{
    CorpusRun, Models, PromptVariant, Selection, choose_models, load_catalog, make_plan,
    prepare_corpus, prompt_fingerprint, run_calls, score_all,
};
use openbook_eval::corpus;
use openbook_eval::data::{self, Question};
use openbook_eval::judge::RUBRIC;

fn evals_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals")
}

fn catalog() -> Catalog {
    load_catalog(&evals_dir().join("../infra/cdk.json")).unwrap()
}

struct Scripted {
    answers: AtomicUsize,
    judgments: AtomicUsize,
    says_not_covered: bool,
}

impl Scripted {
    fn new(says_not_covered: bool) -> Arc<Self> {
        Arc::new(Self {
            answers: AtomicUsize::new(0),
            judgments: AtomicUsize::new(0),
            says_not_covered,
        })
    }
}

#[async_trait]
impl ChatModel for Scripted {
    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        let text = if request.system == RUBRIC {
            self.judgments.fetch_add(1, Ordering::SeqCst);
            format!(
                "{{\"says_not_covered\": {}, \"mentions_course_materials\": {}, \"natural_reply\": true, \"reason\": \"scripted\"}}",
                self.says_not_covered, self.says_not_covered
            )
        } else {
            self.answers.fetch_add(1, Ordering::SeqCst);
            "A scripted answer [1].".to_owned()
        };
        let events = vec![
            Ok(ModelEvent::Delta(text)),
            Ok(ModelEvent::Finish {
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: "end_turn".into(),
            }),
        ];
        Ok(futures::stream::iter(events).boxed())
    }
}

fn question(corpus_id: &str, id: &str) -> Question {
    let evals = evals_dir();
    let corpora = corpus::select(&evals, Some(corpus_id)).unwrap();
    let sections = data::load_sections(&corpora[0].sections_path()).unwrap();
    data::load_golden(&corpora[0].golden_path(), data::page_total(&sections))
        .unwrap()
        .into_iter()
        .find(|q| q.id == id)
        .unwrap()
}

async fn prepared(corpus_id: &str, ids: &[&str], style: PromptVariant, dir: &Path) -> CorpusRun {
    let evals = evals_dir();
    let corpus = corpus::select(&evals, Some(corpus_id)).unwrap().remove(0);
    let choice = choose_models(&catalog(), None, None).unwrap();
    let questions: Vec<Question> = ids.iter().map(|id| question(corpus_id, id)).collect();
    let prepared = prepare_corpus(&corpus, &questions, style, &choice.chat)
        .await
        .unwrap();
    CorpusRun {
        corpus,
        prepared,
        answers: Store::load(dir.join("answers.jsonl")).unwrap(),
        judgments: Store::load(dir.join("judgments.jsonl")).unwrap(),
    }
}

fn plan_for(run: &CorpusRun, style: PromptVariant) -> openbook_eval::answers::Plan {
    let choice = choose_models(&catalog(), None, None).unwrap();
    make_plan(
        std::slice::from_ref(run),
        &choice,
        style,
        &Selection::default(),
        "r".into(),
        "p".into(),
    )
    .unwrap()
}

#[tokio::test]
async fn the_derivative_question_gets_sources_that_do_not_answer_it() {
    let dir = tempfile::tempdir().unwrap();
    let run = prepared(
        "college-algebra",
        &["not-covered-03"],
        PromptVariant::new(TutoringStyle::Direct),
        dir.path(),
    )
    .await;
    let item = &run.prepared[0];
    assert!(!item.gated && item.sources > 0);
    assert!(!item.relevant_available);
    assert!(item.request.system.contains("<source n=\"1\""));
    assert!(!item.request.system.contains(NOT_COVERED_NOTICE));
    assert_eq!(item.request.turns.len(), 1);
    assert_eq!(
        item.request.turns[0].text,
        format!(
            "{SOURCES_TURN_NOTE}\n\n<student_message>\n{}\n</student_message>",
            item.question.question
        )
    );
}

#[tokio::test]
async fn small_talk_is_gated_and_gets_the_notice() {
    let dir = tempfile::tempdir().unwrap();
    let run = prepared(
        "college-algebra",
        &["off-topic-01"],
        PromptVariant::new(TutoringStyle::Socratic),
        dir.path(),
    )
    .await;
    let item = &run.prepared[0];
    assert!(item.gated && item.sources == 0);
    assert!(item.question.small_talk);
    assert!(item.request.turns[0].text.starts_with(GATED_TURN_NOTE));
    assert!(item.request.system.contains(NOT_COVERED_NOTICE));
    assert!(!item.request.system.contains("<source "));
}

#[tokio::test]
async fn a_follow_up_goes_with_its_earlier_question() {
    let dir = tempfile::tempdir().unwrap();
    let run = prepared(
        "college-algebra",
        &["followup-01"],
        PromptVariant::new(TutoringStyle::Direct),
        dir.path(),
    )
    .await;
    let text = &run.prepared[0].request.turns[0].text;
    assert!(text.contains("What is the product rule of exponents?"));
    assert!(text.contains("And what about dividing instead?"));
}

#[tokio::test]
async fn an_unchanged_prompt_never_calls_again_and_a_changed_one_does() {
    let dir = tempfile::tempdir().unwrap();
    let ids = ["lookup-01", "not-covered-03", "off-topic-01"];
    let choice = choose_models(&catalog(), None, None).unwrap();
    let chat = Scripted::new(false);
    let judge = Scripted::new(true);
    let models = Models {
        chat: chat.clone(),
        judge: judge.clone(),
    };

    let mut run = prepared(
        "college-algebra",
        &ids,
        PromptVariant::new(TutoringStyle::Direct),
        dir.path(),
    )
    .await;
    let plan = plan_for(&run, PromptVariant::new(TutoringStyle::Direct));
    assert_eq!((plan.answer_calls, plan.judge_calls), (3, 3));
    assert!(plan.cost_upper_bound_usd > 0.0);
    run_calls(
        std::slice::from_mut(&mut run),
        &models,
        &choice,
        PromptVariant::new(TutoringStyle::Direct),
    )
    .await
    .unwrap();
    assert_eq!(chat.answers.load(Ordering::SeqCst), 3);
    assert_eq!(judge.judgments.load(Ordering::SeqCst), 3);

    let mut again = prepared(
        "college-algebra",
        &ids,
        PromptVariant::new(TutoringStyle::Direct),
        dir.path(),
    )
    .await;
    let plan = plan_for(&again, PromptVariant::new(TutoringStyle::Direct));
    assert!(plan.is_empty(), "{}", plan.describe());
    assert_eq!(plan.cached_answers, 3);
    run_calls(
        std::slice::from_mut(&mut again),
        &models,
        &choice,
        PromptVariant::new(TutoringStyle::Direct),
    )
    .await
    .unwrap();
    assert_eq!(chat.answers.load(Ordering::SeqCst), 3);

    let scored = score_all(std::slice::from_ref(&again), &choice.judge.model_id).unwrap();
    assert_eq!(scored.len(), 3);
    assert!(scored.iter().all(|s| s.says_not_covered));
    let report = AnswerReport::build(
        Settings {
            chat_model: choice.chat.model_id.clone(),
            judge_model: choice.judge.model_id.clone(),
            style: "direct".into(),
            rubric_version: 1,
            prompt_fingerprint: prompt_fingerprint(std::slice::from_ref(&again)),
            selection: "test".into(),
            turn_note: true,
        },
        &scored,
    );
    assert_eq!(
        report.pooled.not_covered.all.says_not_covered_rate,
        Some(1.0)
    );
    assert_eq!(report.pooled.off_topic.small_talk.correct_rate, Some(0.0));

    let socratic = prepared(
        "college-algebra",
        &ids,
        PromptVariant::new(TutoringStyle::Socratic),
        dir.path(),
    )
    .await;
    let plan = plan_for(&socratic, PromptVariant::new(TutoringStyle::Socratic));
    assert_eq!(plan.answer_calls, 3, "another style is another prompt");
}

#[tokio::test]
async fn prune_removes_stale_answers_of_the_same_style_only() {
    use openbook_eval::answer_cache::AnswerEntry;
    use openbook_eval::answers::prune;

    let dir = tempfile::tempdir().unwrap();
    let ids = ["lookup-01", "not-covered-03"];
    let choice = choose_models(&catalog(), None, None).unwrap();
    let models = Models {
        chat: Scripted::new(false),
        judge: Scripted::new(false),
    };
    let mut direct = prepared(
        "college-algebra",
        &ids,
        PromptVariant::new(TutoringStyle::Direct),
        dir.path(),
    )
    .await;
    run_calls(
        std::slice::from_mut(&mut direct),
        &models,
        &choice,
        PromptVariant::new(TutoringStyle::Direct),
    )
    .await
    .unwrap();
    let mut socratic = prepared(
        "college-algebra",
        &ids,
        PromptVariant::new(TutoringStyle::Socratic),
        dir.path(),
    )
    .await;
    socratic.answers = direct.answers.clone();
    socratic.judgments = direct.judgments.clone();
    run_calls(
        std::slice::from_mut(&mut socratic),
        &models,
        &choice,
        PromptVariant::new(TutoringStyle::Socratic),
    )
    .await
    .unwrap();
    assert_eq!(socratic.answers.len(), 4);
    socratic.answers.insert(AnswerEntry {
        key: "stale".into(),
        question_id: "lookup-01".into(),
        style: "socratic".into(),
        prompt_sha256: "old".into(),
        text: "An answer to an old prompt.".into(),
        input_tokens: 1,
        output_tokens: 1,
        stop_reason: "end_turn".into(),
    });
    let removed = prune(
        std::slice::from_mut(&mut socratic),
        PromptVariant::new(TutoringStyle::Socratic),
        &choice.judge.model_id,
    )
    .unwrap();
    assert_eq!(removed, 1);
    assert_eq!(socratic.answers.len(), 4);
    assert!(!socratic.answers.contains("stale"));
}

#[tokio::test]
async fn the_ablation_has_no_note_and_its_own_cache_keys_and_the_check_needs_a_full_cache() {
    use openbook_eval::answers::check_complete;

    let dir = tempfile::tempdir().unwrap();
    let ids = ["not-covered-03"];
    let normal = prepared(
        "college-algebra",
        &ids,
        PromptVariant::new(TutoringStyle::Socratic),
        dir.path(),
    )
    .await;
    let ablation = prepared(
        "college-algebra",
        &ids,
        PromptVariant {
            turn_note: false,
            ..PromptVariant::new(TutoringStyle::Socratic)
        },
        dir.path(),
    )
    .await;
    assert_eq!(
        ablation.prepared[0].request.turns[0].text,
        ablation.prepared[0].question.question
    );
    assert_eq!(
        ablation.prepared[0].request.system,
        normal.prepared[0].request.system
    );
    assert_ne!(ablation.prepared[0].key, normal.prepared[0].key);
    let plan = plan_for(&normal, PromptVariant::new(TutoringStyle::Socratic));
    let error = check_complete(&plan).unwrap_err().to_string();
    assert!(
        error.contains("1 answers and 1 judgments are missing"),
        "{error}"
    );
}
