//! The agent tracer (TS `telemetry/tracer.ts`): `tracing` spans with the TS names and `gen_ai.*`
//! attributes, and the local `AgentTrace` tree. Dynamic attributes and span events need the `otel`
//! feature, where a `tracing-opentelemetry` layer exports the spans.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::{Serialize, Serializer};
use serde_json::{Value as JsonValue, json};
use tokio::time::Instant;
use tracing::Span;

use super::config::{Semconv, process_env};
use super::json::to_json;
use crate::models::streaming::{Metrics, Usage};
use crate::types::messages::{
    ContentBlock, Message, Role, SystemContentBlock, SystemPrompt, ToolResultBlock,
};
use crate::types::snapshot::iso_timestamp;
use crate::types::tools::ToolUse;

fn as_ms<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

/// Epoch milliseconds, as TS `Date.now()`.
fn timestamp_ms<S: Serializer>(timestamp: &SystemTime, serializer: S) -> Result<S::Ok, S::Error> {
    let ms = match timestamp.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    };
    serializer.serialize_i64(ms)
}

fn optional_timestamp_ms<S: Serializer>(
    timestamp: &Option<SystemTime>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match timestamp {
        Some(timestamp) => timestamp_ms(timestamp, serializer),
        None => serializer.serialize_none(),
    }
}

/// One node of the local execution trace: a cycle, a model call or a tool call (TS `AgentTrace`).
///
/// Serializes as TS `toJSON()`; times are epoch milliseconds and the duration whole milliseconds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentTrace {
    id: String,
    name: String,
    parent_id: Option<String>,
    #[serde(serialize_with = "timestamp_ms")]
    start_time: SystemTime,
    #[serde(serialize_with = "optional_timestamp_ms")]
    end_time: Option<SystemTime>,
    #[serde(serialize_with = "as_ms")]
    duration: Duration,
    children: Vec<AgentTrace>,
    metadata: IndexMap<String, String>,
    message: Option<Message>,
    #[serde(skip)]
    started: Instant,
}

impl AgentTrace {
    fn new(name: impl Into<String>, parent_id: Option<String>) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            name: name.into(),
            parent_id,
            start_time: SystemTime::now(),
            end_time: None,
            duration: Duration::ZERO,
            children: Vec::new(),
            metadata: IndexMap::new(),
            message: None,
            started: Instant::now(),
        }
    }

    fn child(&self, name: impl Into<String>) -> Self {
        Self::new(name, Some(self.id.clone()))
    }

    fn end(&mut self) {
        let duration = self.started.elapsed();
        self.duration = duration;
        self.end_time = Some(self.start_time.checked_add(duration).unwrap_or(self.start_time));
    }

    /// A random unique id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `Cycle <n>`, `stream_messages` or `Tool: <name>`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The id of the enclosing trace.
    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    /// When the traced step started.
    pub fn start_time(&self) -> SystemTime {
        self.start_time
    }

    /// When the traced step ended; `None` while it runs.
    pub fn end_time(&self) -> Option<SystemTime> {
        self.end_time
    }

    /// How long the step took; zero while it runs.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// The nested steps, in start order.
    pub fn children(&self) -> &[AgentTrace] {
        &self.children
    }

    /// Step details: `cycleId` on cycles, `toolUseId` and `toolName` on tool calls.
    pub fn metadata(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.metadata
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// One metadata value.
    pub fn metadata_value(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(String::as_str)
    }

    /// The model's output message, on model-call traces.
    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }
}

/// A span attribute value (OpenTelemetry `AttributeValue`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AttributeValue {
    /// A string. Not named `String`: a public item with that name in a direct dependency makes
    /// rustc print `std::string::String` in every diagnostic of the depending crate.
    Str(String),
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A floating-point number.
    Float(f64),
    /// An array of strings.
    StringArray(Vec<String>),
    /// An array of booleans.
    BoolArray(Vec<bool>),
    /// An array of integers.
    IntArray(Vec<i64>),
    /// An array of floating-point numbers.
    FloatArray(Vec<f64>),
}

macro_rules! attribute_value_from {
    ($($source:ty => $variant:ident),* $(,)?) => {$(
        impl From<$source> for AttributeValue {
            fn from(value: $source) -> Self {
                Self::$variant(value.into())
            }
        }
    )*};
}

attribute_value_from! {
    &str => Str, String => Str, bool => Bool, i64 => Int, i32 => Int, u32 => Int, f64 => Float,
    Vec<String> => StringArray, Vec<bool> => BoolArray, Vec<i64> => IntArray, Vec<f64> => FloatArray,
}

/// The attribute keys of the agent spans (TS `telemetry/tracer.ts`).
pub(crate) mod keys {
    pub(crate) const OPERATION_NAME: &str = "gen_ai.operation.name";
    pub(crate) const SYSTEM: &str = "gen_ai.system";
    pub(crate) const PROVIDER_NAME: &str = "gen_ai.provider.name";
    pub(crate) const AGENT_NAME: &str = "gen_ai.agent.name";
    pub(crate) const AGENT_ID: &str = "gen_ai.agent.id";
    pub(crate) const AGENT_TOOLS: &str = "gen_ai.agent.tools";
    pub(crate) const REQUEST_MODEL: &str = "gen_ai.request.model";
    pub(crate) const TOOL_DEFINITIONS: &str = "gen_ai.tool.definitions";
    pub(crate) const SYSTEM_PROMPT: &str = "system_prompt";
    pub(crate) const SPAN_NAME: &str = "name";
    pub(crate) const START_TIME: &str = "gen_ai.event.start_time";
    pub(crate) const END_TIME: &str = "gen_ai.event.end_time";
    pub(crate) const CYCLE_ID: &str = "agent_loop.cycle_id";
    pub(crate) const TOOL_NAME: &str = "gen_ai.tool.name";
    pub(crate) const TOOL_CALL_ID: &str = "gen_ai.tool.call.id";
    pub(crate) const TOOL_STATUS: &str = "gen_ai.tool.status";
    pub(crate) const TOOL_CALL_ARGUMENTS: &str = "gen_ai.tool.call.arguments";
    pub(crate) const TOOL_CALL_RESULT: &str = "gen_ai.tool.call.result";
    pub(crate) const USAGE_PROMPT_TOKENS: &str = "gen_ai.usage.prompt_tokens";
    pub(crate) const USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
    pub(crate) const USAGE_COMPLETION_TOKENS: &str = "gen_ai.usage.completion_tokens";
    pub(crate) const USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
    pub(crate) const USAGE_TOTAL_TOKENS: &str = "gen_ai.usage.total_tokens";
    pub(crate) const USAGE_CACHE_READ: &str = "gen_ai.usage.cache_read.input_tokens";
    pub(crate) const USAGE_CACHE_READ_LEGACY: &str = "gen_ai.usage.cache_read_input_tokens";
    pub(crate) const USAGE_CACHE_CREATION: &str = "gen_ai.usage.cache_creation.input_tokens";
    pub(crate) const USAGE_CACHE_WRITE_LEGACY: &str = "gen_ai.usage.cache_write_input_tokens";
    pub(crate) const SERVER_REQUEST_DURATION: &str = "gen_ai.server.request.duration";
    pub(crate) const LANGFUSE_OBSERVATION_TYPE: &str = "langfuse.observation.type";
    pub(crate) const INPUT_MESSAGES: &str = "gen_ai.input.messages";
    pub(crate) const OUTPUT_MESSAGES: &str = "gen_ai.output.messages";
    pub(crate) const SYSTEM_INSTRUCTIONS: &str = "gen_ai.system_instructions";
    pub(crate) const OPERATION_DETAILS_EVENT: &str = "gen_ai.client.inference.operation.details";
    pub(crate) const STATUS_CODE: &str = "otel.status_code";
    pub(crate) const STATUS_DESCRIPTION: &str = "otel.status_description";
}

/// Opens a `tracing` span whose OpenTelemetry name is `$otel_name`, under `$parent` or else the
/// current span. `tracing` field names are static, so every span declares the keys it may record.
macro_rules! agent_span {
    ($parent:expr, $name:literal, $otel_name:expr) => {
        agent_span!(@open $parent, $name, $otel_name,
            otel.status_code, otel.status_description,
            gen_ai.event.start_time, gen_ai.event.end_time, gen_ai.operation.name, gen_ai.system,
            gen_ai.provider.name, gen_ai.agent.name, gen_ai.agent.id, gen_ai.request.model,
            gen_ai.agent.tools, gen_ai.tool.definitions, system_prompt, agent_loop.cycle_id,
            gen_ai.tool.name, gen_ai.tool.call.id, gen_ai.tool.status, gen_ai.tool.call.arguments,
            gen_ai.tool.call.result, gen_ai.usage.prompt_tokens, gen_ai.usage.input_tokens,
            gen_ai.usage.completion_tokens, gen_ai.usage.output_tokens, gen_ai.usage.total_tokens,
            gen_ai.usage.cache_read.input_tokens, gen_ai.usage.cache_read_input_tokens,
            gen_ai.usage.cache_creation.input_tokens, gen_ai.usage.cache_write_input_tokens,
            gen_ai.server.request.duration, langfuse.observation.type)
    };
    (@open $parent:expr, $name:literal, $otel_name:expr, $($($field:ident).+),+) => {
        match $parent {
            Some(parent) => tracing::info_span!(
                parent: parent, $name, otel.name = $otel_name, $($($field).+ = tracing::field::Empty),+
            ),
            None => tracing::info_span!($name, otel.name = $otel_name, $($($field).+ = tracing::field::Empty),+),
        }
    };
}

/// Dynamic attributes and span events, which exist only on OpenTelemetry spans.
#[cfg(feature = "otel")]
mod otel_span {
    use opentelemetry::{Array, KeyValue, Value};
    use tracing::Span;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    use super::AttributeValue;

    pub(super) fn value(value: &AttributeValue) -> Value {
        match value.clone() {
            AttributeValue::Str(value) => value.into(),
            AttributeValue::Bool(value) => value.into(),
            AttributeValue::Int(value) => value.into(),
            AttributeValue::Float(value) => value.into(),
            AttributeValue::StringArray(values) => {
                Value::Array(Array::String(values.into_iter().map(Into::into).collect()))
            }
            AttributeValue::BoolArray(values) => Value::Array(Array::Bool(values)),
            AttributeValue::IntArray(values) => Value::Array(Array::I64(values)),
            AttributeValue::FloatArray(values) => Value::Array(Array::F64(values)),
        }
    }

    pub(super) fn set_attribute(span: &Span, key: &str, attribute: &AttributeValue) {
        span.set_attribute(key.to_owned(), value(attribute));
    }

    pub(super) fn add_event(span: &Span, name: &'static str, attributes: Vec<(&'static str, String)>) {
        let attributes = attributes
            .into_iter()
            .map(|(key, value)| KeyValue::new(key, value))
            .collect();
        span.add_event(name, attributes);
    }
}

#[cfg(not(feature = "otel"))]
mod otel_span {
    use tracing::Span;

    use super::AttributeValue;

    pub(super) fn set_attribute(_: &Span, _: &str, _: &AttributeValue) {}

    pub(super) fn add_event(_: &Span, _: &'static str, _: Vec<(&'static str, String)>) {}
}

/// OpenTelemetry integers are signed.
fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn now_iso() -> String {
    iso_timestamp(jiff::Timestamp::now())
}

/// What `start_agent` records (TS `StartAgentSpanOptions`).
pub(crate) struct AgentSpanStart<'a> {
    pub(crate) messages: &'a [Message],
    pub(crate) agent_name: &'a str,
    pub(crate) agent_id: &'a str,
    pub(crate) model_id: Option<String>,
    pub(crate) tool_names: Vec<String>,
    /// Tool specs by name, recorded under `gen_ai_tool_definitions`.
    pub(crate) tools_config: Option<JsonValue>,
    pub(crate) system_prompt: Option<&'a SystemPrompt>,
}

/// What `end_model` records (TS `EndModelSpanOptions`).
#[derive(Default)]
pub(crate) struct ModelSpanEnd<'a> {
    pub(crate) output: Option<&'a Message>,
    pub(crate) stop_reason: Option<&'a str>,
    pub(crate) usage: Option<&'a Usage>,
    pub(crate) metrics: Option<&'a Metrics>,
    pub(crate) error: Option<String>,
}

impl ModelSpanEnd<'_> {
    pub(crate) fn failed(error: &dyn std::fmt::Display) -> Self {
        Self {
            error: Some(error.to_string()),
            ..Self::default()
        }
    }
}

/// The agent's tracer (TS `Tracer`): `tracing` spans with the TS names and attributes, and the local
/// trace tree of the current invocation.
#[derive(Debug)]
pub(crate) struct LocalTracer {
    semconv: Semconv,
    trace_attributes: Vec<(String, AttributeValue)>,
    agent_span: Option<Span>,
    loop_span: Option<Span>,
    model_span: Option<Span>,
    traces: Vec<AgentTrace>,
    cycle_open: bool,
    model_open: bool,
}

impl Default for LocalTracer {
    fn default() -> Self {
        Self::new(Semconv::from_env(&process_env))
    }
}

impl LocalTracer {
    pub(crate) fn new(semconv: Semconv) -> Self {
        Self {
            semconv,
            trace_attributes: Vec::new(),
            agent_span: None,
            loop_span: None,
            model_span: None,
            traces: Vec::new(),
            cycle_open: false,
            model_open: false,
        }
    }

    #[cfg(all(test, feature = "otel"))]
    pub(crate) fn set_semconv(&mut self, semconv: Semconv) {
        self.semconv = semconv;
    }

    /// The agent's `trace_attributes`, merged into every span.
    pub(crate) fn set_trace_attributes(&mut self, attributes: Vec<(String, AttributeValue)>) {
        self.trace_attributes = attributes;
    }

    pub(crate) fn traces(&self) -> &[AgentTrace] {
        &self.traces
    }

    /// TS `startAgentSpan` resets the local traces for each invocation.
    pub(crate) fn reset(&mut self) {
        self.traces.clear();
        self.cycle_open = false;
        self.model_open = false;
    }

    /// TS `startAgentSpan`: `invoke_agent <name>` under the current span.
    pub(crate) fn start_agent(&mut self, start: &AgentSpanStart<'_>) {
        self.reset();
        let name = format!("invoke_agent {}", start.agent_name);
        let span = agent_span!(None::<&Span>, "invoke_agent", name.as_str());
        if !span.is_disabled() {
            self.record_common(&span, "invoke_agent");
            span.record(keys::AGENT_NAME, start.agent_name);
            otel_span::set_attribute(&span, keys::SPAN_NAME, &name.as_str().into());
            if !start.agent_id.is_empty() {
                span.record(keys::AGENT_ID, start.agent_id);
            }
            if let Some(model_id) = start.model_id.as_deref().filter(|id| !id.is_empty()) {
                span.record(keys::REQUEST_MODEL, model_id);
            }
            if !start.tool_names.is_empty() {
                span.record(keys::AGENT_TOOLS, to_json(&start.tool_names).as_str());
            }
            if let Some(tools_config) = start
                .tools_config
                .as_ref()
                .filter(|_| self.semconv.tool_definitions)
            {
                span.record(keys::TOOL_DEFINITIONS, to_json(tools_config).as_str());
            }
            if let Some(system_prompt) = start.system_prompt {
                span.record(keys::SYSTEM_PROMPT, to_json(system_prompt).as_str());
            }
            self.record_trace_attributes(&span);
            self.add_message_events(&span, start.messages);
        }
        self.agent_span = Some(span);
    }

    /// TS `endAgentSpan`.
    pub(crate) fn end_agent(
        &mut self,
        response: Option<&Message>,
        stop_reason: Option<&str>,
        usage: &Usage,
        error: Option<&str>,
    ) {
        self.loop_span = None;
        self.model_span = None;
        self.cycle_open = false;
        self.model_open = false;
        let Some(span) = self.agent_span.take() else {
            return;
        };
        if span.is_disabled() {
            return;
        }
        if let Some(response) = response {
            self.add_response_event(&span, response, stop_reason);
        }
        self.record_usage(&span, usage);
        if self.semconv.langfuse {
            span.record(keys::LANGFUSE_OBSERVATION_TYPE, "span");
        }
        end_span(&span, error);
    }

    /// TS `startAgentLoopSpan`: `execute_agent_loop_cycle` under the agent span.
    pub(crate) fn start_cycle(&mut self, cycle_id: &str, messages: &[Message]) {
        let mut trace = AgentTrace::new(format!("Cycle {}", self.traces.len().saturating_add(1)), None);
        trace.metadata.insert("cycleId".to_owned(), cycle_id.to_owned());
        self.traces.push(trace);
        self.cycle_open = true;
        self.model_open = false;

        let span = agent_span!(
            self.agent_span.as_ref(),
            "execute_agent_loop_cycle",
            "execute_agent_loop_cycle"
        );
        if !span.is_disabled() {
            span.record(keys::CYCLE_ID, cycle_id);
            self.record_trace_attributes(&span);
            span.record(keys::START_TIME, now_iso().as_str());
            self.add_message_events(&span, messages);
        }
        self.loop_span = Some(span);
    }

    /// TS `endAgentLoopSpan`.
    pub(crate) fn end_cycle(&mut self, error: Option<&str>) {
        if let Some(cycle) = self.open_cycle() {
            cycle.end();
        }
        self.cycle_open = false;
        if let Some(span) = self.loop_span.take() {
            end_span(&span, error);
        }
    }

    /// TS `startModelInvokeSpan`: `chat` under the cycle span. The local trace is recorded only
    /// inside a cycle, as in TS.
    pub(crate) fn start_model(
        &mut self,
        messages: &[Message],
        model_id: Option<&str>,
        system_prompt: Option<&SystemPrompt>,
    ) {
        if let Some(cycle) = self.open_cycle() {
            let model = cycle.child("stream_messages");
            cycle.children.push(model);
            self.model_open = true;
        }
        let span = agent_span!(self.loop_span.as_ref(), "chat", "chat");
        if !span.is_disabled() {
            self.record_common(&span, "chat");
            if let Some(model_id) = model_id.filter(|id| !id.is_empty()) {
                span.record(keys::REQUEST_MODEL, model_id);
            }
            self.record_trace_attributes(&span);
            self.add_system_prompt_event(&span, system_prompt);
            self.add_message_events(&span, messages);
        }
        self.model_span = Some(span);
    }

    /// TS `endModelInvokeSpan`.
    pub(crate) fn end_model(&mut self, end: &ModelSpanEnd<'_>) {
        if std::mem::take(&mut self.model_open) {
            let model = self.open_cycle().and_then(|cycle| {
                cycle
                    .children
                    .iter_mut()
                    .rev()
                    .find(|child| child.name == "stream_messages")
            });
            if let Some(model) = model {
                model.message = end.output.cloned();
                model.end();
            }
        }
        let Some(span) = self.model_span.take() else {
            return;
        };
        if span.is_disabled() {
            return;
        }
        if let Some(output) = end.output {
            self.add_output_event(&span, output, end.stop_reason);
        }
        if let Some(usage) = end.usage {
            self.record_usage(&span, usage);
            if let Some(latency_ms) = end.metrics.map(Metrics::latency_ms).filter(|ms| *ms > 0) {
                span.record(keys::SERVER_REQUEST_DURATION, int(latency_ms));
            }
        }
        end_span(&span, end.error.as_deref());
    }

    /// TS `startToolCallSpan`: `execute_tool <name>` under the cycle span. The caller runs the tool
    /// inside the returned span, so nested agents and outgoing MCP calls parent under it.
    pub(crate) fn start_tool(&mut self, tool_use: &ToolUse) -> Span {
        let (tool_name, tool_use_id) = (tool_use.name(), tool_use.tool_use_id());
        if let Some(cycle) = self.open_cycle() {
            let mut tool = cycle.child(format!("Tool: {tool_name}"));
            tool.metadata
                .insert("toolUseId".to_owned(), tool_use_id.to_owned());
            tool.metadata.insert("toolName".to_owned(), tool_name.to_owned());
            cycle.children.push(tool);
        }
        let name = format!("execute_tool {tool_name}");
        let span = agent_span!(self.loop_span.as_ref(), "execute_tool", name.as_str());
        if span.is_disabled() {
            return span;
        }
        self.record_common(&span, "execute_tool");
        span.record(keys::TOOL_NAME, tool_name);
        span.record(keys::TOOL_CALL_ID, tool_use_id);
        self.record_trace_attributes(&span);
        if self.semconv.latest {
            span.record(keys::TOOL_CALL_ARGUMENTS, to_json(tool_use.input()).as_str());
            let messages = json!([{
                "role": "tool",
                "parts": [{"type": "tool_call", "name": tool_name, "id": tool_use_id, "arguments": tool_use.input()}],
            }]);
            self.add_details_event(&span, keys::INPUT_MESSAGES, to_json(&messages));
        } else if content_enabled() {
            otel_span::add_event(
                &span,
                "gen_ai.tool.message",
                vec![
                    ("role", "tool".to_owned()),
                    ("content", to_json(tool_use.input())),
                    ("id", tool_use_id.to_owned()),
                ],
            );
        }
        span
    }

    /// TS `endToolCallSpan`.
    pub(crate) fn end_tool(&mut self, tool_use_id: &str, span: &Span, result: Option<&ToolResultBlock>) {
        let tool = self.open_cycle().and_then(|cycle| {
            cycle.children.iter_mut().rev().find(|child| {
                child.end_time.is_none() && child.metadata_value("toolUseId") == Some(tool_use_id)
            })
        });
        if let Some(tool) = tool {
            tool.end();
        }
        if span.is_disabled() {
            return;
        }
        if let Some(result) = result {
            let status = result.status().as_str();
            span.record(keys::TOOL_STATUS, status);
            if self.semconv.latest {
                if status != "error" {
                    span.record(keys::TOOL_CALL_RESULT, to_json(result.content()).as_str());
                }
                let messages = json!([{
                    "role": "tool",
                    "parts": [{
                        "type": "tool_call_response",
                        "id": result.tool_use_id(),
                        "response": result.content(),
                    }],
                }]);
                self.add_details_event(span, keys::OUTPUT_MESSAGES, to_json(&messages));
            } else if content_enabled() {
                otel_span::add_event(
                    span,
                    "gen_ai.choice",
                    vec![
                        ("message", to_json(result.content())),
                        ("id", result.tool_use_id().to_owned()),
                    ],
                );
            }
        }
        let error = result.and_then(ToolResultBlock::error).map(ToString::to_string);
        end_span(span, error.as_deref());
    }

    fn open_cycle(&mut self) -> Option<&mut AgentTrace> {
        if self.cycle_open {
            self.traces.last_mut()
        } else {
            None
        }
    }

    /// TS `_getCommonAttributes`, then the start time `_startSpan` stamps.
    fn record_common(&self, span: &Span, operation: &str) {
        span.record(keys::OPERATION_NAME, operation);
        let provider_key = if self.semconv.latest {
            keys::PROVIDER_NAME
        } else {
            keys::SYSTEM
        };
        span.record(provider_key, self.semconv.service_name.as_str());
        span.record(keys::START_TIME, now_iso().as_str());
    }

    fn record_trace_attributes(&self, span: &Span) {
        for (key, value) in &self.trace_attributes {
            otel_span::set_attribute(span, key, value);
        }
    }

    /// TS `_setUsageAttributes`: the current keys with the legacy ones beside them.
    fn record_usage(&self, span: &Span, usage: &Usage) {
        let prompt_tokens = usage.total_prompt_tokens();
        span.record(keys::USAGE_PROMPT_TOKENS, int(prompt_tokens));
        span.record(keys::USAGE_INPUT_TOKENS, int(prompt_tokens));
        span.record(keys::USAGE_COMPLETION_TOKENS, int(usage.output_tokens()));
        span.record(keys::USAGE_OUTPUT_TOKENS, int(usage.output_tokens()));
        span.record(keys::USAGE_TOTAL_TOKENS, int(usage.total_tokens()));
        if let Some(tokens) = usage.cache_read_input_tokens().filter(|tokens| *tokens > 0) {
            span.record(keys::USAGE_CACHE_READ, int(tokens));
            if !self.semconv.latest {
                span.record(keys::USAGE_CACHE_READ_LEGACY, int(tokens));
            }
        }
        if let Some(tokens) = usage.cache_write_input_tokens().filter(|tokens| *tokens > 0) {
            span.record(keys::USAGE_CACHE_CREATION, int(tokens));
            if !self.semconv.latest {
                span.record(keys::USAGE_CACHE_WRITE_LEGACY, int(tokens));
            }
        }
    }

    /// A latest-convention `gen_ai.client.inference.operation.details` event, or the same key as a
    /// span attribute under Langfuse and `gen_ai_span_attributes_only`.
    fn add_details_event(&self, span: &Span, key: &'static str, value: String) {
        if self.semconv.span_attributes_only {
            otel_span::set_attribute(span, key, &value.into());
        } else {
            otel_span::add_event(span, keys::OPERATION_DETAILS_EVENT, vec![(key, value)]);
        }
    }

    /// TS `_addEventMessages`.
    fn add_message_events(&self, span: &Span, messages: &[Message]) {
        if !content_enabled() {
            return;
        }
        if self.semconv.latest {
            let input: Vec<JsonValue> = messages
                .iter()
                .map(|message| json!({"role": message.role(), "parts": otel_parts(message.content())}))
                .collect();
            self.add_details_event(span, keys::INPUT_MESSAGES, to_json(&input));
            return;
        }
        for message in messages {
            let has_tool_result = message
                .content()
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult(_)));
            let name = match message.role() {
                Role::User if has_tool_result => "gen_ai.tool.message",
                Role::User => "gen_ai.user.message",
                Role::Assistant => "gen_ai.assistant.message",
            };
            otel_span::add_event(span, name, vec![("content", to_json(message.content()))]);
        }
    }

    /// TS `_addSystemPromptEvent`.
    fn add_system_prompt_event(&self, span: &Span, system_prompt: Option<&SystemPrompt>) {
        let Some(system_prompt) = system_prompt.filter(|_| content_enabled()) else {
            return;
        };
        if self.semconv.latest {
            let parts = match system_prompt {
                SystemPrompt::Text(text) => json!([{"type": "text", "content": text}]),
                SystemPrompt::Blocks(blocks) => blocks.iter().map(system_part).collect(),
            };
            self.add_details_event(span, keys::SYSTEM_INSTRUCTIONS, to_json(&parts));
        } else {
            let content = match system_prompt {
                SystemPrompt::Text(text) => to_json(&json!([{"text": text}])),
                SystemPrompt::Blocks(blocks) => to_json(blocks),
            };
            otel_span::add_event(span, "gen_ai.system.message", vec![("content", content)]);
        }
    }

    /// TS `_addResponseEvent`: the agent's final text.
    fn add_response_event(&self, span: &Span, response: &Message, stop_reason: Option<&str>) {
        if !content_enabled() {
            return;
        }
        let finish_reason = stop_reason
            .filter(|reason| !reason.is_empty())
            .unwrap_or("end_turn");
        let text = response
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if self.semconv.latest {
            let messages = json!([{
                "role": "assistant",
                "parts": [{"type": "text", "content": text}],
                "finish_reason": finish_reason,
            }]);
            self.add_details_event(span, keys::OUTPUT_MESSAGES, to_json(&messages));
        } else {
            otel_span::add_event(
                span,
                "gen_ai.choice",
                vec![("message", text), ("finish_reason", finish_reason.to_owned())],
            );
        }
    }

    /// TS `_addOutputEvent`: the model's output message.
    fn add_output_event(&self, span: &Span, message: &Message, stop_reason: Option<&str>) {
        if !content_enabled() {
            return;
        }
        let finish_reason = stop_reason
            .filter(|reason| !reason.is_empty())
            .unwrap_or("unknown");
        if self.semconv.latest {
            let messages = json!([{
                "role": message.role(),
                "parts": otel_parts(message.content()),
                "finish_reason": finish_reason,
            }]);
            self.add_details_event(span, keys::OUTPUT_MESSAGES, to_json(&messages));
        } else {
            otel_span::add_event(
                span,
                "gen_ai.choice",
                vec![
                    ("finish_reason", finish_reason.to_owned()),
                    ("message", to_json(&stable_format(message.content()))),
                ],
            );
        }
    }
}

/// Message content is recorded on OpenTelemetry spans only, so it isn't serialized without `otel`.
fn content_enabled() -> bool {
    cfg!(feature = "otel")
}

/// TS `_endSpan`: the end time, then the status (and the exception event on an error).
fn end_span(span: &Span, error: Option<&str>) {
    if span.is_disabled() {
        return;
    }
    span.record(keys::END_TIME, now_iso().as_str());
    match error {
        Some(message) => {
            span.record(keys::STATUS_DESCRIPTION, message);
            otel_span::add_event(span, "exception", vec![("exception.message", message.to_owned())]);
        }
        None => {
            span.record(keys::STATUS_CODE, "OK");
        }
    }
}

/// The wire key of a block with `Block` appended, which is its TS `type`.
fn block_type<T: Serialize>(block: &T) -> String {
    let key = match serde_json::to_value(block) {
        Ok(JsonValue::Object(map)) => map.keys().next().cloned(),
        _ => None,
    };
    format!("{}Block", key.unwrap_or_default())
}

/// TS `_mapContentBlocksToOtelParts`.
fn otel_parts(blocks: &[ContentBlock]) -> Vec<JsonValue> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => json!({"type": "text", "content": text.text()}),
            ContentBlock::Injected(injected) => json!({"type": "text", "content": injected.text()}),
            ContentBlock::Summary(summary) => json!({"type": "text", "content": summary.text()}),
            ContentBlock::ToolUse(tool_use) => json!({
                "type": "tool_call",
                "name": tool_use.name(),
                "id": tool_use.tool_use_id(),
                "arguments": tool_use.input(),
            }),
            ContentBlock::ToolResult(result) => json!({
                "type": "tool_call_response",
                "id": result.tool_use_id(),
                "response": result.content(),
            }),
            ContentBlock::Unknown(_) => json!({"type": "unknownBlock"}),
            other => json!({"type": block_type(other)}),
        })
        .collect()
}

/// TS `_mapSystemPromptToOtelParts`.
fn system_part(block: &SystemContentBlock) -> JsonValue {
    match block {
        SystemContentBlock::Text(text) => json!({"type": "text", "content": text.text()}),
        SystemContentBlock::CachePoint(cache_point) => {
            json!({"type": "cache_point", "cacheType": cache_point.cache_type()})
        }
        SystemContentBlock::GuardContent(guard) => {
            let mut part = json!({"type": "guard_content"});
            if let Some(text) = guard.text() {
                part["text"] = json!(text);
            }
            if let Some(image) = guard.image() {
                // TS holds the raw bytes here, which `jsonReplacer` writes as the placeholder.
                part["image"] = json!({"format": image.format(), "source": {"bytes": "<replaced>"}});
            }
            part
        }
    }
}

/// TS `_mapContentBlocksToStableFormat`.
fn stable_format(blocks: &[ContentBlock]) -> Vec<JsonValue> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(json!({"text": text.text()})),
            ContentBlock::ToolUse(tool_use) => Some(json!({
                "type": "toolUse",
                "name": tool_use.name(),
                "toolUseId": tool_use.tool_use_id(),
                "input": tool_use.input(),
            })),
            ContentBlock::ToolResult(result) => Some(json!({
                "type": "toolResult",
                "toolUseId": result.tool_use_id(),
                "content": result.content(),
            })),
            _ => None,
        })
        .collect()
}

pub(crate) mod memory;

#[cfg(test)]
mod local_trace_tests;
#[cfg(test)]
mod parts_tests;
