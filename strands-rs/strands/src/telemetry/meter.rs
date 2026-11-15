//! Local agent metrics (TS `telemetry/meter.ts`): the counters an [`AgentResult`] carries.
//! OpenTelemetry instruments join in the `otel` feature.
//!
//! [`AgentResult`]: crate::AgentResult
#![deny(clippy::arithmetic_side_effects)]

use std::time::Duration;

use indexmap::IndexMap;
use serde::{Serialize, Serializer};
use tokio::time::Instant;

use crate::models::streaming::{Metrics, ModelMetadataEvent, Usage};

/// TS `MAX_INVOCATION_HISTORY`: how many invocations [`AgentMetrics`] keeps.
const MAX_INVOCATION_HISTORY: usize = 50;

fn as_ms<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

/// Call counts and time of one tool (TS `ToolMetricsData`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolMetricsData {
    call_count: u64,
    success_count: u64,
    error_count: u64,
    #[serde(serialize_with = "as_ms")]
    total_time: Duration,
}

impl ToolMetricsData {
    /// How many times the tool ran.
    pub fn call_count(&self) -> u64 {
        self.call_count
    }

    /// How many runs returned a success result.
    pub fn success_count(&self) -> u64 {
        self.success_count
    }

    /// How many runs returned an error result.
    pub fn error_count(&self) -> u64 {
        self.error_count
    }

    /// Total run time.
    pub fn total_time(&self) -> Duration {
        self.total_time
    }

    /// Mean run time, zero before the first call (TS `toolUsage[..].averageTime`).
    pub fn average_time(&self) -> Duration {
        u32::try_from(self.call_count)
            .ok()
            .and_then(|count| self.total_time.checked_div(count))
            .unwrap_or_default()
    }

    /// Share of successful runs, zero before the first call (TS `toolUsage[..].successRate`).
    #[expect(
        clippy::cast_precision_loss,
        reason = "a ratio of call counts, as TS computes it"
    )]
    pub fn success_rate(&self) -> f64 {
        if self.call_count == 0 {
            return 0.0;
        }
        self.success_count as f64 / self.call_count as f64
    }
}

/// One agent loop cycle (TS `AgentLoopMetricsData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentLoopMetricsData {
    cycle_id: String,
    #[serde(serialize_with = "as_ms")]
    duration: Duration,
    usage: Usage,
}

impl AgentLoopMetricsData {
    /// The cycle id (`cycle-<n>`).
    pub fn cycle_id(&self) -> &str {
        &self.cycle_id
    }

    /// The cycle duration; zero while the cycle runs.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Token usage of the cycle's model calls.
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
}

/// One invocation (TS `InvocationMetricsData`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InvocationMetricsData {
    cycles: Vec<AgentLoopMetricsData>,
    usage: Usage,
}

impl InvocationMetricsData {
    /// The invocation's cycles, in order.
    pub fn cycles(&self) -> &[AgentLoopMetricsData] {
        &self.cycles
    }

    /// Token usage of the invocation.
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
}

/// Metrics accumulated over an agent's lifetime (TS `AgentMetrics`).
///
/// Serializes as TS `toJSON()`; durations are whole milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentMetrics {
    cycle_count: u64,
    accumulated_usage: Usage,
    accumulated_metrics: Metrics,
    agent_invocations: Vec<InvocationMetricsData>,
    tool_metrics: IndexMap<String, ToolMetricsData>,
    #[serde(serialize_with = "as_ms")]
    total_duration: Duration,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_context_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    projected_context_size: Option<u64>,
}

impl AgentMetrics {
    /// Agent loop cycles run so far.
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }

    /// Token usage over every model call.
    pub fn accumulated_usage(&self) -> &Usage {
        &self.accumulated_usage
    }

    /// Model latency over every model call.
    pub fn accumulated_metrics(&self) -> &Metrics {
        &self.accumulated_metrics
    }

    /// The most recent invocations (at most 50), oldest first.
    pub fn agent_invocations(&self) -> &[InvocationMetricsData] {
        &self.agent_invocations
    }

    /// The most recent invocation (TS `latestAgentInvocation`).
    pub fn latest_agent_invocation(&self) -> Option<&InvocationMetricsData> {
        self.agent_invocations.last()
    }

    /// Per-tool metrics in first-call order.
    pub fn tool_metrics(&self) -> impl ExactSizeIterator<Item = (&str, &ToolMetricsData)> {
        self.tool_metrics
            .iter()
            .map(|(name, metrics)| (name.as_str(), metrics))
    }

    /// The metrics of one tool.
    pub fn tool_metric(&self, tool_name: &str) -> Option<&ToolMetricsData> {
        self.tool_metrics.get(tool_name)
    }

    /// Total time of the finished cycles.
    pub fn total_duration(&self) -> Duration {
        self.total_duration
    }

    /// Mean cycle time, zero before the first cycle (TS `averageCycleTime`).
    pub fn average_cycle_time(&self) -> Duration {
        u32::try_from(self.cycle_count)
            .ok()
            .and_then(|count| self.total_duration.checked_div(count))
            .unwrap_or_default()
    }

    /// Prompt tokens of the latest model call.
    pub fn latest_context_size(&self) -> Option<u64> {
        self.latest_context_size
    }

    /// Prompt plus output tokens of the latest model call: the next call's context size.
    pub fn projected_context_size(&self) -> Option<u64> {
        self.projected_context_size
    }
}

/// Collects [`AgentMetrics`] as the loop runs (TS `Meter`).
#[derive(Debug, Default)]
pub(crate) struct Meter {
    metrics: AgentMetrics,
}

impl Meter {
    pub(crate) fn metrics(&self) -> &AgentMetrics {
        &self.metrics
    }

    pub(crate) fn start_new_invocation(&mut self) {
        let invocations = &mut self.metrics.agent_invocations;
        if invocations.len() >= MAX_INVOCATION_HISTORY {
            invocations.remove(0);
        }
        invocations.push(InvocationMetricsData::default());
    }

    /// Starts a cycle; returns its id and start time.
    pub(crate) fn start_cycle(&mut self) -> (String, Instant) {
        self.metrics.cycle_count = self.metrics.cycle_count.saturating_add(1);
        let cycle_id = format!("cycle-{}", self.metrics.cycle_count);
        if let Some(invocation) = self.metrics.agent_invocations.last_mut() {
            invocation.cycles.push(AgentLoopMetricsData {
                cycle_id: cycle_id.clone(),
                duration: Duration::ZERO,
                usage: Usage::default(),
            });
        }
        (cycle_id, Instant::now())
    }

    pub(crate) fn end_cycle(&mut self, start: Instant) {
        let duration = start.elapsed();
        self.metrics.total_duration = self.metrics.total_duration.saturating_add(duration);
        if let Some(cycle) = self.latest_cycle() {
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
        let Some(metadata) = metadata else { return };
        if let Some(usage) = metadata.usage() {
            self.update_usage(usage);
        }
        if let Some(metrics) = metadata.metrics() {
            let latency_ms = self.metrics.accumulated_metrics.latency_ms();
            self.metrics.accumulated_metrics = Metrics::new(latency_ms.saturating_add(metrics.latency_ms()));
        }
    }

    fn update_usage(&mut self, usage: &Usage) {
        self.metrics.accumulated_usage.accumulate(usage);
        let prompt = usage.total_prompt_tokens();
        self.metrics.latest_context_size = Some(prompt);
        self.metrics.projected_context_size = Some(prompt.saturating_add(usage.output_tokens()));
        if let Some(invocation) = self.metrics.agent_invocations.last_mut() {
            invocation.usage.accumulate(usage);
        }
        if let Some(cycle) = self.latest_cycle() {
            cycle.usage.accumulate(usage);
        }
    }

    fn latest_cycle(&mut self) -> Option<&mut AgentLoopMetricsData> {
        self.metrics
            .agent_invocations
            .last_mut()
            .and_then(|invocation| invocation.cycles.last_mut())
    }
}
