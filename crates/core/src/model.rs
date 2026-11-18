//! Chat model providers: Bedrock (ConverseStream) and a deterministic fake for local runs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ContentBlockDelta, ConversationRole, ConverseStreamOutput,
    InferenceConfiguration, Message, SystemContentBlock,
};
use futures::stream::{self, BoxStream, StreamExt};

use crate::catalog::{Catalog, ModelEntry, ProviderKind};
use crate::domain::MessageRole;
use crate::prompt::{
    NOT_COVERED_NOTICE, REWRITE_LAST_MESSAGE_MARKER, REWRITE_SYSTEM_PROMPT, style_in_prompt,
};

/// One earlier or new message in the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurn {
    pub role: MessageRole,
    pub text: String,
}

/// Everything the server decides for one model call.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub system: String,
    /// Starts with a user turn, alternates roles, and ends with the new user turn.
    pub turns: Vec<ChatTurn>,
    /// Replaces the output limit of the catalog entry (the query rewrite uses a small one).
    pub max_tokens: Option<i32>,
    /// Replaces the temperature of the catalog entry.
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEvent {
    Delta(String),
    /// The last event of a successful call.
    Finish {
        usage: TokenUsage,
        stop_reason: String,
    },
}

/// The model failed or throttled. The text is for logs; it never goes to clients.
#[derive(Debug, Clone, thiserror::Error)]
#[error("model unavailable: {0}")]
pub struct ModelError(pub String);

pub type ModelStream = BoxStream<'static, Result<ModelEvent, ModelError>>;

#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError>;
}

/// Estimated tokens for a text: one token per 4 characters, rounded up.
pub fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

// ---------------------------------------------------------------------------------------------
// Fake provider
// ---------------------------------------------------------------------------------------------

/// Deterministic provider for local runs and tests.
/// It repeats the question, uses Markdown, one inline and one block formula, and streams small deltas.
#[derive(Debug, Clone)]
pub struct FakeChat {
    /// The catalog ID. The answer names it, so tests and users can see which model answered.
    model: String,
    delta_delay: Duration,
}

impl FakeChat {
    /// Characters in one delta.
    const CHUNK_CHARS: usize = 12;

    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            delta_delay: Duration::from_millis(20),
        }
    }

    /// Changes the pause between deltas. Tests use zero.
    pub fn with_delay(model: impl Into<String>, delta_delay: Duration) -> Self {
        Self {
            model: model.into(),
            delta_delay,
        }
    }

    /// The full answer for a question.
    pub fn answer(model: &str, question: &str) -> String {
        Self::answer_with(model, question, "")
    }

    pub fn answer_styled(
        model: &str,
        question: &str,
        style: crate::domain::TutoringStyle,
    ) -> String {
        Self::answer_with(
            model,
            question,
            &format!("Tutoring style in use: `{}`.", style.id()),
        )
    }

    /// The answer with a paragraph about the course materials (empty for none).
    pub fn answer_with(model: &str, question: &str, materials: &str) -> String {
        let materials = if materials.is_empty() {
            String::new()
        } else {
            format!("{materials}\n\n")
        };
        format!(
            "You asked: \"{question}\"\n\n\
             Here is a short **fake** answer in Markdown from the model `{model}`.\n\n\
             {materials}\
             - Inline formula: $a^2 + b^2 = c^2$\n\
             - Block formula:\n\n\
             $$\\int_0^1 x^2 \\, dx = \\frac{{1}}{{3}}$$\n\n\
             This text comes from the local fake model provider."
        )
    }

    /// What the fake model says about the course materials, from the system prompt.
    /// With sources it quotes a few words of the first source and cites `[1]`, and of the second and
    /// cites `[2]` when there are two or more. When the prompt says that the materials do not cover
    /// the question, it says so too.
    pub fn materials_paragraph(system: &str) -> String {
        let sources = parse_sources(system);
        if sources.is_empty() {
            return if system.contains(NOT_COVERED_NOTICE) {
                "The course materials do not cover this question.".to_owned()
            } else {
                String::new()
            };
        }
        sources
            .iter()
            .take(2)
            .enumerate()
            .map(|(i, text)| {
                let quote: Vec<&str> = text.split_whitespace().take(6).collect();
                format!(
                    "The course materials say \"{}\" [{}].",
                    quote.join(" "),
                    i + 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The text of a request: a rewrite, an answer with the materials paragraph, or the plain answer.
    fn reply(model: &str, request: &ChatRequest, question: &str) -> String {
        if request.system == REWRITE_SYSTEM_PROMPT {
            // The "rewrite" is the last student message itself.
            return question
                .rsplit(REWRITE_LAST_MESSAGE_MARKER)
                .next()
                .unwrap_or(question)
                .trim()
                .to_owned();
        }
        let style = style_in_prompt(&request.system)
            .map(|style| format!("Tutoring style in use: `{}`.\n\n", style.id()))
            .unwrap_or_default();
        let materials = Self::materials_paragraph(&request.system);
        let preface = format!("{style}{materials}");
        Self::answer_with(model, question, preface.trim_end())
    }

    /// The answer split into the deltas that the stream sends.
    pub fn deltas(model: &str, question: &str) -> Vec<String> {
        Self::chunks(&Self::answer(model, question))
    }

    fn chunks(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        chars
            .chunks(Self::CHUNK_CHARS)
            .map(|c| c.iter().collect())
            .collect()
    }
}

/// The passage texts in `<source n="..." ...>` tags of a system prompt, unescaped, in order.
fn parse_sources(system: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = system;
    while let Some(start) = rest.find("<source n=\"") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let body = &after[open_end + 1..];
        let Some(close) = body.find("</source>") else {
            break;
        };
        let text = body[..close]
            .trim()
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&");
        out.push(text);
        rest = &body[close + "</source>".len()..];
    }
    out
}

/// What a fake model does instead of answering, chosen by a marker in the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FakeFailure {
    FailToStart,
    FailBeforeDelta,
    FailAfterDelta,
    StallBeforeDelta,
    StallAfterDelta,
}

impl FakeFailure {
    fn from_question(question: &str) -> Option<Self> {
        [
            ("[fake:fail-start]", Self::FailToStart),
            ("[fake:fail-before-delta]", Self::FailBeforeDelta),
            ("[fake:fail-after-delta]", Self::FailAfterDelta),
            ("[fake:stall-before-delta]", Self::StallBeforeDelta),
            ("[fake:stall-after-delta]", Self::StallAfterDelta),
        ]
        .into_iter()
        .find(|(marker, _)| question.contains(marker))
        .map(|(_, failure)| failure)
    }
}

#[async_trait]
impl ChatModel for FakeChat {
    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        let question = request
            .turns
            .iter()
            .rev()
            .find(|t| t.role == MessageRole::User)
            .map(|t| crate::prompt::student_text(&t.text))
            .unwrap_or_default();
        let input_chars = request.system.chars().count()
            + request
                .turns
                .iter()
                .map(|t| t.text.chars().count())
                .sum::<usize>();
        let deltas = Self::chunks(&Self::reply(&self.model, &request, &question));
        let output_chars: usize = deltas.iter().map(|d| d.chars().count()).sum();
        let usage = TokenUsage {
            input_tokens: estimate_tokens(input_chars),
            output_tokens: estimate_tokens(output_chars),
        };
        let delay = self.delta_delay;

        // Failure modes for tests. A marker in the question picks one. Local only: the API refuses fake
        // models in Lambda.
        // Only the last paragraph counts: after a failed turn, the history merges user turns.
        let fail = FakeFailure::from_question(question.rsplit("\n\n").next().unwrap_or(""));
        if fail == Some(FakeFailure::FailToStart) {
            return Err(ModelError("fake: failed to start".into()));
        }

        let mut events: Vec<Result<ModelEvent, ModelError>> = deltas
            .into_iter()
            .map(|d| Ok(ModelEvent::Delta(d)))
            .collect();
        match fail {
            Some(FakeFailure::FailBeforeDelta) => {
                events = vec![Err(ModelError("fake: failed".into()))]
            }
            Some(FakeFailure::FailAfterDelta) => {
                events.truncate(2);
                events.push(Err(ModelError("fake: failed mid-stream".into())));
            }
            Some(FakeFailure::StallBeforeDelta) => events.clear(),
            Some(FakeFailure::StallAfterDelta) => events.truncate(2),
            _ => events.push(Ok(ModelEvent::Finish {
                usage,
                stop_reason: "end_turn".into(),
            })),
        }
        let stalls = matches!(
            fail,
            Some(FakeFailure::StallBeforeDelta | FakeFailure::StallAfterDelta)
        );
        let stream = stream::unfold(events.into_iter().enumerate(), move |mut it| async move {
            match it.next() {
                Some((index, event)) => {
                    if index > 0 && !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    Some((event, it))
                }
                None if stalls => std::future::pending().await,
                None => None,
            }
        });
        Ok(stream.boxed())
    }
}

// ---------------------------------------------------------------------------------------------
// Bedrock provider
// ---------------------------------------------------------------------------------------------

/// Calls Bedrock `ConverseStream` with a server-fixed model ID.
#[derive(Debug, Clone)]
pub struct BedrockChat {
    client: BedrockClient,
    /// The Bedrock model or inference profile ID.
    model_id: String,
    max_tokens: i32,
    temperature: Option<f32>,
}

impl BedrockChat {
    /// Uses the entry's model ID, output limit, and (optional) temperature.
    pub fn new(client: BedrockClient, entry: &ModelEntry) -> Self {
        Self {
            client,
            model_id: entry.model_id.clone(),
            max_tokens: entry.max_tokens(),
            temperature: entry.temperature,
        }
    }
}

fn unavailable(context: &str, error: &dyn std::fmt::Debug) -> ModelError {
    tracing::error!(?error, "bedrock {context} failed");
    ModelError(format!("bedrock {context} failed"))
}

#[async_trait]
impl ChatModel for BedrockChat {
    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        let mut messages = Vec::with_capacity(request.turns.len());
        for turn in &request.turns {
            let role = match turn.role {
                MessageRole::User => ConversationRole::User,
                MessageRole::Assistant => ConversationRole::Assistant,
            };
            let message = Message::builder()
                .role(role)
                .content(ContentBlock::Text(turn.text.clone()))
                .build()
                .map_err(|e| unavailable("message build", &e))?;
            messages.push(message);
        }

        let output = self
            .client
            .converse_stream()
            .model_id(&self.model_id)
            .system(SystemContentBlock::Text(request.system))
            .set_messages(Some(messages))
            .inference_config(
                InferenceConfiguration::builder()
                    .max_tokens(request.max_tokens.unwrap_or(self.max_tokens))
                    .set_temperature(request.temperature.or(self.temperature))
                    .build(),
            )
            .send()
            .await
            .map_err(|e| unavailable("converse_stream", &e))?;

        struct State {
            receiver: aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver<
                ConverseStreamOutput,
                aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError,
            >,
            stop_reason: String,
            usage: TokenUsage,
            finished: bool,
        }
        let state = State {
            receiver: output.stream,
            stop_reason: "end_turn".into(),
            usage: TokenUsage::default(),
            finished: false,
        };

        let stream = stream::unfold(state, |mut state| async move {
            if state.finished {
                return None;
            }
            loop {
                match state.receiver.recv().await {
                    Ok(Some(ConverseStreamOutput::ContentBlockDelta(event))) => {
                        if let Some(ContentBlockDelta::Text(text)) = event.delta()
                            && !text.is_empty()
                        {
                            let delta = ModelEvent::Delta(text.clone());
                            return Some((Ok(delta), state));
                        }
                    }
                    Ok(Some(ConverseStreamOutput::MessageStop(event))) => {
                        state.stop_reason = event.stop_reason().as_str().to_owned();
                    }
                    Ok(Some(ConverseStreamOutput::Metadata(event))) => {
                        if let Some(usage) = event.usage() {
                            state.usage = TokenUsage {
                                input_tokens: u64::try_from(usage.input_tokens()).unwrap_or(0),
                                output_tokens: u64::try_from(usage.output_tokens()).unwrap_or(0),
                            };
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        state.finished = true;
                        let finish = ModelEvent::Finish {
                            usage: state.usage,
                            stop_reason: state.stop_reason.clone(),
                        };
                        return Some((Ok(finish), state));
                    }
                    Err(e) => {
                        state.finished = true;
                        return Some((Err(unavailable("stream", &e)), state));
                    }
                }
            }
        });
        Ok(stream.boxed())
    }
}

// ---------------------------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------------------------

/// The catalog plus one provider per entry.
#[derive(Clone)]
pub struct ModelRegistry {
    catalog: Arc<Catalog>,
    providers: Arc<HashMap<String, Arc<dyn ChatModel>>>,
}

impl ModelRegistry {
    /// Builds a provider for every catalog entry. Bedrock entries need `bedrock`.
    /// `fake_delay` is the pause between deltas of fake entries.
    pub fn from_catalog(
        catalog: Catalog,
        bedrock: Option<BedrockClient>,
        fake_delay: Duration,
    ) -> Result<Self, String> {
        let mut providers: HashMap<String, Arc<dyn ChatModel>> = HashMap::new();
        for entry in catalog.entries() {
            let provider: Arc<dyn ChatModel> = match entry.provider {
                ProviderKind::Fake => Arc::new(FakeChat::with_delay(entry.id.clone(), fake_delay)),
                ProviderKind::Bedrock => {
                    let client = bedrock
                        .clone()
                        .ok_or_else(|| format!("model {:?} needs a Bedrock client", entry.id))?;
                    Arc::new(BedrockChat::new(client, entry))
                }
            };
            providers.insert(entry.id.clone(), provider);
        }
        Ok(Self {
            catalog: Arc::new(catalog),
            providers: Arc::new(providers),
        })
    }

    /// Replaces the provider of a catalog entry. Tests use it to put a scripted model in place.
    pub fn with_provider(self, id: &str, provider: Arc<dyn ChatModel>) -> Self {
        let mut providers = (*self.providers).clone();
        providers.insert(id.to_owned(), provider);
        Self {
            catalog: self.catalog,
            providers: Arc::new(providers),
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The provider for a catalog ID.
    pub fn provider(&self, id: &str) -> Option<Arc<dyn ChatModel>> {
        self.providers.get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(question: &str) -> ChatRequest {
        ChatRequest {
            system: "sys".into(),
            turns: vec![ChatTurn {
                role: MessageRole::User,
                text: question.into(),
            }],
            ..ChatRequest::default()
        }
    }

    async fn collect(model: &FakeChat, question: &str) -> Vec<ModelEvent> {
        let mut stream = model.stream(request(question)).await.unwrap();
        let mut out = Vec::new();
        while let Some(event) = stream.next().await {
            out.push(event.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn fake_streams_deltas_then_finish() {
        let events = collect(
            &FakeChat::with_delay("fake", Duration::ZERO),
            "What is 2+2?",
        )
        .await;
        let (last, deltas) = events.split_last().unwrap();
        assert!(deltas.len() > 3, "expected small deltas");
        let text: String = deltas
            .iter()
            .map(|e| match e {
                ModelEvent::Delta(t) => t.as_str(),
                ModelEvent::Finish { .. } => panic!("finish before the end"),
            })
            .collect();
        assert_eq!(text, FakeChat::answer("fake", "What is 2+2?"));
        assert!(text.contains("What is 2+2?"));
        assert!(text.contains("$a^2 + b^2 = c^2$"));
        assert!(text.contains("$$\\int_0^1 x^2 \\, dx = \\frac{1}{3}$$"));
        let ModelEvent::Finish { usage, stop_reason } = last else {
            panic!("no finish")
        };
        assert_eq!(stop_reason, "end_turn");
        assert_eq!(usage.output_tokens, estimate_tokens(text.chars().count()));
        assert_eq!(
            usage.input_tokens,
            estimate_tokens("sys".len() + "What is 2+2?".len())
        );
    }

    #[tokio::test]
    async fn fake_is_deterministic() {
        let model = FakeChat::with_delay("fake", Duration::ZERO);
        assert_eq!(collect(&model, "same").await, collect(&model, "same").await);
        assert_ne!(collect(&model, "one").await, collect(&model, "two").await);
    }

    #[tokio::test(start_paused = true)]
    async fn fake_pauses_between_deltas() {
        let started = tokio::time::Instant::now();
        let events = collect(&FakeChat::new("fake"), "q").await;
        // One pause between each pair of events.
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(20) * (events.len() as u32 - 1)
        );
    }

    #[test]
    fn token_estimate_is_one_per_four_chars() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
        assert_eq!(estimate_tokens(400), 100);
    }

    fn system_with(sources: &[&str]) -> String {
        let mut text = String::from("You are a tutor.");
        for (i, source) in sources.iter().enumerate() {
            text.push_str(&format!(
                "\n<source n=\"{}\" document=\"d.pdf\" page=\"2\">\n{source}\n</source>\n",
                i + 1
            ));
        }
        text
    }

    async fn say(model: &FakeChat, system: &str, question: &str) -> String {
        let mut request = request(question);
        request.system = system.to_owned();
        let mut stream = model.stream(request).await.unwrap();
        let mut out = String::new();
        while let Some(event) = stream.next().await {
            if let ModelEvent::Delta(d) = event.unwrap() {
                out.push_str(&d);
            }
        }
        out
    }

    #[tokio::test]
    async fn with_one_source_the_fake_answer_quotes_it_and_cites_one() {
        let model = FakeChat::with_delay("fake", Duration::ZERO);
        let system = system_with(&[
            "The Pythagorean theorem relates the sides of a right triangle &amp; more.",
        ]);
        let answer = say(&model, &system, "What is it?").await;
        assert!(
            answer.contains("\"The Pythagorean theorem relates the sides\" [1]."),
            "{answer}"
        );
        assert!(!answer.contains("[2]"));
        assert!(answer.contains("You asked: \"What is it?\""));
    }

    #[tokio::test]
    async fn with_two_sources_it_cites_one_and_two() {
        let model = FakeChat::with_delay("fake", Duration::ZERO);
        let system = system_with(&[
            "First passage about triangles and angles.",
            "Second passage about circles.",
        ]);
        let answer = say(&model, &system, "q").await;
        assert!(
            answer.contains("\"First passage about triangles and angles.\" [1]."),
            "{answer}"
        );
        assert!(
            answer.contains("\"Second passage about circles.\" [2]."),
            "{answer}"
        );
        let three = system_with(&["a b", "c d", "e f"]);
        assert!(!say(&model, &three, "q").await.contains("[3]"));
    }

    #[tokio::test]
    async fn without_sources_the_fake_says_so_only_when_the_prompt_asks_for_it() {
        let model = FakeChat::with_delay("fake", Duration::ZERO);
        let gated = format!("tutor\n\n{NOT_COVERED_NOTICE}");
        assert!(
            say(&model, &gated, "q")
                .await
                .contains("The course materials do not cover this question.")
        );
        // No retrieval at all: the plain answer, as before.
        assert_eq!(
            say(&model, "tutor", "q").await,
            FakeChat::answer("fake", "q")
        );
    }

    #[tokio::test]
    async fn the_fake_rewrite_returns_the_last_student_message() {
        let model = FakeChat::with_delay("fake", Duration::ZERO);
        let mut request =
            request("Earlier messages:\nStudent: hi\n\nLast student message: Why does it work?");
        request.system = REWRITE_SYSTEM_PROMPT.to_owned();
        request.max_tokens = Some(100);
        let mut stream = model.stream(request).await.unwrap();
        let mut out = String::new();
        while let Some(event) = stream.next().await {
            if let ModelEvent::Delta(d) = event.unwrap() {
                out.push_str(&d);
            }
        }
        assert_eq!(out, "Why does it work?");
    }

    #[test]
    fn source_parsing_survives_odd_input() {
        assert!(parse_sources("no sources here").is_empty());
        assert!(parse_sources("<source n=\"1\" ").is_empty());
        assert_eq!(
            parse_sources("<source n=\"1\">x &lt;b&gt; &quot;q&quot;</source>"),
            vec!["x <b> \"q\"".to_owned()]
        );
    }
}
