//! Local agent metrics (the non-OpenTelemetry part of TS `telemetry/meter.ts`).

use std::time::Duration;

use indexmap::IndexMap;
use serde::{Serialize, Serializer};
use tokio::time::Instant;

use crate::models::{Metrics, ModelMetadataEvent, Usage};

const MAX_INVOCATION_HISTORY: usize = 50;

fn as_millis<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

/// Per-tool call counts and time (TS `ToolMetricsData`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolMetricsData {
    /// Calls made.
    pub call_count: u64,
    /// Calls that succeeded.
    pub success_count: u64,
    /// Calls that failed.
    pub error_count: u64,
    /// Time spent in the tool, serialized as milliseconds.
    #[serde(serialize_with = "as_millis")]
    pub total_time: Duration,
}

impl ToolMetricsData {
    /// The mean call time.
    pub fn average_time(&self) -> Duration {
        u32::try_from(self.call_count)
            .ok()
            .filter(|count| *count > 0)
            .map_or(Duration::ZERO, |count| self.total_time / count)
    }

    /// The share of calls that succeeded, from 0 to 1.
    #[expect(clippy::cast_precision_loss, reason = "a ratio of counters")]
    pub fn success_rate(&self) -> f64 {
        if self.call_count == 0 {
            return 0.0;
        }
        self.success_count as f64 / self.call_count as f64
    }
}

/// Metrics of one agent loop cycle (TS `AgentLoopMetricsData`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentLoopMetricsData {
    /// The cycle id (`cycle-<n>`).
    pub cycle_id: String,
    /// The cycle's duration, serialized as milliseconds.
    #[serde(serialize_with = "as_millis")]
    pub duration: Duration,
    /// Token usage of the cycle.
    pub usage: Usage,
}

/// Metrics of one invocation (TS `InvocationMetricsData`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InvocationMetricsData {
    /// The invocation's cycles.
    pub cycles: Vec<AgentLoopMetricsData>,
    /// Token usage of the invocation.
    pub usage: Usage,
}

/// Metrics accumulated across an agent's invocations (TS `AgentMetrics`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentMetrics {
    /// Cycles run.
    pub cycle_count: u64,
    /// Token usage of every model call.
    pub accumulated_usage: Usage,
    /// Latency of every model call.
    pub accumulated_metrics: Metrics,
    /// The latest invocations (at most 50).
    pub agent_invocations: Vec<InvocationMetricsData>,
    /// Per-tool metrics, by tool name.
    pub tool_metrics: IndexMap<String, ToolMetricsData>,
    /// Time spent in cycles, serialized as milliseconds.
    #[serde(serialize_with = "as_millis")]
    pub total_duration: Duration,
    /// Prompt tokens of the latest model call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_context_size: Option<u64>,
    /// Prompt plus output tokens of the latest model call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_context_size: Option<u64>,
}

impl AgentMetrics {
    /// The latest invocation's metrics.
    pub fn latest_agent_invocation(&self) -> Option<&InvocationMetricsData> {
        self.agent_invocations.last()
    }

    /// The mean cycle duration.
    pub fn average_cycle_time(&self) -> Duration {
        u32::try_from(self.cycle_count)
            .ok()
            .filter(|count| *count > 0)
            .map_or(Duration::ZERO, |count| self.total_duration / count)
    }
}

/// Collects [`AgentMetrics`] as the loop runs (TS `Meter`).
#[derive(Debug, Default)]
pub(crate) struct Meter {
    metrics: AgentMetrics,
}

impl Meter {
    pub(crate) fn metrics(&self) -> AgentMetrics {
        self.metrics.clone()
    }

    pub(crate) fn start_new_invocation(&mut self) {
        if self.metrics.agent_invocations.len() >= MAX_INVOCATION_HISTORY {
            self.metrics.agent_invocations.remove(0);
        }
        self.metrics
            .agent_invocations
            .push(InvocationMetricsData::default());
    }

    pub(crate) fn start_cycle(&mut self) -> (String, Instant) {
        self.metrics.cycle_count = self.metrics.cycle_count.saturating_add(1);
        let cycle_id = format!("cycle-{}", self.metrics.cycle_count);
        if let Some(invocation) = self.metrics.agent_invocations.last_mut() {
            invocation.cycles.push(AgentLoopMetricsData {
                cycle_id: cycle_id.clone(),
                ..Default::default()
            });
        }
        (cycle_id, Instant::now())
    }

    pub(crate) fn end_cycle(&mut self, start: Instant) {
        let duration = start.elapsed();
        self.metrics.total_duration = self.metrics.total_duration.saturating_add(duration);
        if let Some(cycle) = self
            .metrics
            .agent_invocations
            .last_mut()
            .and_then(|invocation| invocation.cycles.last_mut())
        {
            cycle.duration = duration;
        }
    }

    pub(crate) fn end_tool_call(&mut self, tool_name: &str, duration: Duration, success: bool) {
        let entry = self.metrics.tool_metrics.entry(tool_name.to_owned()).or_default();
        entry.call_count = entry.call_count.saturating_add(1);
        entry.total_time = entry.total_time.saturating_add(duration);
        if success {
            entry.success_count = entry.success_count.saturating_add(1);
        } else {
            entry.error_count = entry.error_count.saturating_add(1);
        }
    }

    pub(crate) fn update_cycle(&mut self, metadata: Option<&ModelMetadataEvent>) {
        let Some(metadata) = metadata else {
            return;
        };
        if let Some(usage) = &metadata.usage {
            self.update_usage(usage);
        }
        if let Some(metrics) = &metadata.metrics {
            let accumulated = &mut self.metrics.accumulated_metrics;
            accumulated.latency_ms = accumulated.latency_ms.saturating_add(metrics.latency_ms);
        }
    }

    fn update_usage(&mut self, usage: &Usage) {
        self.metrics.accumulated_usage.accumulate(usage);
        let prompt = usage.total_prompt_tokens();
        self.metrics.latest_context_size = Some(prompt);
        self.metrics.projected_context_size = Some(prompt.saturating_add(usage.output_tokens));
        if let Some(invocation) = self.metrics.agent_invocations.last_mut() {
            invocation.usage.accumulate(usage);
            if let Some(cycle) = invocation.cycles.last_mut() {
                cycle.usage.accumulate(usage);
            }
        }
    }
}
