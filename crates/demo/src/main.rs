//! `openbook-demo provision [--manifest PATH] [--force] [--wait-seconds N]`
//!
//! Creates or refreshes the public demo course (see `docs/architecture.md`, "Public demo"). It reads
//! these variables: `OPENBOOK_STAGE`, `OPENBOOK_TABLE_NAME`, `OPENBOOK_DOCS_BUCKET`,
//! `OPENBOOK_INGEST_QUEUE_URL`, and for a local stack also `AWS_ENDPOINT_URL`, `OPENBOOK_CA_BUNDLE`,
//! and `AWS_REGION`. For `dev` and `prod` the AWS credentials come from the normal chain
//! (`AWS_PROFILE`). Use `make demo-provision`, which sets all of this.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use openbook_core::aws::load_sdk_config_with;
use openbook_core::files::Files;
use openbook_core::queue::IngestQueue;
use openbook_core::store::Store;
use openbook_demo::{Loaded, Outcome, Provisioner};
use tracing_subscriber::EnvFilter;

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn required(name: &str) -> Result<String, String> {
    var(name).ok_or_else(|| format!("{name} is required"))
}

struct Args {
    manifest: PathBuf,
    force: bool,
    wait: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("provision") {
        return Err(
            "usage: openbook-demo provision [--manifest PATH] [--force] [--wait-seconds N]".into(),
        );
    }
    let mut parsed = Args {
        manifest: PathBuf::from("demo/course.json"),
        force: false,
        wait: Duration::from_secs(20 * 60),
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                parsed.manifest = args.next().ok_or("--manifest needs a path")?.into();
            }
            "--force" => parsed.force = true,
            "--wait-seconds" => {
                let seconds = args
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or("--wait-seconds needs a number")?;
                parsed.wait = Duration::from_secs(seconds);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(parsed)
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let stage = required("OPENBOOK_STAGE")?;
    let endpoint = var("AWS_ENDPOINT_URL");
    // A local run must talk to the emulator, and a deployed run must not. This stops a mix-up in
    // either direction before any write.
    match (stage.as_str(), &endpoint) {
        ("local", None) => {
            return Err(
                "OPENBOOK_STAGE is local but AWS_ENDPOINT_URL is not set: refusing to use real AWS"
                    .into(),
            );
        }
        ("dev" | "prod", Some(_)) => {
            return Err(format!(
                "OPENBOOK_STAGE is {stage} but AWS_ENDPOINT_URL is set: refusing"
            ));
        }
        ("local" | "dev" | "prod", _) => {}
        _ => {
            return Err(format!(
                "OPENBOOK_STAGE must be local, dev, or prod (got {stage:?})"
            ));
        }
    }
    let table = required("OPENBOOK_TABLE_NAME")?;
    let bucket = required("OPENBOOK_DOCS_BUCKET")?;
    let queue_url = required("OPENBOOK_INGEST_QUEUE_URL")?;
    let ca = var("OPENBOOK_CA_BUNDLE").map(PathBuf::from);
    let region = var("AWS_REGION").or_else(|| var("AWS_DEFAULT_REGION"));

    let loaded = Loaded::load(&args.manifest).map_err(|e| e.to_string())?;
    tracing::info!(
        stage = %stage,
        sources = loaded.pdfs.len(),
        fingerprint = %loaded.fingerprint(),
        "provisioning the demo course"
    );

    let sdk = load_sdk_config_with(region.as_deref(), endpoint.as_deref(), ca.as_deref()).await?;
    let provisioner = Provisioner {
        store: Store::new(&sdk, table),
        files: Files::new(&sdk, bucket, endpoint.is_some(), None),
        queue: IngestQueue::new(&sdk, queue_url),
        model: var("OPENBOOK_FAST_MODEL").unwrap_or_else(|| "fast".into()),
        wait: args.wait,
        poll: Duration::from_secs(2),
    };
    match provisioner
        .provision(&loaded, args.force)
        .await
        .map_err(|e| e.to_string())?
    {
        Outcome::UpToDate { course_id } => {
            println!("demo: up to date (course {course_id})");
        }
        Outcome::Provisioned {
            course_id,
            replaced,
        } => {
            println!("demo: provisioned (course {course_id})");
            if let Some(old) = replaced {
                println!("demo: the old course {old} is being deleted by the ingest worker");
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
