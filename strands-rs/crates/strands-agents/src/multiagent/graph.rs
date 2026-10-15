//! Directed-graph orchestration (`strands-ts/src/multiagent/graph.ts`).
//!
//! Nodes run as a dependency graph: source nodes first, then every node whose incoming edges
//! are all satisfied. [`Graph`] is a cheap `Clone` handle over the shared topology.

use std::any::Any;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use indexmap::IndexMap;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::edge::{Edge, EdgeDefinition, EdgeHandler};
use super::events::{
    AfterMultiAgentInvocationEvent, AfterNodeCallEvent, BeforeMultiAgentInvocationEvent, BeforeNodeCallEvent,
    MultiAgentHandoffEvent, MultiAgentInitializedEvent, MultiAgentResultEvent, MultiAgentStreamEvent,
    NodeCancelEvent, NodeResultEvent, OrchestratorRef,
};
use super::multiagent::{
    apply_orchestrator_hook_responses, drop_stale_interrupted_result, group_interrupt_responses_by_node,
    record_hook_interrupt, MultiAgent, MultiAgentInput, MultiAgentInvokeOptions, MultiAgentStream,
};
use super::nodes::{run_node, Node, NodeDefinition, NodeInputOptions, NodeSink, NodeType};
use super::plugins::{MultiAgentPlugin, MultiAgentPluginRegistry};
use super::snapshot::{self, TakeMultiAgentSnapshotOptions};
use super::state::{
    now_ms, MultiAgentResult, MultiAgentState, MultiAgentStateHandle, NodeResult, ResultStatus, Status,
};
use super::{EVENT_BUFFER, HOOK_CANCEL_MESSAGE};
use crate::agent::InvocationState;
use crate::error::{Error, Result};
use crate::hooks::{Cancel, HookEvent, HookFn, HookRegistry};
use crate::logging::warn_once;
use crate::session::Snapshot;
use crate::telemetry::tracer::{MultiAgentSpanOptions, SpanHandle};
use crate::telemetry::{AttrValue, TelemetryConfig, Tracer};
use crate::types::{ContentBlock, InterruptResponseContent};

/// Default graph id, matching TS `id ?? 'graph'`.
pub const DEFAULT_GRAPH_ID: &str = "graph";

const UNBOUNDED_WARNING: &str = "graph has no max_steps or timeout set; execution is unbounded";

/// Runtime limits for a graph invocation (TS `GraphConfig`).
///
/// Every limit defaults to "no limit"; [`GraphBuilder::build`] warns once when neither
/// `max_steps` nor `timeout` is set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphConfig {
    /// Max nodes executing in parallel; `None` means no limit.
    pub max_concurrency: Option<usize>,
    /// Max total steps, bounding cyclic graphs; `None` means no limit.
    pub max_steps: Option<u64>,
    /// Wall-clock ceiling for the whole invocation; `None` means no limit.
    ///
    /// Does not propagate into a nested orchestrator wrapped as a node — that runs under its
    /// own config, and this timeout only fires once the nested node returns.
    pub timeout: Option<Duration>,
    /// Fallback per-node wall-clock ceiling, used by any node without its own; `None` means no
    /// limit. Cancellation is cooperative, so a tool that ignores its cancel token can overrun.
    pub node_timeout: Option<Duration>,
}

struct GraphInner {
    id: String,
    name: String,
    description: Option<String>,
    nodes: IndexMap<String, Arc<dyn Node>>,
    edges: Vec<Edge>,
    sources: Vec<Arc<dyn Node>>,
    config: GraphConfig,
    // Only the tests consume it: TS reads it for `bufferOutput`, which Rust has no equivalent of.
    #[cfg_attr(not(test), allow(dead_code))]
    can_run_concurrently: bool,
    hooks: HookRegistry,
    plugins: MultiAgentPluginRegistry,
    initialized: AtomicBool,
    pending_interrupt_state: Mutex<Option<MultiAgentState>>,
    last_state: Mutex<Option<MultiAgentStateHandle>>,
    tracer: Tracer,
}

/// Directed graph orchestrator; a cheap `Clone` handle over one shared topology.
///
/// Build it with [`Graph::builder`]. Clones share the node set and the hook registry; every
/// invocation gets its own execution state, so concurrent invocations never interfere.
#[derive(Clone)]
pub struct Graph {
    inner: Arc<GraphInner>,
}

impl Graph {
    /// A fresh [`GraphBuilder`].
    pub fn builder() -> GraphBuilder {
        GraphBuilder::new()
    }

    /// Unique identifier for this graph.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// Human-readable name; defaults to the id.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// What this graph does, when it was given a description.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.inner.description.as_deref()
    }

    /// The graph's nodes, in insertion order.
    #[must_use]
    pub fn nodes(&self) -> &IndexMap<String, Arc<dyn Node>> {
        &self.inner.nodes
    }

    /// The graph's edges, in insertion order.
    #[must_use]
    pub fn edges(&self) -> &[Edge] {
        &self.inner.edges
    }

    /// The entry-point nodes execution starts from.
    #[must_use]
    pub fn sources(&self) -> &[Arc<dyn Node>] {
        &self.inner.sources
    }

    /// The graph's runtime limits.
    #[must_use]
    pub fn config(&self) -> &GraphConfig {
        &self.inner.config
    }

    /// Whether this topology can ever run two nodes at once.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn can_run_concurrently(&self) -> bool {
        self.inner.can_run_concurrently
    }

    /// The graph's hook registry.
    #[must_use]
    pub fn hooks(&self) -> &HookRegistry {
        &self.inner.hooks
    }

    /// The graph's plugin registry.
    #[must_use]
    pub fn plugins(&self) -> &MultiAgentPluginRegistry {
        &self.inner.plugins
    }
}

impl fmt::Debug for Graph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Graph")
            .field("id", &self.inner.id)
            .field("nodes", &self.inner.nodes.keys().collect::<Vec<_>>())
            .field("edges", &self.inner.edges)
            .field("config", &self.inner.config)
            .finish()
    }
}

/// Builds a [`Graph`]: nodes, edges, entry points, limits, plugins, and hooks.
#[must_use = "a GraphBuilder does nothing until you call build()"]
#[derive(Default)]
pub struct GraphBuilder {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    nodes: Vec<NodeDefinition>,
    edges: Vec<EdgeDefinition>,
    entry_points: Option<Vec<String>>,
    config: GraphConfig,
    plugins: Vec<Arc<dyn MultiAgentPlugin>>,
    hooks: HookRegistry,
    trace_attributes: Vec<(String, AttrValue)>,
}

impl GraphBuilder {
    /// A fresh builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Custom attributes merged onto every span this graph emits.
    pub fn trace_attributes(mut self, attrs: impl IntoIterator<Item = (String, AttrValue)>) -> Self {
        self.trace_attributes = attrs.into_iter().collect();
        self
    }

    /// Set the graph's id (default `"graph"`).
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the graph's name (defaults to the id).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Describe what the graph does, for use as a node of a larger orchestration.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a node; its id comes from the node itself (an agent node uses the agent's id).
    pub fn node(mut self, definition: impl Into<NodeDefinition>) -> Self {
        self.nodes.push(definition.into());
        self
    }

    /// Add an unconditional edge from `source` to `target`.
    pub fn edge(mut self, source: impl Into<String>, target: impl Into<String>) -> Self {
        self.edges.push(EdgeDefinition::new(source, target));
        self
    }

    /// Add an edge traversed only when `handler` returns true.
    pub fn edge_when(
        mut self,
        source: impl Into<String>,
        target: impl Into<String>,
        handler: EdgeHandler,
    ) -> Self {
        self.edges
            .push(EdgeDefinition::new(source, target).with_handler(handler));
        self
    }

    /// Add an already-built [`EdgeDefinition`].
    pub fn edge_definition(mut self, definition: impl Into<EdgeDefinition>) -> Self {
        self.edges.push(definition.into());
        self
    }

    /// Mark `id` as an entry point. Without any, nodes with no incoming edges are used
    /// (TS `sources`).
    pub fn entry_point(mut self, id: impl Into<String>) -> Self {
        self.entry_points.get_or_insert_with(Vec::new).push(id.into());
        self
    }

    /// Cap how many nodes run in parallel.
    pub fn max_concurrency(mut self, max_concurrency: usize) -> Self {
        self.config.max_concurrency = Some(max_concurrency);
        self
    }

    /// Cap total node executions, bounding cyclic graphs (TS `maxSteps`).
    pub fn max_steps(mut self, max_steps: u64) -> Self {
        self.config.max_steps = Some(max_steps);
        self
    }

    /// Set the wall-clock ceiling for the whole invocation.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = Some(timeout);
        self
    }

    /// Set the fallback per-node wall-clock ceiling.
    pub fn node_timeout(mut self, node_timeout: Duration) -> Self {
        self.config.node_timeout = Some(node_timeout);
        self
    }

    /// Register a multi-agent plugin.
    pub fn plugin(mut self, plugin: Arc<dyn MultiAgentPlugin>) -> Self {
        self.plugins.push(plugin);
        self
    }

    /// Register a hook callback on the graph built by [`Self::build`].
    pub fn hook<E, F>(self, callback: F) -> Self
    where
        E: HookEvent,
        F: for<'a> HookFn<'a, E> + Send + Sync + 'static,
    {
        self.hooks.add_callback(callback);
        self
    }

    /// Build the graph.
    ///
    /// # Errors
    /// Mirrors the TS constructor validation, message for message: a limit below 1, a duplicate
    /// node id, an edge or entry point naming an unknown node, no entry points, or a node
    /// unreachable from any entry point.
    pub fn build(self) -> Result<Graph> {
        validate_config(&self.config)?;
        if self.config.max_steps.is_none() && self.config.timeout.is_none() {
            warn_once(UNBOUNDED_WARNING);
        }

        let nodes = resolve_nodes(self.nodes)?;
        let edges = resolve_edges(&nodes, self.edges)?;
        let sources = resolve_sources(&nodes, &edges, self.entry_points)?;
        validate_sources(&nodes, &edges, &sources)?;

        let id = self.id.unwrap_or_else(|| DEFAULT_GRAPH_ID.to_owned());
        let name = self.name.unwrap_or_else(|| id.clone());

        Ok(Graph {
            inner: Arc::new(GraphInner {
                id,
                name,
                description: self.description,
                can_run_concurrently: can_run_concurrently(&self.config, &edges, &sources),
                nodes,
                edges,
                sources,
                config: self.config,
                hooks: self.hooks,
                plugins: MultiAgentPluginRegistry::new(self.plugins),
                initialized: AtomicBool::new(false),
                pending_interrupt_state: Mutex::new(None),
                last_state: Mutex::new(None),
                tracer: Tracer::new(TelemetryConfig::from_env().with_trace_attributes(self.trace_attributes)),
            }),
        })
    }
}

fn validate_config(config: &GraphConfig) -> Result<()> {
    if config.max_concurrency == Some(0) {
        return Err(Error::other("max_concurrency=<0> | must be at least 1"));
    }
    if config.max_steps == Some(0) {
        return Err(Error::other("max_steps=<0> | must be at least 1"));
    }
    if config.timeout == Some(Duration::ZERO) {
        return Err(Error::other("timeout=<0> | must be at least 1"));
    }
    if config.node_timeout == Some(Duration::ZERO) {
        return Err(Error::other("node_timeout=<0> | must be at least 1"));
    }
    Ok(())
}

fn resolve_nodes(definitions: Vec<NodeDefinition>) -> Result<IndexMap<String, Arc<dyn Node>>> {
    let mut nodes = IndexMap::with_capacity(definitions.len());
    for definition in definitions {
        let node = definition.into_node()?;
        if nodes.contains_key(node.id()) {
            return Err(Error::other(format!(
                "node_id=<{}> | duplicate node id",
                node.id()
            )));
        }
        nodes.insert(node.id().to_owned(), node);
    }
    Ok(nodes)
}

fn resolve_edges(
    nodes: &IndexMap<String, Arc<dyn Node>>,
    definitions: Vec<EdgeDefinition>,
) -> Result<Vec<Edge>> {
    definitions
        .into_iter()
        .map(|definition| {
            let source = nodes.get(&definition.source).ok_or_else(|| {
                Error::other(format!(
                    "source=<{}> | edge references unknown source node",
                    definition.source
                ))
            })?;
            let target = nodes.get(&definition.target).ok_or_else(|| {
                Error::other(format!(
                    "target=<{}> | edge references unknown target node",
                    definition.target
                ))
            })?;
            Ok(Edge {
                source: Arc::clone(source),
                target: Arc::clone(target),
                handler: definition.handler,
            })
        })
        .collect()
}

fn resolve_sources(
    nodes: &IndexMap<String, Arc<dyn Node>>,
    edges: &[Edge],
    entry_points: Option<Vec<String>>,
) -> Result<Vec<Arc<dyn Node>>> {
    match entry_points {
        Some(ids) => {
            ids.into_iter()
                .map(|id| {
                    nodes.get(&id).map(Arc::clone).ok_or_else(|| {
                        Error::other(format!("source=<{id}> | source references unknown node"))
                    })
                })
                .collect()
        }
        None => {
            let targets: HashSet<&str> = edges.iter().map(|edge| edge.target.id()).collect();
            Ok(nodes
                .iter()
                .filter(|(id, _)| !targets.contains(id.as_str()))
                .map(|(_, node)| Arc::clone(node))
                .collect())
        }
    }
}

fn validate_sources(
    nodes: &IndexMap<String, Arc<dyn Node>>,
    edges: &[Edge],
    sources: &[Arc<dyn Node>],
) -> Result<()> {
    if sources.is_empty() {
        return Err(Error::other("graph has no source nodes"));
    }

    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        adjacency
            .entry(edge.source.id())
            .or_default()
            .push(edge.target.id());
    }

    let mut visited: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = sources.iter().map(|node| node.id()).collect();
    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(targets) = adjacency.get(id) {
            queue.extend(targets.iter().copied());
        }
    }

    for id in nodes.keys() {
        if !visited.contains(id.as_str()) {
            return Err(Error::other(format!(
                "node_id=<{id}> | unreachable from any source node"
            )));
        }
    }
    Ok(())
}

/// Static check: can two nodes ever run at once? False when concurrency is capped at 1, there is
/// a single source, and no node fans out to more than one target.
fn can_run_concurrently(config: &GraphConfig, edges: &[Edge], sources: &[Arc<dyn Node>]) -> bool {
    if config.max_concurrency.is_some_and(|max| max < 2) {
        return false;
    }
    if sources.len() > 1 {
        return true;
    }
    let mut fan_out: HashMap<&str, usize> = HashMap::new();
    for edge in edges {
        *fan_out.entry(edge.source.id()).or_default() += 1;
    }
    fan_out.values().any(|count| *count > 1)
}

impl Graph {
    /// Run to completion with default options.
    pub async fn invoke(&self, input: impl Into<MultiAgentInput>) -> Result<MultiAgentResult> {
        self.invoke_with(input, MultiAgentInvokeOptions::default()).await
    }

    /// Run to completion with explicit invocation options.
    ///
    /// # Errors
    /// Fails when a limit is exceeded, the run is cancelled, or a hook or plugin fails. A failing
    /// node is not an error: it lands as a `FAILED` node result in the aggregate.
    pub async fn invoke_with(
        &self,
        input: impl Into<MultiAgentInput>,
        options: MultiAgentInvokeOptions,
    ) -> Result<MultiAgentResult> {
        let (sink, mut events) = mpsc::channel(EVENT_BUFFER);
        let drain = async move { while events.recv().await.is_some() {} };
        let (result, ()) = tokio::join!(self.run(input.into(), options, sink), drain);
        result
    }

    /// Stream events as they occur, with default options.
    pub fn stream(&self, input: impl Into<MultiAgentInput>) -> MultiAgentStream {
        self.stream_with(input, MultiAgentInvokeOptions::default())
    }

    /// Stream events as they occur; the last event carries the aggregate result.
    ///
    /// The invocation runs in a spawned task; dropping the stream cancels it, as TS cancels the
    /// run in `gen.return()`. Node cancellation stays cooperative.
    ///
    /// # Panics
    /// Panics if called outside a tokio runtime (it spawns the invocation task).
    pub fn stream_with(
        &self,
        input: impl Into<MultiAgentInput>,
        mut options: MultiAgentInvokeOptions,
    ) -> MultiAgentStream {
        let (sink, events) = mpsc::channel(EVENT_BUFFER);
        let graph = self.clone();
        let input = input.into();
        // A child of the caller's token, so dropping the stream cancels this run only.
        let cancel = options
            .cancel
            .as_ref()
            .map_or_else(CancellationToken::new, CancellationToken::child_token);
        options.cancel = Some(cancel.clone());
        let driver = CancelOnDrop {
            cancel,
            task: tokio::spawn(async move {
                if let Err(error) = graph.run(input, options, sink.clone()).await {
                    let _ = sink.send(Err(error)).await;
                }
            }),
        };
        MultiAgentStream::new(Box::pin(futures::stream::unfold(
            (events, driver),
            |(mut events, driver)| async move { events.recv().await.map(|event| (event, (events, driver))) },
        )))
    }

    /// Capture the graph's identity as a snapshot (`scope: "multiAgent"`).
    ///
    /// Pass the state handle of a live invocation to include its execution state; `None` uses the
    /// most recent invocation's state.
    ///
    /// # Errors
    /// Fails when the state cannot be serialized, or when `None` is passed before the first run.
    pub fn take_snapshot(
        &self,
        state: Option<&MultiAgentStateHandle>,
        options: TakeMultiAgentSnapshotOptions,
    ) -> Result<Snapshot> {
        match state {
            Some(state) => snapshot::take_snapshot(self, Some(state), options),
            None => {
                let last = self.inner.last_state.lock().clone().ok_or_else(|| {
                    Error::other(format!(
                        "graph_id=<{}> | no invocation state to snapshot; run the graph or pass a state handle",
                        self.inner.id
                    ))
                })?;
                snapshot::take_snapshot(self, Some(&last), options)
            }
        }
    }

    /// Restore a snapshot into `state`, or arm the next invocation to resume from it when `None`.
    ///
    /// # Errors
    /// Fails when the snapshot's scope, schema version, or orchestrator id do not match.
    pub fn load_snapshot(&self, snapshot: &Snapshot, state: Option<&MultiAgentStateHandle>) -> Result<()> {
        match state {
            Some(state) => snapshot::load_snapshot(self, snapshot, Some(state))?,
            None => {
                let restored =
                    MultiAgentStateHandle::new(MultiAgentState::new(self.inner.nodes.keys().cloned()));
                snapshot::load_snapshot(self, snapshot, Some(&restored))?;
                *self.inner.pending_interrupt_state.lock() = Some(restored.snapshot());
            }
        }
        Ok(())
    }

    /// Initialize plugins and fire `MultiAgentInitializedEvent`; the first invocation calls this.
    ///
    /// # Errors
    /// Propagates plugin and hook initialization failures.
    pub async fn initialize(&self) -> Result<()> {
        let orchestrator: OrchestratorRef = Arc::new(self.clone());
        self.inner.plugins.initialize(&orchestrator).await?;
        if !self.inner.initialized.swap(true, Ordering::SeqCst) {
            let mut event = MultiAgentInitializedEvent { orchestrator };
            self.inner.hooks.invoke_callbacks(&mut event).await?;
        }
        Ok(())
    }

    async fn run(
        &self,
        input: MultiAgentInput,
        options: MultiAgentInvokeOptions,
        sink: NodeSink,
    ) -> Result<MultiAgentResult> {
        self.initialize().await?;
        let orchestrator: OrchestratorRef = Arc::new(self.clone());

        // Shared by reference across every node, so mutations in one are visible to the next.
        let invocation_state = options.invocation_state.unwrap_or_default();
        // A fresh state per invocation, except when resuming a prior INTERRUPTED run on this
        // instance, so concurrent invocations cannot see each other's node results.
        let resumed = self.inner.pending_interrupt_state.lock().take();
        let state = MultiAgentStateHandle::new(
            resumed.unwrap_or_else(|| MultiAgentState::new(self.inner.nodes.keys().cloned())),
        );
        *self.inner.last_state.lock() = Some(state.clone());

        let span = Arc::new(self.inner.tracer.start_multi_agent_span(MultiAgentSpanOptions {
            orchestrator_id: &self.inner.id,
            orchestrator_type: "graph",
            input: serde_json::to_value(&input).ok().as_ref(),
        }));

        let mut before = BeforeMultiAgentInvocationEvent {
            orchestrator: Arc::clone(&orchestrator),
            state: state.clone(),
            invocation_state: invocation_state.clone(),
        };
        self.inner.hooks.invoke_callbacks(&mut before).await?;
        send(&sink, MultiAgentStreamEvent::BeforeMultiAgentInvocation(before)).await;

        let execution = Execution {
            orchestrator: Arc::clone(&orchestrator),
            state: state.clone(),
            invocation_state: invocation_state.clone(),
            sink: sink.clone(),
            cancel: CancellationToken::new(),
            done: CancellationToken::new(),
            external: options.cancel.clone(),
            timed_out: Arc::new(AtomicBool::new(false)),
            span: Arc::clone(&span),
        };
        if let Some(timeout) = self.inner.config.timeout {
            let (cancel, done, timed_out) = (
                execution.cancel.clone(),
                execution.done.clone(),
                Arc::clone(&execution.timed_out),
            );
            tokio::spawn(async move {
                tokio::select! {
                    () = tokio::time::sleep(timeout) => {
                        timed_out.store(true, Ordering::SeqCst);
                        cancel.cancel();
                    }
                    () = done.cancelled() => {}
                }
            });
        }
        if let Some(external) = options.cancel {
            let (cancel, done) = (execution.cancel.clone(), execution.done.clone());
            tokio::spawn(async move {
                tokio::select! {
                    () = external.cancelled() => cancel.cancel(),
                    () = done.cancelled() => {}
                }
            });
        }

        let outcome = self.execute(input, &execution).await;
        execution.done.cancel();
        let duration = state.with(|state| now_ms().saturating_sub(state.start_time));
        let error = outcome.as_ref().err().map(ToString::to_string);
        self.inner.tracer.end_multi_agent_span(
            &span,
            Some(duration),
            outcome.as_ref().ok().map(|result| &result.usage),
            error.as_deref(),
        );

        let mut after = AfterMultiAgentInvocationEvent {
            orchestrator,
            state,
            invocation_state: invocation_state.clone(),
        };
        let after_result = self.inner.hooks.invoke_callbacks(&mut after).await;
        send(&sink, MultiAgentStreamEvent::AfterMultiAgentInvocation(after)).await;
        let result = outcome?;
        after_result?;

        let mut event = MultiAgentResultEvent {
            result: result.clone(),
            invocation_state,
        };
        self.inner.hooks.invoke_callbacks(&mut event).await?;
        send(&sink, MultiAgentStreamEvent::MultiAgentResult(event)).await;
        Ok(result)
    }

    async fn execute(&self, input: MultiAgentInput, execution: &Execution) -> Result<MultiAgentResult> {
        let resume_responses = input.resume_responses().map(<[InterruptResponseContent]>::to_vec);
        let content_input = if resume_responses.is_some() {
            execution.state.with(|state| state.pending_input.clone())
        } else {
            execution
                .state
                .with_mut(|state| state.pending_input = Some(input.clone()));
            Some(input)
        };
        let routed = match &resume_responses {
            Some(responses) => Some(
                execution
                    .state
                    .with(|state| group_interrupt_responses_by_node(responses, state))?,
            ),
            None => None,
        };

        let targets: VecDeque<Arc<dyn Node>> = match &routed {
            Some(routed) => routed
                .keys()
                .map(|id| {
                    self.inner.nodes.get(id).map(Arc::clone).ok_or_else(|| {
                        Error::other(format!(
                            "node_id=<{id}>, graph_id=<{}> | resume response targets a node missing from the graph; topology changed between save and resume?",
                            self.inner.id
                        ))
                    })
                })
                .collect::<Result<_>>()?,
            None => match self.find_resume_targets(&execution.state).await {
                Some(ready) => ready.into(),
                None => self.inner.sources.iter().map(Arc::clone).collect(),
            },
        };
        let mut pending = Pending {
            targets,
            ..Pending::default()
        };

        let (node_sink, mut node_events) = mpsc::channel::<Result<MultiAgentStreamEvent>>(EVENT_BUFFER);
        let mut tasks = JoinSet::new();
        let max_concurrency = self.inner.config.max_concurrency.unwrap_or(usize::MAX);

        let outcome: Result<()> = 'run: loop {
            if execution.timed_out.load(Ordering::SeqCst) {
                break Err(Error::other(format!(
                    "timeout=<{}>, graph_id=<{}> | graph exceeded wall-clock budget",
                    self.inner.config.timeout.unwrap_or_default().as_millis(),
                    self.inner.id
                )));
            }
            if execution
                .external
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                break Err(Error::other(format!(
                    "graph_id=<{}> | graph cancelled by external signal",
                    self.inner.id
                )));
            }

            while !pending.interrupted
                && !pending.targets.is_empty()
                && pending.running.len() < max_concurrency
            {
                let node = pending.targets.pop_front().expect("targets is not empty");
                if let Some(max_steps) = self.inner.config.max_steps {
                    let steps = execution.state.steps();
                    if steps >= max_steps {
                        break 'run Err(Error::other(format!("steps=<{steps}> | max steps reached")));
                    }
                }
                execution.state.with_mut(|state| state.steps += 1);
                let routed = routed.as_ref().and_then(|routed| routed.get(node.id())).cloned();
                let node_input =
                    match self.resolve_input_for_scheduling(&node, routed, content_input.as_ref(), execution)
                    {
                        Ok(input) => input,
                        Err(error) => break 'run Err(error),
                    };
                let node_span = self.inner.tracer.start_node_span(
                    Some(&execution.span),
                    node.id(),
                    node.node_type().as_str(),
                );
                match self.run_before_node_call(&node, execution).await {
                    Ok(Some(result)) => {
                        self.inner.tracer.end_node_span(
                            &node_span,
                            Some(result.status.as_str()),
                            Some(result.duration),
                            None,
                            None,
                        );
                        pending.gated.push_back((node, result));
                    }
                    Ok(None) => {
                        self.spawn_node(&node, node_input, execution, &node_sink, &mut tasks, node_span);
                        pending.running.insert(node.id().to_owned());
                    }
                    Err(error) => break 'run Err(error),
                }
            }

            // A gated node's result is queued while scheduling, so it is handled before any node
            // event that arrives later, as in TS where both share one queue.
            if let Some((node, result)) = pending.gated.pop_front() {
                if let Err(error) = pending.handle_result(self, execution, &node, result).await {
                    break Err(error);
                }
                continue;
            }

            if tasks.is_empty() {
                // Nothing in flight: finish once the channel is empty too. An interrupt stops
                // scheduling, so any target left over stays PENDING.
                let Ok(event) = node_events.try_recv() else {
                    break Ok(());
                };
                if let Err(error) = pending.handle_event(self, execution, event).await {
                    break Err(error);
                }
                continue;
            }

            tokio::select! {
                event = node_events.recv() => {
                    let Some(event) = event else { break Ok(()) };
                    if let Err(error) = pending.handle_event(self, execution, event).await {
                        break Err(error);
                    }
                }
                _ = tasks.join_next() => {}
            }
        };

        drop(node_sink);
        // A node task sends its after-call event and any timeout error after `run_node` returns,
        // so drain to channel close — which happens only once every task has dropped its sender.
        let mut drained: Result<()> = Ok(());
        while let Some(event) = node_events.recv().await {
            let handled = pending.handle_event(self, execution, event).await;
            if drained.is_ok() {
                drained = handled;
            }
        }
        outcome?;
        drained?;

        let content = self.resolve_content(&execution.state);
        let (results, duration) = execution
            .state
            .with(|state| (state.results.clone(), now_ms().saturating_sub(state.start_time)));
        let result = MultiAgentResult::new(results, duration).with_content(content);
        if result.status == ResultStatus::Interrupted {
            *self.inner.pending_interrupt_state.lock() = Some(execution.state.snapshot());
        } else {
            *self.inner.pending_interrupt_state.lock() = None;
            execution.state.with_mut(|state| state.pending_input = None);
        }
        Ok(result)
    }

    fn spawn_node(
        &self,
        node: &Arc<dyn Node>,
        input: MultiAgentInput,
        execution: &Execution,
        node_sink: &NodeSink,
        tasks: &mut JoinSet<()>,
        node_span: SpanHandle,
    ) {
        let node_timeout = match node.node_type() {
            NodeType::Agent => node.timeout().or(self.inner.config.node_timeout),
            _ => None,
        };
        let cancel = execution.cancel.child_token();
        if let Some(timeout) = node_timeout {
            let (node_cancel, done) = (cancel.clone(), execution.done.clone());
            tokio::spawn(async move {
                tokio::select! {
                    () = tokio::time::sleep(timeout) => node_cancel.cancel(),
                    () = done.cancelled() => {}
                }
            });
        }
        let options = NodeInputOptions::default()
            .with_invocation_state(execution.invocation_state.clone())
            .with_cancel(cancel.clone());
        let node = Arc::clone(node);
        let sink = node_sink.clone();
        let state = execution.state.clone();
        let orchestrator = Arc::clone(&execution.orchestrator);
        let invocation_state = execution.invocation_state.clone();
        let graph_cancel = execution.cancel.clone();
        let graph_id = self.inner.id.clone();
        let tracer = self.inner.tracer.clone();
        tasks.spawn(async move {
            let result = run_node(node.as_ref(), input, &state, &options, &sink).await;
            let error = (cancel.is_cancelled() && !graph_cancel.is_cancelled()).then(|| {
                format!(
                    "node_timeout=<{}>, node_id=<{}>, graph_id=<{graph_id}> | node exceeded wall-clock budget",
                    node_timeout.unwrap_or_default().as_millis(),
                    node.id()
                )
            });
            tracer.end_node_span(
                &node_span,
                Some(result.status.as_str()),
                Some(result.duration),
                result.usage.as_ref(),
                error.as_deref(),
            );
            if let Some(error) = &error {
                let _ = sink.send(Err(Error::other(error.clone()))).await;
            }
            let _ = sink
                .send(Ok(MultiAgentStreamEvent::AfterNodeCall(AfterNodeCallEvent {
                    orchestrator,
                    state,
                    node_id: node.id().to_owned(),
                    invocation_state,
                    error,
                })))
                .await;
        });
    }

    /// Fires `BeforeNodeCallEvent`, handling a hook-raised interrupt or cancel inline. `Some`
    /// result means a hook gated the node and it must not run.
    async fn run_before_node_call(
        &self,
        node: &Arc<dyn Node>,
        execution: &Execution,
    ) -> Result<Option<NodeResult>> {
        let mut event = BeforeNodeCallEvent {
            orchestrator: Arc::clone(&execution.orchestrator),
            state: execution.state.clone(),
            node_id: node.id().to_owned(),
            invocation_state: execution.invocation_state.clone(),
            cancel: Cancel::default(),
        };
        match self.inner.hooks.invoke_callbacks(&mut event).await {
            Ok(()) => {}
            Err(error) if error.is_control_flow() => {
                let result = execution.state.with_mut(|state| {
                    let node_state = state.node_or_insert(node.id());
                    record_hook_interrupt(node.id(), node_state)
                });
                send(&execution.sink, MultiAgentStreamEvent::BeforeNodeCall(event)).await;
                self.emit_node_result(node, &result, execution).await?;
                self.emit_after_node_call(node, execution).await?;
                return Ok(Some(result));
            }
            Err(error) => return Err(error),
        }
        let cancel = event.cancel.clone();
        send(&execution.sink, MultiAgentStreamEvent::BeforeNodeCall(event)).await;
        if !cancel.is_cancelled() {
            return Ok(None);
        }
        let message = cancel
            .message_or(HOOK_CANCEL_MESSAGE)
            .unwrap_or(HOOK_CANCEL_MESSAGE)
            .to_owned();
        let result = execution.state.with_mut(|state| {
            drop_stale_interrupted_result(node.id(), state);
            let result = NodeResult::new(node.id(), ResultStatus::Cancelled, 0);
            let node_state = state.node_or_insert(node.id());
            node_state.status = Status::Cancelled;
            node_state.results.push(result.clone());
            result
        });
        let mut event = NodeCancelEvent {
            node_id: node.id().to_owned(),
            state: execution.state.clone(),
            message,
            invocation_state: execution.invocation_state.clone(),
        };
        self.inner.hooks.invoke_callbacks(&mut event).await?;
        send(&execution.sink, MultiAgentStreamEvent::NodeCancel(event)).await;
        self.emit_after_node_call(node, execution).await?;
        Ok(Some(result))
    }

    async fn emit_node_result(
        &self,
        node: &Arc<dyn Node>,
        result: &NodeResult,
        execution: &Execution,
    ) -> Result<()> {
        let mut event = NodeResultEvent {
            node_id: node.id().to_owned(),
            node_type: node.node_type(),
            state: execution.state.clone(),
            result: result.clone(),
            invocation_state: execution.invocation_state.clone(),
        };
        self.inner.hooks.invoke_callbacks(&mut event).await?;
        send(&execution.sink, MultiAgentStreamEvent::NodeResult(event)).await;
        Ok(())
    }

    async fn emit_after_node_call(&self, node: &Arc<dyn Node>, execution: &Execution) -> Result<()> {
        let mut event = AfterNodeCallEvent {
            orchestrator: Arc::clone(&execution.orchestrator),
            state: execution.state.clone(),
            node_id: node.id().to_owned(),
            invocation_state: execution.invocation_state.clone(),
            error: None,
        };
        self.inner.hooks.invoke_callbacks(&mut event).await?;
        send(&execution.sink, MultiAgentStreamEvent::AfterNodeCall(event)).await;
        Ok(())
    }

    /// Nodes whose incoming edges are all satisfied by `node`'s completion.
    async fn find_ready(
        &self,
        node: &Arc<dyn Node>,
        state: &MultiAgentStateHandle,
        pending: &Pending,
    ) -> Vec<Arc<dyn Node>> {
        if state.node(node.id()).map(|node| node.status) != Some(Status::Completed) {
            return Vec::new();
        }
        let mut ready = Vec::new();
        for edge in self
            .inner
            .edges
            .iter()
            .filter(|edge| edge.source.id() == node.id())
        {
            let target = &edge.target;
            if pending.running.contains(target.id())
                || pending.targets.iter().any(|queued| queued.id() == target.id())
            {
                continue;
            }
            if self.dependencies_satisfied(target.id(), state).await {
                ready.push(Arc::clone(target));
            }
        }
        ready
    }

    async fn dependencies_satisfied(&self, node_id: &str, state: &MultiAgentStateHandle) -> bool {
        for edge in self.inner.edges.iter().filter(|edge| edge.target.id() == node_id) {
            if state.node(edge.source.id()).map(|node| node.status) != Some(Status::Completed) {
                return false;
            }
            if !edge.should_traverse(state).await {
                return false;
            }
        }
        true
    }

    /// Nodes to restart from when the state carries completed nodes from an earlier run.
    async fn find_resume_targets(&self, state: &MultiAgentStateHandle) -> Option<Vec<Arc<dyn Node>>> {
        let resumable = state.with(|state| {
            state
                .nodes()
                .values()
                .any(|node| node.status == Status::Completed)
        });
        if !resumable {
            return None;
        }
        let mut ready = Vec::new();
        for (id, node) in &self.inner.nodes {
            if state.node(id).map(|node| node.status) == Some(Status::Completed) {
                continue;
            }
            if self.dependencies_satisfied(id, state).await {
                ready.push(Arc::clone(node));
            }
        }
        if ready.is_empty() {
            tracing::debug!("all nodes completed in restored state | starting fresh");
            return None;
        }
        tracing::debug!(
            resume_targets = %ready.iter().map(|node| node.id()).collect::<Vec<_>>().join(", "),
            prior_steps = state.steps(),
            "resuming graph from restored state"
        );
        Some(ready)
    }

    /// Content of every terminal node, which is what the aggregate result reports.
    fn resolve_content(&self, state: &MultiAgentStateHandle) -> Vec<ContentBlock> {
        state.with_mut(|state| {
            let terminus: Vec<(String, bool)> = state
                .nodes()
                .iter()
                .filter_map(|(id, node)| match node.status {
                    Status::Failed | Status::Cancelled => Some((id.clone(), true)),
                    Status::Completed => {
                        let open =
                            self.inner
                                .edges
                                .iter()
                                .filter(|edge| edge.source.id() == id)
                                .any(|edge| {
                                    state.node(edge.target.id()).map(|node| node.status)
                                        != Some(Status::Pending)
                                });
                        Some((id.clone(), !open))
                    }
                    _ => None,
                })
                .collect();
            for (id, value) in terminus {
                if let Some(node) = state.node_mut(&id) {
                    node.terminus = value;
                }
            }
            state
                .nodes()
                .values()
                .filter(|node| node.terminus)
                .flat_map(|node| node.content().to_vec())
                .collect()
        })
    }

    fn resolve_input_for_scheduling(
        &self,
        node: &Arc<dyn Node>,
        routed: Option<Vec<InterruptResponseContent>>,
        content_input: Option<&MultiAgentInput>,
        execution: &Execution,
    ) -> Result<MultiAgentInput> {
        if let Some(routed) = routed {
            let forwarded = execution.state.with_mut(|state| {
                let node_state = state.node_mut(node.id()).ok_or_else(|| {
                    Error::other(format!(
                        "node_id=<{}>, graph_id=<{}> | routed interrupt response targets a node missing from state; topology changed between save and resume?",
                        node.id(),
                        self.inner.id
                    ))
                })?;
                Ok::<_, Error>(apply_orchestrator_hook_responses(node_state, routed))
            })?;
            if !forwarded.is_empty() {
                return Ok(MultiAgentInput::InterruptResponses(forwarded));
            }
        }
        let Some(content_input) = content_input else {
            tracing::warn!(
                node_id = %node.id(),
                graph_id = %self.inner.id,
                "no pending input on resume; using empty"
            );
            return Ok(self.resolve_node_input(
                node,
                &MultiAgentInput::Text(String::new()),
                &execution.state,
            ));
        };
        Ok(self.resolve_node_input(node, content_input, &execution.state))
    }

    /// Appends every dependency's output to the node's input, labelled by source node id.
    fn resolve_node_input(
        &self,
        node: &Arc<dyn Node>,
        input: &MultiAgentInput,
        state: &MultiAgentStateHandle,
    ) -> MultiAgentInput {
        let mut deps = Vec::new();
        for edge in self
            .inner
            .edges
            .iter()
            .filter(|edge| edge.target.id() == node.id())
        {
            let Some(source) = state.node(edge.source.id()) else {
                continue;
            };
            let content: Vec<ContentBlock> = source
                .content()
                .iter()
                .filter(|block| !matches!(block, ContentBlock::Reasoning(_)))
                .cloned()
                .collect();
            if !content.is_empty() {
                deps.push(ContentBlock::Text(format!("[node: {}]", edge.source.id())));
                deps.extend(content);
            }
        }
        if deps.is_empty() {
            return input.clone();
        }
        let mut blocks = match input.clone() {
            MultiAgentInput::Text(text) => vec![ContentBlock::Text(text)],
            MultiAgentInput::Blocks(blocks) => blocks,
            _ => Vec::new(),
        };
        blocks.extend(deps);
        MultiAgentInput::Blocks(blocks)
    }
}

#[async_trait]
impl MultiAgent for Graph {
    fn id(&self) -> &str {
        &self.inner.id
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn description(&self) -> Option<&str> {
        self.inner.description.as_deref()
    }

    fn hooks(&self) -> &HookRegistry {
        &self.inner.hooks
    }

    async fn invoke_with(
        &self,
        input: MultiAgentInput,
        options: MultiAgentInvokeOptions,
    ) -> Result<MultiAgentResult> {
        Graph::invoke_with(self, input, options).await
    }

    fn stream_with(&self, input: MultiAgentInput, options: MultiAgentInvokeOptions) -> MultiAgentStream {
        Graph::stream_with(self, input, options)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl From<Graph> for NodeDefinition {
    fn from(graph: Graph) -> Self {
        NodeDefinition::from(Arc::new(graph) as Arc<dyn MultiAgent>)
    }
}

/// Cancels the invocation, then aborts its task, when the consumer drops the stream.
struct CancelOnDrop {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// Everything one invocation threads through the execution loop.
struct Execution {
    orchestrator: OrchestratorRef,
    state: MultiAgentStateHandle,
    invocation_state: InvocationState,
    sink: NodeSink,
    cancel: CancellationToken,
    done: CancellationToken,
    external: Option<CancellationToken>,
    timed_out: Arc<AtomicBool>,
    span: Arc<SpanHandle>,
}

/// The execution loop's scheduling state.
#[derive(Default)]
struct Pending {
    targets: VecDeque<Arc<dyn Node>>,
    running: HashSet<String>,
    gated: VecDeque<(Arc<dyn Node>, NodeResult)>,
    interrupted: bool,
}

impl Pending {
    async fn handle_event(
        &mut self,
        graph: &Graph,
        execution: &Execution,
        event: Result<MultiAgentStreamEvent>,
    ) -> Result<()> {
        match event? {
            MultiAgentStreamEvent::NodeResult(mut event) => {
                graph.inner.hooks.invoke_callbacks(&mut event).await?;
                let (node_id, result) = (event.node_id.clone(), event.result.clone());
                send(&execution.sink, MultiAgentStreamEvent::NodeResult(event)).await;
                self.running.remove(&node_id);
                let node = graph.inner.nodes.get(&node_id).map(Arc::clone);
                if let Some(node) = node {
                    self.handle_result(graph, execution, &node, result).await?;
                }
            }
            MultiAgentStreamEvent::NodeStreamUpdate(mut event) => {
                graph.inner.hooks.invoke_callbacks(&mut event).await?;
                send(&execution.sink, MultiAgentStreamEvent::NodeStreamUpdate(event)).await;
            }
            MultiAgentStreamEvent::AfterNodeCall(mut event) => {
                graph.inner.hooks.invoke_callbacks(&mut event).await?;
                send(&execution.sink, MultiAgentStreamEvent::AfterNodeCall(event)).await;
            }
            other => send(&execution.sink, other).await,
        }
        Ok(())
    }

    async fn handle_result(
        &mut self,
        graph: &Graph,
        execution: &Execution,
        node: &Arc<dyn Node>,
        result: NodeResult,
    ) -> Result<()> {
        execution
            .state
            .with_mut(|state| state.results.push(result.clone()));
        if self.interrupted {
            return Ok(());
        }
        if result.status == ResultStatus::Interrupted {
            self.interrupted = true;
            return Ok(());
        }
        let ready = graph.find_ready(node, &execution.state, self).await;
        if ready.is_empty() {
            return Ok(());
        }
        let mut event = MultiAgentHandoffEvent {
            source: node.id().to_owned(),
            targets: ready.iter().map(|node| node.id().to_owned()).collect(),
            state: execution.state.clone(),
            invocation_state: execution.invocation_state.clone(),
        };
        graph.inner.hooks.invoke_callbacks(&mut event).await?;
        send(&execution.sink, MultiAgentStreamEvent::MultiAgentHandoff(event)).await;
        self.targets.extend(ready);
        Ok(())
    }
}

async fn send(sink: &NodeSink, event: MultiAgentStreamEvent) {
    let _ = sink.send(Ok(event)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::multiagent::edge::edge_handler;
    use crate::multiagent::nodes::{AgentNode, AgentNodeOptions, NodeConfig, NodeType};
    use crate::multiagent::state::NodeResultUpdate;
    use crate::test_support::{MockMessageModel, MockResponse};
    use crate::tools::FunctionTool;
    use crate::types::{InterruptParams, Message, Role, ToolUseBlock};
    use futures::StreamExt;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    fn agent(id: &str) -> Agent {
        Agent::builder()
            .id(id)
            .model(MockMessageModel::shared(vec![MockResponse::message(
                Message::assistant("reply"),
            )]))
            .build()
            .expect("agent builds")
    }

    fn one_node(id: &str) -> GraphBuilder {
        Graph::builder().node(agent(id))
    }

    fn error(builder: GraphBuilder) -> String {
        builder.build().expect_err("build fails").to_string()
    }

    // ---- execution ----

    fn saying(id: &str, text: &str) -> Agent {
        Agent::builder()
            .id(id)
            .model(MockMessageModel::shared(vec![MockResponse::message(
                Message::assistant(text),
            )]))
            .build()
            .expect("agent builds")
    }

    fn with_model(id: &str, model: Arc<MockMessageModel>) -> Agent {
        Agent::builder()
            .id(id)
            .model(model)
            .build()
            .expect("agent builds")
    }

    /// Agent whose single tool call sleeps, so a timeout or cancel has something to interrupt.
    fn sleeping(id: &str, delay: Duration) -> Agent {
        let tool = Arc::new(
            FunctionTool::builder("wait")
                .description("Sleeps")
                .input_schema(json!({ "type": "object", "properties": {} }))
                .handler(move |_input, _ctx| async move {
                    tokio::time::sleep(delay).await;
                    Ok(json!("done"))
                }),
        );
        Agent::builder()
            .id(id)
            .model(MockMessageModel::shared(vec![
                MockResponse::message(Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse(ToolUseBlock::new(
                        "wait",
                        "call-1",
                        json!({}),
                    ))],
                )),
                MockResponse::message(Message::assistant("slept")),
            ]))
            .tool(tool)
            .build()
            .expect("agent builds")
    }

    /// `approve`: interrupts for an approval, then reports the answer (as in `nodes.rs`).
    fn approving(id: &str) -> Agent {
        let tool = Arc::new(
            FunctionTool::builder("approve")
                .description("Interrupts for approval")
                .input_schema(json!({ "type": "object", "properties": {} }))
                .handler(|_input, ctx| async move {
                    let answer: String = ctx.interrupt(InterruptParams::new("confirm"))?;
                    Ok(json!(format!("did it: {answer}")))
                }),
        );
        Agent::builder()
            .id(id)
            .model(MockMessageModel::shared(vec![
                MockResponse::message(Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse(ToolUseBlock::new(
                        "approve",
                        "call-1",
                        json!({}),
                    ))],
                )),
                MockResponse::message(Message::assistant("approved")),
            ]))
            .tool(tool)
            .build()
            .expect("agent builds")
    }

    /// Node that always fails, which `MockMessageModel` cannot express.
    struct FailingNode {
        id: String,
        config: NodeConfig,
    }

    impl FailingNode {
        fn shared(id: &str) -> Arc<dyn Node> {
            Arc::new(Self {
                id: id.to_owned(),
                config: NodeConfig::default(),
            })
        }
    }

    #[async_trait]
    impl Node for FailingNode {
        fn id(&self) -> &str {
            &self.id
        }

        fn node_type(&self) -> NodeType {
            NodeType::Custom("node".to_owned())
        }

        fn config(&self) -> &NodeConfig {
            &self.config
        }

        async fn handle(
            &self,
            _input: MultiAgentInput,
            _state: &MultiAgentStateHandle,
            _options: &NodeInputOptions,
            _events: &NodeSink,
        ) -> Result<NodeResultUpdate> {
            Err(Error::other("node blew up"))
        }
    }

    /// Node that succeeds across an await point; unlike one `Agent`, it can run concurrently.
    struct EchoNode {
        id: String,
        config: NodeConfig,
    }

    impl EchoNode {
        fn shared(id: &str) -> Arc<dyn Node> {
            Arc::new(Self {
                id: id.to_owned(),
                config: NodeConfig::default(),
            })
        }
    }

    #[async_trait]
    impl Node for EchoNode {
        fn id(&self) -> &str {
            &self.id
        }

        fn node_type(&self) -> NodeType {
            NodeType::Custom("node".to_owned())
        }

        fn config(&self) -> &NodeConfig {
            &self.config
        }

        async fn handle(
            &self,
            _input: MultiAgentInput,
            _state: &MultiAgentStateHandle,
            _options: &NodeInputOptions,
            _events: &NodeSink,
        ) -> Result<NodeResultUpdate> {
            tokio::task::yield_now().await;
            Ok(NodeResultUpdate::default())
        }
    }

    async fn collect(stream: MultiAgentStream) -> Vec<MultiAgentStreamEvent> {
        stream
            .map(|event| event.expect("stream event"))
            .collect::<Vec<_>>()
            .await
    }

    fn names(events: &[MultiAgentStreamEvent]) -> Vec<&'static str> {
        events.iter().map(MultiAgentStreamEvent::type_name).collect()
    }

    fn node_ids(result: &MultiAgentResult) -> Vec<&str> {
        result.results.iter().map(|r| r.node_id.as_str()).collect()
    }

    fn text(result: &MultiAgentResult) -> Vec<String> {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn executes_a_linear_graph_in_order() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .node(saying("c", "from c"))
            .edge("a", "b")
            .edge("b", "c")
            .max_steps(10)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), vec!["a", "b", "c"]);
        assert_eq!(text(&result), vec!["from c".to_owned()]);
    }

    #[tokio::test]
    async fn executes_parallel_branches_after_the_source() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .node(saying("c", "from c"))
            .edge("a", "b")
            .edge("a", "c")
            .max_steps(10)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result)[0], "a");
        let mut rest = node_ids(&result)[1..].to_vec();
        rest.sort_unstable();
        assert_eq!(rest, vec!["b", "c"]);
    }

    #[tokio::test]
    async fn waits_for_every_dependency_before_a_join_node() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .node(saying("c", "from c"))
            .edge("a", "c")
            .edge("b", "c")
            .max_steps(10)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(node_ids(&result).len(), 3, "the join node runs exactly once");
        assert_eq!(node_ids(&result)[2], "c", "the join node runs last");
    }

    #[tokio::test]
    async fn runs_a_nested_graph_through_a_multi_agent_node() {
        let inner: Arc<dyn MultiAgent> = Arc::new(
            Graph::builder()
                .id("inner")
                .node(saying("x", "from x"))
                .max_steps(5)
                .build()
                .unwrap(),
        );
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(inner)
            .edge("a", "inner")
            .max_steps(10)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(node_ids(&result), vec!["a", "inner"]);
        assert_eq!(text(&result), vec!["from x".to_owned()]);
    }

    #[tokio::test]
    async fn nests_a_graph_handed_to_the_builder_directly() {
        let inner = Graph::builder()
            .id("inner")
            .node(saying("x", "from x"))
            .max_steps(5)
            .build()
            .unwrap();
        let graph = Graph::builder().node(inner).max_steps(5).build().unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(
            graph.nodes().get("inner").unwrap().node_type(),
            NodeType::MultiAgent
        );
        assert_eq!(text(&result), vec!["from x".to_owned()]);
    }

    #[tokio::test]
    async fn uses_explicit_entry_points_instead_of_auto_detection() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .edge_when("b", "a", edge_handler(|_| false))
            .entry_point("a")
            .max_steps(10)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(node_ids(&result), vec!["a", "b"]);
    }

    #[tokio::test]
    async fn skips_a_conditional_edge_whose_handler_is_false() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge_when("a", "b", edge_handler(|_| false))
            .max_steps(10)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(node_ids(&result), vec!["a"]);
    }

    #[tokio::test]
    async fn passes_the_task_and_dependency_content_to_a_downstream_node() {
        let downstream = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("from b"))]);
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(with_model("b", Arc::clone(&downstream)))
            .edge("a", "b")
            .max_steps(10)
            .build()
            .unwrap();

        graph.invoke("go").await.unwrap();

        let prompt = downstream.calls()[0].0[0].content.clone();
        assert_eq!(
            prompt,
            vec![
                ContentBlock::Text("go".into()),
                ContentBlock::Text("[node: a]".into()),
                ContentBlock::Text("from a".into()),
            ]
        );
    }

    #[tokio::test]
    async fn a_failing_node_lands_as_a_failed_result_and_stops_its_branch() {
        let graph = Graph::builder()
            .node(FailingNode::shared("a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(10)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(result.status, ResultStatus::Failed);
        assert_eq!(node_ids(&result), vec!["a"]);
        assert_eq!(result.results[0].error.as_deref(), Some("node blew up"));
    }

    #[tokio::test]
    async fn errors_when_max_steps_is_exceeded() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(1)
            .build()
            .unwrap();

        let error = graph.invoke("go").await.expect_err("max steps trips");

        assert_eq!(error.to_string(), "steps=<1> | max steps reached");
    }

    #[tokio::test]
    async fn errors_when_a_node_exceeds_the_node_timeout() {
        let graph = Graph::builder()
            .node(sleeping("a", Duration::from_millis(400)))
            .node_timeout(Duration::from_millis(20))
            .max_steps(5)
            .build()
            .unwrap();

        let error = graph.invoke("go").await.expect_err("node timeout trips");

        assert!(
            error.to_string().contains("node exceeded wall-clock budget"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn a_per_node_timeout_overrides_the_graph_node_timeout() {
        let graph = Graph::builder()
            .node(
                AgentNodeOptions::new(sleeping("a", Duration::from_millis(10)))
                    .with_timeout(Duration::from_secs(5)),
            )
            .node_timeout(Duration::from_millis(20))
            .max_steps(5)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(result.status, ResultStatus::Completed);
    }

    #[tokio::test]
    async fn a_per_node_timeout_of_duration_max_disables_the_graph_node_timeout() {
        let graph = Graph::builder()
            .node(AgentNodeOptions::new(sleeping("a", Duration::from_millis(60))).with_timeout(Duration::MAX))
            .node_timeout(Duration::from_millis(20))
            .max_steps(5)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(result.status, ResultStatus::Completed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_stream_cancels_the_invocation() {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let tool = Arc::new(
            FunctionTool::builder("wait")
                .description("Sleeps, then records that it finished")
                .input_schema(json!({ "type": "object", "properties": {} }))
                .handler(move |_input, ctx| {
                    let flag = Arc::clone(&flag);
                    async move {
                        tokio::select! {
                            () = tokio::time::sleep(Duration::from_millis(200)) => {
                                flag.store(true, Ordering::SeqCst);
                            }
                            () = ctx.cancel.cancelled() => {}
                        }
                        Ok(json!("done"))
                    }
                }),
        );
        let agent = Agent::builder()
            .id("a")
            .model(MockMessageModel::shared(vec![
                MockResponse::message(Message::new(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse(ToolUseBlock::new(
                        "wait",
                        "call-1",
                        json!({}),
                    ))],
                )),
                MockResponse::message(Message::assistant("slept")),
            ]))
            .tool(tool)
            .build()
            .unwrap();
        let graph = Graph::builder().node(agent).max_steps(5).build().unwrap();

        let mut stream = graph.stream("go");
        stream.next().await.expect("first event").expect("stream event");
        drop(stream);
        tokio::time::sleep(Duration::from_millis(400)).await;

        assert!(
            !ran.load(Ordering::SeqCst),
            "the node kept running after the drop"
        );
    }

    #[tokio::test]
    async fn errors_when_the_wall_clock_timeout_is_exceeded() {
        let graph = Graph::builder()
            .node(sleeping("a", Duration::from_millis(40)))
            .node(sleeping("b", Duration::from_millis(40)))
            .edge("a", "b")
            .timeout(Duration::from_millis(20))
            .build()
            .unwrap();

        let error = graph.invoke("go").await.expect_err("timeout trips");

        assert!(
            error.to_string().contains("graph exceeded wall-clock budget"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn errors_when_cancelled_through_the_external_token() {
        let graph = Graph::builder()
            .node(sleeping("a", Duration::from_millis(40)))
            .node(sleeping("b", Duration::from_millis(40)))
            .edge("a", "b")
            .max_steps(5)
            .build()
            .unwrap();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            token.cancel();
        });

        let error = graph
            .invoke_with("go", MultiAgentInvokeOptions::default().with_cancel(cancel))
            .await
            .expect_err("external cancel trips");

        assert_eq!(
            error.to_string(),
            "graph_id=<graph> | graph cancelled by external signal"
        );
    }

    #[tokio::test]
    async fn respects_max_concurrency() {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (running, highest) = (Arc::clone(&current), Arc::clone(&peak));
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .node(saying("c", "from c"))
            .edge("a", "b")
            .edge("a", "c")
            .max_concurrency(1)
            .max_steps(10)
            .hook(move |_event: &mut BeforeNodeCallEvent| {
                let (running, highest) = (Arc::clone(&running), Arc::clone(&highest));
                async move {
                    let in_flight = running.fetch_add(1, Ordering::SeqCst) + 1;
                    highest.fetch_max(in_flight, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .build()
            .unwrap();

        graph.invoke("go").await.unwrap();

        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "never more than one node in flight"
        );
    }

    #[tokio::test]
    async fn initializes_plugins_only_once_across_invocations() {
        #[derive(Default)]
        struct CountingPlugin(Arc<Mutex<usize>>);

        #[async_trait]
        impl MultiAgentPlugin for CountingPlugin {
            fn name(&self) -> &str {
                "counting"
            }

            async fn init_multi_agent(&self, _orchestrator: &OrchestratorRef) -> Result<()> {
                *self.0.lock() += 1;
                Ok(())
            }
        }

        let calls = Arc::new(Mutex::new(0));
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .plugin(Arc::new(CountingPlugin(Arc::clone(&calls))))
            .max_steps(5)
            .build()
            .unwrap();

        graph.invoke("go").await.unwrap();
        graph.invoke("again").await.unwrap();

        assert_eq!(*calls.lock(), 1);
    }

    #[tokio::test]
    async fn re_executes_a_node_in_a_cycle_while_the_edge_allows_re_entry() {
        let visits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&visits);
        let agent = Agent::builder()
            .id("a")
            .model(MockMessageModel::shared(vec![
                MockResponse::message(Message::assistant("first")),
                MockResponse::message(Message::assistant("second")),
            ]))
            .build()
            .unwrap();
        let graph = Graph::builder()
            .node(agent)
            .edge_when(
                "a",
                "a",
                edge_handler(move |_| seen.fetch_add(1, Ordering::SeqCst) < 1),
            )
            .entry_point("a")
            .max_steps(5)
            .build()
            .unwrap();

        let result = graph.invoke("go").await.unwrap();

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), vec!["a", "a"]);
        assert_eq!(visits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn yields_lifecycle_events_in_order_for_a_single_node() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .max_steps(5)
            .build()
            .unwrap();

        let events = collect(graph.stream("go")).await;
        let order = names(&events);

        assert_eq!(order.first(), Some(&"beforeMultiAgentInvocationEvent"));
        assert_eq!(order.last(), Some(&"multiAgentResultEvent"));
        let lifecycle: Vec<_> = order
            .iter()
            .filter(|name| **name != "nodeStreamUpdateEvent")
            .copied()
            .collect();
        assert_eq!(
            lifecycle,
            vec![
                "beforeMultiAgentInvocationEvent",
                "beforeNodeCallEvent",
                "nodeResultEvent",
                "afterNodeCallEvent",
                "afterMultiAgentInvocationEvent",
                "multiAgentResultEvent",
            ]
        );
    }

    #[tokio::test]
    async fn yields_a_handoff_event_between_nodes() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(5)
            .build()
            .unwrap();

        let events = collect(graph.stream("go")).await;
        let handoff = events
            .iter()
            .find_map(|event| match event {
                MultiAgentStreamEvent::MultiAgentHandoff(event) => Some(event),
                _ => None,
            })
            .expect("a handoff event");

        assert_eq!(handoff.source, "a");
        assert_eq!(handoff.targets, vec!["b".to_owned()]);
    }

    #[tokio::test]
    async fn a_hook_cancels_a_node_with_its_own_message() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .max_steps(5)
            .hook(|event: &mut BeforeNodeCallEvent| {
                event.cancel = Cancel::Reason("not today".to_owned());
                async move { Ok(()) }
            })
            .build()
            .unwrap();

        let events = collect(graph.stream("go")).await;
        let cancel = events
            .iter()
            .find_map(|event| match event {
                MultiAgentStreamEvent::NodeCancel(event) => Some(event),
                _ => None,
            })
            .expect("a node cancel event");

        assert_eq!(cancel.message, "not today");
        assert_eq!(
            names(&events),
            vec![
                "beforeMultiAgentInvocationEvent",
                "beforeNodeCallEvent",
                "nodeCancelEvent",
                "afterNodeCallEvent",
                "afterMultiAgentInvocationEvent",
                "multiAgentResultEvent",
            ]
        );
        let result = events.last().unwrap().result().expect("the aggregate result");
        assert_eq!(result.status, ResultStatus::Cancelled);
    }

    #[tokio::test]
    async fn forwards_one_invocation_state_to_every_node_and_event() {
        let invocation_state = InvocationState::new();
        invocation_state.insert("seen", "yes".to_owned());
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(5)
            .build()
            .unwrap();

        let events = collect(graph.stream_with(
            "go",
            MultiAgentInvokeOptions::default().with_invocation_state(invocation_state.clone()),
        ))
        .await;

        let before: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                MultiAgentStreamEvent::BeforeNodeCall(event) => Some(&event.invocation_state),
                _ => None,
            })
            .collect();
        assert_eq!(before.len(), 2);
        assert!(
            before.iter().all(|state| state.ptr_eq(&invocation_state)),
            "every node sees the same invocation state"
        );
    }

    #[tokio::test]
    async fn resumes_a_child_agent_interrupt_on_the_same_graph() {
        let graph = Graph::builder()
            .node(approving("a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(10)
            .build()
            .unwrap();

        let interrupted = graph.invoke("go").await.unwrap();
        assert_eq!(interrupted.status, ResultStatus::Interrupted);
        assert_eq!(interrupted.interrupts.len(), 1);
        assert_eq!(node_ids(&interrupted), vec!["a"]);

        let responses = vec![InterruptResponseContent::new(
            &interrupted.interrupts[0].id,
            json!("yes"),
        )];
        let resumed = graph
            .invoke(MultiAgentInput::InterruptResponses(responses))
            .await
            .unwrap();

        assert_eq!(resumed.status, ResultStatus::Completed);
        assert_eq!(node_ids(&resumed), vec!["a", "b"]);
    }

    #[tokio::test]
    async fn a_before_node_call_interrupt_gates_a_node_and_resumes() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .max_steps(5)
            .hook(|event: &mut BeforeNodeCallEvent| {
                let event = event.clone();
                async move {
                    let _: String = event.interrupt(InterruptParams::new("approve"))?;
                    Ok(())
                }
            })
            .build()
            .unwrap();

        let events = collect(graph.stream("go")).await;
        assert_eq!(
            names(&events),
            vec![
                "beforeMultiAgentInvocationEvent",
                "beforeNodeCallEvent",
                "nodeResultEvent",
                "afterNodeCallEvent",
                "afterMultiAgentInvocationEvent",
                "multiAgentResultEvent",
            ],
            "a gated node still reports a result and an after-call event"
        );
        let interrupted = events.last().unwrap().result().expect("the aggregate result");
        assert_eq!(interrupted.status, ResultStatus::Interrupted);

        let responses = vec![InterruptResponseContent::new(
            &interrupted.interrupts[0].id,
            json!("ok"),
        )];
        let resumed = graph
            .invoke(MultiAgentInput::InterruptResponses(responses))
            .await
            .unwrap();

        assert_eq!(resumed.status, ResultStatus::Completed);
        assert_eq!(text(&resumed), vec!["from a".to_owned()]);
    }

    #[tokio::test]
    async fn resuming_a_graph_whose_topology_changed_errors() {
        let graph = Graph::builder()
            .node(approving("a"))
            .max_steps(5)
            .build()
            .unwrap();
        let interrupted = graph.invoke("go").await.unwrap();
        let responses = vec![InterruptResponseContent::new(
            &interrupted.interrupts[0].id,
            json!("yes"),
        )];

        let other = Graph::builder()
            .node(saying("z", "from z"))
            .max_steps(5)
            .build()
            .unwrap();
        let error = other
            .invoke(MultiAgentInput::InterruptResponses(responses))
            .await
            .expect_err("the response matches no node");

        assert!(
            error
                .to_string()
                .contains("no node found with matching interrupt"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn resumes_from_the_next_ready_node_after_a_loaded_snapshot() {
        let first = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(1)
            .build()
            .unwrap();
        first.invoke("go").await.expect_err("max steps stops after a");
        let snapshot = first
            .take_snapshot(None, TakeMultiAgentSnapshotOptions::default())
            .unwrap();

        let resumed = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(5)
            .build()
            .unwrap();
        resumed.load_snapshot(&snapshot, None).unwrap();
        let result = resumed.invoke("go").await.unwrap();

        assert_eq!(node_ids(&result), vec!["a", "b"], "a's result carries over");
        assert_eq!(text(&result), vec!["from b".to_owned()]);
    }

    #[test]
    fn defaults_id_to_graph() {
        let graph = one_node("a").build().unwrap();
        assert_eq!(graph.id(), DEFAULT_GRAPH_ID);
        assert_eq!(graph.name(), "graph");
    }

    #[test]
    fn accepts_a_custom_id() {
        let graph = one_node("a").id("custom").build().unwrap();
        assert_eq!(graph.id(), "custom");
    }

    #[test]
    fn accepts_agent_node_options() {
        let graph = Graph::builder()
            .node(AgentNodeOptions::new(agent("a")).with_description("does a"))
            .build()
            .unwrap();
        let node = graph.nodes().get("a").unwrap();
        assert_eq!(node.node_type(), NodeType::Agent);
        assert_eq!(node.config().description.as_deref(), Some("does a"));
    }

    #[test]
    fn accepts_multi_agent_node_options() {
        let inner: Arc<dyn Node> = Arc::new(AgentNode::new(AgentNodeOptions::new(agent("inner"))).unwrap());
        let graph = Graph::builder().node(inner).build().unwrap();
        assert_eq!(graph.nodes().get("inner").unwrap().node_type(), NodeType::Agent);
    }

    #[test]
    fn accepts_pre_built_node_instances() {
        let node: Arc<dyn Node> = Arc::new(AgentNode::new(AgentNodeOptions::new(agent("a"))).unwrap());
        let graph = Graph::builder().node(Arc::clone(&node)).build().unwrap();
        assert!(Arc::ptr_eq(graph.nodes().get("a").unwrap(), &node));
    }

    #[test]
    fn takes_node_ids_from_the_nodes_themselves() {
        let graph = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .edge("a", "b")
            .build()
            .unwrap();
        assert_eq!(graph.nodes().keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(graph.edges()[0].source.id(), "a");
        assert_eq!(graph.edges()[0].target.id(), "b");
    }

    #[test]
    fn accepts_edge_options() {
        let graph = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .edge_when("a", "b", edge_handler(|_| true))
            .build()
            .unwrap();
        assert_eq!(graph.edges().len(), 1);
        assert!(graph.edges()[0].handler.is_some());
    }

    #[test]
    fn errors_on_duplicate_node_ids() {
        let builder = Graph::builder().node(agent("a")).node(agent("a"));
        assert_eq!(error(builder), "node_id=<a> | duplicate node id");
    }

    #[test]
    fn errors_on_edge_referencing_unknown_source_node() {
        let builder = one_node("a").edge("missing", "a");
        assert_eq!(
            error(builder),
            "source=<missing> | edge references unknown source node"
        );
    }

    #[test]
    fn errors_on_edge_referencing_unknown_target_node() {
        let builder = one_node("a").edge("a", "missing");
        assert_eq!(
            error(builder),
            "target=<missing> | edge references unknown target node"
        );
    }

    #[test]
    fn errors_when_graph_has_no_source_nodes() {
        let builder = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .edge("a", "b")
            .edge("b", "a");
        assert_eq!(error(builder), "graph has no source nodes");
    }

    #[test]
    fn errors_on_unreachable_nodes() {
        let builder = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .node(agent("c"))
            .edge("a", "b")
            .edge("c", "c");
        assert_eq!(error(builder), "node_id=<c> | unreachable from any source node");
    }

    #[test]
    fn errors_when_explicit_entry_point_references_unknown_node() {
        let builder = one_node("a").entry_point("missing");
        assert_eq!(
            error(builder),
            "source=<missing> | source references unknown node"
        );
    }

    #[test]
    fn errors_when_max_steps_is_below_one() {
        assert_eq!(
            error(one_node("a").max_steps(0)),
            "max_steps=<0> | must be at least 1"
        );
    }

    #[test]
    fn errors_when_max_concurrency_is_below_one() {
        assert_eq!(
            error(one_node("a").max_concurrency(0)),
            "max_concurrency=<0> | must be at least 1"
        );
    }

    #[test]
    fn errors_when_timeout_is_below_one() {
        assert_eq!(
            error(one_node("a").timeout(Duration::ZERO)),
            "timeout=<0> | must be at least 1"
        );
    }

    #[test]
    fn errors_when_node_timeout_is_below_one() {
        assert_eq!(
            error(one_node("a").node_timeout(Duration::ZERO)),
            "node_timeout=<0> | must be at least 1"
        );
    }

    #[test]
    fn defaults_every_limit_to_unbounded() {
        let graph = one_node("a").build().unwrap();
        let config = graph.config();
        assert!(config.max_concurrency.is_none());
        assert!(config.max_steps.is_none());
        assert!(config.timeout.is_none());
        assert!(config.node_timeout.is_none());
    }

    #[test]
    fn auto_detects_entry_points_from_nodes_without_incoming_edges() {
        let graph = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .edge("a", "b")
            .build()
            .unwrap();
        let ids: Vec<_> = graph.sources().iter().map(|node| node.id().to_owned()).collect();
        assert_eq!(ids, ["a"]);
    }

    #[test]
    fn explicit_entry_points_override_auto_detection() {
        let graph = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .edge("a", "b")
            .entry_point("a")
            .entry_point("b")
            .build()
            .unwrap();
        assert_eq!(graph.sources().len(), 2);
    }

    #[test]
    fn can_run_concurrently_is_false_for_a_single_linear_path() {
        let graph = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .edge("a", "b")
            .max_concurrency(1)
            .build()
            .unwrap();
        assert!(!graph.can_run_concurrently());
    }

    #[test]
    fn can_run_concurrently_is_true_when_a_node_fans_out() {
        let graph = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .node(agent("c"))
            .edge("a", "b")
            .edge("a", "c")
            .build()
            .unwrap();
        assert!(graph.can_run_concurrently());
    }

    #[tokio::test]
    async fn seeds_a_fresh_invocation_state_with_every_node_id() {
        let graph = Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(5)
            .build()
            .unwrap();

        let events = collect(graph.stream("go")).await;

        let ids = events
            .iter()
            .find_map(|event| match event {
                MultiAgentStreamEvent::BeforeMultiAgentInvocation(event) => Some(
                    event
                        .state
                        .with(|state| state.nodes().keys().cloned().collect::<Vec<_>>()),
                ),
                _ => None,
            })
            .expect("before-invocation event");
        assert_eq!(ids, ["a", "b"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_invocations_each_see_only_their_own_node_results() {
        let graph = Graph::builder()
            .node(EchoNode::shared("a"))
            .node(EchoNode::shared("b"))
            .edge("a", "b")
            .max_steps(5)
            .build()
            .unwrap();

        let (first, second) = tokio::join!(graph.invoke("first"), graph.invoke("second"));

        let (first, second) = (first.unwrap(), second.unwrap());
        assert_eq!(node_ids(&first), vec!["a", "b"], "first: {:?}", first.results);
        assert_eq!(node_ids(&second), vec!["a", "b"], "second: {:?}", second.results);
    }

    #[test]
    fn edge_handlers_read_the_invocation_state() {
        let graph = Graph::builder()
            .node(agent("a"))
            .node(agent("b"))
            .edge_when("a", "b", edge_handler(|state| state.steps > 0))
            .build()
            .unwrap();
        let handler = graph.edges()[0].handler.clone().unwrap();
        let state = MultiAgentStateHandle::new(MultiAgentState::new(["a".to_owned(), "b".to_owned()]));
        assert!(!futures::executor::block_on(handler(&state)));
        state.with_mut(|state| state.steps = 1);
        assert!(futures::executor::block_on(handler(&state)));
    }

    #[test]
    fn node_description_falls_back_to_the_agent_description() {
        let described = Agent::builder()
            .id("a")
            .description("writes things")
            .model(MockMessageModel::shared(vec![MockResponse::message(
                Message::assistant("reply"),
            )]))
            .build()
            .unwrap();
        let graph = Graph::builder().node(described).build().unwrap();
        assert_eq!(
            graph.nodes().get("a").unwrap().config().description.as_deref(),
            Some("writes things")
        );
    }
    #[tokio::test]
    async fn an_interrupt_leaves_queued_targets_pending() {
        let graph = Graph::builder()
            .node(approving("a"))
            .node(saying("b", "from b"))
            .max_concurrency(1)
            .max_steps(10)
            .build()
            .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(5), graph.invoke("go"))
            .await
            .expect("the loop terminates instead of spinning on the unrun target")
            .unwrap();

        assert_eq!(result.status, ResultStatus::Interrupted);
        assert_eq!(node_ids(&result), vec!["a"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn emits_an_after_node_call_event_for_every_node_that_ran() {
        for _ in 0..40 {
            let graph = Graph::builder()
                .node(saying("a", "from a"))
                .node(saying("b", "from b"))
                .edge("a", "b")
                .max_steps(10)
                .build()
                .unwrap();

            let events = collect(graph.stream("go")).await;

            let after = names(&events)
                .iter()
                .filter(|name| **name == "afterNodeCallEvent")
                .count();
            assert_eq!(after, 2, "events: {:?}", names(&events));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_node_timeout_error_survives_the_post_loop_drain() {
        for _ in 0..40 {
            let graph = Graph::builder()
                .node(sleeping("a", Duration::from_millis(400)))
                .node_timeout(Duration::from_millis(20))
                .max_steps(5)
                .build()
                .unwrap();

            let error = graph.invoke("go").await.expect_err("node timeout trips");

            assert!(
                error.to_string().contains("node exceeded wall-clock budget"),
                "unexpected error: {error}"
            );
        }
    }
}
