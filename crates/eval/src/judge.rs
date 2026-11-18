use futures::StreamExt;
use openbook_core::domain::MessageRole;
use openbook_core::model::{ChatModel, ChatRequest, ChatTurn, ModelEvent, TokenUsage};
use serde::Deserialize;

use crate::Error;

pub const RUBRIC_VERSION: u32 = 1;
pub const JUDGE_MAX_TOKENS: i32 = 300;

pub const RUBRIC: &str = "You grade one answer from a course tutor. You get the student's message and the tutor's answer. \
Reply with one JSON object and nothing else. The object has these keys:\n\
- \"says_not_covered\" (boolean): true only when the answer clearly tells the student, in plain words, that the course materials do not cover the question or its topic. \
Examples that count: \"The course materials do not cover derivatives\", \"This does not seem to be in your course materials\", \"Your course does not teach this\". \
Examples that do not count: a general warning that the tutor may be wrong, a refusal that says nothing about the course materials, an answer that only helps, and a note that one small detail is missing.\n\
- \"mentions_course_materials\" (boolean): true when the answer talks about the course materials, the sources, or what the course does or does not contain, in any way.\n\
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

pub async fn complete(model: &dyn ChatModel, request: ChatRequest) -> Result<Completion, String> {
    let mut stream = model.stream(request).await.map_err(|e| e.to_string())?;
    let mut text = String::new();
    let mut finish = None;
    while let Some(event) = stream.next().await {
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

pub async fn judge(model: &dyn ChatModel, question: &str, answer: &str) -> Result<Verdict, Error> {
    let completion = complete(model, judge_request(question, answer))
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
            (1, "8d4d7150ebd8"),
            "the rubric text changed: raise RUBRIC_VERSION and pin the new fingerprint"
        );
    }
}
