//! The result of an agent invocation, with its metrics and traces (TS `types/agent.ts`
//! `AgentResult`, `telemetry/meter.ts` `AgentMetrics` and `telemetry/tracer.ts` `AgentTrace`).

use std::fmt;

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::agent::invocation_context::InvocationContext;
use crate::agent::invocation_state::InvocationState;
use crate::error::{Result, StructuredOutputError};
use crate::interrupt::Interrupt;
use crate::models::streaming::{Metrics, Usage};
use crate::types::json::{self, JsonValue, present_null};
use crate::types::messages::{ContentBlock, Message, StopReason};

/// What an agent invocation returns (TS `AgentResult`).
///
/// It serializes to the TS `toJSON()` shape: `type`, `stopReason`, `lastMessage` and, when
/// present, `structuredOutput`. Traces, metrics, interrupts, the invocation context and the
/// invocation state stay out of the JSON and are reachable through their getters.
///
/// [`Display`](fmt::Display) gives the TS `toString()` text: the interrupts as JSON when there
/// are any, else the structured output as JSON, else the text, reasoning and citation text of the
/// last message joined by newlines.
///
/// ```
/// use strands::AgentResult;
/// use strands::types::messages::{ContentBlock, Message, Role, StopReason};
///
/// let message = Message::new(Role::Assistant, vec![ContentBlock::from("Hello")]);
/// let result = AgentResult::new(StopReason::EndTurn, message);
/// assert_eq!(result.to_string(), "Hello");
/// assert_eq!(result.stop_reason(), StopReason::EndTurn);
/// assert_eq!(
///     serde_json::to_value(&result).unwrap()["type"],
///     serde_json::json!("agentResult")
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename = "agentResult", rename_all = "camelCase")]
pub struct AgentResult {
    stop_reason: StopReason,
    last_message: Message,
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    structured_output: Option<JsonValue>,
    #[serde(skip)]
    traces: Vec<AgentTrace>,
    #[serde(skip)]
    metrics: Option<Box<AgentMetrics>>,
    #[serde(skip)]
    interrupts: Vec<Interrupt>,
    #[serde(skip)]
    invocation_context: Option<InvocationContext>,
    #[serde(skip)]
    invocation_state: InvocationState,
}

impl AgentResult {
    /// Creates a result with no traces, metrics, structured output or interrupts and an empty
    /// invocation state.
    #[must_use]
    pub fn new(stop_reason: StopReason, last_message: Message) -> Self {
        Self {
            stop_reason,
            last_message,
            structured_output: None,
            traces: Vec::new(),
            metrics: None,
            interrupts: Vec::new(),
            invocation_context: None,
            invocation_state: InvocationState::default(),
        }
    }

    /// Sets the validated structured output.
    #[must_use]
    pub fn with_structured_output(mut self, structured_output: JsonValue) -> Self {
        self.structured_output = Some(structured_output);
        self
    }

    /// Sets the execution traces.
    #[must_use]
    pub fn with_traces(mut self, traces: Vec<AgentTrace>) -> Self {
        self.traces = traces;
        self
    }

    /// Sets the metrics.
    #[must_use]
    pub fn with_metrics(mut self, metrics: AgentMetrics) -> Self {
        self.metrics = Some(Box::new(metrics));
        self
    }

    /// Sets the unanswered interrupts that stopped the invocation.
    #[must_use]
    pub fn with_interrupts(mut self, interrupts: Vec<Interrupt>) -> Self {
        self.interrupts = interrupts;
        self
    }

    /// Sets the invocation context the call ran in.
    #[must_use]
    pub fn with_invocation_context(mut self, invocation_context: InvocationContext) -> Self {
        self.invocation_context = Some(invocation_context);
        self
    }

    /// Sets the invocation state the call was given.
    #[must_use]
    pub fn with_invocation_state(mut self, invocation_state: InvocationState) -> Self {
        self.invocation_state = invocation_state;
        self
    }

    /// Why the final model response stopped.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason.clone()
    }

    /// The last message added to the conversation.
    #[must_use]
    pub fn last_message(&self) -> &Message {
        &self.last_message
    }

    /// The structured output, deserialized as `T`.
    ///
    /// # Errors
    ///
    /// [`Error::StructuredOutput`](crate::Error::StructuredOutput) when the invocation produced
    /// no structured output, and [`Error::Other`](crate::Error::Other) when it doesn't
    /// deserialize as `T`.
    pub fn structured_output<T: DeserializeOwned>(&self) -> Result<T> {
        let output = self
            .structured_output
            .as_ref()
            .ok_or_else(|| StructuredOutputError::new("the result has no structured output"))?;
        Ok(T::deserialize(output)?)
    }

    /// The execution traces, one root per cycle.
    #[must_use]
    pub fn traces(&self) -> &[AgentTrace] {
        &self.traces
    }

    /// The agent's metrics, when the loop recorded them.
    #[must_use]
    pub fn metrics(&self) -> Option<&AgentMetrics> {
        self.metrics.as_deref()
    }

    /// The unanswered interrupts, when the stop reason is [`StopReason::Interrupt`].
    #[must_use]
    pub fn interrupts(&self) -> &[Interrupt] {
        &self.interrupts
    }

    /// The invocation context the call ran in; pass it to a later call to share its limits and
    /// usage.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// The invocation state the call was given.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The prompt size of the last model call, including cached tokens
    /// ([`AgentMetrics::latest_context_size`]).
    #[must_use]
    pub fn context_size(&self) -> Option<u64> {
        self.metrics.as_ref()?.latest_context_size()
    }

    /// The projected prompt size of the next model call
    /// ([`AgentMetrics::projected_context_size`]).
    #[must_use]
    pub fn projected_context_size(&self) -> Option<u64> {
        self.metrics.as_ref()?.projected_context_size()
    }
}

/// Compares everything except the invocation context and the invocation state, which hold live
/// handles.
impl PartialEq for AgentResult {
    fn eq(&self, other: &Self) -> bool {
        self.stop_reason == other.stop_reason
            && self.last_message == other.last_message
            && self.structured_output == other.structured_output
            && self.traces == other.traces
            && self.metrics == other.metrics
            && self.interrupts == other.interrupts
    }
}

impl fmt::Display for AgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Serializing interrupts and JSON values can't fail; the error arm only satisfies the type.
        if !self.interrupts.is_empty() {
            return f.write_str(&json::to_string(&self.interrupts).map_err(|_| fmt::Error)?);
        }
        if let Some(output) = &self.structured_output {
            return f.write_str(&json::to_string(output).map_err(|_| fmt::Error)?);
        }
        let parts: Vec<String> = self.last_message.content.iter().flat_map(block_text).collect();
        f.write_str(&parts.join("\n"))
    }
}

fn block_text(block: &ContentBlock) -> Vec<String> {
    match block {
        ContentBlock::Text(text) => vec![text.text.clone()],
        ContentBlock::Reasoning(reasoning) => reasoning
            .text
            .iter()
            .filter(|text| !text.is_empty())
            .map(|text| format!("💭 Reasoning:\n   {}", text.replace('\n', "\n   ")))
            .collect(),
        ContentBlock::Citations(citations) => citations
            .content
            .iter()
            .filter_map(|content| content.text.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// Aggregated metrics of an agent's loop (TS `AgentMetrics`), serialized as TS `toJSON()` writes
/// them. Durations are whole milliseconds, as TS measures them with `Date.now()`.
///
/// ```
/// use strands::models::streaming::Usage;
/// use strands::types::agent::{AgentMetrics, ToolMetricsData};
///
/// let metrics = AgentMetrics::new()
///     .with_cycle_count(2)
///     .with_accumulated_usage(Usage::new(10, 5, 15))
///     .with_tool_metric("calc", ToolMetricsData::new(4, 3, 1, 100));
/// assert_eq!(metrics.tool_metric("calc").unwrap().success_rate(), 0.75);
/// assert_eq!(
///     serde_json::to_value(&metrics).unwrap()["toolMetrics"],
///     serde_json::json!({ "calc": { "callCount": 4, "successCount": 3, "errorCount": 1, "totalTime": 100 } })
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentMetrics {
    cycle_count: u64,
    accumulated_usage: Usage,
    accumulated_metrics: Metrics,
    agent_invocations: Vec<InvocationMetricsData>,
    tool_metrics: IndexMap<String, ToolMetricsData>,
    total_duration: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_context_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    projected_context_size: Option<u64>,
}

impl AgentMetrics {
    /// Creates empty metrics: no cycles, zero usage and latency, no invocations or tools.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the number of cycles.
    #[must_use]
    pub fn with_cycle_count(mut self, cycle_count: u64) -> Self {
        self.cycle_count = cycle_count;
        self
    }

    /// Sets the accumulated usage.
    #[must_use]
    pub fn with_accumulated_usage(mut self, usage: Usage) -> Self {
        self.accumulated_usage = usage;
        self
    }

    /// Sets the accumulated model metrics.
    #[must_use]
    pub fn with_accumulated_metrics(mut self, metrics: Metrics) -> Self {
        self.accumulated_metrics = metrics;
        self
    }

    /// Sets the per-invocation metrics, oldest first.
    #[must_use]
    pub fn with_agent_invocations(mut self, invocations: Vec<InvocationMetricsData>) -> Self {
        self.agent_invocations = invocations;
        self
    }

    /// Sets the metrics of one tool, replacing any earlier entry for it in place.
    #[must_use]
    pub fn with_tool_metric(mut self, tool_name: impl Into<String>, metrics: ToolMetricsData) -> Self {
        self.tool_metrics.insert(tool_name.into(), metrics);
        self
    }

    /// Sets the total cycle duration in milliseconds.
    #[must_use]
    pub fn with_total_duration(mut self, total_duration: u64) -> Self {
        self.total_duration = total_duration;
        self
    }

    /// Sets the prompt size of the last model call.
    #[must_use]
    pub fn with_latest_context_size(mut self, size: u64) -> Self {
        self.latest_context_size = Some(size);
        self
    }

    /// Sets the projected prompt size of the next model call.
    #[must_use]
    pub fn with_projected_context_size(mut self, size: u64) -> Self {
        self.projected_context_size = Some(size);
        self
    }

    /// The number of loop cycles across all invocations.
    #[must_use]
    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }

    /// Token usage accumulated across all model calls.
    #[must_use]
    pub fn accumulated_usage(&self) -> &Usage {
        &self.accumulated_usage
    }

    /// Model metrics accumulated across all model calls.
    #[must_use]
    pub fn accumulated_metrics(&self) -> &Metrics {
        &self.accumulated_metrics
    }

    /// Per-invocation metrics of the most recent invocations, oldest first.
    #[must_use]
    pub fn agent_invocations(&self) -> &[InvocationMetricsData] {
        &self.agent_invocations
    }

    /// The most recent invocation's metrics.
    #[must_use]
    pub fn latest_agent_invocation(&self) -> Option<&InvocationMetricsData> {
        self.agent_invocations.last()
    }

    /// Per-tool metrics in the order the tools first ran.
    pub fn tool_metrics(&self) -> impl ExactSizeIterator<Item = (&str, &ToolMetricsData)> {
        self.tool_metrics
            .iter()
            .map(|(name, metrics)| (name.as_str(), metrics))
    }

    /// The metrics of one tool.
    #[must_use]
    pub fn tool_metric(&self, tool_name: &str) -> Option<&ToolMetricsData> {
        self.tool_metrics.get(tool_name)
    }

    /// The total duration of all cycles in milliseconds.
    #[must_use]
    pub fn total_duration(&self) -> u64 {
        self.total_duration
    }

    /// The average cycle duration in milliseconds, or 0 without cycles.
    #[must_use]
    pub fn average_cycle_time(&self) -> f64 {
        ratio(self.total_duration, self.cycle_count)
    }

    /// The prompt the model processed on the last call, including cached tokens.
    #[must_use]
    pub fn latest_context_size(&self) -> Option<u64> {
        self.latest_context_size
    }

    /// The projected prompt of the next model call: the last prompt plus its output.
    #[must_use]
    pub fn projected_context_size(&self) -> Option<u64> {
        self.projected_context_size
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "TS divides the same numbers as doubles"
)]
fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 0.0;
    }
    numerator as f64 / denominator as f64
}

/// Execution metrics of one tool (TS `ToolMetricsData`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetricsData {
    /// Calls to the tool.
    pub call_count: u64,
    /// Successful calls.
    pub success_count: u64,
    /// Failed calls.
    pub error_count: u64,
    /// Total execution time in milliseconds.
    pub total_time: u64,
}

impl ToolMetricsData {
    /// Creates tool metrics.
    #[must_use]
    pub fn new(call_count: u64, success_count: u64, error_count: u64, total_time: u64) -> Self {
        Self {
            call_count,
            success_count,
            error_count,
            total_time,
        }
    }

    /// The average call time in milliseconds, or 0 without calls (TS `toolUsage` `averageTime`).
    #[must_use]
    pub fn average_time(&self) -> f64 {
        ratio(self.total_time, self.call_count)
    }

    /// The share of successful calls, or 0 without calls (TS `toolUsage` `successRate`).
    #[must_use]
    pub fn success_rate(&self) -> f64 {
        ratio(self.success_count, self.call_count)
    }
}

/// Metrics of one invocation (TS `InvocationMetricsData`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationMetricsData {
    /// The invocation's cycles.
    pub cycles: Vec<AgentLoopMetricsData>,
    /// The invocation's accumulated usage.
    pub usage: Usage,
}

impl InvocationMetricsData {
    /// Creates invocation metrics.
    #[must_use]
    pub fn new(cycles: Vec<AgentLoopMetricsData>, usage: Usage) -> Self {
        Self { cycles, usage }
    }
}

/// Metrics of one loop cycle (TS `AgentLoopMetricsData`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLoopMetricsData {
    /// The cycle id, for example `cycle-1`.
    pub cycle_id: String,
    /// The cycle duration in milliseconds.
    pub duration: u64,
    /// The cycle's usage.
    pub usage: Usage,
}

impl AgentLoopMetricsData {
    /// Creates cycle metrics.
    #[must_use]
    pub fn new(cycle_id: impl Into<String>, duration: u64, usage: Usage) -> Self {
        Self {
            cycle_id: cycle_id.into(),
            duration,
            usage,
        }
    }
}

/// A timed node in an invocation's execution trace (TS `AgentTrace`), serialized as TS
/// `toJSON()` writes it. Times are milliseconds since the Unix epoch.
///
/// ```
/// use strands::types::agent::AgentTrace;
///
/// let mut cycle = AgentTrace::new("Cycle 1", 1_000).with_child(AgentTrace::new("stream_messages", 1_010));
/// cycle.end(1_250);
/// assert_eq!(cycle.duration(), 250);
/// assert_eq!(cycle.children()[0].parent_id(), Some(cycle.id()));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTrace {
    id: String,
    name: String,
    parent_id: Option<String>,
    start_time: u64,
    end_time: Option<u64>,
    duration: u64,
    children: Vec<AgentTrace>,
    metadata: IndexMap<String, String>,
    message: Option<Message>,
}

impl AgentTrace {
    /// Starts a root trace with a random UUID id.
    #[must_use]
    pub fn new(name: impl Into<String>, start_time: u64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            parent_id: None,
            start_time,
            end_time: None,
            duration: 0,
            children: Vec::new(),
            metadata: IndexMap::new(),
            message: None,
        }
    }

    /// Sets the id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        for child in &mut self.children {
            child.parent_id = Some(self.id.clone());
        }
        self
    }

    /// Nests `child` under this trace and points its parent id here.
    #[must_use]
    pub fn with_child(mut self, mut child: AgentTrace) -> Self {
        child.parent_id = Some(self.id.clone());
        self.children.push(child);
        self
    }

    /// Adds a metadata entry, for example `toolName`.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Sets the message the trace produced, for example the model output.
    #[must_use]
    pub fn with_message(mut self, message: Message) -> Self {
        self.message = Some(message);
        self
    }

    /// Ends the trace at `end_time` and records its duration.
    pub fn end(&mut self, end_time: u64) {
        self.end_time = Some(end_time);
        self.duration = end_time.saturating_sub(self.start_time);
    }

    /// The trace id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The display name, for example `Cycle 1` or `Tool: calc`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The parent trace's id, `None` for a root.
    #[must_use]
    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    /// The start time.
    #[must_use]
    pub fn start_time(&self) -> u64 {
        self.start_time
    }

    /// The end time, `None` until the trace ends.
    #[must_use]
    pub fn end_time(&self) -> Option<u64> {
        self.end_time
    }

    /// The duration in milliseconds, 0 until the trace ends.
    #[must_use]
    pub fn duration(&self) -> u64 {
        self.duration
    }

    /// The nested traces.
    #[must_use]
    pub fn children(&self) -> &[AgentTrace] {
        &self.children
    }

    /// The metadata entries in insertion order.
    pub fn metadata(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.metadata
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// The message the trace produced.
    #[must_use]
    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }
}

#[cfg(test)]
mod tests;
