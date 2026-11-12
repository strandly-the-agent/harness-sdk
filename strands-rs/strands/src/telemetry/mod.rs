//! Telemetry (TS `telemetry/`): local metrics and traces. OpenTelemetry export arrives with
//! the `otel` feature.

pub(crate) mod meter;
pub(crate) mod tracer;

pub use meter::{AgentLoopMetricsData, AgentMetrics, InvocationMetricsData, ToolMetricsData};
pub use tracer::AgentTrace;
