//! OTel spans for the agent loop (`strands-ts/src/telemetry/tracer.ts`).

use parking_lot::Mutex;
use std::sync::Arc;

use super::json::{object, to_telemetry_json};
use super::types::{now_ms, AgentTrace};
use super::{AttrValue, TelemetryConfig};
use crate::types::{Message, Metrics, Role, StopReason, ToolResultBlock, ToolSpec, ToolUse, Usage};

type Attrs = Vec<(String, AttrValue)>;

fn attr(attrs: &mut Attrs, key: &str, value: impl Into<AttrValue>) {
    attrs.push((key.to_string(), value.into()));
}

/// A started span (or a no-op placeholder). Every `end_*` tolerates an unstarted span.
#[derive(Debug, Default)]
pub struct SpanHandle {
    inner: backend::RawSpan,
    local_trace: Option<String>,
    start_time: u64,
}

impl SpanHandle {
    /// The OTel context carrying this span, for explicit propagation into tool futures.
    #[cfg(feature = "telemetry")]
    pub fn context(&self) -> Option<&opentelemetry::Context> {
        self.inner.as_ref()
    }

    /// Milliseconds since the Unix epoch when the span started.
    pub fn start_time(&self) -> u64 {
        self.start_time
    }
}

/// Where the agent span sits in the local trace tree.
#[derive(Default)]
struct TraceState {
    traces: Vec<AgentTrace>,
    current_cycle: Option<String>,
}

/// Options for [`Tracer::start_agent_span`].
#[derive(Default)]
pub struct AgentSpanOptions<'a> {
    /// Agent name, part of the span name.
    pub agent_name: &'a str,
    /// Agent id.
    pub agent_id: Option<&'a str>,
    /// Model id (`gen_ai.request.model`).
    pub model_id: Option<&'a str>,
    /// Provider name (`gen_ai.system` / `gen_ai.provider.name`).
    pub provider: Option<&'a str>,
    /// Tool names (`gen_ai.agent.tools`).
    pub tools: &'a [String],
    /// Full tool schemas (`gen_ai.tool.definitions`, opt-in).
    pub tool_specs: &'a [ToolSpec],
    /// System prompt, recorded as an attribute and an event.
    pub system_prompt: Option<&'a str>,
    /// Input messages, recorded as events.
    pub messages: &'a [Message],
}

/// Options for [`Tracer::start_model_invoke_span`].
#[derive(Default)]
pub struct ModelSpanOptions<'a> {
    /// Model id (`gen_ai.request.model`).
    pub model_id: Option<&'a str>,
    /// Provider name.
    pub provider: Option<&'a str>,
    /// Messages sent to the model, recorded as events.
    pub messages: &'a [Message],
}

/// Creates and ends the `gen_ai.*` spans, and collects local [`AgentTrace`]s.
#[derive(Clone, Default)]
pub struct Tracer {
    config: TelemetryConfig,
    state: Arc<Mutex<TraceState>>,
    inner: backend::Backend,
}

impl std::fmt::Debug for Tracer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracer").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Tracer {
    /// A tracer using the global OTel tracer provider (a no-op provider by default).
    pub fn new(config: TelemetryConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(TraceState::default())),
            inner: backend::Backend::new(),
        }
    }

    /// This tracer's configuration.
    pub fn config(&self) -> &TelemetryConfig {
        &self.config
    }

    /// Local traces collected so far (TS `get localTraces`).
    pub fn local_traces(&self) -> Vec<AgentTrace> {
        self.state.lock().traces.clone()
    }

    /// `invoke_agent <name>` — the root span of an invocation.
    pub fn start_agent_span(&self, options: AgentSpanOptions<'_>) -> SpanHandle {
        let name = format!("invoke_agent {}", options.agent_name);
        let mut attrs = self.common_attributes(Some("invoke_agent"), options.provider);
        attr(&mut attrs, "gen_ai.agent.name", options.agent_name);
        attr(&mut attrs, "name", name.as_str());
        if let Some(id) = options.agent_id {
            attr(&mut attrs, "gen_ai.agent.id", id);
        }
        if let Some(model) = options.model_id {
            attr(&mut attrs, "gen_ai.request.model", model);
        }
        if !options.tools.is_empty() {
            attr(&mut attrs, "gen_ai.agent.tools", to_telemetry_json(options.tools));
        }
        if self.config.include_tool_definitions && !options.tool_specs.is_empty() {
            attr(&mut attrs, "gen_ai.tool.definitions", to_telemetry_json(options.tool_specs));
        }
        if let Some(prompt) = options.system_prompt {
            attr(&mut attrs, "system_prompt", to_telemetry_json(prompt));
        }
        if self.config.is_langfuse {
            attr(&mut attrs, "langfuse.observation.type", "span");
        }
        self.apply_custom(&mut attrs);

        {
            let mut state = self.state.lock();
            state.current_cycle = None;
        }
        let mut span = self.begin(&name, backend::Kind::Internal, attrs, None);
        span.local_trace = Some(self.push_trace(AgentTrace::new(name, None), None));
        self.record_messages(&span, options.messages, options.system_prompt);
        span
    }

    /// Close the agent span.
    pub fn end_agent_span(&self, span: &SpanHandle, message: Option<&Message>, error: Option<&str>) {
        if let Some(message) = message {
            self.add_choice_event(span, message, "end_turn");
        }
        self.end(span, error);
    }

    /// `execute_agent_loop_cycle` — one model↔tool cycle, parented to the agent span.
    pub fn start_agent_loop_span(&self, parent: &SpanHandle, cycle_id: &str) -> SpanHandle {
        let mut attrs = self.common_attributes(None, None);
        attr(&mut attrs, "agent_loop.cycle_id", cycle_id);
        self.apply_custom(&mut attrs);
        let mut span = self.begin("execute_agent_loop_cycle", backend::Kind::Default, attrs, Some(parent));
        let trace = AgentTrace::new("Cycle", parent.local_trace.clone());
        let id = self.push_trace(trace, parent.local_trace.as_deref());
        self.state.lock().current_cycle = Some(id.clone());
        span.local_trace = Some(id);
        span
    }

    /// Close the cycle span.
    pub fn end_agent_loop_span(&self, span: &SpanHandle, error: Option<&str>) {
        self.end(span, error);
        self.state.lock().current_cycle = None;
    }

    /// `chat` — one model invocation, parented to the cycle span.
    pub fn start_model_invoke_span(&self, parent: &SpanHandle, options: ModelSpanOptions<'_>) -> SpanHandle {
        let mut attrs = self.common_attributes(Some("chat"), options.provider);
        if let Some(model) = options.model_id {
            attr(&mut attrs, "gen_ai.request.model", model);
        }
        self.apply_custom(&mut attrs);
        let mut span = self.begin("chat", backend::Kind::Default, attrs, Some(parent));
        span.local_trace = Some(self.push_child_trace("Model invoke", parent));
        self.record_messages(&span, options.messages, None);
        span
    }

    /// Close the model span, recording usage and latency.
    pub fn end_model_invoke_span(
        &self,
        span: &SpanHandle,
        message: Option<&Message>,
        usage: Option<&Usage>,
        metrics: Option<&Metrics>,
        stop_reason: Option<&StopReason>,
        error: Option<&str>,
    ) {
        let mut attrs = Attrs::new();
        if let Some(usage) = usage {
            attr(&mut attrs, "gen_ai.usage.prompt_tokens", usage.input_tokens);
            attr(&mut attrs, "gen_ai.usage.input_tokens", usage.input_tokens);
            attr(&mut attrs, "gen_ai.usage.completion_tokens", usage.output_tokens);
            attr(&mut attrs, "gen_ai.usage.output_tokens", usage.output_tokens);
            attr(&mut attrs, "gen_ai.usage.total_tokens", usage.total_tokens);
            if let Some(read) = usage.cache_read_input_tokens.filter(|v| *v > 0) {
                attr(&mut attrs, "gen_ai.usage.cache_read.input_tokens", read);
                if !self.config.use_latest_conventions {
                    attr(&mut attrs, "gen_ai.usage.cache_read_input_tokens", read);
                }
            }
            if let Some(write) = usage.cache_write_input_tokens.filter(|v| *v > 0) {
                attr(&mut attrs, "gen_ai.usage.cache_creation.input_tokens", write);
                if !self.config.use_latest_conventions {
                    attr(&mut attrs, "gen_ai.usage.cache_write_input_tokens", write);
                }
            }
        }
        if let Some(metrics) = metrics {
            attr(&mut attrs, "gen_ai.server.request.duration", metrics.latency_ms);
        }
        backend::set_attributes(&span.inner, &attrs);
        if let Some(message) = message {
            let finish = stop_reason.map(StopReason::as_str).unwrap_or("end_turn");
            self.add_choice_event(span, message, finish);
        }
        self.end(span, error);
    }

    /// `execute_tool <name>` — one tool call, parented to the cycle span.
    pub fn start_tool_call_span(&self, parent: &SpanHandle, tool_use: &ToolUse) -> SpanHandle {
        let name = format!("execute_tool {}", tool_use.name);
        let mut attrs = self.common_attributes(Some("execute_tool"), None);
        attr(&mut attrs, "gen_ai.tool.name", tool_use.name.as_str());
        attr(&mut attrs, "gen_ai.tool.call.id", tool_use.tool_use_id.as_str());
        if self.config.include_tool_definitions {
            attr(&mut attrs, "gen_ai.tool.call.arguments", to_telemetry_json(&tool_use.input));
        }
        self.apply_custom(&mut attrs);
        let mut span = self.begin(&name, backend::Kind::Default, attrs, Some(parent));
        span.local_trace = Some(self.push_child_trace(format!("Tool: {}", tool_use.name), parent));
        span
    }

    /// Close the tool span, recording status and (opt-in) the result payload.
    pub fn end_tool_call_span(&self, span: &SpanHandle, result: Option<&ToolResultBlock>, error: Option<&str>) {
        let mut attrs = Attrs::new();
        if let Some(result) = result {
            let status = tool_status(&result.status).to_string();
            attr(&mut attrs, "gen_ai.tool.status", status);
            if self.config.include_tool_definitions {
                attr(&mut attrs, "gen_ai.tool.call.result", to_telemetry_json(&result.content));
            }
        }
        backend::set_attributes(&span.inner, &attrs);
        if let Some(result) = result {
            let payload = object(vec![
                ("message", serde_json::json!({ "content": result.content })),
                ("id", serde_json::json!(result.tool_use_id)),
                ("finish_reason", serde_json::json!(tool_status(&result.status))),
            ]);
            self.add_event(span, "gen_ai.choice", vec![("content".to_string(), payload)]);
        }
        self.end(span, error);
    }

    fn common_attributes(&self, operation: Option<&str>, provider: Option<&str>) -> Attrs {
        let mut attrs = Attrs::new();
        if let Some(operation) = operation {
            attr(&mut attrs, "gen_ai.operation.name", operation);
        }
        if let Some(provider) = provider {
            attr(&mut attrs, self.config.provider_key(), provider);
        }
        attr(&mut attrs, "gen_ai.event.start_time", iso_now());
        attrs
    }

    fn apply_custom(&self, attrs: &mut Attrs) {
        for (key, value) in &self.config.trace_attributes {
            attrs.retain(|(existing, _)| existing != key);
            attrs.push((key.clone(), value.clone()));
        }
    }

    fn begin(&self, name: &str, kind: backend::Kind, attrs: Attrs, parent: Option<&SpanHandle>) -> SpanHandle {
        SpanHandle {
            inner: self.inner.start(name, kind, &attrs, parent.map(|p| &p.inner)),
            local_trace: None,
            start_time: now_ms(),
        }
    }

    fn end(&self, span: &SpanHandle, error: Option<&str>) {
        let attrs = vec![("gen_ai.event.end_time".to_string(), AttrValue::Str(iso_now()))];
        backend::set_attributes(&span.inner, &attrs);
        backend::finish(&span.inner, error);
        if let Some(id) = &span.local_trace {
            self.end_trace(id);
        }
    }

    fn record_messages(&self, span: &SpanHandle, messages: &[Message], system_prompt: Option<&str>) {
        if self.config.use_latest_conventions {
            let mut entries = vec![("gen_ai.input.messages", serde_json::json!(messages))];
            if let Some(prompt) = system_prompt {
                entries.push(("gen_ai.system_instructions", serde_json::json!(prompt)));
            }
            let payload = object(entries);
            let attrs = match payload {
                serde_json::Value::Object(map) => map.into_iter().collect(),
                other => vec![("gen_ai.input.messages".to_string(), other)],
            };
            self.add_event(span, "gen_ai.client.inference.operation.details", attrs);
            return;
        }
        if let Some(prompt) = system_prompt {
            self.add_event(
                span,
                "gen_ai.system.message",
                vec![("content".to_string(), serde_json::json!(prompt))],
            );
        }
        for message in messages {
            self.add_event(
                span,
                event_name_for_message(message),
                vec![("content".to_string(), serde_json::json!(message.content))],
            );
        }
    }

    fn add_choice_event(&self, span: &SpanHandle, message: &Message, finish_reason: &str) {
        let payload = object(vec![
            ("message", serde_json::json!({ "content": message.content })),
            ("finish_reason", serde_json::json!(finish_reason)),
        ]);
        self.add_event(span, "gen_ai.choice", vec![("content".to_string(), payload)]);
    }

    fn add_event(&self, span: &SpanHandle, name: &str, payload: Vec<(String, serde_json::Value)>) {
        let attrs: Attrs = payload
            .into_iter()
            .map(|(key, value)| (key, AttrValue::Str(to_telemetry_json(&value))))
            .collect();
        if self.config.events_as_attributes() {
            let prefixed: Attrs = attrs
                .into_iter()
                .map(|(key, value)| (format!("{name}.{key}"), value))
                .collect();
            backend::set_attributes(&span.inner, &prefixed);
        } else {
            backend::add_event(&span.inner, name, &attrs);
        }
    }

    fn push_trace(&self, trace: AgentTrace, parent: Option<&str>) -> String {
        let id = trace.id.clone();
        let mut state = self.state.lock();
        match parent.and_then(|p| find_trace(&mut state.traces, p)) {
            Some(node) => node.children.push(trace),
            None => state.traces.push(trace),
        }
        id
    }

    fn push_child_trace(&self, name: impl Into<String>, parent: &SpanHandle) -> String {
        let cycle = self.state.lock().current_cycle.clone();
        let parent_id = cycle.or_else(|| parent.local_trace.clone());
        let trace = AgentTrace::new(name, parent_id.clone());
        self.push_trace(trace, parent_id.as_deref())
    }

    fn end_trace(&self, id: &str) {
        let mut state = self.state.lock();
        if let Some(trace) = find_trace(&mut state.traces, id) {
            trace.end(None);
        }
    }
}

fn find_trace<'a>(traces: &'a mut [AgentTrace], id: &str) -> Option<&'a mut AgentTrace> {
    for trace in traces.iter_mut() {
        if trace.id == id {
            return Some(trace);
        }
        let found = find_trace(&mut trace.children, id);
        if found.is_some() {
            return found;
        }
    }
    None
}

/// TS `_getEventNameForMessage`.
fn event_name_for_message(message: &Message) -> &'static str {
    let has_tool_result = message
        .content
        .iter()
        .any(|block| matches!(block, crate::types::ContentBlock::ToolResult(_)));
    match message.role {
        Role::User if has_tool_result => "gen_ai.tool.message",
        Role::User => "gen_ai.user.message",
        Role::Assistant => "gen_ai.assistant.message",
    }
}

/// TS writes `toolResult.status` verbatim (`success` / `error`).
fn tool_status(status: &crate::types::ToolResultStatus) -> &'static str {
    match status {
        crate::types::ToolResultStatus::Success => "success",
        crate::types::ToolResultStatus::Error => "error",
    }
}

fn iso_now() -> String {
    let ms = now_ms();
    let secs = (ms / 1000) as i64;
    let millis = ms % 1000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Span kinds used by this module.
#[cfg(feature = "telemetry")]
mod backend {
    use super::{AttrValue, Attrs};
    use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer as _};
    use opentelemetry::{Context, KeyValue};
    use std::sync::Arc;

    /// A started span, carried inside an OTel [`Context`] so it can be ended out of order.
    pub(super) type RawSpan = Option<Context>;

    /// Span kind for the spans this module creates.
    #[derive(Clone, Copy)]
    pub(super) enum Kind {
        /// `SpanKind::Internal`.
        Internal,
        /// The provider default.
        Default,
    }

    /// Holds the global OTel tracer.
    #[derive(Clone)]
    pub(super) struct Backend(Arc<opentelemetry::global::BoxedTracer>);

    impl Default for Backend {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Backend {
        pub(super) fn new() -> Self {
            Self(Arc::new(opentelemetry::global::tracer(super::super::service_name())))
        }

        pub(super) fn start(&self, name: &str, kind: Kind, attrs: &Attrs, parent: Option<&RawSpan>) -> RawSpan {
            let mut builder = self.0.span_builder(name.to_string()).with_attributes(key_values(attrs));
            if matches!(kind, Kind::Internal) {
                builder = builder.with_kind(SpanKind::Internal);
            }
            let parent_cx = match parent {
                Some(Some(cx)) => cx.clone(),
                _ => Context::current(),
            };
            let span = builder.start_with_context(self.0.as_ref(), &parent_cx);
            Some(parent_cx.with_span(span))
        }
    }

    pub(super) fn set_attributes(span: &RawSpan, attrs: &Attrs) {
        if let Some(cx) = span {
            for kv in key_values(attrs) {
                cx.span().set_attribute(kv);
            }
        }
    }

    pub(super) fn add_event(span: &RawSpan, name: &str, attrs: &Attrs) {
        if let Some(cx) = span {
            cx.span().add_event(name.to_string(), key_values(attrs));
        }
    }

    pub(super) fn finish(span: &RawSpan, error: Option<&str>) {
        if let Some(cx) = span {
            let span = cx.span();
            match error {
                Some(message) => {
                    span.add_event(
                        "exception",
                        vec![KeyValue::new("exception.message", message.to_string())],
                    );
                    span.set_status(Status::error(message.to_string()));
                }
                None => span.set_status(Status::Ok),
            }
            span.end();
        }
    }

    fn key_values(attrs: &Attrs) -> Vec<KeyValue> {
        attrs
            .iter()
            .map(|(key, value)| match value {
                AttrValue::Str(v) => KeyValue::new(key.clone(), v.clone()),
                AttrValue::Int(v) => KeyValue::new(key.clone(), *v),
                AttrValue::Float(v) => KeyValue::new(key.clone(), *v),
                AttrValue::Bool(v) => KeyValue::new(key.clone(), *v),
            })
            .collect()
    }
}

#[cfg(not(feature = "telemetry"))]
mod backend {
    use super::Attrs;

    /// No-op span.
    pub(super) type RawSpan = ();

    /// Span kind (ignored without the `telemetry` feature).
    #[derive(Clone, Copy)]
    pub(super) enum Kind {
        Internal,
        Default,
    }

    /// No-op backend.
    #[derive(Clone, Default)]
    pub(super) struct Backend;

    impl Backend {
        pub(super) fn new() -> Self {
            Self
        }

        pub(super) fn start(&self, _name: &str, _kind: Kind, _attrs: &Attrs, _parent: Option<&RawSpan>) -> RawSpan {}
    }

    pub(super) fn set_attributes(_span: &RawSpan, _attrs: &Attrs) {}
    pub(super) fn add_event(_span: &RawSpan, _name: &str, _attrs: &Attrs) {}
    pub(super) fn finish(_span: &RawSpan, _error: Option<&str>) {}
}
