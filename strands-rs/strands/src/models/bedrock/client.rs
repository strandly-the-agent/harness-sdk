//! Client setup: region resolution, SDK config, and the lazily created transport.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use aws_config::imds::region::ImdsRegionProvider;
use aws_config::meta::region::ProvideRegion as _;
use aws_config::profile::ProfileFileRegionProvider;
use aws_config::{BehaviorVersion, SdkConfig};
use aws_sdk_bedrockruntime::config::{AppName, Region, Token};
use aws_smithy_types::timeout::TimeoutConfig;
use tokio::sync::OnceCell;

use super::transport::{SdkTransport, Transport};
use super::{BedrockModelOptions, DEFAULT_REQUEST_TIMEOUT};
use crate::models::defaults::MODEL_DEFAULTS;

/// Reads an environment variable; tests pass a fixed map instead of the process environment.
pub(crate) type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub(crate) fn process_env() -> EnvLookup {
    Arc::new(|name| std::env::var(name).ok())
}

const APP_NAME: &str = "strands-agents-rust-sdk";

/// The client half of [`BedrockModelOptions`].
#[derive(Clone, Default)]
pub(crate) struct ClientOptions {
    pub(crate) region: Option<String>,
    pub(crate) sdk_config: Option<SdkConfig>,
    pub(crate) request_timeout: Option<Duration>,
    pub(crate) api_key: Option<String>,
}

impl ClientOptions {
    pub(crate) fn from_options(options: &BedrockModelOptions) -> Self {
        Self {
            region: options.region.clone(),
            sdk_config: options.client_config.clone(),
            request_timeout: options.request_timeout,
            api_key: options.api_key.clone(),
        }
    }
}

/// The region, in the JS SDK's order (TS lets `@smithy/core` resolve it): explicit, then the
/// SDK config's, then `AWS_REGION`, then the shared config profile, then the instance metadata
/// service, then the Bedrock default (TS falls back to it on "Region is missing").
pub(crate) async fn resolve_region<P, I>(
    options: &ClientOptions,
    env: &EnvLookup,
    profile_region: P,
    imds_region: I,
) -> String
where
    P: Future<Output = Option<String>>,
    I: Future<Output = Option<String>>,
{
    let configured = options
        .region
        .clone()
        .filter(|region| !region.is_empty())
        .or_else(|| {
            options
                .sdk_config
                .as_ref()
                .and_then(SdkConfig::region)
                .map(ToString::to_string)
        })
        .or_else(|| env("AWS_REGION").filter(|region| !region.is_empty()));
    if let Some(region) = configured {
        return region;
    }
    if let Some(region) = profile_region.await {
        return region;
    }
    if let Some(region) = imds_region.await {
        return region;
    }
    MODEL_DEFAULTS.bedrock.region.unwrap_or("us-west-2").to_owned()
}

/// `sdk_config` without `SigV4` credentials when an API key is set, so the client signs with the
/// bearer token instead.
pub(crate) fn effective_sdk_config(options: &ClientOptions, sdk_config: &SdkConfig) -> SdkConfig {
    if options.api_key.is_none() {
        return sdk_config.clone();
    }
    let mut builder = sdk_config.to_builder();
    builder.set_credentials_provider(None);
    builder.build()
}

/// The Bedrock client config for `sdk_config`, the resolved region and the options.
pub(crate) fn client_config(
    options: &ClientOptions,
    sdk_config: &SdkConfig,
    region: String,
) -> aws_sdk_bedrockruntime::Config {
    let read_timeout = options
        .request_timeout
        .or_else(|| sdk_config.timeout_config().and_then(TimeoutConfig::read_timeout))
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT);
    let timeouts = sdk_config
        .timeout_config()
        .map_or_else(TimeoutConfig::builder, TimeoutConfig::to_builder)
        .read_timeout(read_timeout)
        .build();
    let mut builder =
        aws_sdk_bedrockruntime::config::Builder::from(&effective_sdk_config(options, sdk_config))
            .region(Region::new(region))
            .timeout_config(timeouts);
    if sdk_config.app_name().is_none()
        && let Ok(app_name) = AppName::new(APP_NAME)
    {
        builder = builder.app_name(app_name);
    }
    if let Some(api_key) = &options.api_key {
        builder = builder.bearer_token(Token::new(api_key.clone(), None));
    }
    builder.build()
}

async fn build_transport(options: &ClientOptions, env: &EnvLookup) -> Arc<dyn Transport> {
    let profile_region = async {
        ProfileFileRegionProvider::new()
            .region()
            .await
            .map(|region| region.to_string())
    };
    let imds_region = async {
        ImdsRegionProvider::builder()
            .build()
            .region()
            .await
            .map(|region| region.to_string())
    };
    let region = resolve_region(options, env, profile_region, imds_region).await;
    let sdk_config = match &options.sdk_config {
        Some(sdk_config) => sdk_config.clone(),
        None => {
            aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new(region.clone()))
                .load()
                .await
        }
    };
    let client = aws_sdk_bedrockruntime::Client::from_conf(client_config(options, &sdk_config, region));
    Arc::new(SdkTransport::new(client))
}

/// The transport, created on first use so building a model stays synchronous and offline.
pub(crate) struct LazyClient {
    options: ClientOptions,
    env: EnvLookup,
    transport: OnceCell<Arc<dyn Transport>>,
}

impl fmt::Debug for LazyClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazyClient")
            .field("initialized", &self.transport.initialized())
            .finish_non_exhaustive()
    }
}

impl LazyClient {
    pub(crate) fn new(options: ClientOptions, env: EnvLookup) -> Self {
        Self {
            options,
            env,
            transport: OnceCell::new(),
        }
    }

    pub(crate) fn ready(transport: Arc<dyn Transport>) -> Self {
        Self {
            options: ClientOptions::default(),
            env: process_env(),
            transport: OnceCell::new_with(Some(transport)),
        }
    }

    pub(crate) async fn transport(&self) -> Arc<dyn Transport> {
        Arc::clone(
            self.transport
                .get_or_init(|| build_transport(&self.options, &self.env))
                .await,
        )
    }
}
