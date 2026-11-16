//! The real binary must refuse to start in Lambda with local-only settings (fail closed).
//! These tests start the binary with its own environment. They do not change this process's environment.

use std::process::{Command, Output};

const CATALOG: &str = r#"[{"id":"claude","label":"Claude","vendor":"anthropic","provider":"bedrock","modelId":"au.anthropic.claude-x","residency":"au","vision":true}]"#;
const FAKE_CATALOG: &str = r#"[{"id":"fake","label":"Fake","vendor":"openbook","provider":"fake","modelId":"fake","residency":"au","vision":true}]"#;

fn run(env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_openbook-api"));
    command.env_clear().envs([
        ("OPENBOOK_STAGE", "dev"),
        ("OPENBOOK_TABLE_NAME", "table"),
        ("OPENBOOK_DOCS_BUCKET", "bucket"),
        ("OPENBOOK_COGNITO_USER_POOL_ID", "ap-southeast-2_abc"),
        ("OPENBOOK_COGNITO_CLIENT_ID", "client"),
        ("OPENBOOK_INGEST_QUEUE_URL", "https://sqs.example/q.fifo"),
        ("OPENBOOK_VECTOR_BUCKET", "vectors"),
        ("OPENBOOK_MODELS", CATALOG),
        ("OPENBOOK_DEFAULT_CHAT_MODEL", "claude"),
        ("OPENBOOK_FAST_MODEL", "claude"),
        ("AWS_REGION", "ap-southeast-2"),
        // An address that nothing serves, so a start that gets through the checks fails fast.
        ("AWS_LAMBDA_RUNTIME_API", "127.0.0.1:1"),
        ("AWS_LAMBDA_FUNCTION_NAME", "test"),
        ("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "128"),
        ("AWS_LAMBDA_FUNCTION_VERSION", "1"),
        ("AWS_ACCESS_KEY_ID", "test"),
        ("AWS_SECRET_ACCESS_KEY", "test"),
    ]);
    command.envs(env.iter().copied());
    command.output().expect("run the binary")
}

fn refused(output: &Output, reason: &str) {
    assert!(
        !output.status.success(),
        "the binary must exit with an error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let logs = format!("{stderr}{stdout}");
    assert!(
        logs.contains("refusing to start"),
        "expected a refusal, got: {logs}"
    );
    assert!(logs.contains(reason), "expected {reason:?} in: {logs}");
}

#[test]
fn lambda_refuses_stage_local() {
    refused(
        &run(&[("OPENBOOK_STAGE", "local")]),
        "OPENBOOK_STAGE is local",
    );
}

#[test]
fn lambda_refuses_a_fake_model() {
    let output = run(&[
        ("OPENBOOK_MODELS", FAKE_CATALOG),
        ("OPENBOOK_DEFAULT_CHAT_MODEL", "fake"),
        ("OPENBOOK_FAST_MODEL", "fake"),
    ]);
    refused(&output, "fake provider");
}

#[test]
fn lambda_refuses_each_local_only_variable() {
    for name in [
        "AWS_ENDPOINT_URL",
        "OPENBOOK_COGNITO_JWKS_URL",
        "OPENBOOK_COGNITO_ISSUER",
        "OPENBOOK_CA_BUNDLE",
        "OPENBOOK_PRESIGN_ENDPOINT",
        "OPENBOOK_BEDROCK_PROFILE",
    ] {
        let value = if name.contains("ENDPOINT") {
            "http://localhost:1"
        } else {
            "x"
        };
        refused(&run(&[(name, value)]), &format!("{name} is set"));
    }
}

#[test]
fn an_invalid_catalog_stops_the_start() {
    let output = run(&[("OPENBOOK_DEFAULT_CHAT_MODEL", "missing")]);
    refused(&output, "OPENBOOK_DEFAULT_CHAT_MODEL");
}
