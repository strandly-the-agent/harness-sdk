//! The OpenAI provider (TS `models/openai/`), behind the `openai` feature: the Responses API
//! (the default) and the Chat Completions API.
//!
//! TS wraps the `openai` package; this module speaks both APIs over `reqwest` and reproduces
//! that package's transport: its environment defaults (`OPENAI_API_KEY`, `OPENAI_BASE_URL`,
//! `OPENAI_ORG_ID`, `OPENAI_PROJECT_ID`, `OPENAI_CUSTOM_HEADERS`), two retries with backoff for
//! connection errors and retryable statuses, and a ten-minute timeout.
//!
//! ```no_run
//! use strands::models::openai::{OpenAIModel, OpenAIModelConfig};
//!
//! # fn main() -> strands::Result<()> {
//! // The API key comes from `OPENAI_API_KEY`.
//! let model = OpenAIModel::new(OpenAIModelConfig {
//!     model_id: Some("gpt-5.4".into()),
//!     ..Default::default()
//! })?;
//! let agent = strands::Agent::builder().model(model).build()?;
//! # drop(agent);
//! # Ok(())
//! # }
//! ```

mod cache;
mod chat;
mod client;
mod errors;
mod formatting;
#[cfg(feature = "bedrock")]
mod mantle;
mod responses;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use http::{HeaderMap, HeaderValue};

use self::chat::{ChatStream, format_chat_request};
pub use self::client::ApiKeySetter;
use self::client::{
    ApiError, ApiErrorKind, ApiRequest, CHAT_COMPLETIONS_PATH, DEFAULT_BASE_URL, DEFAULT_MAX_RETRIES,
    DEFAULT_TIMEOUT, EventStream, HttpClient, HttpClientParts, KeySource, OpenAIClient, RESPONSES_PATH,
    merge_headers, parse_custom_headers,
};
use self::errors::{OpenAIErrorKind, classify_openai_error};
use self::formatting::warn_managed_params;
#[cfg(feature = "bedrock")]
pub use self::mantle::BedrockMantleConfig;
use self::responses::{ResponsesStream, format_responses_request};
use crate::cancel::CancelSignal;
use crate::error::{
    ContextWindowOverflowError, Error, ModelError, ModelNetworkError, ModelThrottledError, Result,
};
use crate::logging::warn_once::warn_once;
use crate::models::defaults::{MODEL_DEFAULTS, default_model_warning_message};
use crate::models::model::{
    BaseModelConfig, BoxStream, CacheConfig, Model, ModelRequest, base_model_info, resolve_config_metadata,
};
use crate::models::model_info::{ModelCapabilities, ModelInfo, SERVER_SIDE_SUMMARY_OFF, merge_model_info};
use crate::models::streaming::ModelStreamEvent;
use crate::retry::model_retry_strategy::{Sleeper, tokio_sleeper};
use crate::types::json::{JsonObject, JsonValue};

const API_KEY_ENV: &str = "OPENAI_API_KEY";
const BASE_URL_ENV: &str = "OPENAI_BASE_URL";
const ORG_ID_ENV: &str = "OPENAI_ORG_ID";
const PROJECT_ID_ENV: &str = "OPENAI_PROJECT_ID";
const CUSTOM_HEADERS_ENV: &str = "OPENAI_CUSTOM_HEADERS";

/// Which OpenAI API a model speaks (TS `OpenAIApi`); fixed at construction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenAIApi {
    /// The Responses API (`responses`), optionally with server-side conversation state.
    #[default]
    Responses,
    /// The Chat Completions API (`chat`), always stateless.
    Chat,
}

impl OpenAIApi {
    /// The TS literal (`'responses'` or `'chat'`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::Chat => "chat",
        }
    }
}

/// Configuration of an [`OpenAIModel`] (TS `OpenAIModelConfig`).
///
/// `frequency_penalty` and `presence_penalty` apply to Chat Completions only, and `stateful` to
/// the Responses API only; the other API ignores them, as in TS.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
#[builder(on(String, into))]
pub struct OpenAIModelConfig {
    /// The model id; defaults to `gpt-5.4`, which may change between versions.
    pub model_id: Option<String>,
    /// The largest number of tokens to generate (`max_output_tokens`, or
    /// `max_completion_tokens` on Chat Completions).
    pub max_tokens: Option<u64>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Nucleus sampling probability.
    pub top_p: Option<f64>,
    /// The context window in tokens; overrides the model-info table.
    pub context_window_limit: Option<u64>,
    /// Model facts merged over the shared model-info table.
    pub model_info: Option<ModelInfo>,
    /// Extra request fields, merged into the request first. The provider-managed keys (Responses:
    /// `model`, `input`, `stream`, `store`; Chat Completions: `model`, `messages`, `stream`,
    /// `stream_options`) are overwritten, with a warning. Built-in Responses tools such as
    /// `{"type": "web_search"}` go in `tools` and are sent before the agent's function tools.
    pub params: Option<JsonObject>,
    /// Prompt caching. `cache_key` maps to `prompt_cache_key` (defaulting to
    /// `strands-<sessionId>`), and the retention literals `in_memory` and `24h` map to
    /// `prompt_cache_retention`. On the Responses API, models whose model info takes cache points
    /// get a checkpoint on each durable user message (`messages_ttl: false` turns that off), and
    /// a `30m` or `12h` TTL sets the checkpoint and `prompt_cache_options` TTL. Native caching
    /// fields in `params` win.
    pub cache_config: Option<CacheConfig>,
    /// Chat Completions only: penalizes repeated tokens (-2.0 to 2.0).
    pub frequency_penalty: Option<f64>,
    /// Chat Completions only: encourages new topics (-2.0 to 2.0).
    pub presence_penalty: Option<f64>,
    /// Responses only: the server keeps the conversation (`store: true` and
    /// `previous_response_id` chaining). Construction-only; defaults to `false`.
    pub stateful: Option<bool>,
}

fn replace_if_set<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

impl OpenAIModelConfig {
    /// Shallow-merges `update` into `self`: every field `update` sets replaces the current value.
    pub fn merge(&mut self, update: OpenAIModelConfig) {
        let OpenAIModelConfig {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
            params,
            cache_config,
            frequency_penalty,
            presence_penalty,
            stateful,
        } = update;
        replace_if_set(&mut self.model_id, model_id);
        replace_if_set(&mut self.max_tokens, max_tokens);
        replace_if_set(&mut self.temperature, temperature);
        replace_if_set(&mut self.top_p, top_p);
        replace_if_set(&mut self.context_window_limit, context_window_limit);
        replace_if_set(&mut self.model_info, model_info);
        replace_if_set(&mut self.params, params);
        replace_if_set(&mut self.cache_config, cache_config);
        replace_if_set(&mut self.frequency_penalty, frequency_penalty);
        replace_if_set(&mut self.presence_penalty, presence_penalty);
        replace_if_set(&mut self.stateful, stateful);
    }

    fn to_base(&self) -> BaseModelConfig {
        BaseModelConfig {
            model_id: self.model_id.clone(),
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            top_p: self.top_p,
            context_window_limit: self.context_window_limit,
            model_info: self.model_info.clone(),
        }
    }

    fn from_base(base: BaseModelConfig) -> Self {
        let BaseModelConfig {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
        } = base;
        Self {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
            ..Default::default()
        }
    }

    fn model_id_or_default(&self) -> &str {
        self.model_id.as_deref().unwrap_or(MODEL_DEFAULTS.openai.model_id)
    }
}

/// An API key: a string, or an async function run before every request (TS `string | ApiKeySetter`).
#[derive(Clone)]
pub struct OpenAIApiKey(KeySource);

impl OpenAIApiKey {
    /// A fixed key.
    pub fn new(key: impl Into<String>) -> Self {
        Self(KeySource::Static(key.into()))
    }

    /// A function that returns the key for each request.
    pub fn from_setter(setter: ApiKeySetter) -> Self {
        Self(KeySource::Setter(setter))
    }
}

impl fmt::Debug for OpenAIApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl From<&str> for OpenAIApiKey {
    fn from(key: &str) -> Self {
        Self::new(key)
    }
}

impl From<String> for OpenAIApiKey {
    fn from(key: String) -> Self {
        Self::new(key)
    }
}

/// How the HTTP client is built (the TS `clientConfig`, a subset of the vendor SDK's `ClientOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct OpenAIClientConfig {
    /// The API base URL; defaults to `OPENAI_BASE_URL`, else `https://api.openai.com/v1`.
    pub base_url: Option<String>,
    /// The API key; wins over [`OpenAIModelOptions::api_key`].
    #[builder(into)]
    pub api_key: Option<OpenAIApiKey>,
    /// The `OpenAI-Organization` header; defaults to `OPENAI_ORG_ID`.
    pub organization: Option<String>,
    /// The `OpenAI-Project` header; defaults to `OPENAI_PROJECT_ID`.
    pub project: Option<String>,
    /// Headers sent with every request, over the defaults and `OPENAI_CUSTOM_HEADERS`.
    #[builder(default)]
    pub default_headers: HeaderMap,
    /// Retries for connection errors and the statuses 408, 409, 429 and 5xx; defaults to 2.
    pub max_retries: Option<u32>,
    /// How long to wait for the response headers; defaults to ten minutes.
    pub timeout: Option<Duration>,
}

impl fmt::Debug for OpenAIClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self
            .default_headers
            .keys()
            .map(http::HeaderName::as_str)
            .collect();
        f.debug_struct("OpenAIClientConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key)
            .field("organization", &self.organization)
            .field("project", &self.project)
            .field("default_headers", &header_names)
            .field("max_retries", &self.max_retries)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Everything [`OpenAIModel::new`] takes (TS `OpenAIModelOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
pub struct OpenAIModelOptions {
    /// The API; defaults to the Responses API.
    #[builder(default)]
    pub api: OpenAIApi,
    /// The API key; defaults to `OPENAI_API_KEY`.
    #[builder(into)]
    pub api_key: Option<OpenAIApiKey>,
    /// How the HTTP client is built.
    #[builder(default)]
    pub client_config: OpenAIClientConfig,
    /// The model configuration.
    #[builder(default)]
    pub config: OpenAIModelConfig,
    /// Routes requests through Amazon Bedrock's OpenAI-compatible Mantle endpoint, which
    /// derives the base URL and API key (TS `bedrockMantleConfig`).
    #[cfg(feature = "bedrock")]
    pub bedrock_mantle_config: Option<BedrockMantleConfig>,
}

impl fmt::Debug for OpenAIModelOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAIModelOptions")
            .field("api", &self.api)
            .field("api_key", &self.api_key)
            .field("client_config", &self.client_config)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl From<OpenAIModelConfig> for OpenAIModelOptions {
    fn from(config: OpenAIModelConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }
}

type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type WarnOnce = Arc<dyn Fn(&str) + Send + Sync>;
/// Builds the client from its parts; tests record the parts instead.
pub(crate) type ClientFactory = Arc<dyn Fn(HttpClientParts) -> Arc<dyn OpenAIClient> + Send + Sync>;

/// Where the OpenAI client comes from.
pub(crate) enum Transport {
    /// A client built from its parts by this factory (the `reqwest` client by default).
    Built(ClientFactory),
    /// A pre-built client (TS `client` option): no key check.
    #[cfg_attr(not(test), allow(dead_code, reason = "constructed by tests"))]
    Injected(Arc<dyn OpenAIClient>),
}

fn http_factory() -> ClientFactory {
    Arc::new(|parts| Arc::new(HttpClient::new(parts)) as Arc<dyn OpenAIClient>)
}

/// The process seams the provider reads through, so tests never touch env, time or the network.
pub(crate) struct Seams {
    pub(crate) env: EnvLookup,
    pub(crate) warn_once: WarnOnce,
    pub(crate) sleeper: Sleeper,
    pub(crate) random: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub(crate) transport: Transport,
    #[cfg(feature = "bedrock")]
    pub(crate) token_provider: mantle::TokenProviderFactory,
}

impl Default for Seams {
    fn default() -> Self {
        Self {
            env: Arc::new(|name| std::env::var(name).ok()),
            warn_once: Arc::new(warn_once),
            sleeper: tokio_sleeper(),
            random: Arc::new(fastrand::f64),
            transport: Transport::Built(http_factory()),
            #[cfg(feature = "bedrock")]
            token_provider: mantle::sigv4_token_provider_factory(),
        }
    }
}

/// A model served by the OpenAI Responses or Chat Completions API (TS `OpenAIModel`).
///
/// The config sits behind a lock, so [`update_config`](OpenAIModel::update_config) applies to
/// every holder of a shared model.
pub struct OpenAIModel {
    inner: Arc<Inner>,
}

struct Inner {
    api: OpenAIApi,
    config: RwLock<OpenAIModelConfig>,
    client: RwLock<Arc<dyn OpenAIClient>>,
    /// Set on the Mantle pathway, so a model swap across base paths can rebuild the client.
    mantle: Option<MantleRoute>,
    warn_once: WarnOnce,
}

/// What rebuilding the Mantle client needs: the region, the current base URL, and the client
/// parts with the construction-time key setter (it memoizes the token provider).
struct MantleRoute {
    region: String,
    template: HttpClientParts,
    base_url: Mutex<String>,
    factory: ClientFactory,
}

impl fmt::Debug for OpenAIModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAIModel")
            .field("api", &self.inner.api)
            .field("config", &self.config_snapshot())
            .finish_non_exhaustive()
    }
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_owned)
}

/// The headers of every request: organization and project, then `OPENAI_CUSTOM_HEADERS`, then
/// the configured headers.
fn client_headers(client_config: &OpenAIClientConfig, env: &EnvLookup) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let organization = non_empty(client_config.organization.as_deref()).or_else(|| trimmed(env(ORG_ID_ENV)));
    let project = non_empty(client_config.project.as_deref()).or_else(|| trimmed(env(PROJECT_ID_ENV)));
    for (name, value) in [("openai-organization", organization), ("openai-project", project)] {
        if let Some(value) = value.and_then(|value| HeaderValue::from_str(&value).ok()) {
            headers.insert(name, value);
        }
    }
    let custom = trimmed(env(CUSTOM_HEADERS_ENV))
        .map(|value| parse_custom_headers(&value))
        .unwrap_or_default();
    let configured = merge_headers(custom, &client_config.default_headers);
    merge_headers(headers, &configured)
}

/// The client parts for `base_url` and `key`, with the transport settings of `client_config`.
fn client_parts(
    base_url: String,
    key: KeySource,
    client_config: &OpenAIClientConfig,
    seams: &Seams,
) -> Result<HttpClientParts> {
    let http = reqwest::Client::builder().build().map_err(Error::other)?;
    Ok(HttpClientParts {
        http,
        base_url,
        key,
        headers: client_headers(client_config, &seams.env),
        max_retries: client_config.max_retries.unwrap_or(DEFAULT_MAX_RETRIES),
        timeout: client_config.timeout.unwrap_or(DEFAULT_TIMEOUT),
        sleeper: Arc::clone(&seams.sleeper),
        random: Arc::clone(&seams.random),
    })
}

/// The OpenAI client: the configured key (or `OPENAI_API_KEY`) and base URL (or `OPENAI_BASE_URL`).
fn resolve_client(
    api_key: Option<OpenAIApiKey>,
    client_config: &OpenAIClientConfig,
    seams: &Seams,
) -> Result<Arc<dyn OpenAIClient>> {
    let factory = match &seams.transport {
        Transport::Injected(client) => return Ok(Arc::clone(client)),
        Transport::Built(factory) => factory,
    };
    let env_key = trimmed((seams.env)(API_KEY_ENV));
    if api_key.is_none() && env_key.is_none() {
        return Err(Error::other(
            "OpenAI API key is required. Provide it via the 'api_key' option (string or function) or set the OPENAI_API_KEY environment variable.",
        ));
    }
    let key = client_config
        .api_key
        .clone()
        .or(api_key)
        .map_or_else(|| KeySource::Static(env_key.unwrap_or_default()), |key| key.0);
    let base_url = non_empty(client_config.base_url.as_deref())
        .or_else(|| trimmed((seams.env)(BASE_URL_ENV)))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
    Ok(factory(client_parts(base_url, key, client_config, seams)?))
}

#[cfg(feature = "bedrock")]
fn mantle_base_url(region: &str, model_id: &str) -> String {
    mantle::bedrock_mantle_base_url(region, model_id)
}

#[cfg(not(feature = "bedrock"))]
fn mantle_base_url(_region: &str, _model_id: &str) -> String {
    String::new()
}

/// The Mantle client parts: the region's endpoint for `model_id` and a minting key setter (TS
/// `buildMantleClient`).
#[cfg(feature = "bedrock")]
fn build_mantle_route(
    mantle_config: &BedrockMantleConfig,
    api_key: Option<&OpenAIApiKey>,
    client_config: &OpenAIClientConfig,
    model_id: &str,
    seams: &Seams,
) -> Result<MantleRoute> {
    let factory = match &seams.transport {
        Transport::Injected(_) => {
            return Err(Error::other(
                "'bedrock_mantle_config' cannot be combined with a pre-built 'client'.",
            ));
        }
        Transport::Built(factory) => Arc::clone(factory),
    };
    if api_key.is_some() {
        return Err(Error::other(
            "'api_key' cannot be combined with 'bedrock_mantle_config'; the API key is derived from the Mantle config automatically.",
        ));
    }
    let conflicting: Vec<&str> = [
        ("api_key", client_config.api_key.is_some()),
        ("base_url", client_config.base_url.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, set)| set.then_some(name))
    .collect();
    if !conflicting.is_empty() {
        return Err(Error::other(format!(
            "client_config must not contain {} when bedrock_mantle_config is set; these are derived from the Mantle config automatically.",
            conflicting.join(", ")
        )));
    }
    // Resolved eagerly, so a missing or malformed region fails before any token is minted.
    let region = mantle::resolve_mantle_region(mantle_config, seams.env.as_ref())?;
    let base_url = mantle::bedrock_mantle_base_url(&region, model_id);
    let setter = mantle::mantle_api_key_setter(mantle_config, &region, Arc::clone(&seams.token_provider));
    let template = client_parts(base_url.clone(), KeySource::Setter(setter), client_config, seams)?;
    Ok(MantleRoute {
        region,
        template,
        base_url: Mutex::new(base_url),
        factory,
    })
}

/// Maps throttling, a context overflow or a transport failure to the SDK's error types (TS `_rewrapError`).
fn translate_error(error: ApiError) -> Error {
    let message = error.message.clone();
    match classify_openai_error(error.status, error.code.as_deref(), &message) {
        Some(OpenAIErrorKind::Throttling) => {
            tracing::debug!("throttled | error_message=<{message}>");
            ModelThrottledError::new(message).with_source(error).into()
        }
        Some(OpenAIErrorKind::ContextOverflow) => ContextWindowOverflowError::new(message).into(),
        None => match error.kind {
            ApiErrorKind::Connection | ApiErrorKind::Timeout => {
                ModelNetworkError::new(message).with_source(error).into()
            }
            ApiErrorKind::Api => ModelError::new(message).with_source(error).into(),
            ApiErrorKind::Other => Error::other(error),
        },
    }
}

/// The next stream item, or `None` once `cancel` fires.
async fn next_or_cancel(
    events: &mut EventStream,
    cancel: Option<&CancelSignal>,
) -> Option<Option<std::result::Result<JsonValue, ApiError>>> {
    let Some(cancel) = cancel else {
        return Some(events.next().await);
    };
    tokio::select! {
        biased;
        () = cancel.cancelled() => None,
        item = events.next() => Some(item),
    }
}

/// The adapter of the stream in flight.
enum StreamState {
    Responses(ResponsesStream),
    Chat(ChatStream),
}

impl StreamState {
    fn on_event(
        &mut self,
        event: &JsonValue,
        request: &ModelRequest,
    ) -> std::result::Result<Vec<ModelStreamEvent>, ApiError> {
        match self {
            Self::Responses(state) => state.on_event(event, &request.model_state),
            Self::Chat(state) => Ok(state.on_chunk(event)),
        }
    }

    fn finish(self) -> Option<Vec<ModelStreamEvent>> {
        match self {
            Self::Responses(state) => state.finish(),
            Self::Chat(state) => Some(state.finish()),
        }
    }
}

impl OpenAIModel {
    /// Builds a model over the OpenAI API (TS `new OpenAIModel(options)`).
    ///
    /// # Errors
    ///
    /// Fails when no API key is set in the options or `OPENAI_API_KEY`, or when the HTTP client's
    /// TLS backend can't start.
    pub fn new(options: impl Into<OpenAIModelOptions>) -> Result<Self> {
        Self::with_seams(options.into(), Seams::default())
    }

    pub(crate) fn with_seams(options: OpenAIModelOptions, seams: Seams) -> Result<Self> {
        let OpenAIModelOptions {
            api,
            api_key,
            client_config,
            config,
            #[cfg(feature = "bedrock")]
            bedrock_mantle_config,
        } = options;
        if config.model_id.is_none() {
            (seams.warn_once)(&default_model_warning_message(MODEL_DEFAULTS.openai.model_id));
        }
        warn_api_managed_params(api, config.params.as_ref());
        #[cfg(feature = "bedrock")]
        let (client, mantle) = match bedrock_mantle_config {
            Some(mantle_config) => {
                let route = build_mantle_route(
                    &mantle_config,
                    api_key.as_ref(),
                    &client_config,
                    config.model_id_or_default(),
                    &seams,
                )?;
                let client = (route.factory)(route.template.clone());
                (client, Some(route))
            }
            None => (resolve_client(api_key, &client_config, &seams)?, None),
        };
        #[cfg(not(feature = "bedrock"))]
        let (client, mantle) = (resolve_client(api_key, &client_config, &seams)?, None);
        Ok(Self {
            inner: Arc::new(Inner {
                api,
                config: RwLock::new(config),
                client: RwLock::new(client),
                mantle,
                warn_once: seams.warn_once,
            }),
        })
    }

    /// The API this model speaks; fixed at construction (TS `api`).
    pub fn api(&self) -> OpenAIApi {
        self.inner.api
    }

    fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    fn config_snapshot(&self) -> OpenAIModelConfig {
        self.inner
            .config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn client(&self) -> Arc<dyn OpenAIClient> {
        Arc::clone(&self.inner.client.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// The config, with the context window resolved for the model when unset (TS `getConfig`).
    pub fn config(&self) -> OpenAIModelConfig {
        let mut config = self.config_snapshot();
        let mut base = config.to_base();
        resolve_config_metadata(&mut base, config.model_id_or_default());
        config.context_window_limit = base.context_window_limit;
        config
    }

    /// Shallow-merges `config` into the model's config (TS `updateConfig`). `stateful` is
    /// construction-only: a value here is dropped with a warning.
    pub fn update_config(&self, mut config: OpenAIModelConfig) {
        if let Some(stateful) = config.stateful.take() {
            tracing::warn!(
                "stateful=<{stateful}> | 'stateful' is construction-only and cannot be changed via updateConfig — ignoring"
            );
        }
        if let (Some(route), Some(model_id)) = (&self.inner.mantle, config.model_id.as_deref()) {
            self.reroute(route, model_id);
        }
        warn_api_managed_params(self.inner.api, config.params.as_ref());
        self.inner
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .merge(config);
    }

    /// Rebuilds the Mantle client when `model_id` lives on the other base path.
    fn reroute(&self, route: &MantleRoute, model_id: &str) {
        let next = mantle_base_url(&route.region, model_id);
        let mut base_url = route.base_url.lock().unwrap_or_else(PoisonError::into_inner);
        if *base_url == next {
            return;
        }
        tracing::debug!(
            "model_id=<{model_id}> | rerouting the Bedrock Mantle client from <{base_url}> to <{next}>"
        );
        let client = (route.factory)(HttpClientParts {
            base_url: next.clone(),
            ..route.template.clone()
        });
        *self.inner.client.write().unwrap_or_else(PoisonError::into_inner) = client;
        *base_url = next;
    }

    fn stream_events(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        let model = self.handle();
        Box::pin(async_stream::stream! {
            if request.messages.is_empty() {
                yield Err(Error::other("At least one message is required"));
                return;
            }
            let (api_request, mut state) = match model.prepare(&request) {
                Ok(prepared) => prepared,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            let mut events = model.client().stream(api_request);
            let cancel = request.cancel_signal.clone();
            loop {
                let item = match next_or_cancel(&mut events, cancel.as_ref()).await {
                    None => Some(Err(ApiError::aborted())),
                    Some(item) => item,
                };
                let mapped = match item {
                    None => break,
                    Some(Ok(event)) => state.on_event(&event, &request),
                    Some(Err(error)) => Err(error),
                };
                match mapped {
                    Ok(mapped) => {
                        for event in mapped {
                            yield Ok(event);
                        }
                    }
                    Err(error) => {
                        yield Err(translate_error(error));
                        return;
                    }
                }
            }
            if cancel.as_ref().is_some_and(CancelSignal::is_cancelled) {
                yield Err(translate_error(ApiError::aborted()));
                return;
            }
            let Some(events) = state.finish() else {
                yield Err(ModelError::new("Stream ended without completing a message").into());
                return;
            };
            for event in events {
                yield Ok(event);
            }
        })
    }

    /// The request for `request` and the adapter that maps its events.
    fn prepare(&self, request: &ModelRequest) -> Result<(ApiRequest, StreamState)> {
        let config = self.config_snapshot();
        let warn_once = self.inner.warn_once.as_ref();
        if self.inner.api == OpenAIApi::Chat {
            let body = format_chat_request(&config, request, warn_once)?;
            let api_request = ApiRequest {
                path: CHAT_COMPLETIONS_PATH,
                body: JsonValue::Object(body),
            };
            return Ok((api_request, StreamState::Chat(ChatStream::default())));
        }
        let stateful = self.stateful();
        let body = format_responses_request(&config, request, stateful, &self.model_info(), warn_once);
        let api_request = ApiRequest {
            path: RESPONSES_PATH,
            body: JsonValue::Object(body),
        };
        Ok((
            api_request,
            StreamState::Responses(ResponsesStream::new(stateful)),
        ))
    }
}

fn warn_api_managed_params(api: OpenAIApi, params: Option<&JsonObject>) {
    match api {
        OpenAIApi::Responses => warn_managed_params(params, &responses::MANAGED_PARAMS),
        OpenAIApi::Chat => warn_managed_params(params, &chat::MANAGED_PARAMS),
    }
}

#[async_trait]
impl Model for OpenAIModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        self.stream_events(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.config().to_base()
    }

    fn update_config(&self, config: BaseModelConfig) {
        OpenAIModel::update_config(self, OpenAIModelConfig::from_base(config));
    }

    /// The configured model id, else the default one that gets sent.
    fn model_id(&self) -> Option<String> {
        Some(self.config_snapshot().model_id_or_default().to_owned())
    }

    /// Adds whether the server keeps the conversation: only the Responses API with `stateful`
    /// on. Turns server-side summary off.
    fn model_info(&self) -> ModelInfo {
        let config = self.config_snapshot();
        let stateful = self.inner.api == OpenAIApi::Responses && config.stateful == Some(true);
        let facts = ModelInfo {
            capabilities: Some(ModelCapabilities {
                stateful: Some(stateful),
                ..Default::default()
            }),
            limits: None,
        };
        merge_model_info([
            Some(&base_model_info(self)),
            Some(&SERVER_SIDE_SUMMARY_OFF),
            Some(&facts),
            config.model_info.as_ref(),
        ])
    }
}

#[cfg(test)]
mod tests;
