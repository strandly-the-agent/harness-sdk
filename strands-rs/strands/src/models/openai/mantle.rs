//! Amazon Bedrock's OpenAI-compatible Mantle endpoint (TS `models/openai/mantle.ts`), behind the
//! `openai` and `bedrock` features: the region, the per-model base path and the bearer token.
//!
//! The token is the one `@aws/bedrock-token-generator` mints: `bedrock-api-key-` and the base64
//! of a SigV4-presigned `CallWithBearerToken` URL with `&Version=1`, valid for 12 hours unless
//! `expires_in_seconds` says otherwise. A fresh token is minted for every request.

use std::fmt;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, SystemTime};

use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_credential_types::provider::{ProvideCredentials as _, SharedCredentialsProvider};
use aws_sdk_bedrockruntime::config::Region;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SignatureLocation, SigningSettings, sign};
use aws_sigv4::sign::v4;
use base64::Engine as _;
use futures::future::BoxFuture;
use regex::Regex;
use tokio::sync::OnceCell;

use super::client::ApiKeySetter;
use crate::error::{BoxError, Error, Result};

const MANTLE_DOCS_URL: &str = "https://docs.aws.amazon.com/bedrock/latest/userguide/inference-openai.html";

/// Mantle model lines served from `/openai/v1`; every other model uses `/v1`, and the wrong base
/// path fails with HTTP 400. Each prefix names one model line, never a whole vendor.
const OPENAI_PATH_MODEL_PREFIXES: [&str; 6] = [
    "openai.gpt-5.",
    "openai.gpt-daybreak-",
    "openai.gpt-6-",
    "openai.gpt-6.",
    "xai.grok-4.",
    "google.gemma-4-",
];

/// Anchored, so a region with URL control characters can't re-point the endpoint to another host.
static VALID_REGION: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new("^[a-z]{2}(-[a-z]+)+-[0-9]+$").ok());

const MAX_TOKEN_EXPIRES_IN_SECONDS: u64 = 43_200;
const SERVICE_NAME: &str = "bedrock";
const TOKEN_HOST: &str = "bedrock.amazonaws.com";
const AUTH_PREFIX: &str = "bedrock-api-key-";
const TOKEN_VERSION: &str = "&Version=1";

/// Routes an [`OpenAIModel`](super::OpenAIModel) through Amazon Bedrock's OpenAI-compatible
/// Mantle endpoint (TS `BedrockMantleConfig`).
///
/// The client's base URL and API key come from this config, so it can't be combined with an
/// `api_key` or with `client_config.api_key` or `client_config.base_url`.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct BedrockMantleConfig {
    /// The AWS region of the endpoint; defaults to `AWS_REGION`, else `AWS_DEFAULT_REGION`.
    pub region: Option<String>,
    /// The credentials the token is signed with; defaults to the standard AWS credential chain.
    pub credentials: Option<SharedCredentialsProvider>,
    /// The token lifetime in seconds, at most 12 hours (the default).
    pub expires_in_seconds: Option<u64>,
}

/// What a token provider is built from (the `getTokenProvider` options).
#[derive(Debug, Clone)]
pub(crate) struct TokenOptions {
    pub(crate) region: String,
    pub(crate) credentials: Option<SharedCredentialsProvider>,
    pub(crate) expires_in_seconds: Option<u64>,
}

/// Mints a bearer token per call (TS `TokenProvider`).
pub(crate) type TokenProvider = Arc<dyn Fn() -> BoxFuture<'static, Result<String, BoxError>> + Send + Sync>;
/// Builds the token provider (TS `getTokenProvider`); tests replace it.
pub(crate) type TokenProviderFactory =
    Arc<dyn Fn(TokenOptions) -> Result<TokenProvider, BoxError> + Send + Sync>;

fn validate_region(region: &str) -> Result<String> {
    if VALID_REGION.as_ref().is_some_and(|valid| valid.is_match(region)) {
        return Ok(region.to_owned());
    }
    Err(Error::other(format!("invalid AWS region: '{region}'")))
}

/// The configured region, else `AWS_REGION`, else `AWS_DEFAULT_REGION`, validated (TS `resolveMantleRegion`).
pub(crate) fn resolve_mantle_region(
    config: &BedrockMantleConfig,
    env: &(dyn Fn(&str) -> Option<String> + Send + Sync),
) -> Result<String> {
    let region = config
        .region
        .clone()
        .filter(|region| !region.is_empty())
        .or_else(|| env("AWS_REGION").filter(|region| !region.is_empty()))
        .or_else(|| env("AWS_DEFAULT_REGION").filter(|region| !region.is_empty()));
    match region {
        Some(region) => validate_region(&region),
        None => Err(Error::other(format!(
            "could not resolve an AWS region for Bedrock Mantle. Pass 'region' in bedrock_mantle_config or set AWS_REGION in the environment. See {MANTLE_DOCS_URL} for supported regions."
        ))),
    }
}

/// `/openai/v1` for the model lines served there, else `/v1` (TS `resolveMantleBasePath`).
pub(crate) fn resolve_mantle_base_path(model_id: &str) -> &'static str {
    if OPENAI_PATH_MODEL_PREFIXES
        .iter()
        .any(|prefix| model_id.starts_with(prefix))
    {
        "/openai/v1"
    } else {
        "/v1"
    }
}

/// The Mantle base URL for a region and model (TS `bedrockMantleBaseUrl`).
pub(crate) fn bedrock_mantle_base_url(region: &str, model_id: &str) -> String {
    format!(
        "https://bedrock-mantle.{region}.api.aws{}",
        resolve_mantle_base_path(model_id)
    )
}

/// An API key setter that mints a fresh token per request, building the token provider once on
/// first use (TS `createMantleApiKeySetter`).
pub(crate) fn mantle_api_key_setter(
    config: &BedrockMantleConfig,
    region: &str,
    factory: TokenProviderFactory,
) -> ApiKeySetter {
    let options = TokenOptions {
        region: region.to_owned(),
        credentials: config.credentials.clone(),
        expires_in_seconds: config.expires_in_seconds,
    };
    let provider: Arc<OnceLock<Result<TokenProvider, String>>> = Arc::new(OnceLock::new());
    Arc::new(move || {
        let provider = provider
            .get_or_init(|| factory(options.clone()).map_err(|error| error.to_string()))
            .clone();
        let region = options.region.clone();
        Box::pin(async move {
            let provide_token = provider.map_err(BoxError::from)?;
            provide_token().await.map_err(|cause| {
                tracing::debug!("region=<{region}>, error=<{cause}> | bearer token minting failed");
                BoxError::from(MintError { region, cause })
            })
        })
    })
}

/// A failed token mint, with the actionable TS wording.
#[derive(Debug)]
struct MintError {
    region: String,
    cause: BoxError,
}

impl fmt::Display for MintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to mint Bedrock Mantle bearer token for region '{}' | verify your AWS credentials and network connectivity",
            self.region
        )
    }
}

impl std::error::Error for MintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// The default factory: `SigV4` tokens from the configured credentials or the default chain.
pub(crate) fn sigv4_token_provider_factory() -> TokenProviderFactory {
    Arc::new(|options| sigv4_token_provider(options, Arc::new(SystemTime::now)))
}

/// A provider that signs with `options.credentials`, resolving the default chain once when unset.
pub(crate) fn sigv4_token_provider(
    options: TokenOptions,
    clock: Arc<dyn Fn() -> SystemTime + Send + Sync>,
) -> Result<TokenProvider, BoxError> {
    if let Some(seconds) = options.expires_in_seconds
        && (seconds == 0 || seconds > MAX_TOKEN_EXPIRES_IN_SECONDS)
    {
        return Err(format!(
            "ExpiresInSeconds must be in the range (0, {MAX_TOKEN_EXPIRES_IN_SECONDS}] seconds."
        )
        .into());
    }
    let expires_in = Duration::from_secs(options.expires_in_seconds.unwrap_or(MAX_TOKEN_EXPIRES_IN_SECONDS));
    let chain: Arc<OnceCell<Option<Arc<CachedCredentials>>>> = Arc::new(OnceCell::new());
    Ok(Arc::new(move || {
        let configured = options.credentials.clone();
        let region = options.region.clone();
        let chain = Arc::clone(&chain);
        let clock = Arc::clone(&clock);
        Box::pin(async move {
            let now = clock();
            let credentials = match configured {
                Some(provider) => provider.provide_credentials().await?,
                None => {
                    let cached = chain
                        .get_or_init(|| default_credentials(region.clone()))
                        .await
                        .clone()
                        .ok_or("no AWS credentials provider is configured")?;
                    cached.get(now).await?
                }
            };
            create_token(&credentials, &region, expires_in, now)
        })
    }))
}

async fn default_credentials(region: String) -> Option<Arc<CachedCredentials>> {
    let provider = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region))
        .load()
        .await
        .credentials_provider()?;
    Some(Arc::new(CachedCredentials::new(provider)))
}

/// Credentials treated as expired this long before their expiry (the JS default chain's 5 minutes).
const CREDENTIALS_REFRESH_WINDOW: Duration = Duration::from_secs(300);

/// The default chain's credentials, memoized as the JS `fromNodeProviderChain` does: fetched again
/// only within five minutes of their expiry, never when they carry none.
pub(crate) struct CachedCredentials {
    provider: SharedCredentialsProvider,
    cached: tokio::sync::Mutex<Option<Credentials>>,
}

impl CachedCredentials {
    pub(crate) fn new(provider: SharedCredentialsProvider) -> Self {
        Self {
            provider,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// The cached credentials, fetched again when missing or about to expire at `now`. A failed
    /// refresh of credentials that haven't expired yet keeps them.
    pub(crate) async fn get(&self, now: SystemTime) -> Result<Credentials, BoxError> {
        let mut cached = self.cached.lock().await;
        let expires_within = |credentials: &Credentials, window: Duration| {
            credentials.expiry().is_some_and(|expiry| expiry <= now + window)
        };
        if let Some(credentials) = cached.as_ref()
            && !expires_within(credentials, CREDENTIALS_REFRESH_WINDOW)
        {
            return Ok(credentials.clone());
        }
        match self.provider.provide_credentials().await {
            Ok(fresh) => {
                *cached = Some(fresh.clone());
                Ok(fresh)
            }
            Err(error) => match cached.as_ref() {
                Some(credentials) if !expires_within(credentials, Duration::ZERO) => Ok(credentials.clone()),
                _ => Err(error.into()),
            },
        }
    }
}

/// AWS URI encoding: everything but `A-Z a-z 0-9 - _ . ~` as uppercase `%XX`.
fn uri_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// A bearer token from a presigned `CallWithBearerToken` URL (the token generator's `createToken`).
pub(crate) fn create_token(
    credentials: &Credentials,
    region: &str,
    expires_in: Duration,
    now: SystemTime,
) -> Result<String, BoxError> {
    let identity = credentials.clone().into();
    let mut settings = SigningSettings::default();
    settings.signature_location = SignatureLocation::QueryParams;
    settings.expires_in = Some(expires_in);
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(SERVICE_NAME)
        .time(now)
        .settings(settings)
        .build()?
        .into();
    let url = format!("https://{TOKEN_HOST}/?Action=CallWithBearerToken");
    let request = SignableRequest::new(
        "POST",
        &url,
        std::iter::once(("host", TOKEN_HOST)),
        SignableBody::Bytes(&[]),
    )?;
    let (instructions, _) = sign(request, &params)?.into_parts();
    let mut query: Vec<(String, String)> = vec![("Action".to_owned(), "CallWithBearerToken".to_owned())];
    query.extend(
        instructions
            .params()
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.to_string())),
    );
    query.sort();
    let query: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{}={}", uri_encode(name), uri_encode(value)))
        .collect();
    let presigned = format!("{TOKEN_HOST}/?{}{TOKEN_VERSION}", query.join("&"));
    Ok(format!(
        "{AUTH_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(presigned)
    ))
}

#[cfg(test)]
mod tests;
