use std::time::{Duration, Instant};

use futures::StreamExt;
use openbook_core::domain::MessageRole;
use openbook_core::model::{ChatModel, ChatRequest, ChatTurn, ModelEvent, TokenUsage};
use serde::Deserialize;

use crate::Error;

pub const RUBRIC_VERSION: u32 = 2;
pub const JUDGE_MAX_TOKENS: i32 = 300;

pub const RUBRIC: &str = "You grade one answer from a course tutor. You get the student's message and the tutor's answer. \
Reply with one JSON object and nothing else. The object has these keys:\n\
- \"says_not_covered\" (boolean): true only when the answer tells the student in plain words that the question or its topic is not covered by the course or its materials, or is outside the course or its subject. \
Examples that count: \"The course materials do not cover derivatives\", \"This does not seem to be in your course materials\", \"That is outside the scope of American Government\", \"That is a history question, not a College Algebra topic\", \"Derivatives are a calculus topic, beyond this course\". \
Examples that do not count: a general warning that the tutor may be wrong, a polite redirect that never says the question is outside the course (\"What would you like to study today?\"), an answer that only helps, a note that one small detail is missing, and a remark that the topic connects to what the student is studying.\n\
- \"mentions_course_materials\" (boolean): true only when the answer talks about what the course materials, the sources, or the course content contain or do not contain. \
A greeting that only names the subject (\"Ready to help you with College Algebra\") is false.\n\
- \"natural_reply\" (boolean): true when the answer is a natural, friendly, short reply to the student's message, as a person would write it. \
It is false when the answer refuses, lectures, pushes the student back to the course without a reason, or ignores the message.\n\
- \"reason\" (string): one short sentence.\n\
Judge only the words of the answer. Do not judge whether the math or the facts are right.";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Verdict {
    pub says_not_covered: bool,
    pub mentions_course_materials: bool,
    pub natural_reply: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub usage: TokenUsage,
    pub stop_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallLimits {
    pub first_byte: Duration,
    pub idle: Duration,
    pub total: Duration,
}

pub const ANSWER_LIMITS: CallLimits = CallLimits {
    first_byte: Duration::from_secs(60),
    idle: Duration::from_secs(30),
    total: Duration::from_secs(240),
};

pub const JUDGE_LIMITS: CallLimits = CallLimits {
    first_byte: Duration::from_secs(30),
    idle: Duration::from_secs(20),
    total: Duration::from_secs(90),
};

pub async fn complete(
    model: &dyn ChatModel,
    request: ChatRequest,
    limits: CallLimits,
) -> Result<Completion, String> {
    tokio::time::timeout(limits.total, read_stream(model, request, limits))
        .await
        .map_err(|_| format!("no finished answer within {:?}", limits.total))?
}

async fn read_stream(
    model: &dyn ChatModel,
    request: ChatRequest,
    limits: CallLimits,
) -> Result<Completion, String> {
    let started = Instant::now();
    let mut stream = tokio::time::timeout(limits.first_byte, model.stream(request))
        .await
        .map_err(|_| format!("the call did not start within {:?}", limits.first_byte))?
        .map_err(|e| e.to_string())?;
    let mut text = String::new();
    let mut finish = None;
    let mut first = true;
    loop {
        let wait = if first {
            limits.first_byte.saturating_sub(started.elapsed())
        } else {
            limits.idle
        };
        let next = tokio::time::timeout(wait, stream.next())
            .await
            .map_err(|_| {
                if first {
                    format!("no first byte within {:?}", limits.first_byte)
                } else {
                    format!("the stream was idle for {:?}", limits.idle)
                }
            })?;
        let Some(event) = next else { break };
        first = false;
        match event.map_err(|e| e.to_string())? {
            ModelEvent::Delta(delta) => text.push_str(&delta),
            ModelEvent::Finish { usage, stop_reason } => finish = Some((usage, stop_reason)),
        }
    }
    let (usage, stop_reason) = finish.ok_or("the model stream ended without a finish event")?;
    Ok(Completion {
        text,
        usage,
        stop_reason,
    })
}

pub fn judge_request(question: &str, answer: &str) -> ChatRequest {
    ChatRequest {
        system: RUBRIC.to_owned(),
        turns: vec![ChatTurn {
            role: MessageRole::User,
            text: format!(
                "Student message:\n<<<\n{question}\n>>>\n\nTutor answer:\n<<<\n{answer}\n>>>"
            ),
        }],
        max_tokens: Some(JUDGE_MAX_TOKENS),
        temperature: Some(0.0),
    }
}

pub fn parse_verdict(text: &str) -> Result<Verdict, Error> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err(Error::Run(format!("the judge gave no JSON: {text:?}")));
    };
    if end < start {
        return Err(Error::Run(format!("the judge gave no JSON: {text:?}")));
    }
    serde_json::from_str(&text[start..=end])
        .map_err(|e| Error::Run(format!("the judge gave a bad verdict ({e}): {text:?}")))
}

pub async fn judge(
    model: &dyn ChatModel,
    question: &str,
    answer: &str,
    limits: CallLimits,
) -> Result<Verdict, Error> {
    let completion = complete(model, judge_request(question, answer), limits)
        .await
        .map_err(Error::Aws)?;
    parse_verdict(&completion.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_is_read_from_plain_json_and_from_json_in_a_fence() {
        let plain = r#"{"says_not_covered": true, "mentions_course_materials": true, "natural_reply": false, "reason": "It says so."}"#;
        let verdict = parse_verdict(plain).unwrap();
        assert!(verdict.says_not_covered && verdict.mentions_course_materials);
        assert!(!verdict.natural_reply);
        let fenced = format!("```json\n{plain}\n```");
        assert_eq!(parse_verdict(&fenced).unwrap(), verdict);
    }

    #[test]
    fn a_broken_verdict_is_an_error() {
        assert!(parse_verdict("I think yes").is_err());
        assert!(parse_verdict("} {").is_err());
        assert!(parse_verdict(r#"{"says_not_covered": "maybe"}"#).is_err());
        assert!(parse_verdict(r#"{"says_not_covered": true}"#).is_err());
    }

    #[test]
    fn the_request_holds_the_rubric_and_both_texts() {
        let request = judge_request("What is 2+2?", "It is 4.");
        assert_eq!(request.system, RUBRIC);
        assert!(request.turns[0].text.contains("What is 2+2?"));
        assert!(request.turns[0].text.contains("It is 4."));
        assert_eq!(request.temperature, Some(0.0));
    }

    #[test]
    fn the_rubric_version_follows_the_text_of_the_rubric() {
        let fingerprint = crate::answer_cache::sha256_parts(&[RUBRIC]);
        assert_eq!(
            (RUBRIC_VERSION, &fingerprint[..12]),
            (2, "afff63e77624"),
            "the rubric text changed: raise RUBRIC_VERSION and pin the new fingerprint"
        );
    }

    struct Stalled;

    #[async_trait::async_trait]
    impl ChatModel for Stalled {
        async fn stream(
            &self,
            _request: ChatRequest,
        ) -> Result<openbook_core::model::ModelStream, openbook_core::model::ModelError> {
            futures::future::pending().await
        }
    }

    struct StallsAfterOneDelta;

    #[async_trait::async_trait]
    impl ChatModel for StallsAfterOneDelta {
        async fn stream(
            &self,
            _request: ChatRequest,
        ) -> Result<openbook_core::model::ModelStream, openbook_core::model::ModelError> {
            let first = futures::stream::iter(vec![Ok(ModelEvent::Delta("a".into()))]);
            Ok(first.chain(futures::stream::pending()).boxed())
        }
    }

    fn limits(first_byte: u64, idle: u64, total: u64) -> CallLimits {
        CallLimits {
            first_byte: Duration::from_millis(first_byte),
            idle: Duration::from_millis(idle),
            total: Duration::from_millis(total),
        }
    }

    #[tokio::test]
    async fn a_call_that_never_starts_times_out() {
        let error = complete(&Stalled, judge_request("q", "a"), limits(30, 30, 500))
            .await
            .unwrap_err();
        assert!(error.contains("did not start"), "{error}");
    }

    #[tokio::test]
    async fn a_stream_that_goes_quiet_times_out_as_idle() {
        let error = complete(
            &StallsAfterOneDelta,
            judge_request("q", "a"),
            limits(500, 30, 5000),
        )
        .await
        .unwrap_err();
        assert!(error.contains("idle"), "{error}");
    }

    #[tokio::test]
    async fn the_total_time_is_a_hard_limit() {
        let error = complete(
            &StallsAfterOneDelta,
            judge_request("q", "a"),
            limits(500, 5000, 60),
        )
        .await
        .unwrap_err();
        assert!(error.contains("no finished answer"), "{error}");
    }
}
