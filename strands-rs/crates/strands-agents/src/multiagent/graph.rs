//! Directed-graph orchestration (`strands-ts/src/multiagent/graph.ts`).
//!
//! Nodes run as a dependency graph: source nodes first, then every node whose incoming edges
//! are all satisfied. [`Graph`] is a cheap `Clone` handle over the shared topology.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use indexmap::IndexMap;

use super::edge::{Edge, EdgeDefinition, EdgeHandler};
use super::multiagent::MultiAgentInput;
use super::nodes::{Node, NodeConfig, NodeDefinition, NodeInputOptions, NodeSink, NodeType};
use super::plugins::{MultiAgentPlugin, MultiAgentPluginRegistry};
use super::state::{MultiAgentState, MultiAgentStateHandle, NodeResultUpdate};
use crate::agent::InvocationState;
use crate::error::{Error, Result};
use crate::hooks::{HookEvent, HookFn, HookRegistry};
use crate::logging::warn_once;

/// Default graph id, matching TS `id ?? 'graph'`.
pub const DEFAULT_GRAPH_ID: &str = "graph";

const UNBOUNDED_WARNING: &str = "graph has no max_steps or timeout set; execution is unbounded";

/// Runtime limits for a graph invocation (TS `GraphConfig`).
///
/// Every limit defaults to "no limit"; [`GraphBuilder::build`] warns once when neither
/// `max_steps` nor `timeout` is set.
#[derive(Clone, Debug, Default)]
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
    can_run_concurrently: bool,
    invocation_state: InvocationState,
    state: MultiAgentStateHandle,
    hooks: HookRegistry,
    plugins: MultiAgentPluginRegistry,
}

/// Directed graph orchestrator; a cheap `Clone` handle over one shared topology and state.
///
/// Build it with [`Graph::builder`]. Clones share the node set, the hook registry, and the
/// invocation state, so cloning a graph never forks its execution state.
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
    #[must_use]
    pub fn can_run_concurrently(&self) -> bool {
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

    /// Shared execution state: node statuses, results, and the app-owned store.
    #[must_use]
    pub fn state(&self) -> &MultiAgentStateHandle {
        &self.inner.state
    }

    /// State forwarded to every node's child agent, shared by all clones of this handle.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.inner.invocation_state
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
    nodes: Vec<(String, NodeDefinition)>,
    edges: Vec<EdgeDefinition>,
    entry_points: Option<Vec<String>>,
    config: GraphConfig,
    invocation_state: Option<InvocationState>,
    plugins: Vec<Arc<dyn MultiAgentPlugin>>,
    hooks: HookRegistry,
}

impl GraphBuilder {
    /// A fresh builder.
    pub fn new() -> Self {
        Self::default()
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

    /// Add a node under `id`, overriding the wrapped agent's or orchestrator's own id.
    pub fn node(mut self, id: impl Into<String>, definition: impl Into<NodeDefinition>) -> Self {
        self.nodes.push((id.into(), definition.into()));
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

    /// Seed the state forwarded to every node's child agent.
    pub fn set_state(mut self, state: InvocationState) -> Self {
        self.invocation_state = Some(state);
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
        let state = MultiAgentStateHandle::new(MultiAgentState::new(nodes.keys().cloned()));

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
                invocation_state: self.invocation_state.unwrap_or_default(),
                state,
                hooks: self.hooks,
                plugins: MultiAgentPluginRegistry::new(self.plugins),
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

fn resolve_nodes(definitions: Vec<(String, NodeDefinition)>) -> Result<IndexMap<String, Arc<dyn Node>>> {
    let mut nodes = IndexMap::with_capacity(definitions.len());
    for (id, definition) in definitions {
        if nodes.contains_key(&id) {
            return Err(Error::other(format!("node_id=<{id}> | duplicate node id")));
        }
        let node = definition.into_node()?;
        let node = if node.id() == id {
            node
        } else {
            Arc::new(RenamedNode {
                id: id.clone(),
                inner: node,
            }) as Arc<dyn Node>
        };
        nodes.insert(id, node);
    }
    Ok(nodes)
}

/// A node addressed by the id its graph gave it rather than the one its agent carries.
///
/// TS takes a node's id from the wrapped agent; the builder lets a caller name the node, so the
/// graph-facing id has to win everywhere topology is compared.
struct RenamedNode {
    id: String,
    inner: Arc<dyn Node>,
}

#[async_trait]
impl Node for RenamedNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> NodeType {
        self.inner.node_type()
    }

    fn config(&self) -> &NodeConfig {
        self.inner.config()
    }

    fn timeout(&self) -> Option<Duration> {
        self.inner.timeout()
    }

    async fn handle(
        &self,
        input: MultiAgentInput,
        state: &MultiAgentStateHandle,
        options: &NodeInputOptions,
        events: &NodeSink,
    ) -> Result<NodeResultUpdate> {
        self.inner.handle(input, state, options, events).await
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::multiagent::edge::edge_handler;
    use crate::multiagent::nodes::{AgentNode, AgentNodeOptions, NodeType};
    use crate::test_support::{MockMessageModel, MockResponse};
    use crate::types::Message;

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
        Graph::builder().node(id, agent(id))
    }

    fn error(builder: GraphBuilder) -> String {
        builder.build().expect_err("build fails").to_string()
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
            .node("a", AgentNodeOptions::new(agent("a")).with_description("does a"))
            .build()
            .unwrap();
        let node = graph.nodes().get("a").unwrap();
        assert_eq!(node.node_type(), NodeType::Agent);
        assert_eq!(node.config().description.as_deref(), Some("does a"));
    }

    #[test]
    fn accepts_multi_agent_node_options() {
        let inner: Arc<dyn Node> = Arc::new(AgentNode::new(AgentNodeOptions::new(agent("x"))).unwrap());
        let graph = Graph::builder().node("inner", inner).build().unwrap();
        assert_eq!(graph.nodes().get("inner").unwrap().node_type(), NodeType::Agent);
    }

    #[test]
    fn accepts_pre_built_node_instances() {
        let node: Arc<dyn Node> = Arc::new(AgentNode::new(AgentNodeOptions::new(agent("a"))).unwrap());
        let graph = Graph::builder().node("a", Arc::clone(&node)).build().unwrap();
        assert!(Arc::ptr_eq(graph.nodes().get("a").unwrap(), &node));
    }

    #[test]
    fn renames_a_node_whose_agent_id_differs_from_its_graph_id() {
        let graph = Graph::builder()
            .node("first", agent("a"))
            .node("second", agent("b"))
            .edge("first", "second")
            .build()
            .unwrap();
        assert_eq!(graph.nodes().get("first").unwrap().id(), "first");
        assert_eq!(graph.edges()[0].source.id(), "first");
        assert_eq!(graph.edges()[0].target.id(), "second");
    }

    #[test]
    fn accepts_edge_options() {
        let graph = Graph::builder()
            .node("a", agent("a"))
            .node("b", agent("b"))
            .edge_when("a", "b", edge_handler(|_| true))
            .build()
            .unwrap();
        assert_eq!(graph.edges().len(), 1);
        assert!(graph.edges()[0].handler.is_some());
    }

    #[test]
    fn errors_on_duplicate_node_ids() {
        let builder = Graph::builder().node("a", agent("a")).node("a", agent("a2"));
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
            .node("a", agent("a"))
            .node("b", agent("b"))
            .edge("a", "b")
            .edge("b", "a");
        assert_eq!(error(builder), "graph has no source nodes");
    }

    #[test]
    fn errors_on_unreachable_nodes() {
        let builder = Graph::builder()
            .node("a", agent("a"))
            .node("b", agent("b"))
            .node("c", agent("c"))
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
            .node("a", agent("a"))
            .node("b", agent("b"))
            .edge("a", "b")
            .build()
            .unwrap();
        let ids: Vec<_> = graph.sources().iter().map(|node| node.id().to_owned()).collect();
        assert_eq!(ids, ["a"]);
    }

    #[test]
    fn explicit_entry_points_override_auto_detection() {
        let graph = Graph::builder()
            .node("a", agent("a"))
            .node("b", agent("b"))
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
            .node("a", agent("a"))
            .node("b", agent("b"))
            .edge("a", "b")
            .max_concurrency(1)
            .build()
            .unwrap();
        assert!(!graph.can_run_concurrently());
    }

    #[test]
    fn can_run_concurrently_is_true_when_a_node_fans_out() {
        let graph = Graph::builder()
            .node("a", agent("a"))
            .node("b", agent("b"))
            .node("c", agent("c"))
            .edge("a", "b")
            .edge("a", "c")
            .build()
            .unwrap();
        assert!(graph.can_run_concurrently());
    }

    #[test]
    fn seeds_the_shared_state_with_every_node_id() {
        let graph = Graph::builder()
            .node("a", agent("a"))
            .node("b", agent("b"))
            .edge("a", "b")
            .build()
            .unwrap();
        let ids: Vec<_> = graph
            .state()
            .with(|state| state.nodes().keys().cloned().collect());
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn clones_share_one_state_handle() {
        let graph = one_node("a").build().unwrap();
        let clone = graph.clone();
        clone.state().with_mut(|state| state.steps = 7);
        assert_eq!(graph.state().steps(), 7);
    }

    #[test]
    fn edge_handlers_read_the_shared_state() {
        let graph = Graph::builder()
            .node("a", agent("a"))
            .node("b", agent("b"))
            .edge_when("a", "b", edge_handler(|state| state.steps > 0))
            .build()
            .unwrap();
        let handler = graph.edges()[0].handler.clone().unwrap();
        assert!(!futures::executor::block_on(handler(graph.state())));
        graph.state().with_mut(|state| state.steps = 1);
        assert!(futures::executor::block_on(handler(graph.state())));
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
        let graph = Graph::builder().node("a", described).build().unwrap();
        assert_eq!(
            graph.nodes().get("a").unwrap().config().description.as_deref(),
            Some("writes things")
        );
    }
}
