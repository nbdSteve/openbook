//! Wiring: build the dependencies, handle SQS events in Lambda, and poll the queue locally.

use std::sync::Arc;
use std::time::Duration;

use aws_lambda_events::event::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sqs::types::MessageSystemAttributeName;
use lambda_runtime::LambdaEvent;
use openbook_core::aws;
use openbook_core::catalog::ProviderKind;
use openbook_core::config::{Config, FAKE_EMBEDDING_MODEL_ID};
use openbook_core::files::Files;
use openbook_core::queue::{IngestMessage, IngestQueue};
use openbook_core::store::Store;
use openbook_search::bm25::Bm25Store;
use openbook_search::embed::{Embedder, FakeEmbedder, TitanEmbedder};
use openbook_search::vectors::VectorStore;

use crate::pipeline::{Deps, handle_message};
use crate::transcribe::{BedrockTranscriber, FakeTranscriber, Transcriber};

/// The Lambda timeout is 900 s. The handler stops this long before the deadline.
const DEADLINE_MARGIN: Duration = Duration::from_secs(20);
/// The budget in poll mode.
const POLL_BUDGET: Duration = Duration::from_secs(900);

/// Builds everything from the configuration and the environment.
pub async fn build_deps(config: Config) -> Result<(Deps, IngestQueue), String> {
    let sdk = aws::load_sdk_config(&config).await?;
    let custom_endpoint = config.aws_endpoint_url.is_some();
    let store = Store::new(&sdk, config.table_name.clone());
    let files = Files::new(&sdk, config.docs_bucket.clone(), custom_endpoint, None);
    let mut s3_conf = aws_sdk_s3::config::Builder::from(&sdk);
    if custom_endpoint {
        s3_conf = s3_conf.force_path_style(true);
    }
    let s3 = S3Client::from_conf(s3_conf.build());
    let vectors = VectorStore::new(&sdk, config.vector_bucket.clone());
    let bm25 = Bm25Store::new(s3.clone(), config.docs_bucket.clone(), store.clone());

    let uses_bedrock = config.embedding_model_id != FAKE_EMBEDDING_MODEL_ID
        || config
            .catalog
            .get(&config.transcribe_model)
            .is_some_and(|e| e.provider == ProviderKind::Bedrock);
    let bedrock = uses_bedrock
        .then(|| aws::bedrock_client(&config, &sdk))
        .transpose()?;

    let embedder: Arc<dyn Embedder> = match (&bedrock, config.embedding_model_id.as_str()) {
        (_, FAKE_EMBEDDING_MODEL_ID) => Arc::new(FakeEmbedder),
        (Some(client), id) => Arc::new(TitanEmbedder::new(client.clone(), id.to_owned())),
        (None, _) => return Err("no Bedrock client for the embedder".into()),
    };
    let entry = config
        .catalog
        .get(&config.transcribe_model)
        .ok_or("the transcribe model is not in the catalog")?;
    let transcriber: Arc<dyn Transcriber> = match (entry.provider, &bedrock) {
        (ProviderKind::Fake, _) => Arc::new(FakeTranscriber),
        (ProviderKind::Bedrock, Some(client)) => Arc::new(BedrockTranscriber::new(
            client.clone(),
            entry.model_id.clone(),
            entry.max_tokens(),
        )),
        (ProviderKind::Bedrock, None) => return Err("no Bedrock client for the transcriber".into()),
    };

    let queue = IngestQueue::new(&sdk, config.ingest_queue_url.clone());
    let deps = Deps {
        config: Arc::new(config),
        store,
        s3,
        files,
        vectors,
        bm25,
        embedder,
        transcriber,
        work_root: std::env::temp_dir().join("openbook-ingest"),
    };
    Ok((deps, queue))
}

fn receive_count(attributes: &std::collections::HashMap<String, String>) -> u32 {
    attributes
        .get("ApproximateReceiveCount")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

/// The Lambda handler: one message at a time (batch size 1), with batch item failures.
pub async fn lambda_handler(
    deps: &Deps,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    let left = Duration::from_millis(event.context.deadline.saturating_sub(now_ms));
    let budget = left.saturating_sub(DEADLINE_MARGIN);
    let mut failures = Vec::new();
    for record in event.payload.records {
        let id = record.message_id.clone().unwrap_or_default();
        let Some(message) = record
            .body
            .as_deref()
            .and_then(|body| serde_json::from_str::<IngestMessage>(body).ok())
        else {
            // A message that cannot be read cannot be fixed by a retry.
            tracing::error!(message_id = %id, "dropping an unreadable ingest message");
            continue;
        };
        let span = tracing::info_span!("ingest", message_id = %id, action = message.action.as_str(), course_id = %message.course_id, document_id = %message.document_id);
        let result = tracing::Instrument::instrument(
            handle_message(deps, &message, receive_count(&record.attributes), budget),
            span,
        )
        .await;
        match result {
            Ok(outcome) => tracing::info!(?outcome, "message handled"),
            Err(retry) => {
                tracing::error!(error = %retry, "message failed; it will be retried");
                let mut failure = BatchItemFailure::default();
                failure.item_identifier = id;
                failures.push(failure);
            }
        }
    }
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

/// Local mode: long-poll the queue and handle each message with the same handler.
pub async fn poll(deps: &Deps, queue: &IngestQueue) -> Result<(), String> {
    tracing::info!(queue = queue.url(), "polling the ingest queue");
    loop {
        let received = queue
            .client()
            .receive_message()
            .queue_url(queue.url())
            .max_number_of_messages(1)
            .wait_time_seconds(20)
            // A failed message becomes visible again after this time.
            .visibility_timeout(30)
            .message_system_attribute_names(MessageSystemAttributeName::ApproximateReceiveCount)
            .send()
            .await;
        let received = match received {
            Ok(received) => received,
            Err(error) => {
                tracing::error!(error = %aws_smithy_types::error::display::DisplayErrorContext(&error), "could not receive messages");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        for message in received.messages() {
            let count = message
                .attributes()
                .and_then(|a| a.get(&MessageSystemAttributeName::ApproximateReceiveCount))
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let parsed = message
                .body()
                .and_then(|body| serde_json::from_str::<IngestMessage>(body).ok());
            let done = match parsed {
                None => {
                    tracing::error!("dropping an unreadable ingest message");
                    true
                }
                Some(parsed) => match handle_message(deps, &parsed, count, POLL_BUDGET).await {
                    Ok(outcome) => {
                        tracing::info!(?outcome, action = parsed.action.as_str(), document_id = %parsed.document_id, "message handled");
                        true
                    }
                    Err(retry) => {
                        tracing::error!(error = %retry, "message failed; it will be retried");
                        false
                    }
                },
            };
            let Some(handle) = message.receipt_handle().filter(|_| done) else {
                continue;
            };
            if let Err(error) = queue
                .client()
                .delete_message()
                .queue_url(queue.url())
                .receipt_handle(handle)
                .send()
                .await
            {
                tracing::error!(error = %aws_smithy_types::error::display::DisplayErrorContext(&error), "could not delete the message");
            }
        }
    }
}
