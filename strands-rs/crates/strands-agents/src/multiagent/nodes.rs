//! Nodes: the unit of work an orchestrator drives (`strands-ts/src/multiagent/nodes.ts`).
//!
//! [`Node`] implementations provide [`Node::handle`]; [`run_node`] is the shared wrapper around
//! it (TS `Node.stream`) that owns state bookkeeping and emits the terminal [`NodeResultEvent`].

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::StreamExt;
use serde::de::{Deserialize, Deserializer};
use serde::{Serialize, Serializer};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::events::{MultiAgentStreamEvent, NodeInnerEvent, NodeResultEvent, NodeStreamUpdateEvent};
use super::multiagent::{
    drop_stale_interrupted_result, MultiAgent, MultiAgentInput, MultiAgentInvokeOptions,
};
use super::state::{now_ms, MultiAgentStateHandle, NodeResult, NodeResultUpdate, ResultStatus, Status};
use crate::agent::{Agent, AgentStreamEvent, InvocationState, InvokeOptions, TakeSnapshotOptions};
use crate::error::{Error, Result};
use crate::types::{JsonValue, StopReason};

/// What a node wraps: an agent, a nested orchestrator, or a custom implementation
/// (TS `NodeType = 'agentNode' | 'multiAgentNode' | (string & {})`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeType {
    /// The node wraps an `Agent`.
    Agent,
    /// The node wraps a nested orchestrator.
    MultiAgent,
    /// A custom node type, identified by its own name.
    Custom(String),
}

impl NodeType {
    /// The TS wire string for this node type.
    pub fn as_str(&self) -> &str {
        match self {
            NodeType::Agent => "agentNode",
            NodeType::MultiAgent => "multiAgentNode",
            NodeType::Custom(name) => name,
        }
    }
}

impl From<&str> for NodeType {
    fn from(name: &str) -> Self {
        match name {
            "agentNode" => NodeType::Agent,
            "multiAgentNode" => NodeType::MultiAgent,
            other => NodeType::Custom(other.to_owned()),
        }
    }
}

impl Serialize for NodeType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for NodeType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(NodeType::from(String::deserialize(deserializer)?.as_str()))
    }
}

/// Channel a running node writes its stream events to, mirroring the agent event loop's sink.
pub type NodeSink = mpsc::Sender<Result<MultiAgentStreamEvent>>;

/// Per-node configuration (TS `NodeConfig`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeConfig {
    /// What this node does; defaults to the wrapped agent's or orchestrator's description.
    pub description: Option<String>,
}

/// Per-invocation options an orchestrator passes to a node (TS `NodeInputOptions`).
#[derive(Clone, Debug, Default)]
pub struct NodeInputOptions {
    /// Structured output schema for this node invocation.
    pub structured_output_schema: Option<JsonValue>,
    /// State shared by every node of one orchestrator invocation; clones share one map.
    pub invocation_state: InvocationState,
    /// Cancellation forwarded to the node's agent or orchestrator.
    pub cancel: Option<CancellationToken>,
}

impl NodeInputOptions {
    /// Request structured output from this node's invocation.
    pub fn with_structured_output_schema(mut self, schema: JsonValue) -> Self {
        self.structured_output_schema = Some(schema);
        self
    }

    /// Share the orchestrator's invocation state with this node.
    pub fn with_invocation_state(mut self, state: InvocationState) -> Self {
        self.invocation_state = state;
        self
    }

    /// Forward a cancellation token (per-node timeout or external cancel).
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// A unit of work in a multi-agent orchestration.
///
/// Implementations provide [`Node::handle`]; orchestrators drive them through [`run_node`], which
/// adds the duration, status and result bookkeeping every node shares.
pub trait Node: Send + Sync {
    /// Unique id of this node within the orchestration.
    fn id(&self) -> &str;

    /// What this node wraps.
    fn node_type(&self) -> NodeType;

    /// This node's configuration.
    fn config(&self) -> &NodeConfig;

    /// Run the node, writing stream events to `events` and returning the fields of its result
    /// that are not framework-managed.
    fn handle<'a>(
        &'a self,
        input: MultiAgentInput,
        state: &'a MultiAgentStateHandle,
        options: &'a NodeInputOptions,
        events: &'a NodeSink,
    ) -> BoxFuture<'a, Result<NodeResultUpdate>>;
}

/// Runs `node` with the bookkeeping every node shares (TS `Node.stream`): transitions the node
/// state, times the run, turns an error into a `FAILED`/`CANCELLED` result, records the result on
/// the node, and emits the terminal [`NodeResultEvent`].
pub async fn run_node(
    node: &dyn Node,
    input: MultiAgentInput,
    state: &MultiAgentStateHandle,
    options: &NodeInputOptions,
    events: &NodeSink,
) -> NodeResult {
    let node_id = node.id().to_owned();
    let start_time = state.with_mut(|state| {
        drop_stale_interrupted_result(&node_id, state);
        let node_state = state.node_or_insert(&node_id);
        node_state.status = Status::Executing;
        node_state.start_time = now_ms();
        node_state.start_time
    });

    let result = match node.handle(input, state, options, events).await {
        Ok(update) => {
            let interrupted = update.interrupts.as_ref().is_some_and(|i| !i.is_empty());
            let status = if interrupted {
                ResultStatus::Interrupted
            } else {
                ResultStatus::Completed
            };
            NodeResult::new(&node_id, status, now_ms().saturating_sub(start_time)).merge(update)
        }
        Err(error) => {
            // Orchestrator cancellation (short-circuit or external) maps a thrown error to
            // CANCELLED — the node was stopped, not broken.
            let cancelled = options
                .cancel
                .as_ref()
                .is_some_and(|cancel| cancel.is_cancelled());
            let status = if cancelled {
                ResultStatus::Cancelled
            } else {
                tracing::warn!(node_id = %node_id, error = %error, "node execution failed");
                ResultStatus::Failed
            };
            NodeResult::new(&node_id, status, now_ms().saturating_sub(start_time))
                .with_error(error.to_string())
        }
    };

    state.with_mut(|state| {
        let node_state = state.node_or_insert(&node_id);
        node_state.status = result.status.into();
        node_state.results.push(result.clone());
        node_state.interrupts = result.interrupts.clone();
        // Clear the stored snapshot on non-INTERRUPTED terminal states; `handle` repopulates it
        // when this run itself interrupted.
        if result.status != ResultStatus::Interrupted {
            node_state.interrupted_snapshot = None;
        }
    });

    let _ = events
        .send(Ok(MultiAgentStreamEvent::NodeResult(NodeResultEvent {
            node_id,
            node_type: node.node_type(),
            state: state.clone(),
            result: result.clone(),
            invocation_state: options.invocation_state.clone(),
        })))
        .await;

    result
}

/// Options for an [`AgentNode`].
#[derive(Clone, Debug)]
pub struct AgentNodeOptions {
    /// The agent to wrap.
    pub agent: Agent,
    /// What this node does; defaults to the agent's description.
    pub description: Option<String>,
    /// Per-node wall-clock ceiling, overriding the orchestrator's node timeout.
    pub timeout: Option<Duration>,
    /// Let the agent accumulate messages and state across node runs instead of snapshotting it.
    pub preserve_context: bool,
}

impl AgentNodeOptions {
    /// Wrap `agent` with default node options.
    pub fn new(agent: Agent) -> Self {
        Self {
            agent,
            description: None,
            timeout: None,
            preserve_context: false,
        }
    }

    /// Override the node's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set this node's wall-clock ceiling; [`Duration::MAX`] opts out of the orchestrator's.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Keep the wrapped agent's state across node runs.
    pub fn with_preserve_context(mut self, preserve_context: bool) -> Self {
        self.preserve_context = preserve_context;
        self
    }
}

impl From<Agent> for AgentNodeOptions {
    fn from(agent: Agent) -> Self {
        Self::new(agent)
    }
}

/// A node wrapping an [`Agent`].
///
/// The agent's state is snapshot before the run and restored after it, so the node leaves it
/// observably unchanged; [`AgentNodeOptions::preserve_context`] opts out.
pub struct AgentNode {
    id: String,
    config: NodeConfig,
    agent: Agent,
    timeout: Option<Duration>,
    preserve_context: bool,
}

impl AgentNode {
    /// Wrap an agent as a node, taking the node's id from the agent's.
    ///
    /// # Errors
    /// Returns an error when `timeout` is zero.
    pub fn new(options: impl Into<AgentNodeOptions>) -> Result<Self> {
        let options = options.into();
        let id = options.agent.id().to_owned();
        if options.timeout == Some(Duration::ZERO) {
            return Err(Error::other(format!(
                "timeout=<0>, node_id=<{id}> | must be greater than zero"
            )));
        }
        let description = options
            .description
            .or_else(|| options.agent.description().map(str::to_owned));
        Ok(Self {
            id,
            config: NodeConfig { description },
            agent: options.agent,
            timeout: options.timeout,
            preserve_context: options.preserve_context,
        })
    }

    /// The wrapped agent.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// This node's wall-clock ceiling, if it overrides the orchestrator's.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Whether the wrapped agent keeps its state across node runs.
    pub fn preserve_context(&self) -> bool {
        self.preserve_context
    }

    async fn run(
        &self,
        input: MultiAgentInput,
        state: &MultiAgentStateHandle,
        options: &NodeInputOptions,
        events: &NodeSink,
    ) -> Result<NodeResultUpdate> {
        let mut invoke = InvokeOptions::default().with_invocation_state(options.invocation_state.clone());
        if let Some(schema) = &options.structured_output_schema {
            invoke = invoke.with_structured_output_schema(schema.clone());
        }
        if let Some(cancel) = &options.cancel {
            invoke = invoke.with_cancel(cancel.clone());
        }

        let mut stream = self.agent.stream_with(input, invoke);
        let mut agent_result = None;
        while let Some(event) = stream.next().await {
            let event = event?;
            if let AgentStreamEvent::AgentResult(result) = &event {
                agent_result = Some(result.result.clone());
            }
            let _ = events
                .send(Ok(MultiAgentStreamEvent::NodeStreamUpdate(
                    NodeStreamUpdateEvent {
                        node_id: self.id.clone(),
                        node_type: self.node_type(),
                        state: state.clone(),
                        inner: NodeInnerEvent::Agent(Box::new(event)),
                        invocation_state: options.invocation_state.clone(),
                    },
                )))
                .await;
        }

        let agent_result = agent_result
            .ok_or_else(|| Error::other(format!("node_id=<{}> | agent produced no result", self.id)))?;
        let interrupted =
            agent_result.stop_reason == StopReason::Interrupt && !agent_result.interrupts.is_empty();

        // Capture post-interrupt state so the next resume cycle restores the paused agent.
        if interrupted {
            let snapshot = self.agent.take_snapshot(TakeSnapshotOptions::default());
            state.with_mut(|state| state.node_or_insert(&self.id).interrupted_snapshot = Some(snapshot));
        }

        Ok(NodeResultUpdate {
            content: Some(agent_result.last_message.content.clone()),
            structured_output: agent_result.structured_output.clone(),
            usage: Some(agent_result.metrics.accumulated_usage.clone()),
            interrupts: interrupted.then(|| agent_result.interrupts.clone()),
            ..Default::default()
        })
    }
}

impl Node for AgentNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> NodeType {
        NodeType::Agent
    }

    fn config(&self) -> &NodeConfig {
        &self.config
    }

    fn handle<'a>(
        &'a self,
        input: MultiAgentInput,
        state: &'a MultiAgentStateHandle,
        options: &'a NodeInputOptions,
        events: &'a NodeSink,
    ) -> BoxFuture<'a, Result<NodeResultUpdate>> {
        Box::pin(async move {
            let snapshot =
                (!self.preserve_context).then(|| self.agent.take_snapshot(TakeSnapshotOptions::default()));
            let result = self.run(input, state, options, events).await;
            // Restore pre-run state — keeps the agent observably unchanged across runs.
            if let Some(snapshot) = snapshot {
                self.agent.load_snapshot(&snapshot)?;
            }
            result
        })
    }
}

/// Options for a [`MultiAgentNode`].
#[derive(Clone)]
pub struct MultiAgentNodeOptions {
    /// The orchestrator to wrap.
    pub orchestrator: Arc<dyn MultiAgent>,
    /// What this node does; defaults to the orchestrator's description.
    pub description: Option<String>,
}

impl MultiAgentNodeOptions {
    /// Wrap `orchestrator` with default node options.
    pub fn new(orchestrator: Arc<dyn MultiAgent>) -> Self {
        Self {
            orchestrator,
            description: None,
        }
    }

    /// Override the node's description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl From<Arc<dyn MultiAgent>> for MultiAgentNodeOptions {
    fn from(orchestrator: Arc<dyn MultiAgent>) -> Self {
        Self::new(orchestrator)
    }
}

/// A node wrapping a nested orchestrator (TS `MultiAgentNode`).
///
/// Inner [`NodeStreamUpdateEvent`]s pass through unchanged to preserve the original node's
/// identity; every other inner event is re-wrapped under this node's identity.
pub struct MultiAgentNode {
    id: String,
    config: NodeConfig,
    orchestrator: Arc<dyn MultiAgent>,
}

impl MultiAgentNode {
    /// Wrap an orchestrator as a node, taking the node's id from the orchestrator's.
    pub fn new(options: impl Into<MultiAgentNodeOptions>) -> Self {
        let options = options.into();
        let description = options
            .description
            .or_else(|| options.orchestrator.description().map(str::to_owned));
        Self {
            id: options.orchestrator.id().to_owned(),
            config: NodeConfig { description },
            orchestrator: options.orchestrator,
        }
    }

    /// The wrapped orchestrator.
    pub fn orchestrator(&self) -> &Arc<dyn MultiAgent> {
        &self.orchestrator
    }
}

impl Node for MultiAgentNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> NodeType {
        NodeType::MultiAgent
    }

    fn config(&self) -> &NodeConfig {
        &self.config
    }

    fn handle<'a>(
        &'a self,
        input: MultiAgentInput,
        state: &'a MultiAgentStateHandle,
        options: &'a NodeInputOptions,
        events: &'a NodeSink,
    ) -> BoxFuture<'a, Result<NodeResultUpdate>> {
        Box::pin(async move {
            let mut invoke =
                MultiAgentInvokeOptions::default().with_invocation_state(options.invocation_state.clone());
            if let Some(cancel) = &options.cancel {
                invoke = invoke.with_cancel(cancel.clone());
            }

            let mut stream = self.orchestrator.stream_with(input, invoke);
            let mut inner_result = None;
            while let Some(event) = stream.next().await {
                let event = event?;
                if let MultiAgentStreamEvent::MultiAgentResult(result) = &event {
                    inner_result = Some(result.result.clone());
                }
                let forwarded = match event {
                    MultiAgentStreamEvent::NodeStreamUpdate(_) => event,
                    inner => MultiAgentStreamEvent::NodeStreamUpdate(NodeStreamUpdateEvent {
                        node_id: self.id.clone(),
                        node_type: self.node_type(),
                        state: state.clone(),
                        inner: NodeInnerEvent::MultiAgent(Box::new(inner)),
                        invocation_state: options.invocation_state.clone(),
                    }),
                };
                let _ = events.send(Ok(forwarded)).await;
            }

            let inner_result = inner_result.ok_or_else(|| {
                Error::other(format!("node_id=<{}> | orchestrator produced no result", self.id))
            })?;

            Ok(NodeResultUpdate {
                content: Some(inner_result.content),
                usage: Some(inner_result.usage),
                status: (inner_result.status != ResultStatus::Completed).then_some(inner_result.status),
                error: inner_result.error,
                interrupts: (!inner_result.interrupts.is_empty()).then_some(inner_result.interrupts),
                ..Default::default()
            })
        })
    }
}

/// A node as accepted by an orchestrator builder (TS `NodeDefinition`).
///
/// Pass an [`Agent`] or an `Arc<dyn MultiAgent>` for the simple case, the options structs for
/// per-node configuration, or an `Arc<dyn Node>` for full control.
#[derive(Clone)]
pub enum NodeDefinition {
    /// An agent, with its node options.
    Agent(AgentNodeOptions),
    /// A nested orchestrator, with its node options.
    MultiAgent(MultiAgentNodeOptions),
    /// An already-built node.
    Node(Arc<dyn Node>),
}

impl NodeDefinition {
    /// Build the node this definition describes.
    ///
    /// # Errors
    /// Propagates [`AgentNode::new`] validation failures.
    pub fn into_node(self) -> Result<Arc<dyn Node>> {
        match self {
            NodeDefinition::Agent(options) => Ok(Arc::new(AgentNode::new(options)?)),
            NodeDefinition::MultiAgent(options) => Ok(Arc::new(MultiAgentNode::new(options))),
            NodeDefinition::Node(node) => Ok(node),
        }
    }
}

impl From<Agent> for NodeDefinition {
    fn from(agent: Agent) -> Self {
        NodeDefinition::Agent(AgentNodeOptions::new(agent))
    }
}

impl From<AgentNodeOptions> for NodeDefinition {
    fn from(options: AgentNodeOptions) -> Self {
        NodeDefinition::Agent(options)
    }
}

impl From<Arc<dyn MultiAgent>> for NodeDefinition {
    fn from(orchestrator: Arc<dyn MultiAgent>) -> Self {
        NodeDefinition::MultiAgent(MultiAgentNodeOptions::new(orchestrator))
    }
}

impl From<MultiAgentNodeOptions> for NodeDefinition {
    fn from(options: MultiAgentNodeOptions) -> Self {
        NodeDefinition::MultiAgent(options)
    }
}

impl From<Arc<dyn Node>> for NodeDefinition {
    fn from(node: Arc<dyn Node>) -> Self {
        NodeDefinition::Node(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::state::MultiAgentState;
    use serde_json::json;

    #[test]
    fn round_trips_known_and_custom_node_types() {
        for (ty, wire) in [
            (NodeType::Agent, "agentNode"),
            (NodeType::MultiAgent, "multiAgentNode"),
            (NodeType::Custom("a2aNode".to_owned()), "a2aNode"),
        ] {
            assert_eq!(serde_json::to_value(&ty).unwrap(), json!(wire));
            assert_eq!(serde_json::from_value::<NodeType>(json!(wire)).unwrap(), ty);
        }
    }

    fn handle() -> MultiAgentStateHandle {
        MultiAgentStateHandle::new(MultiAgentState::new(["n1"]))
    }
}
