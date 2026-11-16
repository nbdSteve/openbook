//! Embeddings: Titan Text Embeddings v2 on Bedrock, and a fake hashed bag-of-words embedder for local runs.

use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::operation::invoke_model::InvokeModelError;
use aws_sdk_bedrockruntime::primitives::Blob;
use aws_smithy_types::error::display::DisplayErrorContext;
use futures::stream::{self, StreamExt};

/// Titan v2 vectors have this many dimensions here (and the index is created with it).
pub const DIMENSIONS: usize = 1024;
/// Most embedding calls at once.
pub const CONCURRENCY: usize = 8;

#[derive(Debug, Clone, thiserror::Error)]
pub enum EmbedError {
    /// Throttling, timeouts, and service problems. A retry can work.
    #[error("embedding failed (transient): {0}")]
    Transient(String),
    /// The request itself is wrong. A retry does not help.
    #[error("embedding failed (permanent): {0}")]
    Permanent(String),
}

#[async_trait]
pub trait Embedder: Send + Sync {
    /// A normalized vector with `DIMENSIONS` values.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError>;

    /// The vector and the number of input tokens. Titan reports the count. For other embedders it is
    /// an estimate of one token per 4 characters.
    async fn embed_counted(&self, text: &str) -> Result<(Vec<f32>, u64), EmbedError> {
        let vector = self.embed(text).await?;
        Ok((vector, (text.chars().count() as u64).div_ceil(4)))
    }
}

/// Titan Text Embeddings v2 through `InvokeModel`.
pub struct TitanEmbedder {
    client: BedrockClient,
    model_id: String,
}

impl TitanEmbedder {
    pub fn new(client: BedrockClient, model_id: String) -> Self {
        Self { client, model_id }
    }
}

fn classify(error: aws_sdk_bedrockruntime::error::SdkError<InvokeModelError>) -> EmbedError {
    use aws_sdk_bedrockruntime::error::SdkError;
    let text = DisplayErrorContext(&error).to_string();
    match error {
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            EmbedError::Transient(text)
        }
        SdkError::ServiceError(service) => match service.err() {
            InvokeModelError::ThrottlingException(_)
            | InvokeModelError::ModelTimeoutException(_)
            | InvokeModelError::ServiceUnavailableException(_)
            | InvokeModelError::InternalServerException(_)
            | InvokeModelError::ModelNotReadyException(_)
            | InvokeModelError::ServiceQuotaExceededException(_) => EmbedError::Transient(text),
            _ => EmbedError::Permanent(text),
        },
        _ => EmbedError::Permanent(text),
    }
}

#[async_trait]
impl Embedder for TitanEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(self.embed_counted(text).await?.0)
    }

    async fn embed_counted(&self, text: &str) -> Result<(Vec<f32>, u64), EmbedError> {
        let body = serde_json::json!({
            "inputText": text,
            "dimensions": DIMENSIONS,
            "normalize": true,
        });
        let out = self
            .client
            .invoke_model()
            .model_id(&self.model_id)
            .content_type("application/json")
            .accept("application/json")
            .body(Blob::new(body.to_string()))
            .send()
            .await
            .map_err(classify)?;
        let value: serde_json::Value = serde_json::from_slice(out.body().as_ref())
            .map_err(|e| EmbedError::Permanent(format!("bad embedding response: {e}")))?;
        let vector: Vec<f32> = value["embedding"]
            .as_array()
            .ok_or_else(|| EmbedError::Permanent("no embedding in the response".into()))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();
        if vector.len() != DIMENSIONS {
            return Err(EmbedError::Permanent(format!(
                "the embedding has {} dimensions, not {DIMENSIONS}",
                vector.len()
            )));
        }
        let tokens = value["inputTextTokenCount"]
            .as_u64()
            .unwrap_or_else(|| (text.chars().count() as u64).div_ceil(4));
        Ok((vector, tokens))
    }
}

/// Hashes the words of the text into 1024 dimensions and L2-normalizes the vector.
/// Local runs and tests use it, because the result is the same on every run.
#[derive(Debug, Clone, Copy, Default)]
pub struct FakeEmbedder;

impl FakeEmbedder {
    pub fn vector(text: &str) -> Vec<f32> {
        let mut vector = vec![0f32; DIMENSIONS];
        for word in text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            // FNV-1a, 64 bit.
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in word.bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            vector[(hash % DIMENSIONS as u64) as usize] += 1.0;
        }
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm == 0.0 {
            vector[0] = 1.0;
        } else {
            vector.iter_mut().for_each(|v| *v /= norm);
        }
        vector
    }
}

#[async_trait]
impl Embedder for FakeEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(Self::vector(text))
    }
}

/// Embeds all texts, at most `CONCURRENCY` at once. The result is in the order of the texts.
pub async fn embed_all(
    embedder: &dyn Embedder,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, EmbedError> {
    stream::iter(texts.iter())
        .map(|text| embedder.embed(text))
        .buffered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

/// The cosine similarity of two normalized vectors.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_embedder_is_deterministic_and_normalized() {
        let a = FakeEmbedder::vector("The Pythagorean theorem relates the sides of a triangle");
        let b = FakeEmbedder::vector("The Pythagorean theorem relates the sides of a triangle");
        assert_eq!(a, b);
        assert_eq!(a.len(), DIMENSIONS);
        let norm: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "{norm}");
    }

    #[test]
    fn similar_texts_score_higher_than_different_ones() {
        let query = FakeEmbedder::vector("pythagorean theorem right triangle");
        let near = FakeEmbedder::vector(
            "The Pythagorean theorem says that in a right triangle a squared plus b squared equals c squared",
        );
        let far =
            FakeEmbedder::vector("Photosynthesis converts light into chemical energy in plants");
        assert!(dot(&query, &near) > dot(&query, &far) + 0.2);
        assert!(dot(&query, &query) > 0.999);
    }

    #[test]
    fn case_and_punctuation_do_not_matter() {
        assert_eq!(
            FakeEmbedder::vector("Hello, WORLD!"),
            FakeEmbedder::vector("hello world")
        );
    }

    #[test]
    fn empty_text_still_gives_a_unit_vector() {
        for text in ["", "   ", "!!!"] {
            let v = FakeEmbedder::vector(text);
            assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-5);
        }
    }

    #[tokio::test]
    async fn embed_all_keeps_the_order() {
        let texts: Vec<String> = (0..30).map(|i| format!("word{i} unique{i}")).collect();
        let all = embed_all(&FakeEmbedder, &texts).await.unwrap();
        assert_eq!(all.len(), 30);
        for (text, vector) in texts.iter().zip(&all) {
            assert_eq!(*vector, FakeEmbedder::vector(text));
        }
    }

    #[tokio::test]
    async fn embed_all_stops_at_the_first_error() {
        struct Failing;
        #[async_trait]
        impl Embedder for Failing {
            async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
                if text == "bad" {
                    Err(EmbedError::Transient("throttled".into()))
                } else {
                    Ok(vec![0.0])
                }
            }
        }
        let texts = vec!["ok".to_owned(), "bad".to_owned(), "ok".to_owned()];
        assert!(matches!(
            embed_all(&Failing, &texts).await,
            Err(EmbedError::Transient(_))
        ));
    }
}
