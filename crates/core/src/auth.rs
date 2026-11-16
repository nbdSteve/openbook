//! Cognito ID-token verification.
//!
//! The verifier accepts RS256 tokens only. It checks `iss`, `aud`, `token_use`, `exp` (60 s leeway)
//! and the presence of an `email` claim. Signing keys come from the JWKS URL, are cached by `kid`,
//! and are refetched for an unknown `kid` at most once per 60 seconds.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::Config;

/// How long to wait before the next refetch caused by an unknown `kid`.
pub const MIN_REFETCH_INTERVAL: Duration = Duration::from_secs(60);
/// How long to wait before the next fetch after a failed one.
pub const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(2);
const LEEWAY_SECONDS: u64 = 60;

/// The caller, as proven by a valid ID token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: String,
    pub email: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The token is not acceptable. The reason is for logs only.
    #[error("invalid token: {0}")]
    Invalid(String),
    /// The signing keys could not be fetched.
    #[error("signing keys unavailable: {0}")]
    Unavailable(String),
}

/// One RSA public key from a JWKS document.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    pub kid: Option<String>,
    pub kty: String,
    #[serde(default)]
    pub n: String,
    #[serde(default)]
    pub e: String,
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

/// Where signing keys come from.
#[async_trait]
pub trait JwksSource: Send + Sync {
    async fn fetch(&self) -> Result<Vec<Jwk>, String>;
}

/// Fetches a JWKS document over HTTPS.
pub struct HttpJwks {
    client: reqwest::Client,
    url: String,
}

impl HttpJwks {
    pub fn new(url: String, ca_bundle: Option<&Path>) -> Result<Self, String> {
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(5));
        if let Some(path) = ca_bundle {
            let pem =
                std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .map_err(|e| format!("invalid PEM in {}: {e}", path.display()))?;
            builder = builder.add_root_certificate(cert);
        }
        let client = builder
            .build()
            .map_err(|e| format!("cannot build HTTP client: {e}"))?;
        Ok(Self { client, url })
    }
}

#[async_trait]
impl JwksSource for HttpJwks {
    async fn fetch(&self) -> Result<Vec<Jwk>, String> {
        let response = self
            .client
            .get(&self.url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("GET {}: {e}", self.url))?;
        let set: JwkSet = response
            .json()
            .await
            .map_err(|e| format!("bad JWKS document: {e}"))?;
        Ok(set.keys)
    }
}

#[derive(Default)]
struct KeyCache {
    keys: HashMap<String, DecodingKey>,
    /// When the last fetch ended, and whether it worked.
    last_fetch: Option<(Instant, bool)>,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    email: Option<String>,
    /// Must be the JSON boolean `true`. A string such as "true" fails to parse and is refused.
    email_verified: Option<bool>,
    token_use: Option<String>,
}

/// Verifies Cognito ID tokens.
pub struct TokenVerifier {
    issuer: String,
    client_id: String,
    source: Arc<dyn JwksSource>,
    /// Held only for short reads and writes, never during a network call.
    cache: std::sync::Mutex<KeyCache>,
    /// Lets one task fetch at a time. Tokens with a cached `kid` never wait on it.
    fetch_gate: Mutex<()>,
    min_refetch: Duration,
    retry_after_failure: Duration,
}

impl TokenVerifier {
    pub fn new(issuer: String, client_id: String, source: Arc<dyn JwksSource>) -> Self {
        Self {
            issuer,
            client_id,
            source,
            cache: std::sync::Mutex::new(KeyCache::default()),
            fetch_gate: Mutex::new(()),
            min_refetch: MIN_REFETCH_INTERVAL,
            retry_after_failure: RETRY_AFTER_FAILURE,
        }
    }

    /// Builds a verifier from the configuration (JWKS URL and optional CA bundle).
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let source = HttpJwks::new(config.jwks_url(), config.ca_bundle.as_deref())?;
        Ok(Self::new(
            config.cognito_issuer(),
            config.cognito_client_id.clone(),
            Arc::new(source),
        ))
    }

    /// Changes the refetch throttles. Tests use this to avoid waiting.
    pub fn with_intervals(mut self, min_refetch: Duration, retry_after_failure: Duration) -> Self {
        self.min_refetch = min_refetch;
        self.retry_after_failure = retry_after_failure;
        self
    }

    /// Fetches the signing keys now. Call it at start-up. A failure is logged and not fatal.
    pub async fn prefetch(&self) {
        let _gate = self.fetch_gate.lock().await;
        if let Err(error) = self.refresh().await {
            tracing::warn!(%error, "could not prefetch the signing keys");
        }
    }

    /// Checks a token and returns the caller.
    pub async fn verify(&self, token: &str) -> Result<Principal, AuthError> {
        let header =
            decode_header(token).map_err(|e| AuthError::Invalid(format!("bad header: {e}")))?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::Invalid(format!(
                "algorithm {:?} is not allowed",
                header.alg
            )));
        }
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Invalid("no kid".into()))?;
        let key = self.key_for(&kid).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = LEEWAY_SECONDS;
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.client_id]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let data = decode::<Claims>(token, &key, &validation)
            .map_err(|e| AuthError::Invalid(e.to_string()))?;

        let claims = data.claims;
        if claims.token_use.as_deref() != Some("id") {
            return Err(AuthError::Invalid("token_use is not id".into()));
        }
        if claims.email_verified != Some(true) {
            return Err(AuthError::Invalid("email is not verified".into()));
        }
        let email = claims
            .email
            .map(|e| e.trim().to_owned())
            .filter(|e| !e.is_empty())
            .ok_or_else(|| AuthError::Invalid("no email claim".into()))?;
        if claims.sub.is_empty() {
            return Err(AuthError::Invalid("empty sub".into()));
        }
        Ok(Principal {
            user_id: claims.sub,
            email,
        })
    }

    fn cached(&self, kid: &str) -> Option<DecodingKey> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.keys.get(kid).cloned()
    }

    /// The error for a key that is missing and cannot be fetched now.
    /// With no keys at all, the key service is the problem (a 5xx), not the token.
    fn missing_key_error(&self) -> AuthError {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.keys.is_empty() {
            AuthError::Unavailable("no signing keys yet".into())
        } else {
            AuthError::Invalid("unknown kid".into())
        }
    }

    fn may_fetch(&self) -> bool {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.last_fetch {
            None => true,
            Some((at, true)) => at.elapsed() >= self.min_refetch,
            Some((at, false)) => at.elapsed() >= self.retry_after_failure,
        }
    }

    async fn key_for(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        if let Some(key) = self.cached(kid) {
            return Ok(key);
        }
        if !self.may_fetch() {
            return Err(self.missing_key_error());
        }
        let _gate = self.fetch_gate.lock().await;
        // Another task may have fetched while this one waited.
        if let Some(key) = self.cached(kid) {
            return Ok(key);
        }
        if !self.may_fetch() {
            return Err(self.missing_key_error());
        }
        self.refresh().await?;
        self.cached(kid)
            .ok_or_else(|| AuthError::Invalid("unknown kid".into()))
    }

    /// Fetches the keys and replaces the cache. The caller holds the fetch gate.
    async fn refresh(&self) -> Result<(), AuthError> {
        let fetched = self.source.fetch().await;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match fetched {
            Err(reason) => {
                cache.last_fetch = Some((Instant::now(), false));
                Err(AuthError::Unavailable(reason))
            }
            Ok(jwks) => {
                let mut keys = HashMap::new();
                for jwk in jwks {
                    let (Some(id), "RSA") = (jwk.kid.as_deref(), jwk.kty.as_str()) else {
                        continue;
                    };
                    if let Ok(key) = DecodingKey::from_rsa_components(&jwk.n, &jwk.e) {
                        keys.insert(id.to_owned(), key);
                    }
                }
                cache.keys = keys;
                cache.last_fetch = Some((Instant::now(), true));
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use rsa::PrivateKey;
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    use serde_json::{Value, json};

    use super::*;

    const ISS: &str = "https://cognito-idp.us-east-1.amazonaws.com/us-east-1_pool";
    const AUD: &str = "client123";

    struct TestKey {
        kid: String,
        encoding: EncodingKey,
        jwk: Jwk,
    }

    fn strip_zeros(bytes: &[u8]) -> Vec<u8> {
        let start = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
        bytes[start..].to_vec()
    }

    fn test_key(kid: &str) -> TestKey {
        let private = PrivateKey::new(&mut rand::rngs::SysRng, 2048).unwrap();
        let der = private.to_pkcs1_der().unwrap();
        let public = private.to_public_key();
        TestKey {
            kid: kid.into(),
            encoding: EncodingKey::from_rsa_der(der.as_bytes()),
            jwk: Jwk {
                kid: Some(kid.into()),
                kty: "RSA".into(),
                n: URL_SAFE_NO_PAD.encode(strip_zeros(&public.n().to_be_bytes())),
                e: URL_SAFE_NO_PAD.encode(strip_zeros(&public.e().to_be_bytes())),
            },
        }
    }

    struct StaticJwks {
        keys: std::sync::Mutex<Vec<Jwk>>,
        fetches: AtomicUsize,
        failing: std::sync::atomic::AtomicBool,
    }

    impl StaticJwks {
        fn new(keys: Vec<Jwk>) -> Arc<Self> {
            Arc::new(Self {
                keys: std::sync::Mutex::new(keys),
                fetches: AtomicUsize::new(0),
                failing: std::sync::atomic::AtomicBool::new(false),
            })
        }
        fn fetches(&self) -> usize {
            self.fetches.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl JwksSource for StaticJwks {
        async fn fetch(&self) -> Result<Vec<Jwk>, String> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            if self.failing.load(Ordering::SeqCst) {
                return Err("down".into());
            }
            Ok(self.keys.lock().unwrap().clone())
        }
    }

    // Adapter so the test keeps its own handle to the source.
    struct Shared(Arc<StaticJwks>);

    #[async_trait]
    impl JwksSource for Shared {
        async fn fetch(&self) -> Result<Vec<Jwk>, String> {
            self.0.fetch().await
        }
    }

    fn now() -> u64 {
        jsonwebtoken::get_current_timestamp()
    }

    fn claims() -> Value {
        json!({
            "sub": "user-1", "email": "a@example.com", "iss": ISS, "aud": AUD,
            "email_verified": true, "token_use": "id", "exp": now() + 3600, "iat": now()
        })
    }

    fn sign(key: &TestKey, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(key.kid.clone());
        encode(&header, claims, &key.encoding).unwrap()
    }

    fn verifier(source: &Arc<StaticJwks>) -> TokenVerifier {
        TokenVerifier::new(ISS.into(), AUD.into(), Arc::new(Shared(source.clone())))
    }

    fn with(mut base: Value, field: &str, value: Value) -> Value {
        base[field] = value;
        base
    }

    #[tokio::test]
    async fn accepts_a_valid_token() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        let principal = verifier(&source)
            .verify(&sign(&key, &claims()))
            .await
            .unwrap();
        assert_eq!(
            principal,
            Principal {
                user_id: "user-1".into(),
                email: "a@example.com".into()
            }
        );
    }

    #[tokio::test]
    async fn rejects_bad_claims() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        let v = verifier(&source);
        let cases = [
            ("expired", with(claims(), "exp", json!(now() - 3600))),
            ("wrong aud", with(claims(), "aud", json!("other"))),
            (
                "wrong iss",
                with(claims(), "iss", json!("https://evil.example.com/pool")),
            ),
            ("access token", with(claims(), "token_use", json!("access"))),
            ("no token_use", {
                let mut c = claims();
                c.as_object_mut().unwrap().remove("token_use");
                c
            }),
            ("no email", {
                let mut c = claims();
                c.as_object_mut().unwrap().remove("email");
                c
            }),
            ("blank email", with(claims(), "email", json!("  "))),
            (
                "email not verified",
                with(claims(), "email_verified", json!(false)),
            ),
            (
                "email_verified as a string",
                with(claims(), "email_verified", json!("true")),
            ),
            (
                "email_verified null",
                with(claims(), "email_verified", Value::Null),
            ),
            ("no email_verified", {
                let mut c = claims();
                c.as_object_mut().unwrap().remove("email_verified");
                c
            }),
            ("no exp", {
                let mut c = claims();
                c.as_object_mut().unwrap().remove("exp");
                c
            }),
        ];
        for (name, c) in cases {
            let err = v.verify(&sign(&key, &c)).await.unwrap_err();
            assert!(matches!(err, AuthError::Invalid(_)), "{name}: {err}");
        }
    }

    #[tokio::test]
    async fn allows_sixty_seconds_of_clock_skew() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        let v = verifier(&source);
        assert!(
            v.verify(&sign(&key, &with(claims(), "exp", json!(now() - 30))))
                .await
                .is_ok()
        );
        assert!(
            v.verify(&sign(&key, &with(claims(), "exp", json!(now() - 120))))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_a_token_signed_by_another_key() {
        let key = test_key("k1");
        let attacker = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        assert!(
            verifier(&source)
                .verify(&sign(&attacker, &claims()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_other_algorithms() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        let v = verifier(&source);
        // HS256 signed with the public modulus as the secret (the classic confusion attack).
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("k1".into());
        let token = encode(
            &header,
            &claims(),
            &EncodingKey::from_secret(key.jwk.n.as_bytes()).unwrap(),
        )
        .unwrap();
        assert!(v.verify(&token).await.is_err());
        // RS384 with the right key.
        let mut header = Header::new(Algorithm::RS384);
        header.kid = Some("k1".into());
        let token = encode(&header, &claims(), &key.encoding).unwrap();
        assert!(v.verify(&token).await.is_err());
        // alg none.
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","kid":"k1"}"#);
        let body = URL_SAFE_NO_PAD.encode(claims().to_string());
        assert!(v.verify(&format!("{header}.{body}.")).await.is_err());
        // Garbage.
        assert!(v.verify("not-a-jwt").await.is_err());
        assert_eq!(
            source.fetches(),
            0,
            "wrong algorithms must not cause key fetches"
        );
    }

    #[tokio::test]
    async fn rejects_a_token_without_kid() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        let token = encode(&Header::new(Algorithm::RS256), &claims(), &key.encoding).unwrap();
        assert!(verifier(&source).verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn caches_keys_and_throttles_refetch_for_unknown_kid() {
        let known = test_key("k1");
        let unknown = test_key("k2");
        let source = StaticJwks::new(vec![known.jwk.clone()]);
        let v = verifier(&source);

        v.verify(&sign(&known, &claims())).await.unwrap();
        v.verify(&sign(&known, &claims())).await.unwrap();
        assert_eq!(source.fetches(), 1);

        // Unknown kid right after a fetch: no new fetch.
        assert!(v.verify(&sign(&unknown, &claims())).await.is_err());
        assert!(v.verify(&sign(&unknown, &claims())).await.is_err());
        assert_eq!(source.fetches(), 1);
    }

    #[tokio::test]
    async fn refetches_after_the_interval_and_picks_up_rotated_keys() {
        let old = test_key("k1");
        let new = test_key("k2");
        let source = StaticJwks::new(vec![old.jwk.clone()]);
        let v = verifier(&source).with_intervals(Duration::ZERO, Duration::ZERO);
        v.verify(&sign(&old, &claims())).await.unwrap();

        *source.keys.lock().unwrap() = vec![old.jwk.clone(), new.jwk.clone()];
        v.verify(&sign(&new, &claims())).await.unwrap();
        assert_eq!(source.fetches(), 2);
        // Both keys are cached now.
        v.verify(&sign(&old, &claims())).await.unwrap();
        v.verify(&sign(&new, &claims())).await.unwrap();
        assert_eq!(source.fetches(), 2);
    }

    #[tokio::test]
    async fn unknown_kid_is_rejected() {
        let known = test_key("k1");
        let stranger = test_key("zzz");
        let source = StaticJwks::new(vec![known.jwk.clone()]);
        let err = verifier(&source)
            .verify(&sign(&stranger, &claims()))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Invalid(m) if m.contains("kid")));
    }

    #[tokio::test]
    async fn key_service_failure_is_not_an_invalid_token() {
        struct Failing;
        #[async_trait]
        impl JwksSource for Failing {
            async fn fetch(&self) -> Result<Vec<Jwk>, String> {
                Err("down".into())
            }
        }
        let key = test_key("k1");
        let v = TokenVerifier::new(ISS.into(), AUD.into(), Arc::new(Failing));
        let err = v.verify(&sign(&key, &claims())).await.unwrap_err();
        assert!(matches!(err, AuthError::Unavailable(_)));
    }

    #[tokio::test]
    async fn a_failed_fetch_is_a_5xx_for_every_request_until_the_retry_time() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        source.failing.store(true, Ordering::SeqCst);
        let v = verifier(&source);
        let token = sign(&key, &claims());
        // The fetch fails. The next request is throttled and still says "unavailable", not "invalid".
        assert!(matches!(
            v.verify(&token).await,
            Err(AuthError::Unavailable(_))
        ));
        assert!(matches!(
            v.verify(&token).await,
            Err(AuthError::Unavailable(_))
        ));
        assert_eq!(source.fetches(), 1, "the retry is throttled");
    }

    #[tokio::test]
    async fn valid_tokens_work_again_as_soon_as_the_retry_is_allowed() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        source.failing.store(true, Ordering::SeqCst);
        let v = verifier(&source).with_intervals(MIN_REFETCH_INTERVAL, Duration::ZERO);
        let token = sign(&key, &claims());
        assert!(matches!(
            v.verify(&token).await,
            Err(AuthError::Unavailable(_))
        ));
        source.failing.store(false, Ordering::SeqCst);
        assert!(v.verify(&token).await.is_ok());
    }

    #[tokio::test]
    async fn prefetch_loads_the_keys_before_the_first_request() {
        let key = test_key("k1");
        let source = StaticJwks::new(vec![key.jwk.clone()]);
        let v = verifier(&source);
        v.prefetch().await;
        assert_eq!(source.fetches(), 1);
        assert!(v.verify(&sign(&key, &claims())).await.is_ok());
        assert_eq!(source.fetches(), 1);
        // A failed prefetch does not panic and leaves the verifier usable.
        let down = StaticJwks::new(vec![]);
        down.failing.store(true, Ordering::SeqCst);
        verifier(&down).prefetch().await;
    }

    #[tokio::test]
    async fn verifying_does_not_wait_for_a_slow_fetch_of_another_kid() {
        struct Slow(tokio::sync::Notify, Jwk);
        #[async_trait]
        impl JwksSource for Slow {
            async fn fetch(&self) -> Result<Vec<Jwk>, String> {
                self.0.notified().await;
                Ok(vec![self.1.clone()])
            }
        }
        let known = test_key("k1");
        let slow = Arc::new(Slow(tokio::sync::Notify::new(), known.jwk.clone()));
        struct Wrap(Arc<Slow>);
        #[async_trait]
        impl JwksSource for Wrap {
            async fn fetch(&self) -> Result<Vec<Jwk>, String> {
                self.0.fetch().await
            }
        }
        let v = Arc::new(TokenVerifier::new(
            ISS.into(),
            AUD.into(),
            Arc::new(Wrap(slow.clone())),
        ));
        let token = sign(&known, &claims());
        let first = tokio::spawn({
            let (v, token) = (v.clone(), token.clone());
            async move { v.verify(&token).await }
        });
        tokio::task::yield_now().await;
        slow.0.notify_one();
        assert!(first.await.unwrap().is_ok());
        // Cached keys answer without any fetch, so a stuck fetch cannot block them.
        let stuck = tokio::spawn({
            let v = v.clone();
            let stranger = sign(&test_key("zzz"), &claims());
            async move { v.verify(&stranger).await }
        });
        tokio::task::yield_now().await;
        assert!(v.verify(&token).await.is_ok());
        stuck.abort();
    }
}
