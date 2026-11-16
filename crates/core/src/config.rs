//! Configuration from environment variables (see `docs/architecture.md`).
//!
//! `Config::from_vars` takes a snapshot of variables, so tests never touch the process environment.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::catalog::Catalog;

/// Variables that are only for local runs. In Lambda, each of them stops the start-up.
pub const LOCAL_ONLY_VARS: [&str; 5] = [
    "AWS_ENDPOINT_URL",
    "OPENBOOK_COGNITO_JWKS_URL",
    "OPENBOOK_CA_BUNDLE",
    "OPENBOOK_PRESIGN_ENDPOINT",
    "OPENBOOK_BEDROCK_PROFILE",
];

/// Deployment stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Local,
    Dev,
    Prod,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Local => "local",
            Stage::Dev => "dev",
            Stage::Prod => "prod",
        }
    }
}

/// Per-user and per-course limits. The API reports them to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitsConfig {
    pub daily_messages_per_user: u32,
    pub daily_messages_global: u32,
    pub courses_owned: u32,
    pub documents_per_course: u32,
    pub upload_bytes: u64,
    /// Failed join attempts (unknown invite codes) per user per day.
    pub failed_joins_per_day: u32,
    /// Global input plus output tokens per day. At or above it, chat returns 503.
    pub daily_tokens_global: u64,
}

/// The full API configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub stage: Stage,
    pub in_lambda: bool,
    pub table_name: String,
    pub docs_bucket: String,
    pub cognito_user_pool_id: String,
    pub cognito_client_id: String,
    pub cognito_region: String,
    /// `OPENBOOK_COGNITO_JWKS_URL`, when set.
    pub cognito_jwks_url: Option<String>,
    pub catalog: Catalog,
    pub bedrock_profile: Option<String>,
    pub ca_bundle: Option<PathBuf>,
    pub presign_endpoint: Option<String>,
    pub aws_endpoint_url: Option<String>,
    pub aws_region: Option<String>,
    pub limits: LimitsConfig,
    pub listen_addr: SocketAddr,
}

impl Config {
    /// The token issuer: `https://cognito-idp.<region>.amazonaws.com/<poolId>`.
    pub fn cognito_issuer(&self) -> String {
        format!(
            "https://cognito-idp.{}.amazonaws.com/{}",
            self.cognito_region, self.cognito_user_pool_id
        )
    }

    /// Where to fetch the signing keys.
    pub fn jwks_url(&self) -> String {
        self.cognito_jwks_url
            .clone()
            .unwrap_or_else(|| format!("{}/.well-known/jwks.json", self.cognito_issuer()))
    }

    /// Reads the process environment.
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_vars(&std::env::vars().collect())
    }

    /// Parses and validates a snapshot of environment variables.
    pub fn from_vars(vars: &HashMap<String, String>) -> Result<Config, ConfigError> {
        let mut problems: Vec<String> = Vec::new();
        let get = |name: &str| -> Option<String> {
            vars.get(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let required = |problems: &mut Vec<String>, name: &str| -> String {
            get(name).unwrap_or_else(|| {
                problems.push(format!("{name} is required"));
                String::new()
            })
        };

        let stage_raw = required(&mut problems, "OPENBOOK_STAGE");
        let table_name = required(&mut problems, "OPENBOOK_TABLE_NAME");
        let docs_bucket = required(&mut problems, "OPENBOOK_DOCS_BUCKET");
        let cognito_user_pool_id = required(&mut problems, "OPENBOOK_COGNITO_USER_POOL_ID");
        let cognito_client_id = required(&mut problems, "OPENBOOK_COGNITO_CLIENT_ID");

        let stage = match stage_raw.as_str() {
            "local" => Stage::Local,
            "dev" => Stage::Dev,
            "prod" => Stage::Prod,
            "" => Stage::Local, // already reported as missing
            other => {
                problems.push(format!(
                    "OPENBOOK_STAGE must be local, dev, or prod (got {other:?})"
                ));
                Stage::Local
            }
        };

        let aws_region = get("AWS_REGION").or_else(|| get("AWS_DEFAULT_REGION"));
        let cognito_region = match get("OPENBOOK_COGNITO_REGION").or_else(|| aws_region.clone()) {
            Some(r) => r,
            None => {
                problems.push("OPENBOOK_COGNITO_REGION or AWS_REGION is required".into());
                String::new()
            }
        };
        if !cognito_user_pool_id.is_empty() && !cognito_user_pool_id.contains('_') {
            problems.push("OPENBOOK_COGNITO_USER_POOL_ID must look like <region>_<id>".into());
        }

        let models_json = required(&mut problems, "OPENBOOK_MODELS");
        let default_chat = required(&mut problems, "OPENBOOK_DEFAULT_CHAT_MODEL");
        let fast = required(&mut problems, "OPENBOOK_FAST_MODEL");
        let catalog = if models_json.is_empty() || default_chat.is_empty() || fast.is_empty() {
            None
        } else {
            match Catalog::parse(&models_json, &default_chat, &fast) {
                Ok(catalog) => Some(catalog),
                Err(found) => {
                    problems.extend(found);
                    None
                }
            }
        };

        let number = |problems: &mut Vec<String>, name: &str, default: u64| -> u64 {
            match get(name) {
                None => default,
                Some(raw) => match raw.parse::<u64>() {
                    Ok(n) if n > 0 => n,
                    _ => {
                        problems.push(format!("{name} must be a positive integer (got {raw:?})"));
                        default
                    }
                },
            }
        };
        let to_u32 = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
        let limits = LimitsConfig {
            daily_messages_per_user: to_u32(number(
                &mut problems,
                "OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER",
                50,
            )),
            daily_messages_global: to_u32(number(
                &mut problems,
                "OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL",
                500,
            )),
            courses_owned: to_u32(number(&mut problems, "OPENBOOK_LIMIT_COURSES_OWNED", 10)),
            documents_per_course: to_u32(number(
                &mut problems,
                "OPENBOOK_LIMIT_DOCUMENTS_PER_COURSE",
                50,
            )),
            upload_bytes: number(&mut problems, "OPENBOOK_LIMIT_UPLOAD_BYTES", 26_214_400),
            failed_joins_per_day: to_u32(number(
                &mut problems,
                "OPENBOOK_LIMIT_FAILED_JOINS_PER_DAY",
                20,
            )),
            daily_tokens_global: number(
                &mut problems,
                "OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL",
                3_000_000,
            ),
        };

        let listen_raw = get("OPENBOOK_LISTEN_ADDR").unwrap_or_else(|| "127.0.0.1:8787".into());
        let listen_addr = listen_raw.parse::<SocketAddr>().unwrap_or_else(|_| {
            problems.push(format!(
                "OPENBOOK_LISTEN_ADDR must be host:port (got {listen_raw:?})"
            ));
            SocketAddr::from(([127, 0, 0, 1], 8787))
        });

        let presign_endpoint = get("OPENBOOK_PRESIGN_ENDPOINT");
        if let Some(url) = &presign_endpoint
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            problems.push("OPENBOOK_PRESIGN_ENDPOINT must start with http:// or https://".into());
        }

        let in_lambda = get("AWS_LAMBDA_RUNTIME_API").is_some();
        if in_lambda {
            let has_fake = catalog.as_ref().is_some_and(Catalog::has_fake);
            problems.extend(lambda_violations(vars, stage, has_fake));
        }

        let Some(catalog) = catalog.filter(|_| problems.is_empty()) else {
            return Err(ConfigError { problems });
        };

        Ok(Config {
            stage,
            in_lambda,
            table_name,
            docs_bucket,
            cognito_user_pool_id,
            cognito_client_id,
            cognito_region,
            cognito_jwks_url: get("OPENBOOK_COGNITO_JWKS_URL"),
            catalog,
            bedrock_profile: get("OPENBOOK_BEDROCK_PROFILE"),
            ca_bundle: get("OPENBOOK_CA_BUNDLE").map(PathBuf::from),
            presign_endpoint,
            aws_endpoint_url: get("AWS_ENDPOINT_URL"),
            aws_region,
            limits,
            listen_addr,
        })
    }
}

/// Fail closed: local-only settings must never run in Lambda.
fn lambda_violations(
    vars: &HashMap<String, String>,
    stage: Stage,
    has_fake_model: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if stage == Stage::Local {
        out.push("refusing to start in Lambda: OPENBOOK_STAGE is local".into());
    }
    if has_fake_model {
        out.push("refusing to start in Lambda: the model catalog has a fake provider".into());
    }
    let mut names: Vec<&String> = vars
        .iter()
        .filter(|(name, value)| {
            !value.trim().is_empty()
                && (LOCAL_ONLY_VARS.contains(&name.as_str())
                    // Service-specific endpoint overrides, such as AWS_ENDPOINT_URL_DYNAMODB.
                    || name.starts_with("AWS_ENDPOINT_URL_"))
        })
        .map(|(name, _)| name)
        .collect();
    names.sort();
    for name in names {
        out.push(format!("refusing to start in Lambda: {name} is set"));
    }
    out
}

/// Every problem found in the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub problems: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid configuration: {}", self.problems.join("; "))
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_CATALOG: &str = r#"[
        {"id":"claude","label":"Claude","vendor":"anthropic","provider":"bedrock","modelId":"au.anthropic.claude-x","residency":"au"}
    ]"#;
    const FAKE_CATALOG: &str = r#"[
        {"id":"fake","label":"Fake","vendor":"openbook","provider":"fake","modelId":"fake","residency":"au"}
    ]"#;

    fn base() -> HashMap<String, String> {
        [
            ("OPENBOOK_STAGE", "dev"),
            ("OPENBOOK_TABLE_NAME", "t"),
            ("OPENBOOK_DOCS_BUCKET", "b"),
            ("OPENBOOK_COGNITO_USER_POOL_ID", "ap-southeast-2_abc"),
            ("OPENBOOK_COGNITO_CLIENT_ID", "client"),
            ("OPENBOOK_MODELS", REAL_CATALOG),
            ("OPENBOOK_DEFAULT_CHAT_MODEL", "claude"),
            ("OPENBOOK_FAST_MODEL", "claude"),
            ("AWS_REGION", "ap-southeast-2"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    fn lambda() -> HashMap<String, String> {
        let mut v = base();
        v.insert("AWS_LAMBDA_RUNTIME_API".into(), "127.0.0.1:9001".into());
        v
    }

    #[test]
    fn defaults_apply() {
        let c = Config::from_vars(&base()).unwrap();
        assert_eq!(c.stage, Stage::Dev);
        assert!(!c.in_lambda);
        assert_eq!(c.catalog.default_chat_id(), "claude");
        assert_eq!(c.limits.daily_messages_per_user, 50);
        assert_eq!(c.limits.daily_messages_global, 500);
        assert_eq!(c.limits.courses_owned, 10);
        assert_eq!(c.limits.documents_per_course, 50);
        assert_eq!(c.limits.upload_bytes, 26_214_400);
        assert_eq!(c.limits.failed_joins_per_day, 20);
        assert_eq!(c.limits.daily_tokens_global, 3_000_000);
        assert_eq!(c.listen_addr.to_string(), "127.0.0.1:8787");
        assert_eq!(c.cognito_region, "ap-southeast-2");
        assert_eq!(
            c.cognito_issuer(),
            "https://cognito-idp.ap-southeast-2.amazonaws.com/ap-southeast-2_abc"
        );
        assert_eq!(
            c.jwks_url(),
            "https://cognito-idp.ap-southeast-2.amazonaws.com/ap-southeast-2_abc/.well-known/jwks.json"
        );
    }

    #[test]
    fn overrides_apply() {
        let mut v = base();
        v.insert("OPENBOOK_COGNITO_REGION".into(), "us-east-1".into());
        v.insert(
            "OPENBOOK_COGNITO_JWKS_URL".into(),
            "https://127.0.0.1:9301/p/jwks".into(),
        );
        v.insert("OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER".into(), "2".into());
        v.insert("OPENBOOK_LISTEN_ADDR".into(), "127.0.0.1:9999".into());
        let c = Config::from_vars(&v).unwrap();
        assert_eq!(c.cognito_region, "us-east-1");
        assert_eq!(c.jwks_url(), "https://127.0.0.1:9301/p/jwks");
        assert_eq!(c.limits.daily_messages_per_user, 2);
        assert_eq!(c.listen_addr.port(), 9999);
    }

    #[test]
    fn reports_every_missing_variable() {
        let err = Config::from_vars(&HashMap::new()).unwrap_err();
        for name in [
            "OPENBOOK_STAGE",
            "OPENBOOK_TABLE_NAME",
            "OPENBOOK_DOCS_BUCKET",
            "OPENBOOK_COGNITO_USER_POOL_ID",
            "OPENBOOK_COGNITO_CLIENT_ID",
            "OPENBOOK_MODELS",
            "OPENBOOK_DEFAULT_CHAT_MODEL",
            "OPENBOOK_FAST_MODEL",
            "AWS_REGION",
        ] {
            assert!(err.to_string().contains(name), "{name} missing from {err}");
        }
    }

    #[test]
    fn rejects_bad_values() {
        for (name, value) in [
            ("OPENBOOK_STAGE", "staging"),
            ("OPENBOOK_LIMIT_COURSES_OWNED", "0"),
            ("OPENBOOK_LIMIT_UPLOAD_BYTES", "lots"),
            ("OPENBOOK_LIMIT_FAILED_JOINS_PER_DAY", "0"),
            ("OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL", "-1"),
            ("OPENBOOK_LISTEN_ADDR", "nowhere"),
            ("OPENBOOK_MODELS", "not json"),
            ("OPENBOOK_DEFAULT_CHAT_MODEL", "missing"),
            ("OPENBOOK_FAST_MODEL", "missing"),
            ("OPENBOOK_PRESIGN_ENDPOINT", "localhost:5173"),
            ("OPENBOOK_COGNITO_USER_POOL_ID", "nounderscore"),
        ] {
            let mut v = base();
            v.insert(name.into(), value.into());
            let err = Config::from_vars(&v).unwrap_err();
            assert!(err.to_string().contains(name), "{name}: {err}");
        }
    }

    #[test]
    fn empty_values_count_as_unset() {
        let mut v = base();
        v.insert("OPENBOOK_LIMIT_COURSES_OWNED".into(), "  ".into());
        v.insert("OPENBOOK_CA_BUNDLE".into(), String::new());
        let c = Config::from_vars(&v).unwrap();
        assert_eq!(c.limits.courses_owned, 10);
        assert!(c.ca_bundle.is_none());
    }

    #[test]
    fn lambda_with_clean_environment_starts() {
        let c = Config::from_vars(&lambda()).unwrap();
        assert!(c.in_lambda);
    }

    #[test]
    fn lambda_refuses_stage_local() {
        let mut v = lambda();
        v.insert("OPENBOOK_STAGE".into(), "local".into());
        let err = Config::from_vars(&v).unwrap_err();
        assert!(err.to_string().contains("OPENBOOK_STAGE is local"));
    }

    #[test]
    fn lambda_refuses_a_fake_catalog_entry() {
        let mut v = lambda();
        v.insert("OPENBOOK_MODELS".into(), FAKE_CATALOG.into());
        v.insert("OPENBOOK_DEFAULT_CHAT_MODEL".into(), "fake".into());
        v.insert("OPENBOOK_FAST_MODEL".into(), "fake".into());
        let err = Config::from_vars(&v).unwrap_err();
        assert!(err.to_string().contains("fake provider"), "{err}");
        // The same catalog is fine outside Lambda.
        v.remove("AWS_LAMBDA_RUNTIME_API");
        assert!(Config::from_vars(&v).is_ok());
    }

    #[test]
    fn lambda_refuses_a_fake_entry_that_is_not_the_default() {
        let mut v = lambda();
        let both = format!(
            "[{},{}]",
            REAL_CATALOG
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']'),
            FAKE_CATALOG
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
        );
        v.insert("OPENBOOK_MODELS".into(), both);
        let err = Config::from_vars(&v).unwrap_err();
        assert!(err.to_string().contains("fake provider"), "{err}");
    }

    #[test]
    fn lambda_refuses_each_local_only_variable() {
        for name in LOCAL_ONLY_VARS
            .iter()
            .copied()
            .chain(["AWS_ENDPOINT_URL_DYNAMODB"])
        {
            let value = if name == "OPENBOOK_PRESIGN_ENDPOINT" || name.starts_with("AWS_ENDPOINT") {
                "http://localhost:1"
            } else {
                "x"
            };
            let mut v = lambda();
            v.insert(name.into(), value.into());
            let err = Config::from_vars(&v).unwrap_err();
            assert!(
                err.to_string().contains(&format!("{name} is set")),
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn local_only_variables_are_fine_outside_lambda() {
        let mut v = base();
        v.insert("OPENBOOK_STAGE".into(), "local".into());
        v.insert("OPENBOOK_MODELS".into(), FAKE_CATALOG.into());
        v.insert("OPENBOOK_DEFAULT_CHAT_MODEL".into(), "fake".into());
        v.insert("OPENBOOK_FAST_MODEL".into(), "fake".into());
        v.insert("AWS_ENDPOINT_URL".into(), "https://127.0.0.1:9297".into());
        v.insert("OPENBOOK_CA_BUNDLE".into(), "/tmp/ca.pem".into());
        let c = Config::from_vars(&v).unwrap();
        assert!(c.catalog.has_fake());
        assert_eq!(
            c.aws_endpoint_url.as_deref(),
            Some("https://127.0.0.1:9297")
        );
    }
}
