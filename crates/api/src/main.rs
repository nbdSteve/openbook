//! Entry point. In Lambda it runs the router through `lambda_http` with response streaming.
//! Locally it serves on a TCP listener.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use openbook_api::{AppState, build_router};
use openbook_core::auth::TokenVerifier;
use openbook_core::aws;
use openbook_core::catalog::ProviderKind;
use openbook_core::config::Config;
use openbook_core::files::Files;
use openbook_core::model::ModelRegistry;
use openbook_core::queue::IngestQueue;
use openbook_core::store::Store;
use tracing_subscriber::EnvFilter;

fn init_tracing(in_lambda: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if in_lambda {
        // CloudWatch adds its own timestamps.
        builder.json().with_ansi(false).without_time().init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing(std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some());
    // Start-up errors, including the fail-closed Lambda checks, end the process with a non-zero code.
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "refusing to start");
            return ExitCode::FAILURE;
        }
    };
    match run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the API stopped with an error");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sdk = aws::load_sdk_config(&config).await?;
    let store = Store::new(&sdk, config.table_name.clone());
    let files = Files::new(
        &sdk,
        config.docs_bucket.clone(),
        config.aws_endpoint_url.is_some(),
        config.presign_endpoint.as_deref(),
    );
    let queue = IngestQueue::new(&sdk, config.ingest_queue_url.clone());
    let verifier = TokenVerifier::from_config(&config)?;
    let bedrock = config
        .catalog
        .entries()
        .iter()
        .any(|e| e.provider == ProviderKind::Bedrock)
        .then(|| aws::bedrock_client(&config, &sdk))
        .transpose()?;
    let models =
        ModelRegistry::from_catalog(config.catalog.clone(), bedrock, Duration::from_millis(20))?;

    verifier.prefetch().await;
    let in_lambda = config.in_lambda;
    let listen_addr = config.listen_addr;
    tracing::info!(
        stage = config.stage.as_str(),
        in_lambda,
        models = config.catalog.entries().len(),
        "starting"
    );
    let router = build_router(AppState {
        config: Arc::new(config),
        store,
        files,
        verifier: Arc::new(verifier),
        models,
        queue,
        chat_timeouts: Default::default(),
    });

    if in_lambda {
        // The function URL uses RESPONSE_STREAM invoke mode for every route.
        lambda_http::run_with_streaming_response(router).await
    } else {
        let listener = tokio::net::TcpListener::bind(listen_addr).await?;
        tracing::info!(addr = %listen_addr, "listening");
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        Ok(())
    }
}
