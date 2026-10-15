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
    use crate::hooks::HookRegistry;
    use crate::multiagent::events::MultiAgentResultEvent;
    use crate::multiagent::state::{MultiAgentResult, MultiAgentState};
    use crate::multiagent::MultiAgentStream;
    use crate::test_support::{MockMessageModel, MockResponse};
    use crate::types::{ContentBlock, Message};
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

    fn state() -> MultiAgentStateHandle {
        MultiAgentStateHandle::new(MultiAgentState::new(["test-node", "fail-node"]))
    }

    /// Drains a node's sink after the run so tests can assert on the emitted events.
    async fn collect(
        run: impl AsyncFnOnce(&NodeSink) -> NodeNodeOutcome,
    ) -> (Vec<MultiAgentStreamEvent>, NodeNodeOutcome) {
        let (tx, mut rx) = mpsc::channel(256);
        let outcome = run(&tx).await;
        drop(tx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event.unwrap());
        }
        (events, outcome)
    }

    type NodeNodeOutcome = Result<NodeResultUpdate>;

    /// Node whose `handle` returns a canned outcome (TS `TestNode`).
    struct TestNode {
        id: String,
        config: NodeConfig,
        outcome: Box<dyn Fn() -> Result<NodeResultUpdate> + Send + Sync>,
    }

    impl TestNode {
        fn new(id: &str, outcome: impl Fn() -> Result<NodeResultUpdate> + Send + Sync + 'static) -> Self {
            Self {
                id: id.to_owned(),
                config: NodeConfig::default(),
                outcome: Box::new(outcome),
            }
        }
    }

    impl Node for TestNode {
        fn id(&self) -> &str {
            &self.id
        }

        fn node_type(&self) -> NodeType {
            NodeType::Custom("node".to_owned())
        }

        fn config(&self) -> &NodeConfig {
            &self.config
        }

        fn handle<'a>(
            &'a self,
            _input: MultiAgentInput,
            _state: &'a MultiAgentStateHandle,
            _options: &'a NodeInputOptions,
            _events: &'a NodeSink,
        ) -> BoxFuture<'a, Result<NodeResultUpdate>> {
            Box::pin(async move { (self.outcome)() })
        }
    }

    async fn drive(
        node: &dyn Node,
        state: &MultiAgentStateHandle,
    ) -> (Vec<MultiAgentStreamEvent>, NodeResult) {
        let (tx, mut rx) = mpsc::channel(256);
        let options = NodeInputOptions::default();
        let result = run_node(node, MultiAgentInput::None, state, &options, &tx).await;
        drop(tx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event.unwrap());
        }
        (events, result)
    }

    #[tokio::test]
    async fn returns_completed_node_result_on_successful_execution() {
        let content = vec![ContentBlock::Text("result".into())];
        let expected = content.clone();
        let node = TestNode::new("test-node", move || {
            Ok(NodeResultUpdate {
                content: Some(expected.clone()),
                ..Default::default()
            })
        });
        let state = state();

        let (events, result) = drive(&node, &state).await;

        assert_eq!(result.node_id, "test-node");
        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(result.content, content);
        assert_eq!(result.error, None);

        let event = events
            .iter()
            .find_map(|event| match event {
                MultiAgentStreamEvent::NodeResult(event) => Some(event),
                _ => None,
            })
            .expect("node result event");
        assert_eq!(event.node_id, "test-node");
        assert_eq!(event.node_type, NodeType::Custom("node".to_owned()));
        assert_eq!(event.result, result);
        assert_eq!(state.node("test-node").unwrap().status, Status::Completed);
    }

    #[tokio::test]
    async fn catches_errors_and_returns_failed_node_result() {
        let node = TestNode::new("fail-node", || Err(Error::other("boom")));
        let state = state();

        let (events, result) = drive(&node, &state).await;

        assert_eq!(result.status, ResultStatus::Failed);
        assert_eq!(result.content, vec![]);
        assert_eq!(result.error.as_deref(), Some("boom"));
        assert!(events
            .iter()
            .any(|event| matches!(event, MultiAgentStreamEvent::NodeResult(_))));
        assert_eq!(state.node("fail-node").unwrap().status, Status::Failed);
    }

    #[tokio::test]
    async fn maps_an_error_to_cancelled_when_the_run_was_cancelled() {
        let node = TestNode::new("test-node", || Err(Error::other("boom")));
        let state = state();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let options = NodeInputOptions::default().with_cancel(cancel);
        let (tx, _rx) = mpsc::channel(256);

        let result = run_node(&node, MultiAgentInput::None, &state, &options, &tx).await;

        assert_eq!(result.status, ResultStatus::Cancelled);
    }

    fn agent_with(responses: Vec<MockResponse>) -> (Agent, Arc<MockMessageModel>) {
        let model = MockMessageModel::shared(responses);
        let agent = Agent::builder().model(model.clone()).build().unwrap();
        (agent, model)
    }

    fn agent_node(agent: Agent) -> AgentNode {
        AgentNode::new(agent).unwrap()
    }

    #[test]
    fn rejects_a_zero_timeout() {
        let (agent, _) = agent_with(vec![]);
        let error = AgentNode::new(AgentNodeOptions::new(agent).with_timeout(Duration::ZERO))
            .err()
            .expect("zero timeout is rejected");
        assert!(error.to_string().contains("must be greater than zero"), "{error}");
    }

    #[test]
    fn accepts_a_positive_timeout() {
        let (agent, _) = agent_with(vec![]);
        let node = AgentNode::new(AgentNodeOptions::new(agent).with_timeout(Duration::from_secs(5))).unwrap();
        assert_eq!(node.timeout(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn accepts_duration_max_as_an_explicit_timeout_opt_out() {
        let (agent, _) = agent_with(vec![]);
        let node = AgentNode::new(AgentNodeOptions::new(agent).with_timeout(Duration::MAX)).unwrap();
        assert_eq!(node.timeout(), Some(Duration::MAX));
    }

    #[test]
    fn defaults_preserve_context_to_false() {
        let (agent, _) = agent_with(vec![]);
        assert!(!agent_node(agent).preserve_context());
    }

    #[test]
    fn stores_the_preserve_context_flag_when_provided() {
        let (agent, _) = agent_with(vec![]);
        let node = AgentNode::new(AgentNodeOptions::new(agent).with_preserve_context(true)).unwrap();
        assert!(node.preserve_context());
    }

    #[test]
    fn exposes_the_wrapped_agent() {
        let (agent, _) = agent_with(vec![]);
        let node = agent_node(agent.clone());
        assert_eq!(node.agent().id(), agent.id());
        assert_eq!(node.id(), agent.id());
    }

    #[tokio::test]
    async fn wraps_agent_events_and_returns_content() {
        let (agent, _) = agent_with(vec![MockResponse::message(Message::assistant("hello"))]);
        let node = agent_node(agent);
        let state = MultiAgentStateHandle::new(MultiAgentState::new([node.id().to_owned()]));
        let options = NodeInputOptions::default();

        let (events, update) = collect(async |tx| node.handle("go".into(), &state, &options, tx).await).await;
        let update = update.unwrap();

        assert_eq!(update.content, Some(vec![ContentBlock::Text("hello".into())]));
        assert!(!events.is_empty());
        for event in &events {
            match event {
                MultiAgentStreamEvent::NodeStreamUpdate(event) => {
                    assert_eq!(event.node_type, NodeType::Agent);
                    assert_eq!(event.inner.source(), "agent");
                }
                other => panic!("unexpected event: {}", other.type_name()),
            }
        }
    }

    #[tokio::test]
    async fn restores_agent_messages_after_execution() {
        let (agent, _) = agent_with(vec![MockResponse::message(Message::assistant("hello"))]);
        let node = agent_node(agent.clone());
        let state = MultiAgentStateHandle::new(MultiAgentState::new([node.id().to_owned()]));
        let options = NodeInputOptions::default();

        collect(async |tx| node.handle("go".into(), &state, &options, tx).await)
            .await
            .1
            .unwrap();

        assert_eq!(agent.messages(), vec![]);
    }

    #[tokio::test]
    async fn retains_agent_messages_across_executions_when_preserve_context_is_true() {
        let (agent, _) = agent_with(vec![
            MockResponse::message(Message::assistant("first")),
            MockResponse::message(Message::assistant("second")),
        ]);
        let node = AgentNode::new(AgentNodeOptions::new(agent.clone()).with_preserve_context(true)).unwrap();
        let state = MultiAgentStateHandle::new(MultiAgentState::new([node.id().to_owned()]));
        let options = NodeInputOptions::default();

        for _ in 0..2 {
            collect(async |tx| node.handle("go".into(), &state, &options, tx).await)
                .await
                .1
                .unwrap();
        }

        assert_eq!(
            agent.messages().len(),
            4,
            "two user turns and two assistant turns"
        );
    }

    #[tokio::test]
    async fn retains_app_state_mutations_across_executions_when_preserve_context_is_true() {
        let (agent, _) = agent_with(vec![MockResponse::message(Message::assistant("hello"))]);
        let node = AgentNode::new(AgentNodeOptions::new(agent.clone()).with_preserve_context(true)).unwrap();
        let state = MultiAgentStateHandle::new(MultiAgentState::new([node.id().to_owned()]));
        let options = NodeInputOptions::default();

        agent.state().set("seen", 1);
        collect(async |tx| node.handle("go".into(), &state, &options, tx).await)
            .await
            .1
            .unwrap();

        assert_eq!(agent.state().get("seen"), Some(json!(1)));
    }

    #[tokio::test]
    async fn passes_the_structured_output_schema_from_options_to_the_agent() {
        let (agent, model) = agent_with(vec![MockResponse::message(Message::assistant("hello"))]);
        let node = agent_node(agent);
        let state = MultiAgentStateHandle::new(MultiAgentState::new([node.id().to_owned()]));
        let schema = json!({"type": "object", "properties": {"answer": {"type": "string"}}});
        let options = NodeInputOptions::default().with_structured_output_schema(schema.clone());

        // The mock never calls the forced structured-output tool, so the run itself fails; what
        // matters here is that the schema reached the agent.
        let _ = collect(async |tx| node.handle("go".into(), &state, &options, tx).await).await;

        let calls = model.calls();
        assert!(
            calls[0]
                .1
                .tool_specs
                .iter()
                .any(|spec| spec.input_schema.as_ref() == Some(&schema)),
            "structured output schema reached the model as a tool spec"
        );
    }

    /// Minimal orchestrator that replays canned events then its result.
    struct TestOrchestrator {
        id: String,
        hooks: HookRegistry,
        events: Vec<MultiAgentStreamEvent>,
        result: MultiAgentResult,
    }

    impl TestOrchestrator {
        fn shared(
            id: &str,
            events: Vec<MultiAgentStreamEvent>,
            result: MultiAgentResult,
        ) -> Arc<dyn MultiAgent> {
            Arc::new(Self {
                id: id.to_owned(),
                hooks: HookRegistry::default(),
                events,
                result,
            })
        }
    }

    impl MultiAgent for TestOrchestrator {
        fn id(&self) -> &str {
            &self.id
        }

        fn name(&self) -> &str {
            &self.id
        }

        fn hooks(&self) -> &HookRegistry {
            &self.hooks
        }

        fn invoke_with<'a>(
            &'a self,
            _input: MultiAgentInput,
            _options: MultiAgentInvokeOptions,
        ) -> BoxFuture<'a, Result<MultiAgentResult>> {
            Box::pin(async move { Ok(self.result.clone()) })
        }

        fn stream_with(
            &self,
            _input: MultiAgentInput,
            _options: MultiAgentInvokeOptions,
        ) -> MultiAgentStream {
            let result = MultiAgentStreamEvent::MultiAgentResult(MultiAgentResultEvent {
                result: self.result.clone(),
                invocation_state: InvocationState::new(),
            });
            let events: Vec<_> = self
                .events
                .iter()
                .cloned()
                .chain(std::iter::once(result))
                .map(Ok)
                .collect();
            MultiAgentStream::new(Box::pin(futures::stream::iter(events)))
        }
    }

    fn inner_result(status: ResultStatus) -> MultiAgentResult {
        MultiAgentResult::new(vec![], 1)
            .with_status(status)
            .with_content(vec![ContentBlock::Text("inner".into())])
    }

    async fn drive_multiagent(node: &MultiAgentNode) -> (Vec<MultiAgentStreamEvent>, NodeResultUpdate) {
        let state = MultiAgentStateHandle::new(MultiAgentState::new([node.id().to_owned()]));
        let options = NodeInputOptions::default();
        let (events, update) =
            collect(async |tx| node.handle(MultiAgentInput::None, &state, &options, tx).await).await;
        (events, update.unwrap())
    }

    #[test]
    fn derives_id_and_description_from_the_orchestrator() {
        let orchestrator = TestOrchestrator::shared("inner", vec![], inner_result(ResultStatus::Completed));
        let node = MultiAgentNode::new(orchestrator.clone());
        assert_eq!(node.id(), "inner");
        assert_eq!(node.node_type(), NodeType::MultiAgent);
        assert_eq!(node.orchestrator().id(), "inner");
    }

    #[tokio::test]
    async fn passes_through_inner_node_stream_update_events() {
        let state = MultiAgentStateHandle::new(MultiAgentState::new(["deep"]));
        let inner_event = MultiAgentStreamEvent::NodeStreamUpdate(NodeStreamUpdateEvent {
            node_id: "deep".to_owned(),
            node_type: NodeType::Agent,
            state,
            inner: NodeInnerEvent::Custom(json!({"hello": "world"})),
            invocation_state: InvocationState::new(),
        });
        let orchestrator =
            TestOrchestrator::shared("inner", vec![inner_event], inner_result(ResultStatus::Completed));
        let node = MultiAgentNode::new(orchestrator);

        let (events, _) = drive_multiagent(&node).await;

        let MultiAgentStreamEvent::NodeStreamUpdate(first) = &events[0] else {
            panic!("expected a node stream update");
        };
        assert_eq!(first.node_id, "deep", "the original node identity is preserved");
        assert_eq!(first.inner.source(), "custom");
    }

    #[tokio::test]
    async fn wraps_non_node_stream_update_events_with_this_node_identity() {
        let orchestrator = TestOrchestrator::shared("inner", vec![], inner_result(ResultStatus::Completed));
        let node = MultiAgentNode::new(orchestrator);

        let (events, _) = drive_multiagent(&node).await;

        let MultiAgentStreamEvent::NodeStreamUpdate(wrapped) = &events[0] else {
            panic!("expected a node stream update");
        };
        assert_eq!(wrapped.node_id, "inner");
        assert_eq!(wrapped.node_type, NodeType::MultiAgent);
        assert_eq!(wrapped.inner.source(), "multiAgent");
    }

    #[tokio::test]
    async fn returns_orchestrator_content() {
        let orchestrator = TestOrchestrator::shared("inner", vec![], inner_result(ResultStatus::Completed));
        let node = MultiAgentNode::new(orchestrator);

        let (_, update) = drive_multiagent(&node).await;

        assert_eq!(update.content, Some(vec![ContentBlock::Text("inner".into())]));
        assert_eq!(update.status, None, "COMPLETED is left to the framework default");
    }

    #[tokio::test]
    async fn propagates_failed_status_from_the_inner_orchestrator() {
        let orchestrator = TestOrchestrator::shared(
            "inner",
            vec![],
            inner_result(ResultStatus::Failed).with_error("inner boom"),
        );
        let node = MultiAgentNode::new(orchestrator);

        let (_, update) = drive_multiagent(&node).await;

        assert_eq!(update.status, Some(ResultStatus::Failed));
        assert_eq!(update.error.as_deref(), Some("inner boom"));
    }

    #[tokio::test]
    async fn propagates_cancelled_status_from_the_inner_orchestrator() {
        let orchestrator = TestOrchestrator::shared("inner", vec![], inner_result(ResultStatus::Cancelled));
        let node = MultiAgentNode::new(orchestrator);

        let (_, update) = drive_multiagent(&node).await;

        assert_eq!(update.status, Some(ResultStatus::Cancelled));
    }

    #[test]
    fn node_definitions_convert_from_every_accepted_shape() {
        let (agent, _) = agent_with(vec![]);
        let orchestrator = TestOrchestrator::shared("inner", vec![], inner_result(ResultStatus::Completed));
        let built: Arc<dyn Node> = Arc::new(agent_node(agent.clone()));

        let definitions: Vec<NodeDefinition> = vec![
            agent.clone().into(),
            AgentNodeOptions::new(agent.clone())
                .with_description("does things")
                .into(),
            orchestrator.clone().into(),
            MultiAgentNodeOptions::new(orchestrator).into(),
            built.into(),
        ];
        let node_types: Vec<NodeType> = definitions
            .into_iter()
            .map(|definition| definition.into_node().unwrap().node_type())
            .collect();

        assert_eq!(
            node_types,
            vec![
                NodeType::Agent,
                NodeType::Agent,
                NodeType::MultiAgent,
                NodeType::MultiAgent,
                NodeType::Agent
            ]
        );
    }

    #[test]
    fn agent_node_description_defaults_to_the_agents_and_options_override_it() {
        let (agent, _) = agent_with(vec![]);
        let node = AgentNode::new(AgentNodeOptions::new(agent).with_description("explicit")).unwrap();
        assert_eq!(node.config().description.as_deref(), Some("explicit"));
    }
}
