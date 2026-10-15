//! Multi-agent execution state (`strands-ts/src/multiagent/state.ts`).

use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::multiagent::MultiAgentInput;
use crate::interrupt::Interrupt;
use crate::models::streaming::accumulate_usage;
use crate::session::Snapshot;
use crate::state_store::StateStore;
use crate::types::{ContentBlock, JsonValue, Usage};

/// Milliseconds since the Unix epoch, matching the TS `Date.now()` timestamps.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Execution lifecycle status shared across all multi-agent patterns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    /// Execution has not yet started.
    #[default]
    Pending,
    /// Execution is currently in progress.
    Executing,
    /// Execution finished successfully.
    Completed,
    /// Execution encountered an error.
    Failed,
    /// Execution was cancelled before or during processing.
    Cancelled,
    /// Execution paused awaiting an interrupt response; can be resumed.
    Interrupted,
}

/// The subset of [`Status`] a finished [`NodeResult`] can carry (TS `ResultStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResultStatus {
    /// Execution finished successfully.
    Completed,
    /// Execution encountered an error.
    Failed,
    /// Execution was cancelled before or during processing.
    Cancelled,
    /// Execution paused awaiting an interrupt response.
    Interrupted,
}

impl From<ResultStatus> for Status {
    fn from(status: ResultStatus) -> Self {
        match status {
            ResultStatus::Completed => Status::Completed,
            ResultStatus::Failed => Status::Failed,
            ResultStatus::Cancelled => Status::Cancelled,
            ResultStatus::Interrupted => Status::Interrupted,
        }
    }
}

impl Status {
    /// The terminal equivalent of this status, or `None` while still pending/executing.
    pub fn as_result(self) -> Option<ResultStatus> {
        match self {
            Status::Completed => Some(ResultStatus::Completed),
            Status::Failed => Some(ResultStatus::Failed),
            Status::Cancelled => Some(ResultStatus::Cancelled),
            Status::Interrupted => Some(ResultStatus::Interrupted),
            Status::Pending | Status::Executing => None,
        }
    }
}

/// Deserializes a present-but-null field to `Some(JsonValue::Null)` instead of `None`.
fn deserialize_some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Result of executing a single node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "nodeResult", rename_all = "camelCase")]
#[non_exhaustive]
pub struct NodeResult {
    /// Id of the node that produced this result.
    pub node_id: String,
    /// Terminal status of the node run.
    pub status: ResultStatus,
    /// Execution time in milliseconds.
    pub duration: u64,
    /// Content the node produced.
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// Failure message, when the node failed (TS serializes `Error` as its message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Validated structured output, if a schema was provided.
    #[serde(
        default,
        deserialize_with = "deserialize_some",
        skip_serializing_if = "Option::is_none"
    )]
    pub structured_output: Option<JsonValue>,
    /// Token usage from the node execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Interrupts raised by the underlying agent/orchestrator; non-empty iff `status` is
    /// [`ResultStatus::Interrupted`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interrupts: Vec<Interrupt>,
}

impl NodeResult {
    /// A result with no content, error, structured output, usage or interrupts.
    pub fn new(node_id: impl Into<String>, status: ResultStatus, duration: u64) -> Self {
        Self {
            node_id: node_id.into(),
            status,
            duration,
            content: Vec::new(),
            error: None,
            structured_output: None,
            usage: None,
            interrupts: Vec::new(),
        }
    }

    /// Attach the content the node produced.
    #[must_use]
    pub fn with_content(mut self, content: Vec<ContentBlock>) -> Self {
        self.content = content;
        self
    }

    /// Attach a failure message.
    #[must_use]
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Attach validated structured output.
    #[must_use]
    pub fn with_structured_output(mut self, output: JsonValue) -> Self {
        self.structured_output = Some(output);
        self
    }

    /// Attach token usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Attach the interrupts the node stopped on.
    #[must_use]
    pub fn with_interrupts(mut self, interrupts: Vec<Interrupt>) -> Self {
        self.interrupts = interrupts;
        self
    }

    /// Overlay the implementer-controlled fields of `update` onto this result.
    #[must_use]
    pub fn merge(mut self, update: NodeResultUpdate) -> Self {
        if let Some(status) = update.status {
            self.status = status;
        }
        if let Some(content) = update.content {
            self.content = content;
        }
        if let Some(error) = update.error {
            self.error = Some(error);
        }
        if let Some(output) = update.structured_output {
            self.structured_output = Some(output);
        }
        if let Some(usage) = update.usage {
            self.usage = Some(usage);
        }
        if let Some(interrupts) = update.interrupts {
            self.interrupts = interrupts;
        }
        self
    }
}

/// Partial result returned by a node implementation (TS `NodeResultUpdate`).
///
/// Only the set fields overlay the framework-managed defaults; see [`NodeResult::merge`].
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct NodeResultUpdate {
    /// Overrides the terminal status.
    pub status: Option<ResultStatus>,
    /// Overrides the produced content.
    pub content: Option<Vec<ContentBlock>>,
    /// Sets the failure message.
    pub error: Option<String>,
    /// Sets the structured output.
    pub structured_output: Option<JsonValue>,
    /// Sets the token usage.
    pub usage: Option<Usage>,
    /// Sets the raised interrupts.
    pub interrupts: Option<Vec<Interrupt>>,
}

impl NodeResultUpdate {
    /// Override the terminal status.
    #[must_use]
    pub fn with_status(mut self, status: ResultStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Override the produced content.
    #[must_use]
    pub fn with_content(mut self, content: Vec<ContentBlock>) -> Self {
        self.content = Some(content);
        self
    }

    /// Set the failure message.
    #[must_use]
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Set the structured output.
    #[must_use]
    pub fn with_structured_output(mut self, output: JsonValue) -> Self {
        self.structured_output = Some(output);
        self
    }

    /// Set the token usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Set the raised interrupts.
    #[must_use]
    pub fn with_interrupts(mut self, interrupts: Vec<Interrupt>) -> Self {
        self.interrupts = Some(interrupts);
        self
    }
}

/// Execution state of a single node within a multi-agent orchestration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NodeState {
    /// Current lifecycle status.
    pub status: Status,
    /// Whether this node is a terminal node — one where an execution path ended.
    pub terminus: bool,
    /// Node execution start time in milliseconds since epoch.
    pub start_time: u64,
    /// Results of every run of this node, oldest first.
    #[serde(default)]
    pub results: Vec<NodeResult>,
    /// Unanswered interrupts raised during this node's most recent run. Populated when `status`
    /// is [`Status::Interrupted`].
    #[serde(default)]
    pub interrupts: Vec<Interrupt>,
    /// Snapshot of the node's runnable captured when it returned `INTERRUPTED`, reloaded on
    /// resume so it picks up mid-execution. Cleared when the node completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_snapshot: Option<Snapshot>,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            status: Status::Pending,
            terminus: false,
            start_time: now_ms(),
            results: Vec::new(),
            interrupts: Vec::new(),
            interrupted_snapshot: None,
        }
    }
}

impl NodeState {
    /// A pending node state stamped with the current time.
    pub fn new() -> Self {
        Self::default()
    }

    /// Content from the most recent result, or empty if the node has not produced one.
    pub fn content(&self) -> &[ContentBlock] {
        self.results
            .last()
            .map(|r| r.content.as_slice())
            .unwrap_or_default()
    }
}

/// Aggregate result from a multi-agent execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename = "multiAgentResult", rename_all = "camelCase")]
#[non_exhaustive]
pub struct MultiAgentResult {
    /// Aggregate status, derived from the node results unless set explicitly.
    pub status: ResultStatus,
    /// All node results in completion order.
    pub results: Vec<NodeResult>,
    /// Combined content from terminus nodes, in completion order.
    pub content: Vec<ContentBlock>,
    /// Total execution time in milliseconds.
    pub duration: u64,
    /// Token usage aggregated across all node results.
    pub usage: Usage,
    /// Failure message, when the orchestration failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Interrupts aggregated across all node results; non-empty when any node ended
    /// `INTERRUPTED`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interrupts: Vec<Interrupt>,
}

impl MultiAgentResult {
    /// Aggregates `results`: status via [`resolve_status`], usage summed, interrupts collected.
    pub fn new(results: Vec<NodeResult>, duration: u64) -> Self {
        let status = resolve_status(&results);
        let usage = aggregate_node_usage(&results);
        let interrupts: Vec<Interrupt> = results.iter().flat_map(|r| r.interrupts.clone()).collect();
        Self {
            status,
            results,
            content: Vec::new(),
            duration,
            usage,
            error: None,
            interrupts,
        }
    }

    /// Override the derived aggregate status.
    #[must_use]
    pub fn with_status(mut self, status: ResultStatus) -> Self {
        self.status = status;
        self
    }

    /// Attach the combined terminus content.
    #[must_use]
    pub fn with_content(mut self, content: Vec<ContentBlock>) -> Self {
        self.content = content;
        self
    }

    /// Attach a failure message.
    #[must_use]
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Override the interrupts collected from the node results.
    #[must_use]
    pub fn with_interrupts(mut self, interrupts: Vec<Interrupt>) -> Self {
        self.interrupts = interrupts;
        self
    }
}

/// Joined text of the aggregate content, like [`AgentResult`](crate::agent::AgentResult).
impl fmt::Display for MultiAgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        f.write_str(&parts.join("\n"))
    }
}

/// Derives the aggregate status from individual node results.
///
/// Precedence: `FAILED` > `INTERRUPTED` > `CANCELLED` > `COMPLETED`. `INTERRUPTED` outranks
/// `CANCELLED` because a parallel short-circuit cancels siblings when one node interrupts — the
/// actionable "resume me" signal should surface over the collateral cancellations.
pub fn resolve_status(results: &[NodeResult]) -> ResultStatus {
    for status in [
        ResultStatus::Failed,
        ResultStatus::Interrupted,
        ResultStatus::Cancelled,
    ] {
        if results.iter().any(|r| r.status == status) {
            return status;
        }
    }
    ResultStatus::Completed
}

/// Sums token usage across all node results.
pub fn aggregate_node_usage(results: &[NodeResult]) -> Usage {
    let mut usage = Usage::default();
    for result in results {
        if let Some(node_usage) = &result.usage {
            accumulate_usage(&mut usage, node_usage);
        }
    }
    usage
}

fn serialize_app<S: Serializer>(app: &StateStore, serializer: S) -> Result<S::Ok, S::Error> {
    app.to_json().serialize(serializer)
}

fn deserialize_app<'de, D: Deserializer<'de>>(deserializer: D) -> Result<StateStore, D::Error> {
    let json = JsonValue::deserialize(deserializer)?;
    let store = StateStore::new();
    store.load_json(json);
    Ok(store)
}

/// Rehydrates a serialized `_pendingInput`: a string round-trips as text, an array as content
/// blocks (TS `rehydratePendingInput`).
fn deserialize_pending_input<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<MultiAgentInput>, D::Error> {
    let json = JsonValue::deserialize(deserializer)?;
    match json {
        JsonValue::Null => Ok(None),
        JsonValue::String(text) => Ok(Some(MultiAgentInput::Text(text))),
        other => {
            let blocks: Vec<ContentBlock> =
                serde_json::from_value(other).map_err(serde::de::Error::custom)?;
            Ok(Some(MultiAgentInput::Blocks(blocks)))
        }
    }
}

/// Per-execution state for multi-agent orchestration, created fresh each invocation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MultiAgentState {
    /// Execution start time in milliseconds since epoch.
    pub start_time: u64,
    /// Number of node executions started so far.
    pub steps: u64,
    /// All node results in completion order.
    #[serde(default)]
    pub results: Vec<NodeResult>,
    /// App-level key-value state accessible from hooks, edge handlers, and custom nodes.
    #[serde(serialize_with = "serialize_app", deserialize_with = "deserialize_app")]
    pub app: StateStore,
    #[serde(default)]
    nodes: IndexMap<String, NodeState>,
    /// The invocation's input, carried through an interrupt pause so a resumed run can re-enter
    /// nodes that never ran with the original content. Cleared on any non-`INTERRUPTED` exit.
    #[serde(
        rename = "_pendingInput",
        default,
        deserialize_with = "deserialize_pending_input",
        skip_serializing_if = "Option::is_none"
    )]
    pub pending_input: Option<MultiAgentInput>,
}

/// Detaches the app store (every other field is a plain value), so a clone never aliases the
/// original's state.
impl Clone for MultiAgentState {
    fn clone(&self) -> Self {
        Self {
            start_time: self.start_time,
            steps: self.steps,
            results: self.results.clone(),
            app: StateStore::from_map(self.app.get_all()),
            nodes: self.nodes.clone(),
            pending_input: self.pending_input.clone(),
        }
    }
}

impl Default for MultiAgentState {
    fn default() -> Self {
        Self {
            start_time: now_ms(),
            steps: 0,
            results: Vec::new(),
            app: StateStore::new(),
            nodes: IndexMap::new(),
            pending_input: None,
        }
    }
}

impl MultiAgentState {
    /// Fresh state tracking a [`NodeState`] per id in `node_ids`.
    pub fn new(node_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            nodes: node_ids
                .into_iter()
                .map(|id| (id.into(), NodeState::new()))
                .collect(),
            ..Self::default()
        }
    }

    /// State of the node `id`, or `None` if it is not tracked.
    pub fn node(&self, id: &str) -> Option<&NodeState> {
        self.nodes.get(id)
    }

    /// Mutable state of the node `id`, or `None` if it is not tracked.
    pub fn node_mut(&mut self, id: &str) -> Option<&mut NodeState> {
        self.nodes.get_mut(id)
    }

    /// Mutable state of the node `id`, starting to track it if it is new.
    pub fn node_or_insert(&mut self, id: impl Into<String>) -> &mut NodeState {
        self.nodes.entry(id.into()).or_default()
    }

    /// All tracked node states, by id.
    pub fn nodes(&self) -> &IndexMap<String, NodeState> {
        &self.nodes
    }
}

/// Clonable handle to the [`MultiAgentState`] an invocation shares between the orchestrator, its
/// nodes and its hooks, mirroring [`InterruptStateHandle`](crate::interrupt::InterruptStateHandle).
#[derive(Clone, Debug, Default)]
pub struct MultiAgentStateHandle(Arc<Mutex<MultiAgentState>>);

impl MultiAgentStateHandle {
    /// Wraps `state` in a shareable handle.
    pub fn new(state: MultiAgentState) -> Self {
        Self(Arc::new(Mutex::new(state)))
    }

    /// Runs `f` with the state locked for reading.
    ///
    /// Not reentrant: do not call other methods on this handle (or anything that reaches the
    /// state, such as a hook event's `state`) from `f`; the lock is held. For a single read,
    /// prefer [`snapshot`](Self::snapshot), [`results`](Self::results), [`steps`](Self::steps) or
    /// [`node`](Self::node), which copy out and release the lock.
    pub fn with<R>(&self, f: impl FnOnce(&MultiAgentState) -> R) -> R {
        f(&self.0.lock())
    }

    /// Runs `f` with the state locked for writing; use it for compound mutations.
    ///
    /// Not reentrant — see [`with`](Self::with).
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut MultiAgentState) -> R) -> R {
        f(&mut self.0.lock())
    }

    /// A detached copy of the whole state.
    pub fn snapshot(&self) -> MultiAgentState {
        self.0.lock().clone()
    }

    /// A copy of the node `id`'s state, or `None` if it is not tracked.
    pub fn node(&self, id: &str) -> Option<NodeState> {
        self.0.lock().node(id).cloned()
    }

    /// A copy of the node results collected so far, in completion order.
    pub fn results(&self) -> Vec<NodeResult> {
        self.0.lock().results.clone()
    }

    /// Number of node executions started so far.
    pub fn steps(&self) -> u64 {
        self.0.lock().steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupt::InterruptSource;
    use crate::types::ToolUseBlock;
    use serde_json::json;

    fn interrupt(id: &str) -> Interrupt {
        Interrupt::new(id, "approve", InterruptSource::MultiagentHook)
    }

    fn round_trip(value: &JsonValue) -> JsonValue {
        let result: NodeResult = serde_json::from_value(value.clone()).unwrap();
        serde_json::to_value(&result).unwrap()
    }

    #[test]
    fn round_trips_a_completed_result_with_text_content() {
        let result = NodeResult::new("writer", ResultStatus::Completed, 120)
            .with_content(vec![ContentBlock::text("hello")]);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json,
            json!({
                "type": "nodeResult",
                "nodeId": "writer",
                "status": "COMPLETED",
                "duration": 120,
                "content": [{"text": "hello"}],
            })
        );
        assert_eq!(round_trip(&json), json);
    }

    #[test]
    fn round_trips_a_failed_result_with_error() {
        let result = NodeResult::new("writer", ResultStatus::Failed, 5).with_error("boom");
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "FAILED");
        assert_eq!(json["error"], "boom");
        assert_eq!(round_trip(&json), json);
    }

    #[test]
    fn round_trips_structured_output_with_nested_objects() {
        let result = NodeResult::new("n", ResultStatus::Completed, 1)
            .with_structured_output(json!({"a": {"b": [1, 2, 3]}}));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["structuredOutput"], json!({"a": {"b": [1, 2, 3]}}));
        assert_eq!(round_trip(&json), json);
    }

    #[test]
    fn preserves_structured_output_when_value_is_null() {
        let result = NodeResult::new("n", ResultStatus::Completed, 1).with_structured_output(JsonValue::Null);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["structuredOutput"], JsonValue::Null);
        let back: NodeResult = serde_json::from_value(json).unwrap();
        assert_eq!(back.structured_output, Some(JsonValue::Null));
    }

    #[test]
    fn preserves_structured_output_when_value_is_a_primitive() {
        let result = NodeResult::new("n", ResultStatus::Completed, 1).with_structured_output(json!(42));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["structuredOutput"], json!(42));
        assert_eq!(round_trip(&json), json);
    }

    #[test]
    fn round_trips_multiple_content_blocks_including_tool_use() {
        let result = NodeResult::new("n", ResultStatus::Completed, 1).with_content(vec![
            ContentBlock::text("calling"),
            ContentBlock::ToolUse(ToolUseBlock::new("calc", "t1", json!({"x": 1}))),
        ]);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(round_trip(&json), json);
    }

    #[test]
    fn round_trips_a_cancelled_result_with_empty_content() {
        let json = serde_json::to_value(NodeResult::new("n", ResultStatus::Cancelled, 0)).unwrap();
        assert_eq!(json["status"], "CANCELLED");
        assert_eq!(json["content"], json!([]));
        assert_eq!(round_trip(&json), json);
    }

    #[test]
    fn omits_error_from_json_when_not_present() {
        let json = serde_json::to_value(NodeResult::new("n", ResultStatus::Completed, 1)).unwrap();
        assert!(json.get("error").is_none());
    }

    #[test]
    fn omits_structured_output_from_json_when_not_present() {
        let json = serde_json::to_value(NodeResult::new("n", ResultStatus::Completed, 1)).unwrap();
        assert!(json.get("structuredOutput").is_none());
    }

    #[test]
    fn round_trips_usage_with_all_fields() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 4,
            total_tokens: 14,
            cache_read_input_tokens: Some(2),
            cache_write_input_tokens: Some(3),
        };
        let json =
            serde_json::to_value(NodeResult::new("n", ResultStatus::Completed, 1).with_usage(usage)).unwrap();
        assert_eq!(
            json["usage"],
            json!({
                "inputTokens": 10,
                "outputTokens": 4,
                "totalTokens": 14,
                "cacheReadInputTokens": 2,
                "cacheWriteInputTokens": 3,
            })
        );
        assert_eq!(round_trip(&json), json);
    }

    #[test]
    fn omits_usage_from_json_when_not_present() {
        let json = serde_json::to_value(NodeResult::new("n", ResultStatus::Completed, 1)).unwrap();
        assert!(json.get("usage").is_none());
    }

    #[test]
    fn round_trips_a_fresh_node_state() {
        let state = NodeState::new();
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "PENDING");
        assert_eq!(json["terminus"], false);
        assert_eq!(json["results"], json!([]));
        assert_eq!(json["interrupts"], json!([]));
        assert!(json.get("interruptedSnapshot").is_none());
        let back: NodeState = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn round_trips_a_node_state_with_results() {
        let mut state = NodeState::new();
        state.status = Status::Completed;
        state.terminus = true;
        state
            .results
            .push(NodeResult::new("n", ResultStatus::Completed, 7));
        let json = serde_json::to_value(&state).unwrap();
        let back: NodeState = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.results.len(), 1);
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn preserves_content_accessor_after_round_trip() {
        let mut state = NodeState::new();
        state.results.push(
            NodeResult::new("n", ResultStatus::Completed, 1).with_content(vec![ContentBlock::text("last")]),
        );
        let back: NodeState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(back.content(), [ContentBlock::text("last")]);
    }

    #[test]
    fn content_accessor_is_empty_without_results() {
        assert!(NodeState::new().content().is_empty());
    }

    #[test]
    fn round_trips_interrupts_and_interrupted_snapshot_on_an_interrupted_node() {
        let mut state = NodeState::new();
        state.status = Status::Interrupted;
        state.interrupts = vec![interrupt("multiagent-hook:beforeNodeCall:a:approve")];
        state.interrupted_snapshot = Some(Snapshot {
            scope: crate::session::Scope::MultiAgent,
            schema_version: "1".to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            data: serde_json::Map::new(),
            app_data: serde_json::Map::new(),
        });
        let json = serde_json::to_value(&state).unwrap();
        let back: NodeState = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.interrupts.len(), 1);
        assert!(back.interrupted_snapshot.is_some());
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn clears_interrupted_snapshot_when_absent_from_the_serialized_state() {
        let mut state = NodeState::new();
        state.interrupted_snapshot = None;
        let json = serde_json::to_value(&state).unwrap();
        let back: NodeState = serde_json::from_value(json).unwrap();
        assert!(back.interrupted_snapshot.is_none());
    }

    #[test]
    fn round_trips_a_completed_multi_agent_result() {
        let result = MultiAgentResult::new(vec![NodeResult::new("a", ResultStatus::Completed, 3)], 10)
            .with_content(vec![ContentBlock::text("done")]);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["type"], "multiAgentResult");
        assert_eq!(json["status"], "COMPLETED");
        assert_eq!(json["duration"], 10);
        assert!(json.get("interrupts").is_none());
        let back: MultiAgentResult = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn round_trips_a_failed_multi_agent_result_with_error() {
        let result =
            MultiAgentResult::new(vec![NodeResult::new("a", ResultStatus::Failed, 3)], 10).with_error("bad");
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "FAILED");
        assert_eq!(json["error"], "bad");
        let back: MultiAgentResult = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn preserves_explicit_status_override() {
        let result = MultiAgentResult::new(vec![NodeResult::new("a", ResultStatus::Completed, 1)], 1)
            .with_status(ResultStatus::Cancelled);
        assert_eq!(result.status, ResultStatus::Cancelled);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "CANCELLED");
    }

    #[test]
    fn round_trips_with_empty_results_and_content() {
        let json = serde_json::to_value(MultiAgentResult::new(vec![], 0)).unwrap();
        assert_eq!(json["status"], "COMPLETED");
        assert_eq!(json["results"], json!([]));
        assert_eq!(json["content"], json!([]));
        assert_eq!(
            json["usage"],
            json!({"inputTokens": 0, "outputTokens": 0, "totalTokens": 0})
        );
    }

    #[test]
    fn preserves_aggregated_usage_after_round_trip() {
        let usage = |n: u64| Usage {
            input_tokens: n,
            output_tokens: n,
            total_tokens: 2 * n,
            ..Default::default()
        };
        let result = MultiAgentResult::new(
            vec![
                NodeResult::new("a", ResultStatus::Completed, 1).with_usage(usage(2)),
                NodeResult::new("b", ResultStatus::Completed, 1).with_usage(usage(3)),
            ],
            2,
        );
        assert_eq!(result.usage.input_tokens, 5);
        assert_eq!(result.usage.total_tokens, 10);
        let back: MultiAgentResult = serde_json::from_value(serde_json::to_value(&result).unwrap()).unwrap();
        assert_eq!(back.usage, result.usage);
    }

    #[test]
    fn collects_interrupts_from_node_results() {
        let result = MultiAgentResult::new(
            vec![NodeResult::new("a", ResultStatus::Interrupted, 1).with_interrupts(vec![interrupt("i1")])],
            1,
        );
        assert_eq!(result.status, ResultStatus::Interrupted);
        assert_eq!(result.interrupts.len(), 1);
        assert_eq!(
            serde_json::to_value(&result).unwrap()["interrupts"][0]["id"],
            json!("i1")
        );
    }

    #[test]
    fn display_joins_text_content() {
        let result = MultiAgentResult::new(vec![], 0)
            .with_content(vec![ContentBlock::text("one"), ContentBlock::text("two")]);
        assert_eq!(result.to_string(), "one\ntwo");
    }

    #[test]
    fn returns_completed_when_all_node_results_are_completed() {
        let results = vec![
            NodeResult::new("a", ResultStatus::Completed, 1),
            NodeResult::new("b", ResultStatus::Completed, 1),
        ];
        assert_eq!(resolve_status(&results), ResultStatus::Completed);
    }

    #[test]
    fn failed_outranks_interrupted() {
        let results = vec![
            NodeResult::new("a", ResultStatus::Interrupted, 1),
            NodeResult::new("b", ResultStatus::Failed, 1),
        ];
        assert_eq!(resolve_status(&results), ResultStatus::Failed);
    }

    #[test]
    fn interrupted_outranks_cancelled() {
        let results = vec![
            NodeResult::new("a", ResultStatus::Cancelled, 1),
            NodeResult::new("b", ResultStatus::Interrupted, 1),
        ];
        assert_eq!(resolve_status(&results), ResultStatus::Interrupted);
    }

    #[test]
    fn cancelled_outranks_completed() {
        let results = vec![
            NodeResult::new("a", ResultStatus::Completed, 1),
            NodeResult::new("b", ResultStatus::Cancelled, 1),
        ];
        assert_eq!(resolve_status(&results), ResultStatus::Cancelled);
    }

    #[test]
    fn failed_outranks_cancelled() {
        let results = vec![
            NodeResult::new("a", ResultStatus::Cancelled, 1),
            NodeResult::new("b", ResultStatus::Failed, 1),
        ];
        assert_eq!(resolve_status(&results), ResultStatus::Failed);
    }

    #[test]
    fn round_trips_a_fresh_state_with_node_ids() {
        let state = MultiAgentState::new(["a", "b"]);
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["steps"], 0);
        assert_eq!(json["results"], json!([]));
        assert_eq!(json["app"], json!({}));
        assert!(json["nodes"]["a"].is_object());
        assert!(json["nodes"]["b"].is_object());
        assert!(json.get("_pendingInput").is_none());
        let back: MultiAgentState = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
    }

    #[test]
    fn round_trips_state_with_steps_and_results() {
        let mut state = MultiAgentState::new(["a"]);
        state.steps = 3;
        state
            .results
            .push(NodeResult::new("a", ResultStatus::Completed, 1));
        let back: MultiAgentState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(back.steps, 3);
        assert_eq!(back.results.len(), 1);
    }

    #[test]
    fn round_trips_app_state() {
        let state = MultiAgentState::new(["a"]);
        state.app.set("count", 2);
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["app"], json!({"count": 2}));
        let back: MultiAgentState = serde_json::from_value(json).unwrap();
        assert_eq!(back.app.get("count"), Some(json!(2)));
    }

    #[test]
    fn round_trips_node_states_with_modified_status_and_results() {
        let mut state = MultiAgentState::new(["a"]);
        let node = state.node_mut("a").unwrap();
        node.status = Status::Completed;
        node.results
            .push(NodeResult::new("a", ResultStatus::Completed, 1));
        let back: MultiAgentState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        let node = back.node("a").unwrap();
        assert_eq!(node.status, Status::Completed);
        assert_eq!(node.results.len(), 1);
    }

    #[test]
    fn round_trips_an_empty_state_with_no_node_ids() {
        let state = MultiAgentState::new(Vec::<String>::new());
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["nodes"], json!({}));
        let back: MultiAgentState = serde_json::from_value(json).unwrap();
        assert!(back.nodes().is_empty());
    }

    #[test]
    fn handles_a_missing_nodes_key_gracefully() {
        let back: MultiAgentState =
            serde_json::from_value(json!({"startTime": 1, "steps": 0, "results": [], "app": {}})).unwrap();
        assert!(back.nodes().is_empty());
    }

    #[test]
    fn preserves_start_time_exactly() {
        let state = MultiAgentState::new(["a"]);
        let back: MultiAgentState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(back.start_time, state.start_time);
    }

    #[test]
    fn round_trips_pending_input_as_a_string() {
        let mut state = MultiAgentState::new(["a"]);
        state.pending_input = Some(MultiAgentInput::Text("go".to_owned()));
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["_pendingInput"], json!("go"));
        let back: MultiAgentState = serde_json::from_value(json).unwrap();
        assert_eq!(back.pending_input, Some(MultiAgentInput::Text("go".to_owned())));
    }

    #[test]
    fn rehydrates_pending_input_content_blocks() {
        let mut state = MultiAgentState::new(["a"]);
        state.pending_input = Some(MultiAgentInput::Blocks(vec![ContentBlock::text("hi")]));
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["_pendingInput"], json!([{"text": "hi"}]));
        let back: MultiAgentState = serde_json::from_value(json).unwrap();
        assert_eq!(
            back.pending_input,
            Some(MultiAgentInput::Blocks(vec![ContentBlock::text("hi")]))
        );
    }

    #[test]
    fn state_handle_shares_one_state() {
        let handle = MultiAgentStateHandle::new(MultiAgentState::new(["a"]));
        let clone = handle.clone();
        clone.with_mut(|state| state.steps += 1);
        assert_eq!(handle.with(|state| state.steps), 1);
        assert_eq!(handle.node("a").unwrap().status, Status::Pending);
        assert!(handle.node("missing").is_none());
    }

    #[test]
    fn node_or_insert_tracks_new_nodes() {
        let mut state = MultiAgentState::new(Vec::<String>::new());
        state.node_or_insert("late").status = Status::Executing;
        assert_eq!(state.node("late").unwrap().status, Status::Executing);
    }

    #[test]
    fn handle_accessors_copy_out_without_holding_the_lock() {
        let handle = MultiAgentStateHandle::new(MultiAgentState::new(["a"]));
        handle.with_mut(|state| {
            state.steps = 3;
            state
                .results
                .push(NodeResult::new("a", ResultStatus::Completed, 1));
            state.app.set("topic", "rust");
        });

        assert_eq!(handle.steps(), 3);
        assert_eq!(handle.results().len(), 1);

        let copy = handle.snapshot();
        // Reading inside `with` would deadlock if `snapshot` still held the lock.
        handle.with(|state| assert_eq!(state.steps, copy.steps));

        copy.app.set("topic", "ts");
        assert_eq!(handle.snapshot().app.get("topic"), Some(json!("rust")));
    }

    #[test]
    fn update_setters_build_the_same_overlay_as_the_struct_fields() {
        let update = NodeResultUpdate::default()
            .with_status(ResultStatus::Failed)
            .with_content(vec![ContentBlock::text("new")])
            .with_error("boom")
            .with_structured_output(json!({ "ok": false }))
            .with_usage(Usage::default())
            .with_interrupts(vec![interrupt("i-1")]);
        assert_eq!(
            update,
            NodeResultUpdate {
                status: Some(ResultStatus::Failed),
                content: Some(vec![ContentBlock::text("new")]),
                error: Some("boom".to_owned()),
                structured_output: Some(json!({ "ok": false })),
                usage: Some(Usage::default()),
                interrupts: Some(vec![interrupt("i-1")]),
            }
        );
    }

    #[test]
    fn merge_overlays_only_set_update_fields() {
        let base =
            NodeResult::new("n", ResultStatus::Completed, 1).with_content(vec![ContentBlock::text("base")]);
        let merged = base.clone().merge(NodeResultUpdate {
            status: Some(ResultStatus::Failed),
            error: Some("boom".to_owned()),
            ..Default::default()
        });
        assert_eq!(merged.status, ResultStatus::Failed);
        assert_eq!(merged.error.as_deref(), Some("boom"));
        assert_eq!(merged.content, base.content);
    }
}
