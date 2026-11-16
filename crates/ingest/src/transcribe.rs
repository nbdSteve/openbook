//! Page transcription: a vision model turns a rendered page into Markdown with LaTeX.

use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::operation::converse::ConverseError;
use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, ImageBlock, ImageFormat, ImageSource, InferenceConfiguration,
    Message, SystemContentBlock,
};
use aws_smithy_types::error::display::DisplayErrorContext;

/// The longest side of a rendered page, in pixels.
pub const MAX_RENDER_SIDE: u32 = 1568;
/// A rendered page must be smaller than this. Bedrock accepts at most 3.75 MB per image.
pub const MAX_IMAGE_BYTES: usize = 3_500_000;
/// A page of dense math can need more output than a chat answer.
const MIN_TRANSCRIBE_TOKENS: i32 = 4096;

pub const PROMPT: &str = "Transcribe this page of a course document into Markdown.\n\
- Be faithful: copy the text exactly and add nothing of your own.\n\
- Write math as LaTeX: $...$ for inline math and $$...$$ for display math.\n\
- Use Markdown headings (#, ##) for headings and Markdown tables for tables.\n\
- For a figure, write one line: [Figure: short description].\n\
- Leave out page numbers, running headers, and running footers.\n\
- Reply with the transcription only.";

#[derive(Debug, Clone, thiserror::Error)]
pub enum TranscribeError {
    /// Throttling, timeouts, and service problems. A retry can work.
    #[error("transcription failed (transient): {0}")]
    Transient(String),
    /// The model refused or the request is wrong. The page falls back to its text layer.
    #[error("transcription failed (permanent): {0}")]
    Permanent(String),
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    /// `page_number` is 1-based. `image` is the rendered page (JPEG). `text_layer` is what the PDF text layer holds.
    async fn transcribe(
        &self,
        page_number: u32,
        image: &[u8],
        text_layer: &str,
    ) -> Result<String, TranscribeError>;
}

/// Local and test transcriber. It returns the text layer under a deterministic heading, so the
/// transcription path runs without a model.
#[derive(Debug, Clone, Copy, Default)]
pub struct FakeTranscriber;

impl FakeTranscriber {
    pub fn markdown(page_number: u32, text_layer: &str) -> String {
        format!("# Page {page_number}\n\n{}", text_layer.trim())
    }
}

#[async_trait]
impl Transcriber for FakeTranscriber {
    async fn transcribe(
        &self,
        page_number: u32,
        _image: &[u8],
        text_layer: &str,
    ) -> Result<String, TranscribeError> {
        Ok(Self::markdown(page_number, text_layer))
    }
}

/// Transcribes with a Bedrock model through the Converse API and an image block.
pub struct BedrockTranscriber {
    client: BedrockClient,
    model_id: String,
    max_tokens: i32,
}

impl BedrockTranscriber {
    pub fn new(client: BedrockClient, model_id: String, max_output_tokens: i32) -> Self {
        Self {
            client,
            model_id,
            max_tokens: max_output_tokens.max(MIN_TRANSCRIBE_TOKENS),
        }
    }
}

fn classify(error: SdkError<ConverseError>) -> TranscribeError {
    let text = DisplayErrorContext(&error).to_string();
    match error {
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            TranscribeError::Transient(text)
        }
        SdkError::ServiceError(service) => match service.err() {
            ConverseError::ThrottlingException(_)
            | ConverseError::ModelTimeoutException(_)
            | ConverseError::ServiceUnavailableException(_)
            | ConverseError::InternalServerException(_)
            | ConverseError::ModelNotReadyException(_) => TranscribeError::Transient(text),
            _ => TranscribeError::Permanent(text),
        },
        _ => TranscribeError::Permanent(text),
    }
}

#[async_trait]
impl Transcriber for BedrockTranscriber {
    async fn transcribe(
        &self,
        _page_number: u32,
        image: &[u8],
        _text_layer: &str,
    ) -> Result<String, TranscribeError> {
        let image = ImageBlock::builder()
            .format(ImageFormat::Jpeg)
            .source(ImageSource::Bytes(Blob::new(image.to_vec())))
            .build()
            .map_err(|e| TranscribeError::Permanent(e.to_string()))?;
        let message = Message::builder()
            .role(ConversationRole::User)
            .content(ContentBlock::Image(image))
            .content(ContentBlock::Text("Transcribe this page.".into()))
            .build()
            .map_err(|e| TranscribeError::Permanent(e.to_string()))?;
        let out = self
            .client
            .converse()
            .model_id(&self.model_id)
            .system(SystemContentBlock::Text(PROMPT.into()))
            .messages(message)
            .inference_config(
                InferenceConfiguration::builder()
                    .max_tokens(self.max_tokens)
                    .temperature(0.0)
                    .build(),
            )
            .send()
            .await
            .map_err(classify)?;
        let text: String = out
            .output()
            .and_then(|o| o.as_message().ok())
            .map(|m| {
                m.content()
                    .iter()
                    .filter_map(|block| block.as_text().ok().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if text.trim().is_empty() {
            return Err(TranscribeError::Permanent(
                "the model returned no text".into(),
            ));
        }
        Ok(text.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_fake_transcriber_puts_the_text_layer_under_a_heading() {
        let out = FakeTranscriber
            .transcribe(3, b"png", "  The text layer.  ")
            .await
            .unwrap();
        assert_eq!(out, "# Page 3\n\nThe text layer.");
        assert_eq!(out, FakeTranscriber::markdown(3, "The text layer."));
    }

    #[test]
    fn the_prompt_asks_for_a_faithful_transcription_with_latex_and_figures() {
        assert!(PROMPT.contains("faithful"));
        assert!(PROMPT.contains("$...$") && PROMPT.contains("$$...$$"));
        assert!(PROMPT.contains("[Figure: short description]"));
        assert!(PROMPT.contains("add nothing"));
    }
}
