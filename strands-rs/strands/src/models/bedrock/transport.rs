//! The call path: the transport seam, error classification, the stream loop with guardrail
//! redaction, structured-output fallback, stale-thinking recovery and native token counting.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt as _};
use serde_json::json;

use super::Shared;
use super::format::FormatContext;
use super::parse::{has_blocked_guardrail_policy, map_converse_output, map_streamed_event, redaction_events};
use super::thinking_binding::{fields_with_drop_block, is_thinking_binding_error};
use crate::cancel::CancelSignal;
use crate::error::{BoxError, Error, ModelError, ModelNetworkError, ModelThrottledError, Result};
use crate::models::model::{BoxStream, CountTokensOptions, ModelRequest, estimate_tokens_heuristic};
use crate::models::native_output::{StructuredOutputMechanism, to_native_output_schema};
use crate::models::streaming::ModelStreamEvent;
use crate::tools::structured_output::STRUCTURED_OUTPUT_TOOL_NAME;
use crate::types::json::{JsonObject, JsonValue, to_js_string};
use crate::types::messages::Message;
use crate::types::tools::ToolChoice;

pub(crate) use super::wire::SdkTransport;

/// How a failed Bedrock call is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BedrockErrorKind {
    /// `ThrottlingException`: [`ModelThrottledError`].
    Throttled,
    /// The request never got a response: [`ModelNetworkError`].
    Network,
    /// Any other service or client failure: [`ModelError`].
    Service,
}

/// A failed Bedrock call, keeping the exception name the fallbacks branch on.
#[derive(Debug)]
pub(crate) struct BedrockError {
    pub(crate) kind: BedrockErrorKind,
    pub(crate) name: String,
    pub(crate) message: String,
    pub(crate) source: Option<BoxError>,
}

impl fmt::Display for BedrockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BedrockError {}

impl BedrockError {
    pub(crate) fn throttled(message: impl Into<String>) -> Self {
        Self {
            kind: BedrockErrorKind::Throttled,
            name: "ThrottlingException".to_owned(),
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn service(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: BedrockErrorKind::Service,
            name: name.into(),
            message: message.into(),
            source: None,
        }
    }

    fn is_validation(&self) -> bool {
        self.name == "ValidationException"
    }

    pub(crate) fn into_error(self) -> Error {
        let Self {
            kind,
            message,
            source,
            ..
        } = self;
        match kind {
            BedrockErrorKind::Throttled => {
                let error = ModelThrottledError::new(message);
                match source {
                    Some(source) => error.with_source(source).into(),
                    None => error.into(),
                }
            }
            BedrockErrorKind::Network => {
                let error = ModelNetworkError::new(message);
                match source {
                    Some(source) => error.with_source(source).into(),
                    None => error.into(),
                }
            }
            BedrockErrorKind::Service => {
                let error = ModelError::new(message);
                match source {
                    Some(source) => error.with_source(source).into(),
                    None => error.into(),
                }
            }
        }
    }
}

/// Converse events or one Converse response, as their JSON shapes.
pub(crate) type EventStream = BoxStream<'static, std::result::Result<JsonValue, BedrockError>>;

/// What [`Transport`] returns for one call.
pub(crate) enum Response {
    Stream(EventStream),
    Converse(JsonObject),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    ConverseStream,
    Converse,
}

/// Sends Converse requests (JSON shape in, JSON shape out); the seam unit tests replace.
#[async_trait]
pub(crate) trait Transport: Send + Sync + 'static {
    async fn send(
        &self,
        operation: Operation,
        request: JsonObject,
    ) -> std::result::Result<Response, BedrockError>;

    async fn count_tokens(
        &self,
        model_id: &str,
        input: JsonObject,
    ) -> std::result::Result<Option<u64>, BedrockError>;
}

enum SendFailure {
    Format(Error),
    Bedrock(BedrockError),
}

impl From<Error> for SendFailure {
    fn from(error: Error) -> Self {
        Self::Format(error)
    }
}

impl From<BedrockError> for SendFailure {
    fn from(error: BedrockError) -> Self {
        Self::Bedrock(error)
    }
}

impl From<SendFailure> for Error {
    fn from(failure: SendFailure) -> Self {
        match failure {
            SendFailure::Format(error) => error,
            SendFailure::Bedrock(error) => error.into_error(),
        }
    }
}

/// Sends `request`, retrying once with `drop_block` thinking when a history edit invalidated
/// replayed thinking blocks (TS `_send`).
async fn send_once(
    shared: &Shared,
    transport: &dyn Transport,
    operation: Operation,
    mut request: JsonObject,
) -> std::result::Result<Response, BedrockError> {
    let error = match transport.send(operation, request.clone()).await {
        Ok(response) => return Ok(response),
        Err(error) => error,
    };
    if !(error.is_validation() && is_thinking_binding_error(&error.message)) {
        return Err(error);
    }
    let Some(fields) = fields_with_drop_block(request.get("additionalModelRequestFields")) else {
        return Err(error);
    };
    let model_id = request
        .get("modelId")
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned);
    tracing::warn!(
        "model_id=<{}> | history edit invalidated replayed thinking blocks, retrying with drop_block",
        model_id.as_deref().unwrap_or_default()
    );
    shared.set_drop_stale_thinking_model_id(model_id);
    request.insert(
        "additionalModelRequestFields".to_owned(),
        JsonValue::Object(fields),
    );
    transport.send(operation, request).await
}

/// The request for a structured-output `mechanism` (TS `_structuredOutputRequest`).
fn structured_output_request(
    ctx: &FormatContext,
    request: &ModelRequest,
    mechanism: StructuredOutputMechanism,
    schema: &JsonValue,
) -> Result<JsonObject> {
    if mechanism == StructuredOutputMechanism::Forced {
        return ctx.format_request(request, request.tool_choice.as_ref());
    }
    let mut formatted = ctx.format_request(request, Some(&ToolChoice::Auto))?;
    if mechanism == StructuredOutputMechanism::Native {
        let native = to_native_output_schema(schema)?;
        let mut output_config = formatted
            .get("outputConfig")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        output_config.insert(
            "textFormat".to_owned(),
            json!({
                "type": "json_schema",
                "structure": {
                    "jsonSchema": {
                        "schema": to_js_string(&native).map_err(Error::other)?,
                        "name": STRUCTURED_OUTPUT_TOOL_NAME,
                    }
                }
            }),
        );
        formatted.insert("outputConfig".to_owned(), JsonValue::Object(output_config));
    }
    Ok(formatted)
}

/// TS `_sendRequest`: plain requests, or the structured-output mechanisms strongest first.
async fn send_request(
    shared: &Shared,
    transport: &dyn Transport,
    ctx: &FormatContext,
    request: &ModelRequest,
    operation: Operation,
) -> std::result::Result<Response, SendFailure> {
    let model_id = ctx.config.model_id.clone();
    let (Some(schema), Some(model_id)) = (request.structured_output_schema.as_ref(), model_id) else {
        let formatted = ctx.format_request(request, request.tool_choice.as_ref())?;
        return Ok(send_once(shared, transport, operation, formatted).await?);
    };
    let mechanisms = shared.structured_output.mechanisms(
        "bedrock",
        &model_id,
        request.tool_choice.as_ref(),
        schema,
        &ctx.model_info,
    );
    let (fallback, attempts) = match mechanisms.split_last() {
        Some((last, rest)) => (*last, rest),
        None => (StructuredOutputMechanism::Auto, &[][..]),
    };
    for &mechanism in attempts {
        let formatted = structured_output_request(ctx, request, mechanism, schema)?;
        match send_once(shared, transport, operation, formatted).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                let recognized = error.is_validation()
                    && shared.structured_output.handle_rejected_mechanism(
                        "bedrock",
                        &model_id,
                        mechanism,
                        schema,
                        &error.message,
                    );
                if !recognized {
                    return Err(error.into());
                }
            }
        }
    }
    let formatted = structured_output_request(ctx, request, fallback, schema)?;
    Ok(send_once(shared, transport, operation, formatted).await?)
}

/// The next event, or `None` when the stream ended or `cancel` fired.
async fn next_event(
    events: &mut EventStream,
    cancel: Option<&CancelSignal>,
) -> Option<std::result::Result<JsonValue, BedrockError>> {
    let Some(cancel) = cancel else {
        return events.next().await;
    };
    if cancel.is_cancelled() {
        return None;
    }
    tokio::select! {
        biased;
        () = cancel.cancelled() => None,
        event = events.next() => event,
    }
}

/// TS `stream`.
pub(crate) fn stream_events(
    shared: Arc<Shared>,
    request: ModelRequest,
) -> impl Stream<Item = Result<ModelStreamEvent>> + Send + 'static {
    async_stream::try_stream! {
        let ctx = shared.format_context();
        let operation = if ctx.config.stream == Some(false) {
            Operation::Converse
        } else {
            Operation::ConverseStream
        };
        let transport = shared.client.transport().await;
        let cancel = request.cancel_signal.clone();
        let response = tokio::select! {
            biased;
            () = cancelled(cancel.as_ref()) => None,
            response = send_request(&shared, transport.as_ref(), &ctx, &request, operation) => Some(response),
        };
        let guardrail = ctx.config.guardrail_config.clone();
        match response.transpose().map_err(Error::from)? {
            None => {}
            Some(Response::Converse(output)) => {
                for event in map_converse_output(&output, guardrail.as_ref())? {
                    yield event;
                }
            }
            Some(Response::Stream(mut events)) => {
                let mut last_stop_reason: Option<String> = None;
                let mut redaction_emitted = false;
                let mut saw_guardrail_trace = false;
                while let Some(chunk) = next_event(&mut events, cancel.as_ref()).await {
                    let chunk = chunk.map_err(BedrockError::into_error)?;
                    let (mapped, stop_reason) = map_streamed_event(&chunk)?;
                    if stop_reason.is_some() {
                        last_stop_reason = stop_reason;
                    }
                    // The trace on the metadata event decides BLOCKED versus ANONYMIZED (both report
                    // guardrail_intervened); without a trace the stop reason decides.
                    if let (false, Some(guardrail), Some(metadata)) =
                        (redaction_emitted, guardrail.as_ref(), chunk.get("metadata"))
                    {
                        match metadata.get("trace").and_then(|trace| trace.get("guardrail")) {
                            Some(trace) => {
                                saw_guardrail_trace = true;
                                if has_blocked_guardrail_policy(trace) {
                                    redaction_emitted = true;
                                    for event in redaction_events(guardrail, Some(trace)) {
                                        yield event;
                                    }
                                }
                            }
                            None if last_stop_reason.as_deref() == Some("guardrail_intervened") => {
                                redaction_emitted = true;
                                for event in redaction_events(guardrail, None) {
                                    yield event;
                                }
                            }
                            None => {}
                        }
                    }
                    for event in mapped {
                        yield event;
                    }
                }
                if let (false, false, Some("guardrail_intervened"), Some(guardrail)) = (
                    redaction_emitted,
                    saw_guardrail_trace,
                    last_stop_reason.as_deref(),
                    guardrail.as_ref(),
                ) {
                    for event in redaction_events(guardrail, None) {
                        yield event;
                    }
                }
            }
        }
    }
}

async fn cancelled(cancel: Option<&CancelSignal>) {
    match cancel {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending().await,
    }
}

/// TS `countTokens`: the Bedrock `CountTokens` API when `use_native_token_count` is set, the
/// heuristic otherwise or when the API fails.
pub(crate) async fn count_tokens(
    shared: &Shared,
    messages: &[Message],
    options: CountTokensOptions,
) -> Result<u64> {
    let ctx = shared.format_context();
    if ctx.config.use_native_token_count != Some(true) {
        return Ok(estimate_tokens_heuristic(messages, &options));
    }
    let model_id = ctx.config.effective_model_id().to_owned();
    if shared.skips_count_tokens(&model_id) {
        return Ok(estimate_tokens_heuristic(messages, &options));
    }
    let request = ModelRequest {
        messages: messages.to_vec(),
        system_prompt: options.system_prompt.clone(),
        tool_specs: options.tool_specs.clone(),
        ..Default::default()
    };
    let mut input = JsonObject::new();
    match ctx.format_request(&request, None) {
        Ok(formatted) => {
            for key in ["messages", "system", "toolConfig"] {
                if let Some(value) = formatted.get(key) {
                    input.insert(key.to_owned(), value.clone());
                }
            }
        }
        Err(error) => {
            tracing::debug!("error=<{error}> | native token counting failed, falling back to estimation");
            return Ok(estimate_tokens_heuristic(messages, &options));
        }
    }
    let transport = shared.client.transport().await;
    match transport.count_tokens(&model_id, input).await {
        Ok(Some(tokens)) => {
            tracing::debug!("total_tokens=<{tokens}> | native token count");
            return Ok(tokens);
        }
        Ok(None) => tracing::debug!(
            "error=<ProviderTokenCountError: Bedrock CountTokens returned undefined for inputTokens> | native token counting failed, falling back to estimation"
        ),
        Err(error) if error.name == "AccessDeniedException" => {
            crate::logging::warn_once::warn_once(&format!(
                "model_id=<{model_id}> | bedrock:CountTokens permission denied, falling back to heuristic estimation"
            ));
            shared.skip_count_tokens(model_id);
        }
        Err(error) if error.is_validation() && error.message.contains("doesn't support counting tokens") => {
            tracing::debug!(
                "model_id=<{model_id}> | model does not support CountTokens, caching for future calls, falling back to estimation"
            );
            shared.skip_count_tokens(model_id);
        }
        Err(error) => {
            tracing::debug!("error=<{error}> | native token counting failed, falling back to estimation");
        }
    }
    Ok(estimate_tokens_heuristic(messages, &options))
}

impl Shared {
    fn skips_count_tokens(&self, model_id: &str) -> bool {
        self.skip_count_tokens_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(model_id)
    }

    fn skip_count_tokens(&self, model_id: String) {
        self.skip_count_tokens_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(model_id);
    }
}
