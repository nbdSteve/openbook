//! AWS client setup: one `SdkConfig` for the AWS services, and a special Bedrock client for local hybrid mode.

use std::path::{Path, PathBuf};

use aws_config::profile::ProfileFileCredentialsProvider;
use aws_config::provider_config::ProviderConfig;
use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_runtime::env_config::file::{EnvConfigFileKind, EnvConfigFiles};
use aws_smithy_http_client::tls::rustls_provider::CryptoMode;
use aws_smithy_http_client::tls::{Provider, TlsContext, TrustStore};
use aws_smithy_types::error::display::DisplayErrorContext;

use crate::config::Config;

/// Region of the Bedrock client in local hybrid mode (data stays in Australia).
pub const BEDROCK_REGION: &str = "ap-southeast-2";

/// An HTTP client for the AWS SDK that trusts the native roots plus the PEM file.
pub fn http_client_with_ca(
    ca_bundle: &Path,
) -> Result<aws_sdk_dynamodb::config::SharedHttpClient, String> {
    let pem = std::fs::read(ca_bundle)
        .map_err(|e| format!("cannot read {}: {e}", ca_bundle.display()))?;
    let trust_store = TrustStore::empty()
        .with_native_roots(true)
        .with_pem_certificate(pem);
    let tls = TlsContext::builder()
        .with_trust_store(trust_store)
        .build()
        .map_err(|e| format!("cannot build the TLS context: {}", DisplayErrorContext(&e)))?;
    Ok(aws_smithy_http_client::Builder::new()
        .tls_provider(Provider::Rustls(CryptoMode::AwsLc))
        .tls_context(tls)
        .build_https())
}

/// Loads the shared `SdkConfig` for DynamoDB and S3.
pub async fn load_sdk_config(config: &Config) -> Result<SdkConfig, String> {
    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(region) = &config.aws_region {
        loader = loader.region(Region::new(region.clone()));
    }
    if let Some(url) = &config.aws_endpoint_url {
        loader = loader.endpoint_url(url);
    }
    if let Some(ca) = &config.ca_bundle {
        loader = loader.http_client(http_client_with_ca(ca)?);
    }
    Ok(loader.load().await)
}

/// Builds the Bedrock client.
///
/// Without a profile, it uses the shared `SdkConfig` (Lambda role credentials).
/// With `OPENBOOK_BEDROCK_PROFILE` (local hybrid mode), it reads the real `~/.aws/config` and
/// `~/.aws/credentials` for that profile and calls the real regional endpoint. The emulator
/// settings (`AWS_ENDPOINT_URL`, test keys in the environment) do not apply to it.
pub fn bedrock_client(
    config: &Config,
    sdk: &SdkConfig,
) -> Result<aws_sdk_bedrockruntime::Client, String> {
    let Some(profile) = &config.bedrock_profile else {
        return Ok(aws_sdk_bedrockruntime::Client::new(sdk));
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    Ok(hybrid_bedrock_client(profile, &home))
}

/// The local hybrid-mode Bedrock client: the real regional endpoint, Region `ap-southeast-2`, and
/// credentials from the profile files under `home/.aws`.
pub fn hybrid_bedrock_client(profile: &str, home: &Path) -> aws_sdk_bedrockruntime::Client {
    let files = EnvConfigFiles::builder()
        .with_file(EnvConfigFileKind::Config, home.join(".aws").join("config"))
        .with_file(
            EnvConfigFileKind::Credentials,
            home.join(".aws").join("credentials"),
        )
        .build();
    let region = Region::new(BEDROCK_REGION);
    let provider_config = ProviderConfig::without_region().with_region(Some(region.clone()));
    let credentials = ProfileFileCredentialsProvider::builder()
        .configure(&provider_config)
        .profile_files(files)
        .profile_name(profile)
        .build();
    let conf = aws_sdk_bedrockruntime::Config::builder()
        .behavior_version(aws_sdk_bedrockruntime::config::BehaviorVersion::latest())
        .region(region)
        .endpoint_url(format!(
            "https://bedrock-runtime.{BEDROCK_REGION}.amazonaws.com"
        ))
        .credentials_provider(credentials)
        .build();
    aws_sdk_bedrockruntime::Client::from_conf(conf)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use aws_smithy_runtime_api::box_error::BoxError;
    use aws_smithy_runtime_api::client::interceptors::Intercept;
    use aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextRef;
    use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
    use aws_smithy_types::config_bag::ConfigBag;

    use super::*;

    /// Records the request URI and the signing headers, then stops before anything goes on the wire.
    #[derive(Debug, Clone, Default)]
    struct Capture(Arc<Mutex<Option<(String, String)>>>);

    impl Intercept for Capture {
        fn name(&self) -> &'static str {
            "capture"
        }

        fn read_before_transmit(
            &self,
            context: &BeforeTransmitInterceptorContextRef<'_>,
            _components: &RuntimeComponents,
            _config: &mut ConfigBag,
        ) -> Result<(), BoxError> {
            let request = context.request();
            let auth = request
                .headers()
                .get("authorization")
                .unwrap_or_default()
                .to_owned();
            *self.0.lock().unwrap() = Some((request.uri().to_owned(), auth));
            Err("stop before the network".into())
        }
    }

    #[tokio::test]
    async fn hybrid_mode_uses_the_real_regional_endpoint_and_the_profile_keys() {
        // A throw-away home with a profile that has made-up keys.
        let home =
            std::env::temp_dir().join(format!("openbook-test-home-{}", crate::ids::new_id()));
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        std::fs::write(
            home.join(".aws/config"),
            "[profile hybrid]\nregion = us-west-2\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".aws/credentials"),
            "[hybrid]\naws_access_key_id = AKIAPROFILEKEY00000\naws_secret_access_key = not-a-real-secret\n",
        )
        .unwrap();

        let client = hybrid_bedrock_client("hybrid", &home);
        assert_eq!(
            client.config().region().map(|r| r.as_ref()),
            Some("ap-southeast-2")
        );

        let capture = Capture::default();
        let result = client
            .converse_stream()
            .model_id("au.anthropic.claude-test")
            .customize()
            .interceptor(capture.clone())
            .send()
            .await;
        assert!(result.is_err());
        let (uri, auth) = capture
            .0
            .lock()
            .unwrap()
            .clone()
            .expect("the request reached the interceptor");
        assert!(
            uri.starts_with("https://bedrock-runtime.ap-southeast-2.amazonaws.com/"),
            "{uri}"
        );
        assert!(
            !uri.contains("127.0.0.1") && !uri.contains("localhost"),
            "{uri}"
        );
        // The profile keys signed the request, for the Bedrock service in Sydney.
        assert!(auth.contains("AKIAPROFILEKEY00000/"), "{auth}");
        assert!(auth.contains("/ap-southeast-2/bedrock/"), "{auth}");
        std::fs::remove_dir_all(&home).unwrap();
    }
}
