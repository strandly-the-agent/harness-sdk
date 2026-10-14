//! OpenTelemetry tracing and metrics for the agent loop (`strands-ts/src/telemetry/*`).
//!
//! Spans and `gen_ai.*` attributes match the TypeScript SDK verbatim. Without the
//! `telemetry` feature every tracer/meter call is a no-op, so loop code never branches.

pub mod json;
pub mod meter;
pub mod tracer;
pub mod types;

pub use json::to_telemetry_json;
pub use meter::CycleHandle;
pub use meter::Meter;
pub use tracer::{SpanHandle, Tracer};
pub use types::{AgentMetrics, AgentTrace, CycleMetrics, InvocationMetrics, ToolMetrics};

/// Default OTel service name (TS `utils.ts:13`).
pub const DEFAULT_SERVICE_NAME: &str = "strands-agents";

/// Value of a telemetry attribute.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    /// String attribute.
    Str(String),
    /// Signed integer attribute.
    Int(i64),
    /// Floating point attribute.
    Float(f64),
    /// Boolean attribute.
    Bool(bool),
}

impl From<&str> for AttrValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_string())
    }
}
impl From<String> for AttrValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}
impl From<i64> for AttrValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}
impl From<u64> for AttrValue {
    fn from(v: u64) -> Self {
        Self::Int(v as i64)
    }
}
impl From<f64> for AttrValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}
impl From<bool> for AttrValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

/// Telemetry behaviour, parsed once from the environment exactly as TS does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TelemetryConfig {
    /// `gen_ai_latest_experimental`: `gen_ai.provider.name`, aggregated events, no deprecated aliases.
    pub use_latest_conventions: bool,
    /// `gen_ai_tool_definitions`: emit tool definitions, arguments and results.
    pub include_tool_definitions: bool,
    /// `gen_ai_span_attributes_only`: record message content as span attributes, never events.
    pub span_attributes_only: bool,
    /// The OTLP endpoint looks like Langfuse: events become span attributes.
    pub is_langfuse: bool,
    /// Custom attributes merged over the standard ones on every span.
    pub trace_attributes: Vec<(String, AttrValue)>,
}

impl TelemetryConfig {
    /// Config with every toggle off.
    pub fn new() -> Self {
        Self::default()
    }

    /// Config read from the environment (`OTEL_SEMCONV_STABILITY_OPT_IN`, OTLP endpoints).
    pub fn from_env() -> Self {
        let opt_in = std::env::var("OTEL_SEMCONV_STABILITY_OPT_IN").unwrap_or_default();
        let flags: Vec<String> = opt_in
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let has = |name: &str| flags.iter().any(|f| f == name);
        Self {
            use_latest_conventions: has("gen_ai_latest_experimental"),
            include_tool_definitions: has("gen_ai_tool_definitions"),
            span_attributes_only: has("gen_ai_span_attributes_only"),
            is_langfuse: detect_langfuse(),
            trace_attributes: Vec::new(),
        }
    }

    /// Replace the custom span attributes.
    #[must_use]
    pub fn with_trace_attributes(mut self, attrs: impl IntoIterator<Item = (String, AttrValue)>) -> Self {
        self.trace_attributes = attrs.into_iter().collect();
        self
    }

    /// Events are recorded as span attributes (Langfuse-style backends or explicit opt-in).
    pub(crate) fn events_as_attributes(&self) -> bool {
        self.span_attributes_only || self.is_langfuse
    }

    /// `gen_ai.provider.name` under latest conventions, `gen_ai.system` otherwise.
    pub(crate) fn provider_key(&self) -> &'static str {
        if self.use_latest_conventions {
            "gen_ai.provider.name"
        } else {
            "gen_ai.system"
        }
    }
}

/// `OTEL_SERVICE_NAME`, defaulting to `strands-agents` (TS `getServiceName`).
pub fn service_name() -> String {
    std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string())
}

fn detect_langfuse() -> bool {
    [
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok())
    .any(|v| v.to_lowercase().contains("langfuse"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_stable_conventions() {
        let config = TelemetryConfig::new();
        assert_eq!(config.provider_key(), "gen_ai.system");
        assert!(!config.events_as_attributes());
    }

    #[test]
    fn latest_conventions_rename_the_provider_key() {
        let config = TelemetryConfig {
            use_latest_conventions: true,
            ..TelemetryConfig::new()
        };
        assert_eq!(config.provider_key(), "gen_ai.provider.name");
    }

    #[test]
    fn langfuse_records_events_as_span_attributes() {
        let config = TelemetryConfig {
            is_langfuse: true,
            ..TelemetryConfig::new()
        };
        assert!(config.events_as_attributes());
    }
}

/// Keeps one `execute_agent_loop_cycle` span and its metric cycle open for a scope.
///
/// Ending on drop covers the loop's many early returns.
pub struct CycleScope {
    tracer: Tracer,
    meter: Meter,
    span: SpanHandle,
    cycle: CycleHandle,
}

impl CycleScope {
    /// Open a cycle span under `parent` and start the matching metric cycle.
    pub fn start(agent: &crate::agent::Agent, parent: &SpanHandle) -> Self {
        let tracer = agent.tracer().clone();
        let meter = agent.meter().clone();
        let cycle = meter.start_cycle();
        let span = tracer.start_agent_loop_span(parent, &cycle.cycle_id);
        Self {
            tracer,
            meter,
            span,
            cycle,
        }
    }

    /// The cycle span, for parenting model and tool spans.
    pub fn span(&self) -> &SpanHandle {
        &self.span
    }

    /// This cycle's id (`agent_loop.cycle_id`).
    pub fn cycle_id(&self) -> &str {
        &self.cycle.cycle_id
    }
}

impl Drop for CycleScope {
    fn drop(&mut self) {
        self.meter.end_cycle(&self.cycle);
        self.tracer.end_agent_loop_span(&self.span, None);
    }
}
