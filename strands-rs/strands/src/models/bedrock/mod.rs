//! Amazon Bedrock model provider (TS `models/bedrock.ts`): the `ConverseStream` and `Converse` APIs
//! through `aws-sdk-bedrockruntime`.
//!
//! Requests are built as the Converse JSON shape (camelCase keys, `bytes` as base64), mirroring
//! the TS command input, and converted to SDK types at the transport boundary; responses take the
//! reverse path.

mod client;
mod document;
mod format;
mod parse;
mod thinking_binding;
mod transport;
mod wire;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;

#[cfg(test)]
use self::client::EnvLookup;
use self::client::process_env;
#[cfg(test)]
use self::transport::Transport;
use self::transport::{BedrockError, SdkTransport};
use crate::error::{ContextWindowOverflowError, Error, Result};
use crate::logging::warn_once::warn_once;
use crate::models::defaults::{
    MODEL_DEFAULTS, default_max_tokens_warning_message, default_model_warning_message,
};
use crate::models::model::{
    BaseModelConfig, BoxStream, CacheConfig, CacheTtl, CountTokensOptions, Model, ModelRequest,
    resolve_config_metadata, resolve_max_tokens,
};
use crate::models::model_info::{
    ModelCapabilities, ModelInfo, ModelLimits, PromptCaching, SERVER_SIDE_SUMMARY_OFF, lookup_model,
    merge_model_info,
};
use crate::models::native_output::StructuredOutputSupport;
use crate::models::streaming::ModelStreamEvent;
use crate::types::json::JsonValue;
use crate::types::messages::Message;

/// A Bedrock prompt-cache TTL (TS `BedrockCacheTTL`). Bedrock accepts `5m` (default) and `1h`,
/// validates the value server-side, and requires TTLs to be non-increasing across tools, system
/// and messages.
pub type BedrockCacheTtl = CacheTtl;

/// Prompt-caching configuration for the Bedrock provider (TS `BedrockCacheConfig`).
pub type BedrockCacheConfig = CacheConfig;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

const CONTEXT_WINDOW_OVERFLOW_MESSAGES: [&str; 6] = [
    "Input is too long for requested model",
    "input length and `max_tokens` exceed context limit",
    "too many total text bytes",
    "prompt is too long",
    "This model's maximum context length is",
    "Input Tokens Exceeded",
];

/// Redaction of guardrail-blocked content (TS `BedrockGuardrailRedactionConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into))]
pub struct BedrockGuardrailRedactionConfig {
    /// Redact the input when blocked (default `true`).
    pub input: Option<bool>,
    /// The replacement for redacted input (default `[User input redacted.]`).
    pub input_message: Option<String>,
    /// Redact the output when blocked (default `false`).
    pub output: Option<bool>,
    /// The replacement for redacted output (default `[Assistant output redacted.]`).
    pub output_message: Option<String>,
}

/// The guardrail trace mode (TS `'enabled' | 'disabled' | 'enabled_full'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardrailTrace {
    /// `enabled` (the default).
    Enabled,
    /// `disabled`.
    Disabled,
    /// `enabled_full`.
    EnabledFull,
}

impl GuardrailTrace {
    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::EnabledFull => "enabled_full",
        }
    }
}

/// How streamed output is checked by the guardrail (TS `'sync' | 'async'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardrailStreamProcessingMode {
    /// `sync`.
    Sync,
    /// `async`.
    Async,
}

impl GuardrailStreamProcessingMode {
    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Async => "async",
        }
    }
}

/// Bedrock guardrails (TS `BedrockGuardrailConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into))]
pub struct BedrockGuardrailConfig {
    /// The guardrail identifier.
    pub guardrail_identifier: String,
    /// The guardrail version (for example `1` or `DRAFT`).
    pub guardrail_version: String,
    /// The trace mode (default [`GuardrailTrace::Enabled`]).
    pub trace: Option<GuardrailTrace>,
    /// The stream processing mode.
    pub stream_processing_mode: Option<GuardrailStreamProcessingMode>,
    /// What to redact when content is blocked.
    pub redaction: Option<BedrockGuardrailRedactionConfig>,
    /// Evaluate only the latest user message: its text and image content is wrapped in
    /// `guardContent` blocks. The latest user message with text or image content is used, so
    /// tool-result turns that follow it don't move the target (default `false`).
    pub guard_latest_user_message: Option<bool>,
}

/// Whether tool results carry their `status` (TS `'auto' | boolean`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IncludeToolResultStatus {
    /// Decide from the model id: Amazon Nova and Anthropic Claude models get it (the default).
    Auto,
    /// Always (`true`) or never (`false`).
    Enabled(bool),
}

impl From<bool> for IncludeToolResultStatus {
    fn from(include: bool) -> Self {
        Self::Enabled(include)
    }
}

/// Configuration of a [`BedrockModel`] (TS `BedrockModelConfig`).
///
/// ```
/// use strands::models::bedrock::BedrockModelConfig;
///
/// let config = BedrockModelConfig {
///     model_id: Some("global.anthropic.claude-sonnet-4-6".into()),
///     max_tokens: Some(1024),
///     temperature: Some(0.7),
///     ..Default::default()
/// };
/// assert_eq!(config.stream, None);
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
#[builder(on(String, into))]
pub struct BedrockModelConfig {
    /// The Bedrock model id or inference profile (default `global.anthropic.claude-sonnet-4-6`).
    pub model_id: Option<String>,
    /// The largest number of tokens to generate (default: the model's `maxOutputTokens` from the
    /// model info when known; otherwise not sent and Bedrock's per-model default applies).
    pub max_tokens: Option<u64>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Nucleus sampling probability.
    pub top_p: Option<f64>,
    /// The context window in tokens; overrides the model-info table.
    pub context_window_limit: Option<u64>,
    /// Model facts merged over the shared model-info table.
    pub model_info: Option<ModelInfo>,
    /// Sequences that stop generation.
    pub stop_sequences: Option<Vec<String>>,
    /// Prompt caching.
    pub cache_config: Option<BedrockCacheConfig>,
    /// Extra fields for the model (`additionalModelRequestFields`).
    pub additional_request_fields: Option<JsonValue>,
    /// Response field paths to return (`additionalModelResponseFieldPaths`).
    pub additional_response_field_paths: Option<Vec<String>>,
    /// Extra top-level Converse request fields (an object), applied over the built request.
    /// Supported keys: every `Converse` and `ConverseStream` request member.
    pub additional_args: Option<JsonValue>,
    /// Stream through `ConverseStream` (`true`, the default) or call `Converse` (`false`).
    pub stream: Option<bool>,
    /// Whether tool results carry their status (default [`IncludeToolResultStatus::Auto`]).
    pub include_tool_result_status: Option<IncludeToolResultStatus>,
    /// Guardrails.
    pub guardrail_config: Option<BedrockGuardrailConfig>,
    /// Count tokens with the Bedrock `CountTokens` API instead of the heuristic (default `false`).
    pub use_native_token_count: Option<bool>,
    /// Send every tool with `strict: true` and a closed copy of its input schema (default `false`).
    /// Bedrock rejects schemas outside its strict subset with a 400 and compiles each new schema on
    /// first use, which can take minutes. Incompatible with citations for Anthropic models.
    pub strict_tools: Option<bool>,
}

impl BedrockModelConfig {
    /// Shallow-merges `update` into `self`: every field `update` sets replaces the current value.
    pub fn merge(&mut self, update: BedrockModelConfig) {
        let BedrockModelConfig {
            model_id,
            max_tokens,
            temperature,
            top_p,
            context_window_limit,
            model_info,
            stop_sequences,
            cache_config,
            additional_request_fields,
            additional_response_field_paths,
            additional_args,
            stream,
            include_tool_result_status,
            guardrail_config,
            use_native_token_count,
            strict_tools,
        } = update;
        replace_if_set(&mut self.model_id, model_id);
        replace_if_set(&mut self.max_tokens, max_tokens);
        replace_if_set(&mut self.temperature, temperature);
        replace_if_set(&mut self.top_p, top_p);
        replace_if_set(&mut self.context_window_limit, context_window_limit);
        replace_if_set(&mut self.model_info, model_info);
        replace_if_set(&mut self.stop_sequences, stop_sequences);
        replace_if_set(&mut self.cache_config, cache_config);
        replace_if_set(&mut self.additional_request_fields, additional_request_fields);
        replace_if_set(
            &mut self.additional_response_field_paths,
            additional_response_field_paths,
        );
        replace_if_set(&mut self.additional_args, additional_args);
        replace_if_set(&mut self.stream, stream);
        replace_if_set(&mut self.include_tool_result_status, include_tool_result_status);
        replace_if_set(&mut self.guardrail_config, guardrail_config);
        replace_if_set(&mut self.use_native_token_count, use_native_token_count);
        replace_if_set(&mut self.strict_tools, strict_tools);
    }

    fn base(&self) -> BaseModelConfig {
        BaseModelConfig {
            model_id: self.model_id.clone(),
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            top_p: self.top_p,
            context_window_limit: self.context_window_limit,
            model_info: self.model_info.clone(),
        }
    }

    fn effective_model_id(&self) -> &str {
        self.model_id
            .as_deref()
            .unwrap_or(MODEL_DEFAULTS.bedrock.model_id)
    }
}

impl From<BaseModelConfig> for BedrockModelConfig {
    fn from(base: BaseModelConfig) -> Self {
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
}

fn replace_if_set<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

/// Everything a [`BedrockModel`] is built from (TS `BedrockModelOptions`): the model config plus
/// how the client is set up.
///
/// The region resolves in this order: `region`, the region of `sdk_config`, the `AWS_REGION` and
/// `AWS_DEFAULT_REGION` environment variables, the shared AWS config profile, then `us-west-2`.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct BedrockModelOptions {
    /// The model config.
    #[builder(default)]
    pub config: BedrockModelConfig,
    /// The AWS region; takes precedence over `sdk_config`.
    pub region: Option<String>,
    /// AWS SDK settings (credentials, endpoint, retry, app name) for the client (TS
    /// `clientConfig`); loaded from the default provider chain when unset.
    pub sdk_config: Option<aws_config::SdkConfig>,
    /// How long a request may wait for response data (default 120 s); takes precedence over the
    /// read timeout of `sdk_config`.
    pub request_timeout: Option<Duration>,
    /// An Amazon Bedrock API key; requests then use bearer authentication instead of `SigV4`.
    pub api_key: Option<String>,
}

impl fmt::Debug for BedrockModelOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BedrockModelOptions")
            .field("config", &self.config)
            .field("region", &self.region)
            .field("sdk_config", &self.sdk_config.as_ref().map(|_| "SdkConfig"))
            .field("request_timeout", &self.request_timeout)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl From<BedrockModelConfig> for BedrockModelOptions {
    fn from(config: BedrockModelConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }
}

/// The Amazon Bedrock model provider (TS `BedrockModel`), using `ConverseStream` (or `Converse` with
/// `stream: Some(false)`), with prompt caching, guardrails and native structured output.
///
/// The client is created on the first call, so construction never touches the network.
///
/// ```no_run
/// use strands::models::bedrock::{BedrockModel, BedrockModelConfig};
/// use strands::Agent;
///
/// # async fn run() -> strands::Result<()> {
/// let model = BedrockModel::new(BedrockModelConfig {
///     model_id: Some("global.anthropic.claude-sonnet-4-6".into()),
///     ..Default::default()
/// });
/// let mut agent = Agent::builder().model(model).build()?;
/// let result = agent.invoke("Hello").await?;
/// # let _ = result;
/// # Ok(())
/// # }
/// ```
pub struct BedrockModel {
    shared: Arc<Shared>,
}

impl fmt::Debug for BedrockModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BedrockModel")
            .field("config", &self.shared.raw_config())
            .finish_non_exhaustive()
    }
}

impl Default for BedrockModel {
    fn default() -> Self {
        Self::new(BedrockModelOptions::default())
    }
}

pub(crate) struct Shared {
    config: RwLock<BedrockModelConfig>,
    client: client::LazyClient,
    drop_stale_thinking_model_id: Mutex<Option<String>>,
    structured_output: StructuredOutputSupport,
    skip_count_tokens_models: Mutex<HashSet<String>>,
}

impl BedrockModel {
    /// A model built from `options` (a [`BedrockModelOptions`] or a [`BedrockModelConfig`]).
    pub fn new(options: impl Into<BedrockModelOptions>) -> Self {
        let options = options.into();
        let client = client::LazyClient::new(client::ClientOptions::from_options(&options), process_env());
        Self::build(options.config, client)
    }

    /// A model that sends through an existing Bedrock client.
    pub fn from_client(client: aws_sdk_bedrockruntime::Client, config: BedrockModelConfig) -> Self {
        Self::build(
            config,
            client::LazyClient::ready(Arc::new(SdkTransport::new(client))),
        )
    }

    #[cfg(test)]
    pub(crate) fn with_transport(config: BedrockModelConfig, transport: Arc<dyn Transport>) -> Self {
        Self::build(config, client::LazyClient::ready(transport))
    }

    #[cfg(test)]
    pub(crate) fn with_client_options(options: BedrockModelOptions, env: EnvLookup) -> Self {
        let client = client::LazyClient::new(client::ClientOptions::from_options(&options), env);
        Self::build(options.config, client)
    }

    fn build(mut config: BedrockModelConfig, client: client::LazyClient) -> Self {
        if config.model_id.is_none() {
            warn_once(&default_model_warning_message(MODEL_DEFAULTS.bedrock.model_id));
            config.model_id = Some(MODEL_DEFAULTS.bedrock.model_id.to_owned());
        }
        let user_pinned = config.max_tokens.is_some()
            || config
                .model_info
                .as_ref()
                .and_then(ModelInfo::max_output_tokens)
                .is_some();
        let model = Self {
            shared: Arc::new(Shared {
                config: RwLock::new(config),
                client,
                drop_stale_thinking_model_id: Mutex::new(None),
                structured_output: StructuredOutputSupport::default(),
                skip_count_tokens_models: Mutex::new(HashSet::new()),
            }),
        };
        if let (false, Some(max_tokens)) = (user_pinned, model.config().max_tokens) {
            warn_once(&default_max_tokens_warning_message(max_tokens));
        }
        model
    }

    /// The configuration, with the default model id, context window and max tokens resolved
    /// (TS `getConfig`).
    pub fn config(&self) -> BedrockModelConfig {
        self.shared.resolved_config()
    }

    /// Shallow-merges `config` into the current configuration (TS `updateConfig`).
    pub fn update_config(&self, config: BedrockModelConfig) {
        self.shared
            .config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .merge(config);
    }
}

impl Shared {
    fn raw_config(&self) -> BedrockModelConfig {
        self.config.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn resolved_config(&self) -> BedrockModelConfig {
        let mut config = self.raw_config();
        let model_id = config.effective_model_id().to_owned();
        let mut base = config.base();
        resolve_config_metadata(&mut base, &model_id);
        config.context_window_limit = base.context_window_limit;
        if config.max_tokens.is_none() {
            config.max_tokens = resolve_max_tokens(&base, &model_id);
        }
        config.model_id = Some(model_id);
        config
    }

    fn model_info(&self) -> ModelInfo {
        let config = self.resolved_config();
        let model_id = config.effective_model_id();
        let known = ModelInfo {
            capabilities: Some(ModelCapabilities {
                temporary_messages: Some(false),
                ..Default::default()
            }),
            limits: Some(ModelLimits {
                context_window: config.context_window_limit,
                max_output_tokens: None,
            }),
        };
        let openai = model_id.to_lowercase().contains("openai.").then(|| ModelInfo {
            capabilities: Some(ModelCapabilities {
                prompt_caching: Some(PromptCaching::Auto),
                ..Default::default()
            }),
            limits: None,
        });
        merge_model_info([
            lookup_model(model_id),
            Some(&known),
            config.model_info.as_ref(),
            Some(&SERVER_SIDE_SUMMARY_OFF),
            openai.as_ref(),
            config.model_info.as_ref(),
        ])
    }

    fn drop_stale_thinking_model_id(&self) -> Option<String> {
        self.drop_stale_thinking_model_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_drop_stale_thinking_model_id(&self, model_id: Option<String>) {
        *self
            .drop_stale_thinking_model_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = model_id;
    }

    fn format_context(&self) -> format::FormatContext {
        let config = self.raw_config();
        let resolved_max_tokens = self.resolved_config().max_tokens;
        let drop_stale_thinking =
            self.drop_stale_thinking_model_id().as_deref() == config.model_id.as_deref();
        format::FormatContext {
            model_info: self.model_info(),
            resolved_max_tokens,
            drop_stale_thinking,
            config,
        }
    }
}

/// Maps a failed call to [`ContextWindowOverflowError`] when its message says the input was too
/// long, as TS does for every error the stream throws.
fn map_overflow(error: Error) -> Error {
    if matches!(error, Error::ContextWindowOverflow(_)) {
        return error;
    }
    let message = error.to_string();
    if CONTEXT_WINDOW_OVERFLOW_MESSAGES
        .iter()
        .any(|pattern| message.contains(pattern))
    {
        return ContextWindowOverflowError::new(message).into();
    }
    error
}

#[async_trait]
impl Model for BedrockModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        let shared = Arc::clone(&self.shared);
        Box::pin(transport::stream_events(shared, request).map(|item| item.map_err(map_overflow)))
    }

    fn get_config(&self) -> BaseModelConfig {
        self.config().base()
    }

    fn update_config(&self, config: BaseModelConfig) {
        BedrockModel::update_config(self, config.into());
    }

    fn model_info(&self) -> ModelInfo {
        self.shared.model_info()
    }

    async fn count_tokens(&self, messages: &[Message], options: CountTokensOptions) -> Result<u64> {
        transport::count_tokens(&self.shared, messages, options).await
    }
}

impl From<BedrockError> for Error {
    fn from(error: BedrockError) -> Self {
        error.into_error()
    }
}
