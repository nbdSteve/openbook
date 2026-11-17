//! Test harness: one table, one docs bucket, one Cognito user pool and client per test, all on AWS Emulator.
//!
//! The harness builds clients explicitly. It never changes process environment variables.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_sdk_cognitoidentityprovider::types::{AttributeType, AuthFlowType, ExplicitAuthFlowsType};
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use openbook_api::{AppState, build_router};
use openbook_core::auth::TokenVerifier;
use openbook_core::config::Config;
use openbook_core::files::Files;
use openbook_core::ids;
use openbook_core::model::ModelRegistry;
use openbook_core::store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const PASSWORD: &str = "Openbook-test-1";
pub const REGION: &str = "us-east-1";

/// The default test catalog: two fake models, so tests can see which one answered.
pub const TWO_MODELS: &str = r#"[
  {"id":"fake-a","label":"Fake A","vendor":"openbook","provider":"fake","modelId":"fake-a","residency":"au","vision":true},
  {"id":"fake-b","label":"Fake B","vendor":"openbook","provider":"fake","modelId":"fake-b","residency":"global","vision":true}
]"#;
pub const ONE_MODEL: &str = r#"[
  {"id":"fake-a","label":"Fake A","vendor":"openbook","provider":"fake","modelId":"fake-a","residency":"au","vision":true}
]"#;

/// Settings that tests change.
#[derive(Clone)]
pub struct Options {
    pub user_daily_limit: u32,
    pub global_daily_limit: u32,
    pub courses_owned: u32,
    pub documents_per_course: u32,
    pub upload_bytes: u64,
    /// Pause between fake deltas. Zero keeps tests fast.
    pub fake_delay: Duration,
    pub failed_joins_per_day: u32,
    pub global_tokens: u64,
    pub chat_timeouts: Option<openbook_api::routes::chat::ChatTimeouts>,
    /// A scripted fast model: its catalog ID (it must be in the catalog) and the provider.
    pub fast_provider: Option<(String, std::sync::Arc<dyn openbook_core::model::ChatModel>)>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            user_daily_limit: 50,
            global_daily_limit: 500,
            courses_owned: 10,
            documents_per_course: 50,
            upload_bytes: 25 * 1024 * 1024,
            fake_delay: Duration::ZERO,
            failed_joins_per_day: 20,
            global_tokens: 3_000_000,
            chat_timeouts: None,
            fast_provider: None,
        }
    }
}

fn required_env(name: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{name} is not set. These tests need AWS Emulator. Run `make test-integration`, or set \
             OPENBOOK_TEST_EMULATOR_ENDPOINT (for example https://127.0.0.1:9301) and \
             OPENBOOK_TEST_EMULATOR_CA (the emulator's ca.pem)."
        ),
    }
}

pub struct TestUser {
    pub token: String,
    pub id: String,
    pub email: String,
}

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub options: Options,
    pub table: String,
    pub bucket: String,
    pub pool_id: String,
    pub client_id: String,
    pub endpoint: String,
    pub ca: PathBuf,
    pub sdk: aws_config::SdkConfig,
    pub cognito: aws_sdk_cognitoidentityprovider::Client,
    pub dynamodb: aws_sdk_dynamodb::Client,
    pub s3: aws_sdk_s3::Client,
    pub http: reqwest::Client,
    pub queue_url: String,
    pub vector_bucket: String,
    vars: HashMap<String, String>,
}

pub async fn start() -> TestApp {
    start_with(Options::default()).await
}

pub async fn start_with(options: Options) -> TestApp {
    start_with_models(options, TWO_MODELS).await
}

pub async fn start_with_models(options: Options, models: &str) -> TestApp {
    let endpoint = required_env("OPENBOOK_TEST_EMULATOR_ENDPOINT")
        .trim_end_matches('/')
        .to_owned();
    let ca = PathBuf::from(required_env("OPENBOOK_TEST_EMULATOR_CA"));

    let sdk = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(REGION))
        .endpoint_url(&endpoint)
        .credentials_provider(SharedCredentialsProvider::new(Credentials::new(
            "test", "test", None, None, "test",
        )))
        .http_client(openbook_core::aws::http_client_with_ca(&ca).expect("trust the emulator CA"))
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&sdk);
    let cognito = aws_sdk_cognitoidentityprovider::Client::new(&sdk);
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(&sdk)
            .force_path_style(true)
            .build(),
    );

    let suffix = ids::new_id().to_lowercase();
    let table = format!("openbook-test-{suffix}");
    let bucket = format!("openbook-test-{suffix}");
    create_table(&dynamodb, &table).await;
    s3.create_bucket()
        .bucket(&bucket)
        .send()
        .await
        .expect("create bucket");
    // The ingest queue (FIFO, as in the stack) and the vector bucket.
    let queue_url = sdk_queue(&sdk, &format!("openbook-test-{suffix}.fifo")).await;
    let vector_bucket = format!("openbook-test-{suffix}");
    aws_sdk_s3vectors::Client::new(&sdk)
        .create_vector_bucket()
        .vector_bucket_name(&vector_bucket)
        .send()
        .await
        .expect("create vector bucket");
    let pool_id = cognito
        .create_user_pool()
        .pool_name(format!("openbook-test-{suffix}"))
        .send()
        .await
        .expect("create user pool")
        .user_pool
        .and_then(|p| p.id)
        .expect("pool id");
    let client_id = cognito
        .create_user_pool_client()
        .user_pool_id(&pool_id)
        .client_name("test")
        .explicit_auth_flows(ExplicitAuthFlowsType::AllowUserPasswordAuth)
        .explicit_auth_flows(ExplicitAuthFlowsType::AllowRefreshTokenAuth)
        .send()
        .await
        .expect("create user pool client")
        .user_pool_client
        .and_then(|c| c.client_id)
        .expect("client id");

    // AWS Emulator decides what `iss` its tokens have (it changed from the AWS form to its own address
    // in v1.0.471). Read it from a real token once, so the tests pass with either shape.
    let issuer = probe_issuer(&cognito, &pool_id, &client_id).await;

    let mut vars: HashMap<String, String> = [
        ("OPENBOOK_STAGE", "local".to_owned()),
        ("OPENBOOK_TABLE_NAME", table.clone()),
        ("OPENBOOK_DOCS_BUCKET", bucket.clone()),
        ("OPENBOOK_COGNITO_USER_POOL_ID", pool_id.clone()),
        ("OPENBOOK_COGNITO_CLIENT_ID", client_id.clone()),
        ("OPENBOOK_COGNITO_REGION", REGION.to_owned()),
        ("OPENBOOK_COGNITO_ISSUER", issuer),
        (
            "OPENBOOK_COGNITO_JWKS_URL",
            format!("{endpoint}/{pool_id}/.well-known/jwks.json"),
        ),
        ("OPENBOOK_CA_BUNDLE", ca.display().to_string()),
        ("AWS_ENDPOINT_URL", endpoint.clone()),
        ("AWS_REGION", REGION.to_owned()),
        ("OPENBOOK_INGEST_QUEUE_URL", queue_url.clone()),
        ("OPENBOOK_VECTOR_BUCKET", vector_bucket.clone()),
        ("OPENBOOK_EMBEDDING_MODEL_ID", "fake".to_owned()),
        ("OPENBOOK_DEFAULT_CHAT_MODEL", "fake-a".to_owned()),
        (
            "OPENBOOK_FAST_MODEL",
            options
                .fast_provider
                .as_ref()
                .map_or("fake-a".to_owned(), |(id, _)| id.clone()),
        ),
        (
            "OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER",
            options.user_daily_limit.to_string(),
        ),
        (
            "OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL",
            options.global_daily_limit.to_string(),
        ),
        (
            "OPENBOOK_LIMIT_FAILED_JOINS_PER_DAY",
            options.failed_joins_per_day.to_string(),
        ),
        (
            "OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL",
            options.global_tokens.to_string(),
        ),
        (
            "OPENBOOK_LIMIT_COURSES_OWNED",
            options.courses_owned.to_string(),
        ),
        (
            "OPENBOOK_LIMIT_DOCUMENTS_PER_COURSE",
            options.documents_per_course.to_string(),
        ),
        (
            "OPENBOOK_LIMIT_UPLOAD_BYTES",
            options.upload_bytes.to_string(),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    vars.insert("OPENBOOK_MODELS".into(), models.to_owned());

    let http = reqwest::Client::builder()
        .add_root_certificate(
            reqwest::Certificate::from_pem(&std::fs::read(&ca).expect("read CA"))
                .expect("parse CA"),
        )
        .timeout(Duration::from_secs(30))
        .build()
        .expect("http client");

    let state = build_state(
        &vars,
        &sdk,
        options.fake_delay,
        options.chat_timeouts,
        options.fast_provider.clone(),
    );
    TestApp {
        router: build_router(state.clone()),
        state,
        options,
        table,
        bucket,
        pool_id,
        client_id,
        endpoint,
        ca,
        sdk,
        cognito,
        dynamodb,
        s3,
        http,
        queue_url,
        vector_bucket,
        vars,
    }
}

/// The `iss` claim of a token that the emulator issues for this pool.
async fn probe_issuer(
    cognito: &aws_sdk_cognitoidentityprovider::Client,
    pool_id: &str,
    client_id: &str,
) -> String {
    let username = format!("probe-{}", ids::new_id().to_lowercase());
    cognito
        .sign_up()
        .client_id(client_id)
        .username(&username)
        .password(PASSWORD)
        .user_attributes(
            AttributeType::builder()
                .name("email")
                .value(format!("{username}@openbook.test"))
                .build()
                .unwrap(),
        )
        .send()
        .await
        .expect("sign up the probe user");
    cognito
        .admin_confirm_sign_up()
        .user_pool_id(pool_id)
        .username(&username)
        .send()
        .await
        .expect("confirm the probe user");
    let auth = cognito
        .initiate_auth()
        .auth_flow(AuthFlowType::UserPasswordAuth)
        .client_id(client_id)
        .auth_parameters("USERNAME", &username)
        .auth_parameters("PASSWORD", PASSWORD)
        .send()
        .await
        .expect("sign in the probe user");
    let token = auth
        .authentication_result()
        .and_then(|r| r.id_token())
        .expect("an ID token");
    let payload = token.split('.').nth(1).expect("a JWT payload");
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload)
        .expect("a base64 payload");
    let claims: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON claims");
    claims["iss"].as_str().expect("an iss claim").to_owned()
}

fn build_state(
    vars: &HashMap<String, String>,
    sdk: &aws_config::SdkConfig,
    fake_delay: Duration,
    chat_timeouts: Option<openbook_api::routes::chat::ChatTimeouts>,
    fast_provider: Option<(String, std::sync::Arc<dyn openbook_core::model::ChatModel>)>,
) -> AppState {
    let config = Config::from_vars(vars).expect("valid test config");
    let store = Store::new(sdk, config.table_name.clone());
    let files = Files::new(sdk, config.docs_bucket.clone(), true, None);
    let verifier = TokenVerifier::from_config(&config).expect("verifier");
    let models = ModelRegistry::from_catalog(config.catalog.clone(), None, fake_delay)
        .expect("model registry");
    let models = match fast_provider {
        Some((id, provider)) => models.with_provider(&id, provider),
        None => models,
    };
    let s3 = {
        let conf = aws_sdk_s3::config::Builder::from(sdk)
            .force_path_style(true)
            .build();
        aws_sdk_s3::Client::from_conf(conf)
    };
    let retrieval = std::sync::Arc::new(openbook_api::retrieval::Retrieval {
        embedder: std::sync::Arc::new(openbook_search::embed::FakeEmbedder),
        vectors: openbook_search::vectors::VectorStore::new(sdk, config.vector_bucket.clone()),
        bm25: openbook_search::bm25::Bm25ReaderCache::new(
            s3,
            config.docs_bucket.clone(),
            std::env::temp_dir().join(format!("openbook-test-bm25-{}", ids::new_id())),
        ),
    });
    let queue = openbook_core::queue::IngestQueue::new(sdk, config.ingest_queue_url.clone());
    AppState {
        retrieval,
        queue,
        config: std::sync::Arc::new(config),
        store,
        files,
        verifier: std::sync::Arc::new(verifier),
        models,
        chat_timeouts: chat_timeouts.unwrap_or_default(),
        demo_visitors: Default::default(),
    }
}

async fn sdk_queue(sdk: &aws_config::SdkConfig, name: &str) -> String {
    aws_sdk_sqs::Client::new(sdk)
        .create_queue()
        .queue_name(name)
        .attributes(aws_sdk_sqs::types::QueueAttributeName::FifoQueue, "true")
        .send()
        .await
        .expect("create queue")
        .queue_url
        .expect("queue URL")
}

async fn create_table(client: &aws_sdk_dynamodb::Client, table: &str) {
    let attribute = |name: &str| {
        AttributeDefinition::builder()
            .attribute_name(name)
            .attribute_type(ScalarAttributeType::S)
            .build()
            .unwrap()
    };
    let key = |name: &str, kind: KeyType| {
        KeySchemaElement::builder()
            .attribute_name(name)
            .key_type(kind)
            .build()
            .unwrap()
    };
    client
        .create_table()
        .table_name(table)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(attribute("pk"))
        .attribute_definitions(attribute("sk"))
        .attribute_definitions(attribute("gsi1pk"))
        .attribute_definitions(attribute("gsi1sk"))
        .key_schema(key("pk", KeyType::Hash))
        .key_schema(key("sk", KeyType::Range))
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("gsi1")
                .key_schema(key("gsi1pk", KeyType::Hash))
                .key_schema(key("gsi1sk", KeyType::Range))
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        .send()
        .await
        .expect("create table");
    for _ in 0..100 {
        let description = client
            .describe_table()
            .table_name(table)
            .send()
            .await
            .expect("describe table");
        let table_active = description
            .table()
            .and_then(|t| t.table_status())
            .is_some_and(|s| s.as_str() == "ACTIVE");
        let indexes_active = description
            .table()
            .map(|t| {
                t.global_secondary_indexes()
                    .iter()
                    .all(|i| i.index_status().is_none_or(|s| s.as_str() == "ACTIVE"))
            })
            .unwrap_or(false);
        if table_active && indexes_active {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("table {table} did not become ACTIVE");
}

/// A response from the router.
pub struct Reply {
    pub status: StatusCode,
    pub content_type: String,
    pub text: String,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text))
    }

    /// The `error.code` of an error body.
    pub fn error_code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("no error code in {}", self.text))
            .to_owned()
    }

    /// Server-sent events as `(name, data)` pairs. Comment lines are skipped.
    pub fn sse(&self) -> Vec<(String, Value)> {
        let mut events = Vec::new();
        for block in self.text.split("\n\n").filter(|b| !b.trim().is_empty()) {
            let mut name = None;
            let mut data = None;
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    name = Some(v.to_owned());
                } else if let Some(v) = line.strip_prefix("data: ") {
                    data = Some(serde_json::from_str(v).expect("SSE data is JSON"));
                }
            }
            if let (Some(name), Some(data)) = (name, data) {
                events.push((name, data));
            }
        }
        events
    }
}

impl TestApp {
    /// Sends a request through the real router.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        send_to(&self.router, method, path, token, body).await
    }

    pub async fn get(&self, path: &str, user: &TestUser) -> Reply {
        self.send(Method::GET, path, Some(&user.token), None).await
    }

    pub async fn post(&self, path: &str, user: &TestUser, body: Value) -> Reply {
        self.send(Method::POST, path, Some(&user.token), Some(body))
            .await
    }

    /// A `POST` with an empty body.
    pub async fn post_empty(&self, path: &str, user: &TestUser) -> Reply {
        self.send(Method::POST, path, Some(&user.token), None).await
    }

    pub async fn patch(&self, path: &str, user: &TestUser, body: Value) -> Reply {
        self.send(Method::PATCH, path, Some(&user.token), Some(body))
            .await
    }

    pub async fn delete(&self, path: &str, user: &TestUser) -> Reply {
        self.send(Method::DELETE, path, Some(&user.token), None)
            .await
    }

    /// A second router over the same resources with another model catalog.
    pub fn router_with_models(&self, models: &str, default: &str) -> Router {
        let mut vars = self.vars.clone();
        vars.insert("OPENBOOK_MODELS".into(), models.to_owned());
        vars.insert("OPENBOOK_DEFAULT_CHAT_MODEL".into(), default.to_owned());
        vars.insert("OPENBOOK_FAST_MODEL".into(), default.to_owned());
        build_router(build_state(&vars, &self.sdk, Duration::ZERO, None, None))
    }

    /// The environment that the test config was built from.
    pub fn vars(&self) -> &HashMap<String, String> {
        &self.vars
    }

    /// A router over the same resources with another set of variables.
    pub fn router_with_vars(&self, vars: &HashMap<String, String>) -> Router {
        build_router(build_state(vars, &self.sdk, Duration::ZERO, None, None))
    }

    /// A router over the same resources with another set of variables and a scripted fast model.
    pub fn router_with_fast(
        &self,
        vars: &HashMap<String, String>,
        fast: (String, std::sync::Arc<dyn openbook_core::model::ChatModel>),
    ) -> Router {
        build_router(build_state(
            vars,
            &self.sdk,
            Duration::ZERO,
            None,
            Some(fast),
        ))
    }

    /// A router over the same resources with another ingest queue URL.
    pub fn router_with_queue(&self, queue_url: &str) -> Router {
        let mut vars = self.vars.clone();
        vars.insert("OPENBOOK_INGEST_QUEUE_URL".into(), queue_url.to_owned());
        build_router(build_state(&vars, &self.sdk, Duration::ZERO, None, None))
    }

    /// Takes every message from the ingest queue (and deletes it), as parsed JSON.
    pub async fn queued_messages(&self) -> Vec<Value> {
        let sqs = self.state.queue.client();
        let mut out = Vec::new();
        for _ in 0..3 {
            let received = sqs
                .receive_message()
                .queue_url(&self.queue_url)
                .max_number_of_messages(10)
                .wait_time_seconds(1)
                .send()
                .await
                .expect("receive messages");
            if received.messages().is_empty() && !out.is_empty() {
                break;
            }
            for message in received.messages() {
                out.push(
                    serde_json::from_str(message.body().unwrap_or("null")).expect("JSON message"),
                );
                if let Some(handle) = message.receipt_handle() {
                    let _ = sqs
                        .delete_message()
                        .queue_url(&self.queue_url)
                        .receipt_handle(handle)
                        .send()
                        .await;
                }
            }
        }
        out
    }

    /// Signs a new user up, confirms them, and gets a real ID token from the emulator.
    pub async fn user(&self, name: &str) -> TestUser {
        self.user_with(name, true).await
    }

    /// Like `user`, but the email stays unverified (the emulator's default after `AdminConfirmSignUp`).
    pub async fn unverified_user(&self, name: &str) -> TestUser {
        self.user_with(name, false).await
    }

    async fn user_with(&self, name: &str, verify_email: bool) -> TestUser {
        let username = format!("{name}-{}", &ids::new_id().to_lowercase()[16..]);
        let email = format!("{username}@openbook.test");
        let signed_up = self
            .cognito
            .sign_up()
            .client_id(&self.client_id)
            .username(&username)
            .password(PASSWORD)
            .user_attributes(
                AttributeType::builder()
                    .name("email")
                    .value(&email)
                    .build()
                    .unwrap(),
            )
            .send()
            .await
            .expect("sign up");
        self.cognito
            .admin_confirm_sign_up()
            .user_pool_id(&self.pool_id)
            .username(&username)
            .send()
            .await
            .expect("confirm sign up");
        if verify_email {
            self.cognito
                .admin_update_user_attributes()
                .user_pool_id(&self.pool_id)
                .username(&username)
                .user_attributes(
                    AttributeType::builder()
                        .name("email_verified")
                        .value("true")
                        .build()
                        .unwrap(),
                )
                .send()
                .await
                .expect("verify the email");
        }
        let auth = self
            .cognito
            .initiate_auth()
            .auth_flow(AuthFlowType::UserPasswordAuth)
            .client_id(&self.client_id)
            .auth_parameters("USERNAME", &username)
            .auth_parameters("PASSWORD", PASSWORD)
            .send()
            .await
            .expect("initiate auth");
        let token = auth
            .authentication_result()
            .and_then(|r| r.id_token())
            .expect("id token")
            .to_owned();
        TestUser {
            token,
            id: signed_up.user_sub().to_owned(),
            email,
        }
    }

    /// Creates a course owned by `owner` and returns the course JSON.
    pub async fn course(&self, owner: &TestUser, name: &str) -> Value {
        let reply = self
            .post("/api/courses", owner, json!({ "name": name }))
            .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
        reply.json()["course"].clone()
    }

    /// Creates a course and makes `student` a member. Returns the course JSON as the owner sees it.
    pub async fn course_with_student(&self, owner: &TestUser, student: &TestUser) -> Value {
        let course = self.course(owner, "Calculus").await;
        let code = course["inviteCode"].as_str().unwrap().to_owned();
        let joined = self
            .post("/api/join", student, json!({ "inviteCode": code }))
            .await;
        assert_eq!(joined.status, StatusCode::OK, "{}", joined.text);
        course
    }

    /// Sends a chat message and returns the reply.
    pub async fn chat(&self, user: &TestUser, course_id: &str, body: Value) -> Reply {
        self.post(&format!("/api/courses/{course_id}/chat"), user, body)
            .await
    }

    /// Counts DynamoDB items under a partition key.
    pub async fn items_under(&self, pk: &str) -> usize {
        let out = self
            .dynamodb
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(
                ":pk",
                aws_sdk_dynamodb::types::AttributeValue::S(pk.to_owned()),
            )
            .send()
            .await
            .expect("query");
        out.items().len()
    }

    /// The keys of S3 objects under a prefix.
    pub async fn objects_under(&self, prefix: &str) -> Vec<String> {
        let out = self
            .s3
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .send()
            .await
            .expect("list objects");
        out.contents()
            .iter()
            .filter_map(|o| o.key().map(str::to_owned))
            .collect()
    }
}

/// A request without sign-in, as the demo route gets it. `peer` is the socket address that the
/// server sees (axum adds it as `ConnectInfo`); `None` leaves it out, as in a bad Lambda setup.
pub async fn send_anonymous(
    router: &Router,
    path: &str,
    body: Option<Value>,
    peer: Option<&str>,
    headers: &[(&str, &str)],
) -> Reply {
    let mut builder = Request::builder().method(Method::POST).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    if let Some(peer) = peer {
        let address: std::net::SocketAddr = peer.parse().expect("a socket address");
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(address));
    }
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        content_type,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

pub async fn send_to(
    router: &Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header("x-auth-token", token);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = router
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        content_type,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

impl TestApp {
    /// The sort keys under a partition key.
    pub async fn get_item_sk_prefixes(&self, pk: &str) -> Vec<String> {
        let out = self
            .dynamodb
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(
                ":pk",
                aws_sdk_dynamodb::types::AttributeValue::S(pk.to_owned()),
            )
            .consistent_read(true)
            .send()
            .await
            .expect("query");
        out.items()
            .iter()
            .filter_map(|i| match i.get("sk") {
                Some(aws_sdk_dynamodb::types::AttributeValue::S(sk)) => Some(sk.clone()),
                _ => None,
            })
            .collect()
    }
}
