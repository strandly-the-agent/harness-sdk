//! The Anthropic Messages API provider (TS `models/anthropic.ts`), behind the `anthropic` feature.
//!
//! TS wraps `@anthropic-ai/sdk`; this module speaks the Messages API over `reqwest` and
//! reproduces that SDK's transport: its environment defaults (`ANTHROPIC_API_KEY`,
//! `ANTHROPIC_BASE_URL`, `ANTHROPIC_CUSTOM_HEADERS`), the `anthropic-version` it sends, two
//! retries with backoff for connection errors and retryable statuses, and a ten-minute timeout.
//!
//! ```no_run
//! use strands::models::anthropic::{AnthropicModel, AnthropicModelConfig};
//!
//! # fn main() -> strands::Result<()> {
//! // The API key comes from `ANTHROPIC_API_KEY`.
//! let model = AnthropicModel::new(AnthropicModelConfig {
//!     model_id: Some("claude-sonnet-4-6".into()),
//!     max_tokens: Some(4_096),
//!     ..Default::default()
//! })?;
//! let agent = strands::Agent::builder().model(model).build()?;
//! # drop(agent);
//! # Ok(())
//! # }
//! ```

mod client;
mod format;
mod stream;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use http::HeaderMap;

use self::client::{
    ApiError, ApiErrorKind, ApiRequest, DEFAULT_API_VERSION, DEFAULT_BASE_URL, DEFAULT_MAX_RETRIES,
    DEFAULT_TIMEOUT, HttpClient, HttpClientParts, MessagesClient, header_values, merge_headers,
    parse_custom_headers,
};
use self::format::{
    ApiVersion, FormatOptions, Formatter, THINKING_BINDING_BETA, TURN_SCOPED_SYSTEM_BETA,
    from_last_compaction, is_compaction, is_thinking_binding_error, thinking_with_drop_block,
    validate_anthropic_tools,
};
use self::stream::{ResponseState, UsageTotals, format_compaction_usage};
use crate::error::{
    ContextWindowOverflowError, Error, ModelError, ModelNetworkError, ModelThrottledError, Result,
};
use crate::logging::warn_once::warn_once;
use crate::models::defaults::{
    MODEL_DEFAULTS, default_max_tokens_warning_message, default_model_warning_message,
};
use crate::models::model::{
    BaseModelConfig, BoxStream, CacheConfig, CountTokensOptions, Model, ModelRequest, base_model_info,
    estimate_tokens_heuristic, resolve_config_metadata, resolve_max_tokens,
};
use crate::models::model_info::{ModelCapabilities, ModelInfo, lookup_model_info, merge_model_info};
use crate::models::native_output::{
    StructuredOutputMechanism, StructuredOutputSupport, to_native_output_schema,
};
use crate::models::streaming::{ModelStreamEvent, Usage};
use crate::models::summarize::{SummarizeOptions, SummarizeResult, summarize_with_stream};
use crate::retry::model_retry_strategy::{Sleeper, tokio_sleeper};
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{ContentBlock, Message, StopReason, SummaryBlock};
use crate::types::tools::ToolChoice;

/// Error messages (lowercase) that mean the request overflowed the context window.
const CONTEXT_WINDOW_OVERFLOW_ERRORS: [&str; 6] = [
    "prompt is too long",
    "max_tokens exceeded",
    "input too long",
    "input is too long",
    "input length exceeds context window",
    "input and output tokens exceed your context limit",
];
/// Anthropic pauses a long server-side tool turn with `stop_reason=pause_turn` and expects the
/// paused assistant message back as-is to resume it; `stream` does so at most this many times.
const MAX_PAUSE_TURN_CONTINUATIONS: usize = 10;
/// Beta that enables on-demand compaction, and compaction blocks in later requests.
const COMPACTION_BETA: &str = "compact-2026-09-04";
const PROVIDER: &str = "anthropic";
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
const CUSTOM_HEADERS_ENV: &str = "ANTHROPIC_CUSTOM_HEADERS";

/// Configuration of an [`AnthropicModel`] (TS `AnthropicModelConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
#[builder(on(String, into))]
pub struct AnthropicModelConfig {
    /// The model id; defaults to `claude-sonnet-4-6`, which may change between versions.
    pub model_id: Option<String>,
    /// The largest number of tokens to generate; defaults to the model's output limit from the
    /// model info, else 64,000, which may change between versions.
    pub max_tokens: Option<u64>,
    /// Sampling temperature. API version 2026-10-21 (the default) rejects it.
    pub temperature: Option<f64>,
    /// Nucleus sampling probability. API version 2026-10-21 (the default) rejects it.
    pub top_p: Option<f64>,
    /// The context window in tokens; overrides the model-info table.
    pub context_window_limit: Option<u64>,
    /// Model facts merged over the shared model-info table.
    pub model_info: Option<ModelInfo>,
    /// Sequences that stop generation.
    pub stop_sequences: Option<Vec<String>>,
    /// Extra request fields (such as `thinking` or `output_config`), merged into the request last.
    pub params: Option<JsonObject>,
    /// Anthropic server-side tools (such as `web_search`, `web_fetch` and `code_execution`), sent
    /// after the agent's function tools. Function tool definitions are rejected.
    pub anthropic_tools: Option<Vec<JsonValue>>,
    /// Beta features to enable through the `anthropic-beta` header; none by default.
    pub betas: Option<Vec<String>>,
    /// Count tokens with the Anthropic token counting API instead of the heuristic estimate.
    pub use_native_token_count: Option<bool>,
    /// Prompt caching. Setting it caches the tool definitions and adds a cache point to the last
    /// user message; caching is off when unset. Every active Claude model caches, so
    /// `strategy` has no effect beyond honouring a model-info override that says caching is off.
    pub cache_config: Option<CacheConfig>,
}

fn replace_if_set<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

impl AnthropicModelConfig {
    /// Shallow-merges `update` into `self`: every field `update` sets replaces the current value.
    pub fn merge(&mut self, update: AnthropicModelConfig) {
        let AnthropicModelConfig {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
            stop_sequences,
            params,
            anthropic_tools,
            betas,
            use_native_token_count,
            cache_config,
        } = update;
        replace_if_set(&mut self.model_id, model_id);
        replace_if_set(&mut self.max_tokens, max_tokens);
        replace_if_set(&mut self.temperature, temperature);
        replace_if_set(&mut self.top_p, top_p);
        replace_if_set(&mut self.context_window_limit, context_window_limit);
        replace_if_set(&mut self.model_info, model_info);
        replace_if_set(&mut self.stop_sequences, stop_sequences);
        replace_if_set(&mut self.params, params);
        replace_if_set(&mut self.anthropic_tools, anthropic_tools);
        replace_if_set(&mut self.betas, betas);
        replace_if_set(&mut self.use_native_token_count, use_native_token_count);
        replace_if_set(&mut self.cache_config, cache_config);
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

    fn merge_base(&mut self, update: BaseModelConfig) {
        let BaseModelConfig {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
        } = update;
        self.merge(AnthropicModelConfig {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
            ..Default::default()
        });
    }

    fn model_id_or_default(&self) -> &str {
        self.model_id
            .as_deref()
            .unwrap_or(MODEL_DEFAULTS.anthropic.model_id)
    }

    /// The `max_tokens` to send: the configured value, else the model's output limit, else the default.
    fn resolved_max_tokens(&self) -> u64 {
        resolve_max_tokens(&self.to_base(), self.model_id_or_default())
            .or(MODEL_DEFAULTS.anthropic.max_tokens)
            .unwrap_or_default()
    }
}

/// How the HTTP client is built (the TS `clientConfig`, a subset of the vendor SDK's `ClientOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, PartialEq, bon::Builder)]
#[builder(on(String, into))]
pub struct AnthropicClientConfig {
    /// The API base URL; defaults to `ANTHROPIC_BASE_URL`, else `https://api.anthropic.com`.
    pub base_url: Option<String>,
    /// Headers sent with every request, over the defaults and `ANTHROPIC_CUSTOM_HEADERS`. An
    /// `anthropic-version` here pins the API version; `anthropic-beta` values carry over to
    /// requests that need extra betas.
    #[builder(default)]
    pub default_headers: HeaderMap,
    /// Retries for connection errors and the statuses 408, 409, 429 and 5xx; defaults to 2.
    pub max_retries: Option<u32>,
    /// How long to wait for the response headers; defaults to ten minutes.
    pub timeout: Option<Duration>,
}

impl fmt::Debug for AnthropicClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self
            .default_headers
            .keys()
            .map(http::HeaderName::as_str)
            .collect();
        f.debug_struct("AnthropicClientConfig")
            .field("base_url", &self.base_url)
            .field("default_headers", &header_names)
            .field("max_retries", &self.max_retries)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Everything [`AnthropicModel::new`] takes (TS `AnthropicModelOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct AnthropicModelOptions {
    /// The API key; defaults to `ANTHROPIC_API_KEY`.
    pub api_key: Option<String>,
    /// How the HTTP client is built.
    #[builder(default)]
    pub client_config: AnthropicClientConfig,
    /// The model configuration.
    #[builder(default)]
    pub config: AnthropicModelConfig,
}

impl fmt::Debug for AnthropicModelOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicModelOptions")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("client_config", &self.client_config)
            .field("config", &self.config)
            .finish()
    }
}

impl From<AnthropicModelConfig> for AnthropicModelOptions {
    fn from(config: AnthropicModelConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }
}

type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type WarnOnce = Arc<dyn Fn(&str) + Send + Sync>;

/// Where the Messages API client comes from.
pub(crate) enum Transport {
    /// The `reqwest` client.
    Http,
    /// A client resolved like the `reqwest` one (key check, API version) but replaced, for tests.
    #[cfg_attr(not(test), allow(dead_code, reason = "constructed by tests"))]
    Built(Arc<dyn MessagesClient>),
    /// A pre-built client (TS `client` option): no key check and an unknown API version.
    #[cfg_attr(not(test), allow(dead_code, reason = "constructed by tests"))]
    Injected(Arc<dyn MessagesClient>),
}

/// The process seams the provider reads through, so tests never touch env, time or the network.
pub(crate) struct Seams {
    pub(crate) env: EnvLookup,
    pub(crate) warn_once: WarnOnce,
    pub(crate) sleeper: Sleeper,
    pub(crate) random: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub(crate) transport: Transport,
}

impl Default for Seams {
    fn default() -> Self {
        Self {
            env: Arc::new(|name| std::env::var(name).ok()),
            warn_once: Arc::new(warn_once),
            sleeper: tokio_sleeper(),
            random: Arc::new(fastrand::f64),
            transport: Transport::Http,
        }
    }
}

/// A model served by the Anthropic Messages API (TS `AnthropicModel`).
///
/// The config sits behind a lock, so [`update_config`](AnthropicModel::update_config) applies to
/// every holder of a shared model.
pub struct AnthropicModel {
    inner: Arc<Inner>,
}

struct Inner {
    config: RwLock<AnthropicModelConfig>,
    client: Arc<dyn MessagesClient>,
    client_betas: Vec<String>,
    api_version: Option<ApiVersion>,
    drop_stale_thinking_model_id: Mutex<Option<String>>,
    structured_output: StructuredOutputSupport,
    warn_once: WarnOnce,
}

impl fmt::Debug for AnthropicModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicModel")
            .field("config", &self.config_snapshot())
            .field(
                "api_version",
                &self.inner.api_version.as_ref().map(|version| &version.value),
            )
            .finish_non_exhaustive()
    }
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// The `anthropic-beta` values of `headers`, split on commas.
fn header_betas(headers: &HeaderMap) -> Vec<String> {
    header_values(headers, "anthropic-beta")
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|beta| !beta.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The `anthropic-version` the client sends, and where it comes from for error messages.
fn resolve_api_version(sent: &HeaderMap, configured: &HeaderMap) -> ApiVersion {
    let pinned = header_values(sent, "anthropic-version").last();
    let Some(pinned) = pinned else {
        return ApiVersion {
            value: DEFAULT_API_VERSION.to_owned(),
            origin: "the AnthropicModel default".to_owned(),
        };
    };
    let origin = if header_values(configured, "anthropic-version").last() == Some(pinned) {
        "pinned by client_config.default_headers['anthropic-version']"
    } else {
        "pinned by the ANTHROPIC_CUSTOM_HEADERS environment variable"
    };
    ApiVersion {
        value: pinned.to_owned(),
        origin: origin.to_owned(),
    }
}

/// The Messages client and the API version it sends; an injected client's version is unknown.
fn resolve_client(
    api_key: Option<String>,
    client_config: AnthropicClientConfig,
    seams: Seams,
) -> Result<(Arc<dyn MessagesClient>, Option<ApiVersion>)> {
    let replacement = match seams.transport {
        Transport::Injected(client) => return Ok((client, None)),
        Transport::Built(client) => Some(client),
        Transport::Http => None,
    };
    let api_key = trimmed(api_key)
        .or_else(|| trimmed((seams.env)(API_KEY_ENV)))
        .ok_or_else(|| {
            Error::other(
                "Anthropic API key is required. Provide it via the 'api_key' option or set the ANTHROPIC_API_KEY environment variable.",
            )
        })?;
    let custom = trimmed((seams.env)(CUSTOM_HEADERS_ENV))
        .map(|value| parse_custom_headers(&value))
        .unwrap_or_default();
    let headers = merge_headers(custom, &client_config.default_headers);
    let api_version = resolve_api_version(&headers, &client_config.default_headers);
    if let Some(client) = replacement {
        return Ok((client, Some(api_version)));
    }
    let base_url = trimmed(client_config.base_url)
        .or_else(|| trimmed((seams.env)(BASE_URL_ENV)))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
    let client = HttpClient::new(HttpClientParts {
        base_url,
        api_key,
        headers,
        max_retries: client_config.max_retries.unwrap_or(DEFAULT_MAX_RETRIES),
        timeout: client_config.timeout.unwrap_or(DEFAULT_TIMEOUT),
        sleeper: seams.sleeper,
        random: seams.random,
    })
    .map_err(Error::other)?;
    Ok((Arc::new(client), Some(api_version)))
}

/// Maps a context overflow, a 429 or a transport failure to the SDK's error types.
fn translate_error(error: ApiError) -> Error {
    let message = error.message.clone();
    let lowered = message.to_lowercase();
    if CONTEXT_WINDOW_OVERFLOW_ERRORS
        .iter()
        .any(|pattern| lowered.contains(pattern))
    {
        return ContextWindowOverflowError::new(message).into();
    }
    if error.status() == Some(429) {
        tracing::debug!("throttled | error_message=<{message}>");
        return ModelThrottledError::new(message).with_source(error).into();
    }
    match error.kind {
        ApiErrorKind::Connection | ApiErrorKind::Timeout => {
            ModelNetworkError::new(message).with_source(error).into()
        }
        ApiErrorKind::Other => Error::other(error),
        ApiErrorKind::Api => ModelError::new(message).with_source(error).into(),
    }
}

/// The combined usage of two calls, or whichever one reported usage.
fn sum_usage(first: Option<&Usage>, second: Option<&Usage>) -> Option<Usage> {
    match (first, second) {
        (Some(first), Some(second)) => {
            let mut total = *first;
            total.accumulate(second);
            Some(total)
        }
        (first, second) => first.or(second).copied(),
    }
}

/// A request formatted for one attempt of `stream`.
struct Prepared {
    body: JsonObject,
    beta: Option<String>,
}

impl AnthropicModel {
    /// Builds a model over the Messages API (TS `new AnthropicModel(options)`).
    ///
    /// # Errors
    ///
    /// Fails when `anthropic_tools` holds a function tool definition, when no API key is set
    /// in the options or `ANTHROPIC_API_KEY`, or when the HTTP client's TLS backend can't start.
    pub fn new(options: impl Into<AnthropicModelOptions>) -> Result<Self> {
        Self::with_seams(options.into(), Seams::default())
    }

    pub(crate) fn with_seams(options: AnthropicModelOptions, seams: Seams) -> Result<Self> {
        let AnthropicModelOptions {
            api_key,
            client_config,
            mut config,
        } = options;
        let model_id_given = config.model_id.is_some();
        config
            .model_id
            .get_or_insert_with(|| MODEL_DEFAULTS.anthropic.model_id.to_owned());
        if let Some(tools) = &config.anthropic_tools {
            validate_anthropic_tools(tools)?;
        }
        if !model_id_given {
            (seams.warn_once)(&default_model_warning_message(MODEL_DEFAULTS.anthropic.model_id));
        }
        let max_output_pinned = config
            .model_info
            .as_ref()
            .and_then(|info| info.limits.as_ref())
            .and_then(|limits| limits.max_output_tokens)
            .is_some();
        if config.max_tokens.is_none() && !max_output_pinned {
            (seams.warn_once)(&default_max_tokens_warning_message(config.resolved_max_tokens()));
        }
        let client_betas = header_betas(&client_config.default_headers);
        let warn_once = Arc::clone(&seams.warn_once);
        let (client, api_version) = resolve_client(api_key, client_config, seams)?;
        Ok(Self {
            inner: Arc::new(Inner {
                config: RwLock::new(config),
                client,
                client_betas,
                api_version,
                drop_stale_thinking_model_id: Mutex::new(None),
                structured_output: StructuredOutputSupport::default(),
                warn_once,
            }),
        })
    }

    fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    fn config_snapshot(&self) -> AnthropicModelConfig {
        self.inner
            .config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The config, with the context window and max tokens resolved for the model when unset (TS `getConfig`).
    pub fn config(&self) -> AnthropicModelConfig {
        let mut config = self.config_snapshot();
        let mut base = config.to_base();
        resolve_config_metadata(&mut base, config.model_id_or_default());
        config.context_window_limit = base.context_window_limit;
        if config.max_tokens.is_none() {
            config.max_tokens = Some(config.resolved_max_tokens());
        }
        config
    }

    /// Shallow-merges `config` into the model's config (TS `updateConfig`).
    ///
    /// # Errors
    ///
    /// Fails, leaving the config unchanged, when `anthropic_tools` holds a function tool definition.
    pub fn update_config(&self, config: AnthropicModelConfig) -> Result<()> {
        if let Some(tools) = &config.anthropic_tools {
            validate_anthropic_tools(tools)?;
        }
        self.inner
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .merge(config);
        Ok(())
    }

    fn server_side_summary(&self) -> bool {
        self.model_info()
            .capabilities
            .and_then(|capabilities| capabilities.server_side_summary)
            == Some(true)
    }

    /// Whether a binding error already turned `drop_block` on for the configured model.
    fn drops_stale_thinking(&self, config: &AnthropicModelConfig) -> bool {
        let dropped = self
            .inner
            .drop_stale_thinking_model_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        dropped.is_some() && *dropped == config.model_id
    }

    fn format_request(
        &self,
        config: &AnthropicModelConfig,
        messages: &[Message],
        options: &FormatOptions<'_>,
    ) -> Result<JsonObject> {
        let model_id = config
            .model_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::other("Model ID is required"))?;
        let model_info = self.model_info();
        Formatter {
            config,
            model_id,
            max_tokens: config.resolved_max_tokens(),
            model_info: &model_info,
            api_version: self.inner.api_version.as_ref(),
            drops_stale_thinking: self.drops_stale_thinking(config),
            warn_once: self.inner.warn_once.as_ref(),
        }
        .format_request(messages, options)
    }

    /// The `anthropic-beta` header for `request`, or `None` when it needs no betas.
    fn request_betas(
        &self,
        config: &AnthropicModelConfig,
        request: &JsonObject,
        extra: &[&str],
    ) -> Option<String> {
        let sends_turn_scoped = request
            .get("messages")
            .and_then(JsonValue::as_array)
            .is_some_and(|messages| messages.iter().any(|message| message.get("clear_at").is_some()));
        let mut required: Vec<&str> = extra.to_vec();
        if self.drops_stale_thinking(config) {
            required.push(THINKING_BINDING_BETA);
        }
        if sends_turn_scoped {
            required.push(TURN_SCOPED_SYSTEM_BETA);
        }
        let mut betas: Vec<&str> = config.betas.iter().flatten().map(String::as_str).collect();
        // Request headers replace client headers of the same name, so client betas carry over.
        if !required.is_empty() && betas.is_empty() {
            betas = self.inner.client_betas.iter().map(String::as_str).collect();
        }
        let mut unique: Vec<&str> = Vec::new();
        for beta in betas.into_iter().chain(required) {
            if !unique.contains(&beta) {
                unique.push(beta);
            }
        }
        (!unique.is_empty()).then(|| unique.join(","))
    }

    /// The compaction beta when `messages` carry a compaction block this model sends as one.
    fn compaction_betas(&self, messages: &[Message]) -> &'static [&'static str] {
        let carries = messages
            .iter()
            .any(|message| message.content().iter().any(is_compaction));
        if carries && self.server_side_summary() {
            &[COMPACTION_BETA]
        } else {
            &[]
        }
    }

    /// The strongest structured output mechanism not yet rejected for this model.
    fn structured_output_mechanism(&self, request: &ModelRequest) -> Option<StructuredOutputMechanism> {
        let schema = request.structured_output_schema.as_ref()?;
        let model_id = self.config_snapshot().model_id?;
        self.inner
            .structured_output
            .mechanisms(
                PROVIDER,
                &model_id,
                request.tool_choice.as_ref(),
                schema,
                &self.model_info(),
            )
            .first()
            .copied()
    }

    fn prepare(
        &self,
        request: &ModelRequest,
        mechanism: Option<StructuredOutputMechanism>,
    ) -> Result<Prepared> {
        let config = self.config_snapshot();
        let forces_auto = matches!(
            mechanism,
            Some(StructuredOutputMechanism::Native | StructuredOutputMechanism::Auto)
        );
        let tool_choice = if forces_auto {
            Some(&ToolChoice::Auto)
        } else {
            request.tool_choice.as_ref()
        };
        let options = FormatOptions {
            system_prompt: request.system_prompt.as_ref(),
            tool_specs: &request.tool_specs,
            tool_choice,
            dynamic_trailing_blocks: request.dynamic_trailing_blocks.unwrap_or(0),
        };
        let mut body = self.format_request(&config, &request.messages, &options)?;
        if let (Some(StructuredOutputMechanism::Native), Some(schema)) =
            (mechanism, request.structured_output_schema.as_ref())
        {
            with_native_output(&mut body, &to_native_output_schema(schema)?);
        }
        let beta = self.request_betas(&config, &body, self.compaction_betas(&request.messages));
        Ok(Prepared { body, beta })
    }

    /// Whether a failed first request is resent: with `drop_block` after a thinking binding
    /// error, or with the next structured output mechanism after a rejection.
    fn recover(
        &self,
        error: &ApiError,
        thinking: Option<&JsonValue>,
        mechanism: Option<StructuredOutputMechanism>,
        request: &ModelRequest,
    ) -> bool {
        let config = self.config_snapshot();
        let bad_request = error.status() == Some(400);
        if bad_request
            && is_thinking_binding_error(&error.message)
            && thinking_with_drop_block(thinking).is_some()
        {
            // Stale blocks stay in history, so drop_block stays on for every later request to this model.
            let model_id = config.model_id.clone().unwrap_or_default();
            tracing::warn!(
                "model_id=<{model_id}> | history edit invalidated replayed thinking blocks, retrying with drop_block"
            );
            *self
                .inner
                .drop_stale_thinking_model_id
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = config.model_id;
            return true;
        }
        let (Some(mechanism), Some(schema), Some(model_id)) = (
            mechanism,
            request.structured_output_schema.as_ref(),
            config.model_id.as_deref(),
        ) else {
            return false;
        };
        bad_request
            && self.inner.structured_output.handle_rejected_mechanism(
                PROVIDER,
                model_id,
                mechanism,
                schema,
                error.api_message(),
            )
    }

    fn stream_events(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        let model = self.handle();
        Box::pin(async_stream::stream! {
            'attempts: loop {
                let mechanism = model.structured_output_mechanism(&request);
                let Prepared { mut body, beta } = match model.prepare(&request, mechanism) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                };
                let thinking = body.get("thinking").cloned();
                let mut totals = UsageTotals::default();
                let mut continuations = 0usize;
                loop {
                    let api_request = ApiRequest { body: JsonValue::Object(body.clone()), beta: beta.clone() };
                    let mut events = model.inner.client.stream(api_request);
                    let mut response = ResponseState::new(continuations == 0);
                    let mut failure = None;
                    while let Some(item) = events.next().await {
                        match item {
                            Ok(event) => {
                                for mapped in response.on_event(&event) {
                                    yield Ok(mapped);
                                }
                            }
                            Err(error) => {
                                failure = Some(error);
                                break;
                            }
                        }
                    }
                    if let Some(error) = failure {
                        if continuations == 0 && model.recover(&error, thinking.as_ref(), mechanism, &request) {
                            continue 'attempts;
                        }
                        yield Err(translate_error(error));
                        return;
                    }
                    // A stream that ends without message_stop is incomplete; no stop event, so the caller can tell.
                    if !response.message_stopped {
                        return;
                    }
                    response.add_usage_to(&mut totals);
                    if response.stop_reason == StopReason::PauseTurn {
                        if continuations >= MAX_PAUSE_TURN_CONTINUATIONS {
                            yield Err(ModelError::new(format!(
                                "server-side tool turn did not complete after {continuations} continuations"
                            ))
                            .into());
                            return;
                        }
                        continuations = continuations.saturating_add(1);
                        let paused = serde_json::json!({ "role": "assistant", "content": response.snapshot.content() });
                        if let Some(JsonValue::Array(messages)) = body.get_mut("messages") {
                            messages.push(paused);
                        }
                        tracing::debug!("continuation=<{continuations}> | resuming paused server-side tool turn");
                        continue;
                    }
                    for closing in response.closing_events(&totals) {
                        yield Ok(closing);
                    }
                    return;
                }
            }
        })
    }

    /// Counts the request's tokens with the token counting API.
    async fn native_count_tokens(
        &self,
        messages: &[Message],
        options: &CountTokensOptions,
    ) -> Result<u64, String> {
        let config = self.config_snapshot();
        let format_options = FormatOptions {
            system_prompt: options.system_prompt.as_ref(),
            tool_specs: &options.tool_specs,
            ..Default::default()
        };
        let request = self
            .format_request(&config, messages, &format_options)
            .map_err(|error| error.to_string())?;
        let mut params = JsonObject::new();
        for key in ["model", "messages", "system", "tools", "tool_choice"] {
            if let Some(value) = request.get(key) {
                params.insert(key.to_owned(), value.clone());
            }
        }
        let beta = self.request_betas(&config, &request, self.compaction_betas(messages));
        let response = self
            .inner
            .client
            .count_tokens(ApiRequest {
                body: JsonValue::Object(params),
                beta,
            })
            .await
            .map_err(|error| error.to_string())?;
        let tokens = response
            .get("input_tokens")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| "the response has no input_tokens".to_owned())?;
        tracing::debug!("total_tokens=<{tokens}> | native token count");
        Ok(tokens)
    }

    /// Sends `messages` as a compaction request; the result is empty when the response holds no
    /// compaction or the request failed with an API error.
    async fn compact(&self, messages: &[Message], options: &SummarizeOptions) -> Result<SummarizeResult> {
        let config = self.config_snapshot();
        let model_id = config.model_id.clone().unwrap_or_default();
        let format_options = FormatOptions {
            system_prompt: options.system_prompt.as_ref(),
            tool_specs: &options.tool_specs,
            ..Default::default()
        };
        let mut body = self.format_request(&config, messages, &format_options)?;
        // The API rejects these on a compaction request.
        body.remove("stop_sequences");
        body.remove("context_management");
        let mut compaction = serde_json::json!({ "type": "summarize" });
        if let (Some(object), Some(instructions)) = (
            compaction.as_object_mut(),
            options.instructions.as_deref().filter(|text| !text.is_empty()),
        ) {
            object.insert(
                "instructions".to_owned(),
                JsonValue::String(instructions.to_owned()),
            );
        }
        body.insert("compaction".to_owned(), compaction);
        let beta = self.request_betas(&config, &body, &[COMPACTION_BETA]);
        // On-demand compaction is only on the beta endpoint, which a narrower client may not have.
        if !self.inner.client.has_beta() {
            tracing::warn!(
                "model_id=<{model_id}> | client has no beta messages API | summarizing client-side"
            );
            return Ok(SummarizeResult::new(Vec::new()));
        }
        let request = ApiRequest {
            body: JsonValue::Object(body),
            beta,
        };
        let response = match self.inner.client.beta_final_message(request).await {
            Ok(response) => response,
            Err(error) if error.is_api_error() => {
                tracing::warn!(
                    "model_id=<{model_id}>, error=<{error}> | compaction request failed | summarizing client-side"
                );
                return Ok(SummarizeResult::new(Vec::new()));
            }
            Err(error) => return Err(translate_error(error)),
        };
        Ok(compaction_result(&model_id, &response))
    }
}

/// The summary in a compaction response, or an empty result when it holds none.
fn compaction_result(model_id: &str, response: &JsonValue) -> SummarizeResult {
    let usage = response
        .get("usage")
        .and_then(JsonValue::as_object)
        .map(format_compaction_usage);
    let with_usage = |result: SummarizeResult| match usage {
        Some(usage) => result.with_usage(usage),
        None => result,
    };
    let stop_reason = response.get("stop_reason").and_then(JsonValue::as_str);
    let block = response
        .get("content")
        .and_then(JsonValue::as_array)
        .and_then(|content| {
            content
                .iter()
                .find(|item| item.get("type").and_then(JsonValue::as_str) == Some("compaction"))
        });
    let text = block
        .and_then(|block| block.get("content"))
        .and_then(JsonValue::as_str)
        .filter(|text| !text.is_empty());
    let (Some("compaction"), Some(block), Some(text)) = (stop_reason, block, text) else {
        let stop_reason = stop_reason.unwrap_or("undefined");
        tracing::debug!(
            "model_id=<{model_id}>, stop_reason=<{stop_reason}> | compaction returned no summary | summarizing client-side"
        );
        return with_usage(SummarizeResult::new(Vec::new()));
    };
    let mut summary = SummaryBlock::new(text).with_provider(PROVIDER);
    if let Some(signature) = block.get("signature").and_then(JsonValue::as_str) {
        summary = summary.with_signature(signature);
    }
    with_usage(SummarizeResult::new(vec![ContentBlock::Summary(summary)]))
}

/// Constrains the response to the schema, keeping other `output_config` settings. `tool_choice`
/// none keeps the tools while ruling out tool calls, so the response is the JSON object.
fn with_native_output(body: &mut JsonObject, schema: &JsonValue) {
    body.insert("tool_choice".to_owned(), serde_json::json!({ "type": "none" }));
    let mut output_config = body
        .get("output_config")
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default();
    output_config.insert(
        "format".to_owned(),
        serde_json::json!({ "type": "json_schema", "schema": schema }),
    );
    body.insert("output_config".to_owned(), JsonValue::Object(output_config));
}

#[async_trait]
impl Model for AnthropicModel {
    /// Streams a conversation with the model.
    ///
    /// A server-side tool turn that Anthropic pauses is resumed with follow-up requests inside
    /// this call, so an error from one of those requests can follow events already yielded.
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        self.stream_events(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.config().to_base()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.inner
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .merge_base(config);
    }

    /// Turns temporary messages back on for the models the shared model info lists.
    fn model_info(&self) -> ModelInfo {
        let config = self.config_snapshot();
        let temporary_messages = lookup_model_info(config.model_id.as_deref().unwrap_or_default())
            .capabilities
            .and_then(|capabilities| capabilities.temporary_messages);
        let facts = ModelInfo {
            capabilities: Some(ModelCapabilities {
                temporary_messages,
                ..Default::default()
            }),
            limits: None,
        };
        merge_model_info([
            Some(&base_model_info(self)),
            Some(&facts),
            config.model_info.as_ref(),
        ])
    }

    /// Counts tokens with the token counting API when `use_native_token_count` is on, else (and
    /// when that call fails) with the heuristic estimate.
    async fn count_tokens(&self, messages: &[Message], options: CountTokensOptions) -> Result<u64> {
        // Count what a request sends, which starts at the last compaction block.
        let messages = if self.server_side_summary() {
            from_last_compaction(messages)
        } else {
            messages.to_vec()
        };
        let native = self.config_snapshot().use_native_token_count == Some(true);
        if !native || !self.inner.client.has_count_tokens() {
            return Ok(estimate_tokens_heuristic(&messages, &options));
        }
        match self.native_count_tokens(&messages, &options).await {
            Ok(tokens) => Ok(tokens),
            Err(error) => {
                tracing::debug!("error=<{error}> | native token counting failed, falling back to estimation");
                Ok(estimate_tokens_heuristic(&messages, &options))
            }
        }
    }

    /// Summarizes `messages` with Anthropic on-demand compaction when the model info says the
    /// model has server-side summary; otherwise, and when compaction fails with an API error or
    /// returns no compaction block, with the default client-side summary.
    async fn summarize(&self, messages: Vec<Message>, options: SummarizeOptions) -> Result<SummarizeResult> {
        if !self.server_side_summary() {
            return summarize_with_stream(self, messages, options).await;
        }
        let result = self.compact(&messages, &options).await?;
        if !result.content().is_empty() {
            return Ok(result);
        }
        let fallback = summarize_with_stream(self, messages, options).await?;
        let usage = sum_usage(result.usage(), fallback.usage());
        let summary = SummarizeResult::new(fallback.content().to_vec());
        Ok(match usage {
            Some(usage) => summary.with_usage(usage),
            None => summary,
        })
    }
}

#[cfg(test)]
mod tests;
