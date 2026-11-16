//! Entry point. In Lambda it handles SQS events. With `--poll` it long-polls the queue (local runs).

use std::process::ExitCode;
use std::sync::Arc;

use openbook_core::config::{Config, Service};
use openbook_ingest::worker;
use tracing_subscriber::EnvFilter;

fn init_tracing(in_lambda: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if in_lambda {
        builder.json().with_ansi(false).without_time().init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let in_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    init_tracing(in_lambda);
    let poll = std::env::args().any(|a| a == "--poll");
    // Start-up errors, including the fail-closed Lambda checks, end the process with a non-zero code.
    let config = match Config::from_env_for(Service::Ingest) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "refusing to start");
            return ExitCode::FAILURE;
        }
    };
    if poll && config.in_lambda {
        tracing::error!("refusing to start: --poll is for local runs, not for Lambda");
        return ExitCode::FAILURE;
    }
    let (deps, queue) = match worker::build_deps(config).await {
        Ok(built) => built,
        Err(error) => {
            tracing::error!(%error, "could not set up the worker");
            return ExitCode::FAILURE;
        }
    };
    let deps = Arc::new(deps);
    let result = if poll {
        worker::poll(&deps, &queue).await.map_err(|e| e.to_string())
    } else {
        let handler = Arc::clone(&deps);
        lambda_runtime::run(lambda_runtime::service_fn(move |event| {
            let deps = Arc::clone(&handler);
            async move { worker::lambda_handler(&deps, event).await }
        }))
        .await
        .map_err(|e| e.to_string())
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the worker stopped with an error");
            ExitCode::FAILURE
        }
    }
}
