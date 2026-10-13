//! Amazon Bedrock Converse model provider (`strands-ts/src/models/bedrock.ts`).

mod format;
mod parse;

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::config::http::HttpResponse;
use aws_sdk_bedrockruntime::config::Token;
use aws_sdk_bedrockruntime::error::ProvideErrorMetadata;
use aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError;
use aws_sdk_bedrockruntime::{types as bt, Client};
use aws_smithy_types::event_stream::RawMessage;
use futures::stream::BoxStream;
use parking_lot::RwLock;

use super::defaults::{default_model_warning_message, get_context_window_limit, MODEL_DEFAULTS};
use super::model::{estimate_tokens_heuristic, CountTokensOptions, Model, StreamOptions};
use super::streaming::ModelStreamEvent;
use crate::error::{Error, Result};
use crate::types::{JsonValue, Message};

pub use format::{FormattedRequest, ResolvedGuardrailConfig};

/// Default per-request timeout applied to the Bedrock client.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_millis(120_000);

/// Model ids for which tool results carry a `status` field under `auto` detection.
const MODELS_INCLUDE_STATUS: &[&str] = &["anthropic.claude"];
/// Model id fragments that support Anthropic-style prompt caching.
const MODELS_SUPPORTING_ANTHROPIC_CACHING: &[&str] = &["anthropic", "claude"];
/// Provider messages that mean "the prompt did not fit the context window".
const BEDROCK_CONTEXT_WINDOW_OVERFLOW_MESSAGES: &[&str] = &[
    "Input is too long for requested model",
    "input length and `max_tokens` exceed context limit",
    "too many total text bytes",
    "prompt is too long",
];

const DEFAULT_REDACT_INPUT_MESSAGE: &str = "[User input redacted.]";
const DEFAULT_REDACT_OUTPUT_MESSAGE: &str = "[Assistant output redacted.]";

/// Process-global negative cache of model ids whose `CountTokens` call failed.
fn skip_count_tokens_models() -> &'static Mutex<HashSet<String>> {
    static CACHE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Cache strategy (`CacheConfig.strategy` in TS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheStrategy {
    /// Enable caching when the model is known to support it.
    #[default]
    Auto,
    /// Force Anthropic-style cache points.
    Anthropic,
}

/// Per-section cache setting: `boolean | CacheTTL` in TS. Absent means "enabled, no TTL".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheSection {
    /// Section enabled without its own TTL.
    Enabled,
    /// Section disabled.
    Disabled,
    /// Section enabled with this TTL (sent verbatim; Bedrock validates it).
    Ttl(String),
}

impl From<bool> for CacheSection {
    fn from(enabled: bool) -> Self {
        if enabled {
            CacheSection::Enabled
        } else {
            CacheSection::Disabled
        }
    }
}

impl From<&str> for CacheSection {
    fn from(ttl: &str) -> Self {
        CacheSection::Ttl(ttl.to_owned())
    }
}

/// Prompt-caching configuration shared by providers (`CacheConfig` in `models/model.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheConfig {
    /// Strategy; `None` means `auto`.
    pub strategy: Option<CacheStrategy>,
    /// Default TTL for all sections.
    pub ttl: Option<String>,
    /// Tool-definition section.
    pub tools_ttl: Option<CacheSection>,
    /// System-prompt section.
    pub system_prompt_ttl: Option<CacheSection>,
    /// Conversation section.
    pub messages_ttl: Option<CacheSection>,
    /// Provider cache key; a no-op on Bedrock.
    pub cache_key: Option<String>,
}

/// Resolved cache section (`ResolvedCacheSection` in TS).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedCacheSection {
    /// Whether the section is cached.
    pub enabled: bool,
    /// TTL to send, if any.
    pub ttl: Option<String>,
}

/// `enabled` unless the section is explicitly disabled; `ttl` is the first non-empty candidate.
pub fn resolve_cache_section(section: Option<&CacheSection>, ttl_fallbacks: &[Option<&str>]) -> ResolvedCacheSection {
    let enabled = !matches!(section, Some(CacheSection::Disabled));
    let section_ttl = match section {
        Some(CacheSection::Ttl(ttl)) => Some(ttl.as_str()),
        _ => None,
    };
    let ttl = std::iter::once(section_ttl)
        .chain(ttl_fallbacks.iter().copied())
        .flatten()
        .find(|ttl| !ttl.is_empty())
        .map(str::to_owned);

    ResolvedCacheSection { enabled, ttl }
}

/// Whether tool results carry a `status` field (`includeToolResultStatus` in TS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IncludeToolResultStatus {
    /// Decide from the model id.
    #[default]
    Auto,
    /// Always send `status`.
    Always,
    /// Never send `status`.
    Never,
}

impl From<bool> for IncludeToolResultStatus {
    fn from(include: bool) -> Self {
        if include {
            IncludeToolResultStatus::Always
        } else {
            IncludeToolResultStatus::Never
        }
    }
}

/// Guardrail trace level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuardrailTraceMode {
    /// Trace enabled (the default on the wire).
    #[default]
    Enabled,
    /// Trace disabled.
    Disabled,
    /// Full trace.
    EnabledFull,
}

/// Guardrail stream processing mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardrailStreamProcessingMode {
    /// Synchronous processing.
    Sync,
    /// Asynchronous processing.
    Async,
}

/// What to replace when a guardrail blocks content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BedrockGuardrailRedactionConfig {
    /// Redact the user input; defaults to `true`.
    pub input: Option<bool>,
    /// Replacement for the user input.
    pub input_message: Option<String>,
    /// Redact the model output; defaults to `false`.
    pub output: Option<bool>,
    /// Replacement for the model output.
    pub output_message: Option<String>,
}

/// Guardrail configuration (`BedrockGuardrailConfig` in TS).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BedrockGuardrailConfig {
    /// Guardrail id.
    pub guardrail_identifier: String,
    /// Guardrail version.
    pub guardrail_version: String,
    /// Trace level; `None` sends `enabled`.
    pub trace: Option<GuardrailTraceMode>,
    /// Stream processing mode; omitted when `None`.
    pub stream_processing_mode: Option<GuardrailStreamProcessingMode>,
    /// SDK-side redaction behaviour.
    pub redaction: Option<BedrockGuardrailRedactionConfig>,
    /// Wrap the latest user text/image message in `guardContent`.
    pub guard_latest_user_message: Option<bool>,
}

/// Configuration for [`BedrockModel`] (`BedrockModelConfig` in TS).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BedrockModelConfig {
    /// Model id; defaults to `global.anthropic.claude-sonnet-4-6`.
    pub model_id: Option<String>,
    /// Max output tokens.
    pub max_tokens: Option<i32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Nucleus sampling cutoff.
    pub top_p: Option<f32>,
    /// Stop sequences.
    pub stop_sequences: Option<Vec<String>>,
    /// Context window size; auto-resolved from the model id when unset.
    pub context_window_limit: Option<u64>,
    /// Prompt caching.
    pub cache_config: Option<CacheConfig>,
    /// Sent as `additionalModelRequestFields`.
    pub additional_request_fields: Option<JsonValue>,
    /// Sent as `additionalModelResponseFieldPaths`.
    pub additional_response_field_paths: Option<Vec<String>>,
    /// Raw request overrides; not supported by this port (see the crate docs).
    pub additional_args: Option<JsonValue>,
    /// Use `ConverseStream` (the default) or `Converse` when `Some(false)`.
    pub stream: Option<bool>,
    /// Whether tool results carry a `status` field.
    pub include_tool_result_status: Option<IncludeToolResultStatus>,
    /// Guardrail configuration.
    pub guardrail_config: Option<BedrockGuardrailConfig>,
    /// Count tokens with Bedrock `CountTokens` instead of the heuristic.
    pub use_native_token_count: Option<bool>,
    /// Send tool schemas in Bedrock strict mode.
    pub strict_tools: Option<bool>,
}

impl BedrockModelConfig {
    /// Config with just a model id.
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: Some(model_id.into()),
            ..Self::default()
        }
    }

    fn merge(&mut self, update: BedrockModelConfig) {
        macro_rules! merge_field {
            ($($field:ident),+ $(,)?) => {
                $(if update.$field.is_some() {
                    self.$field = update.$field;
                })+
            };
        }
        merge_field!(
            model_id,
            max_tokens,
            temperature,
            top_p,
            stop_sequences,
            context_window_limit,
            cache_config,
            additional_request_fields,
            additional_response_field_paths,
            additional_args,
            stream,
            include_tool_result_status,
            guardrail_config,
            use_native_token_count,
            strict_tools,
        );
    }
}

/// Builder for [`BedrockModel`]; `build` resolves credentials and region from the AWS chain.
#[derive(Clone, Debug, Default)]
pub struct BedrockModelBuilder {
    config: BedrockModelConfig,
    region: Option<String>,
    api_key: Option<String>,
    request_timeout: Option<Duration>,
}

impl BedrockModelBuilder {
    /// Set the whole model config.
    pub fn config(mut self, config: BedrockModelConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the model id.
    pub fn model_id(mut self, model_id: impl Into<String>) -> Self {
        self.config.model_id = Some(model_id.into());
        self
    }

    /// Override the region; takes precedence over the environment.
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Use a bearer token (`Authorization: Bearer …`) instead of SigV4.
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Per-request timeout; defaults to [`DEFAULT_REQUEST_TIMEOUT`].
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// Resolve the AWS config and build the model.
    pub async fn build(self) -> Result<BedrockModel> {
        let config = resolve_config(self.config);
        let sdk_config = aws_config::defaults(BehaviorVersion::latest()).load().await;

        let region = self
            .region
            .clone()
            .or_else(|| sdk_config.region().map(|r| r.to_string()))
            .unwrap_or_else(|| {
                MODEL_DEFAULTS
                    .bedrock
                    .region
                    .expect("bedrock defaults carry a region")
                    .to_owned()
            });

        let mut client_config = aws_sdk_bedrockruntime::config::Builder::from(&sdk_config)
            .region(aws_sdk_bedrockruntime::config::Region::new(region))
            .app_name(
                aws_sdk_bedrockruntime::config::AppName::new("strands-agents-rs-sdk")
                    .map_err(|e| model_error("invalid app name", e))?,
            )
            .timeout_config(
                aws_sdk_bedrockruntime::config::timeout::TimeoutConfig::builder()
                    .operation_attempt_timeout(self.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT))
                    .build(),
            );

        if let Some(api_key) = self.api_key {
            client_config = client_config
                .bearer_token(Token::new(api_key, None))
                .auth_scheme_preference([aws_smithy_runtime_api::client::auth::http::HTTP_BEARER_AUTH_SCHEME_ID]);
        }

        Ok(BedrockModel {
            config: RwLock::new(config),
            client: Client::from_conf(client_config.build()),
        })
    }
}

fn resolve_config(mut config: BedrockModelConfig) -> BedrockModelConfig {
    if config.model_id.is_none() {
        crate::logging::warn_once(&default_model_warning_message(MODEL_DEFAULTS.bedrock.model_id));
        config.model_id = Some(MODEL_DEFAULTS.bedrock.model_id.to_owned());
    }
    config
}

/// Amazon Bedrock Converse / ConverseStream model provider.
#[derive(Debug)]
pub struct BedrockModel {
    config: RwLock<BedrockModelConfig>,
    client: Client,
}

impl BedrockModel {
    /// Build a model from a config, resolving credentials and region from the AWS chain.
    ///
    /// `AWS_BEARER_TOKEN_BEDROCK` is honoured by the AWS SDK's default chain; use
    /// [`BedrockModelBuilder::api_key`] to pass a bearer token explicitly.
    pub async fn new(config: BedrockModelConfig) -> Result<Self> {
        BedrockModelBuilder::default().config(config).build().await
    }

    /// Builder for region / bearer token / timeout overrides.
    pub fn builder() -> BedrockModelBuilder {
        BedrockModelBuilder::default()
    }

    /// Wrap an existing Bedrock Runtime client.
    pub fn from_client(client: Client, config: BedrockModelConfig) -> Self {
        Self {
            config: RwLock::new(resolve_config(config)),
            client,
        }
    }

    /// The current configuration, with `context_window_limit` resolved from the model id.
    pub fn config(&self) -> BedrockModelConfig {
        let config = self.config.read().clone();
        if config.context_window_limit.is_some() {
            return config;
        }
        let limit = get_context_window_limit(self.model_id_or_default(&config).as_str());
        BedrockModelConfig {
            context_window_limit: limit,
            ..config
        }
    }

    /// Shallow-merge an update into the configuration; `None` fields are left alone.
    pub fn update_config(&self, update: BedrockModelConfig) {
        self.config.write().merge(update);
    }

    /// Clear the process-global cache of model ids that skip native token counting.
    pub fn clear_count_tokens_cache() {
        skip_count_tokens_models()
            .lock()
            .expect("count tokens cache not poisoned")
            .clear();
    }

    /// The Bedrock Runtime client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    fn model_id_or_default(&self, config: &BedrockModelConfig) -> String {
        config
            .model_id
            .clone()
            .unwrap_or_else(|| MODEL_DEFAULTS.bedrock.model_id.to_owned())
    }
}

#[async_trait]
impl Model for BedrockModel {
    fn model_id(&self) -> Option<String> {
        self.config.read().model_id.clone()
    }

    fn context_window_limit(&self) -> Option<u64> {
        let config = self.config.read();
        config
            .context_window_limit
            .or_else(|| get_context_window_limit(&self.model_id_or_default(&config)))
    }

    fn stream<'a>(
        &'a self,
        messages: &'a [Message],
        options: &'a StreamOptions,
    ) -> BoxStream<'a, Result<ModelStreamEvent>> {
        self.stream_impl(messages, options)
    }

    async fn count_tokens(&self, messages: &[Message], options: &CountTokensOptions) -> Result<u64> {
        let heuristic = || estimate_tokens_heuristic(messages, options);
        if self.config.read().use_native_token_count != Some(true) {
            return Ok(heuristic());
        }
        let model_id = {
            let config = self.config.read();
            self.model_id_or_default(&config)
        };
        if skip_count_tokens_models()
            .lock()
            .expect("count tokens cache not poisoned")
            .contains(&model_id)
        {
            return Ok(heuristic());
        }
        match self.count_tokens_native(&model_id, messages, options).await {
            Ok(count) => Ok(count),
            Err(_) => Ok(heuristic()),
        }
    }
}

/// `true` when the message (or any nested service message) reports a context-window overflow.
pub(crate) fn is_context_window_overflow(message: &str) -> bool {
    BEDROCK_CONTEXT_WINDOW_OVERFLOW_MESSAGES
        .iter()
        .any(|needle| message.contains(needle))
}

/// `Error::Model` with a source attached.
pub(crate) fn model_error(message: impl Into<String>, source: impl Into<crate::error::BoxError>) -> Error {
    Error::Model {
        message: message.into(),
        source: Some(source.into()),
    }
}

/// Map a Bedrock SDK error: throttling, context overflow, then a generic model error.
fn map_sdk_error<E>(error: aws_sdk_bedrockruntime::error::SdkError<E, HttpResponse>) -> Error
where
    E: std::error::Error + ProvideErrorMetadata + Send + Sync + 'static,
{
    let code = error.code().map(str::to_owned);
    let message = flatten_error_message(&error);

    if code.as_deref() == Some("ThrottlingException") {
        tracing::debug!("throttled | error_message=<{message}>");
        return Error::ModelThrottled {
            message,
            source: Some(Box::new(error)),
        };
    }
    if is_context_window_overflow(&message) {
        return Error::ContextWindowOverflow(message);
    }
    model_error(message, error)
}

/// The error's own message plus every source message, so nested service messages are searchable.
fn flatten_error_message(error: &(impl std::error::Error + ProvideErrorMetadata)) -> String {
    if let Some(message) = error.message() {
        return message.to_owned();
    }
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(current) = source {
        parts.push(current.to_string());
        source = current.source();
    }
    parts.join(": ")
}

/// Map a mid-stream event error (`ConverseStreamOutputError`), mirroring the TS rethrow cases.
fn map_stream_event_error(
    error: aws_sdk_bedrockruntime::error::SdkError<ConverseStreamOutputError, RawMessage>,
) -> Error {
    match error.into_service_error() {
        ConverseStreamOutputError::ThrottlingException(exception) => {
            let message = exception
                .message()
                .unwrap_or("Request was throttled by the model provider")
                .to_owned();
            tracing::debug!("throttled | error_message=<{message}>");
            Error::ModelThrottled {
                message,
                source: Some(Box::new(exception)),
            }
        }
        other => {
            let message = other.message().map(str::to_owned).unwrap_or_else(|| other.to_string());
            if is_context_window_overflow(&message) {
                return Error::ContextWindowOverflow(message);
            }
            model_error(message, other)
        }
    }
}

impl BedrockModel {
    /// Format a Converse request from the current config (`_formatRequest`).
    pub(crate) fn format_request(&self, messages: &[Message], options: &StreamOptions) -> Result<FormattedRequest> {
        format::format_request(&self.config.read(), messages, options)
    }

    fn stream_impl<'a>(
        &'a self,
        messages: &'a [Message],
        options: &'a StreamOptions,
    ) -> BoxStream<'a, Result<ModelStreamEvent>> {
        let config = self.config.read().clone();
        let streaming = config.stream != Some(false);
        let request = format::format_request(&config, messages, options);

        Box::pin(async_stream::try_stream! {
            let request = request?;
            if streaming {
                let mut builder = self
                    .client
                    .converse_stream()
                    .model_id(&request.model_id)
                    .set_messages(Some(request.messages.clone()))
                    .set_system(request.system.clone())
                    .set_tool_config(request.tool_config.clone())
                    .set_inference_config(request.inference_config.clone())
                    .set_additional_model_request_fields(request.additional_model_request_fields.clone())
                    .set_additional_model_response_field_paths(
                        request.additional_model_response_field_paths.clone(),
                    );
                if let Some(guardrail) = &request.guardrail {
                    builder = builder.guardrail_config(guardrail.to_stream()?);
                }

                let response = builder.send().await.map_err(map_sdk_error)?;
                let mut stream = response.stream;
                let mut last_stop_reason: Option<String> = None;
                let mut redaction_emitted = false;
                let mut saw_guardrail_trace = false;

                loop {
                    let chunk = match stream.recv().await {
                        Ok(Some(chunk)) => chunk,
                        Ok(None) => break,
                        Err(error) => Err(map_stream_event_error(error))?,
                    };

                    let (events, stop_reason) = parse::map_streamed_event(&chunk, last_stop_reason.take())?;
                    last_stop_reason = stop_reason;

                    if !redaction_emitted {
                        if let (Some(guardrail_config), bt::ConverseStreamOutput::Metadata(metadata)) =
                            (config.guardrail_config.as_ref(), &chunk)
                        {
                            match metadata.trace().and_then(|trace| trace.guardrail()) {
                                Some(guardrail) => {
                                    saw_guardrail_trace = true;
                                    if parse::has_blocked_guardrail_policy(guardrail) {
                                        redaction_emitted = true;
                                        for event in parse::generate_redaction_events(guardrail_config, Some(guardrail)) {
                                            yield event;
                                        }
                                    }
                                }
                                None if last_stop_reason.as_deref() == Some("guardrail_intervened") => {
                                    redaction_emitted = true;
                                    for event in parse::generate_redaction_events(guardrail_config, None) {
                                        yield event;
                                    }
                                }
                                None => {}
                            }
                        }
                    }

                    for event in events {
                        yield event;
                    }
                }

                if !redaction_emitted && !saw_guardrail_trace
                    && last_stop_reason.as_deref() == Some("guardrail_intervened")
                {
                    if let Some(guardrail_config) = config.guardrail_config.as_ref() {
                        for event in parse::generate_redaction_events(guardrail_config, None) {
                            yield event;
                        }
                    }
                }
            } else {
                let mut builder = self
                    .client
                    .converse()
                    .model_id(&request.model_id)
                    .set_messages(Some(request.messages.clone()))
                    .set_system(request.system.clone())
                    .set_tool_config(request.tool_config.clone())
                    .set_inference_config(request.inference_config.clone())
                    .set_additional_model_request_fields(request.additional_model_request_fields.clone())
                    .set_additional_model_response_field_paths(
                        request.additional_model_response_field_paths.clone(),
                    );
                if let Some(guardrail) = &request.guardrail {
                    builder = builder.guardrail_config(guardrail.to_converse()?);
                }

                let response = builder.send().await.map_err(map_sdk_error)?;
                for event in parse::map_converse_output(&config, &response)? {
                    yield event;
                }
            }
        })
    }

    async fn count_tokens_native(
        &self,
        model_id: &str,
        messages: &[Message],
        options: &CountTokensOptions,
    ) -> Result<u64> {
        let stream_options = StreamOptions {
            system_prompt: options.system_prompt.clone(),
            tool_specs: options.tool_specs.clone(),
            ..StreamOptions::default()
        };
        let request = self.format_request(messages, &stream_options)?;

        let converse = bt::ConverseTokensRequest::builder()
            .set_messages(Some(request.messages))
            .set_system(request.system)
            .set_tool_config(request.tool_config)
            .build();

        let response = self
            .client
            .count_tokens()
            .model_id(model_id)
            .input(bt::CountTokensInput::Converse(converse))
            .send()
            .await;

        match response {
            Ok(output) => {
                let count = output.input_tokens().max(0) as u64;
                tracing::debug!("total_tokens=<{count}> | native token count");
                Ok(count)
            }
            Err(error) => {
                let code = error.code().map(str::to_owned);
                let message = flatten_error_message(&error);
                match code.as_deref() {
                    Some("AccessDeniedException") => {
                        crate::logging::warn_once(&format!(
                            "model_id=<{model_id}> | bedrock:CountTokens permission denied, falling back to heuristic estimation"
                        ));
                        skip_count_tokens_models()
                            .lock()
                            .expect("count tokens cache not poisoned")
                            .insert(model_id.to_owned());
                    }
                    Some("ValidationException") if message.contains("doesn't support counting tokens") => {
                        tracing::debug!(
                            "model_id=<{model_id}> | model does not support CountTokens, caching for future calls, falling back to estimation"
                        );
                        skip_count_tokens_models()
                            .lock()
                            .expect("count tokens cache not poisoned")
                            .insert(model_id.to_owned());
                    }
                    _ => tracing::debug!("error=<{message}> | native token counting failed, falling back to estimation"),
                }
                Err(model_error(message, error))
            }
        }
    }
}
