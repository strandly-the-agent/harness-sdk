//! Local traces and metric snapshots (`strands-ts/src/telemetry/{tracer,meter}.ts`).

use serde::Serialize;
use std::collections::BTreeMap;

use crate::types::{Message, Metrics, Usage};

/// Most recent invocations retained in [`AgentMetrics::agent_invocations`] (TS: 50).
pub(crate) const MAX_RETAINED_INVOCATIONS: usize = 50;

/// A local, always-collected trace node mirroring TS `AgentTrace`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTrace {
    /// Unique id.
    pub id: String,
    /// Trace name (e.g. `Model invoke`).
    pub name: String,
    /// Parent trace id, when nested.
    pub parent_id: Option<String>,
    /// Start time in milliseconds since the Unix epoch.
    pub start_time: u64,
    /// End time in milliseconds, `None` while open.
    pub end_time: Option<u64>,
    /// `end_time - start_time`; 0 until ended.
    pub duration: u64,
    /// Nested traces.
    pub children: Vec<AgentTrace>,
    /// Free-form string metadata.
    pub metadata: BTreeMap<String, String>,
    /// Message attached to this trace, when any.
    pub message: Option<Message>,
}

impl AgentTrace {
    /// A new open trace.
    pub fn new(name: impl Into<String>, parent_id: Option<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            parent_id,
            start_time: now_ms(),
            end_time: None,
            duration: 0,
            children: Vec::new(),
            metadata: BTreeMap::new(),
            message: None,
        }
    }

    /// Close the trace, computing `duration` (TS `end()`).
    pub fn end(&mut self, end_time: Option<u64>) {
        let end = end_time.unwrap_or_else(now_ms);
        self.end_time = Some(end);
        self.duration = end.saturating_sub(self.start_time);
    }

    /// Add a metadata entry.
    pub fn add_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }
}

/// Per-tool aggregates (TS `ToolMetricsData`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetrics {
    /// Total calls.
    pub call_count: u64,
    /// Successful calls.
    pub success_count: u64,
    /// Failed calls.
    pub error_count: u64,
    /// Summed call duration in milliseconds.
    pub total_time: u64,
}

/// One event-loop cycle (TS `AgentLoopMetricsData`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleMetrics {
    /// Cycle id.
    pub cycle_id: String,
    /// Cycle duration in milliseconds.
    pub duration: u64,
    /// Usage accumulated during the cycle.
    pub usage: Usage,
}

/// One agent invocation (TS `InvocationMetricsData`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationMetrics {
    /// Cycles executed in this invocation.
    pub cycles: Vec<CycleMetrics>,
    /// Usage accumulated in this invocation.
    pub usage: Usage,
}

/// Read-only metric snapshot surfaced on `AgentResult::metrics` (TS `AgentMetrics`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetrics {
    /// Cycles executed across the agent's life.
    pub cycle_count: u64,
    /// Usage accumulated across all invocations.
    pub accumulated_usage: Usage,
    /// Latency accumulated across all invocations.
    pub accumulated_metrics: Metrics,
    /// Most recent invocations (at most 50).
    pub agent_invocations: Vec<InvocationMetrics>,
    /// Per-tool aggregates keyed by tool name.
    pub tool_metrics: BTreeMap<String, ToolMetrics>,
    /// Total prompt tokens (incl. cached) of the last invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_context_size: Option<u64>,
    /// Latest prompt plus the last output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_context_size: Option<u64>,
    /// Sum of all cycle durations in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_duration: Option<u64>,
}

impl AgentMetrics {
    /// Wire form (TS `toJSON()`).
    pub fn to_json(&self) -> crate::types::JsonValue {
        serde_json::to_value(self).expect("AgentMetrics is serializable")
    }
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_end_computes_duration() {
        let mut trace = AgentTrace::new("Model invoke", None);
        let start = trace.start_time;
        trace.end(Some(start + 25));
        assert_eq!(trace.duration, 25);
        assert_eq!(trace.end_time, Some(start + 25));
    }

    #[test]
    fn metrics_serialize_with_camel_case_keys() {
        let json = AgentMetrics::default().to_json();
        assert!(json.get("cycleCount").is_some());
        assert!(json.get("accumulatedUsage").is_some());
        assert!(json.get("latestContextSize").is_none());
    }
}
