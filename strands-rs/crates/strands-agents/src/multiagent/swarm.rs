//! Swarm orchestration: agents run sequentially, each deciding via structured output whether to
//! hand off to another agent or produce the final response (`strands-ts/src/multiagent/swarm.ts`).

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::{json, Value as JsonValue};

use super::multiagent::{MultiAgent, MultiAgentInput, MultiAgentInvokeOptions, MultiAgentStream};
use super::nodes::{Node, NodeDefinition};
use super::plugins::{MultiAgentPlugin, MultiAgentPluginRegistry};
use super::snapshot::{load_snapshot, take_snapshot, TakeMultiAgentSnapshotOptions};
use super::state::{MultiAgentResult, MultiAgentStateHandle};
use crate::error::{Error, Result};
use crate::hooks::HookRegistry;
use crate::session::types::Snapshot;

/// Default swarm id, matching TS `id ?? 'swarm'`.
const DEFAULT_ID: &str = "swarm";

/// Runtime limits for a [`Swarm`] (TS `SwarmConfig`).
///
/// `None` means no limit, mirroring TS's `Infinity` defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SwarmConfig {
    /// Max total node executions, including the first. `None` for no limit.
    pub max_steps: Option<u64>,
    /// Detection window over completed nodes; pairs with `repetitive_handoff_min_unique_agents`.
    pub repetitive_handoff_detection_window: usize,
    /// Required unique nodes within the detection window.
    pub repetitive_handoff_min_unique_agents: usize,
    /// Wall-clock ceiling for the whole invocation. `None` for no limit.
    pub timeout: Option<Duration>,
    /// Fallback per-node wall-clock ceiling, used when a node sets none. `None` for no limit.
    pub node_timeout: Option<Duration>,
}

/// The structured output each swarm agent produces to decide the next step (TS `HandoffResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffResult {
    /// Agent to hand off to; `None` ends the swarm with `message` as the final response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Instructions for the next agent, or the final response when there is no handoff.
    pub message: String,
    /// Structured data passed to the next agent as a JSON text block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Map<String, JsonValue>>,
}

/// Swarm multi-agent orchestration: a `Clone` handle over shared state.
///
/// Agents execute one at a time; each is given a handoff schema and routes the swarm by
/// returning a target agent id, or ends it by omitting one. Build one with [`Swarm::builder`].
#[derive(Clone)]
pub struct Swarm(Arc<SwarmInner>);

struct SwarmInner {
    id: String,
    name: String,
    description: Option<String>,
    nodes: IndexMap<String, Arc<dyn Node>>,
    start: String,
    config: SwarmConfig,
    hooks: HookRegistry,
    plugins: MultiAgentPluginRegistry,
    initialized: Mutex<bool>,
}

impl Swarm {
    /// Start building a swarm.
    #[must_use]
    pub fn builder() -> SwarmBuilder {
        SwarmBuilder::default()
    }

    /// The swarm's unique id.
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// The swarm's human-readable name.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// What this swarm does, when it has a description.
    pub fn description(&self) -> Option<&str> {
        self.0.description.as_deref()
    }

    /// The swarm's nodes in insertion order, keyed by node id.
    pub fn nodes(&self) -> &IndexMap<String, Arc<dyn Node>> {
        &self.0.nodes
    }

    /// The node that receives the initial input.
    pub fn start(&self) -> &Arc<dyn Node> {
        &self.0.nodes[&self.0.start]
    }

    /// The swarm's runtime limits.
    pub fn config(&self) -> &SwarmConfig {
        &self.0.config
    }

    /// The swarm's hook registry.
    pub fn hooks(&self) -> &HookRegistry {
        &self.0.hooks
    }

    /// The swarm's plugin registry.
    pub fn plugins(&self) -> &MultiAgentPluginRegistry {
        &self.0.plugins
    }

    /// Capture the swarm's identity as a snapshot (`scope: "multiAgent"`).
    ///
    /// Pass the state handle of a live invocation to include its execution state.
    ///
    /// # Errors
    /// Propagates [`take_snapshot`] serialization failures.
    pub fn take_snapshot(
        &self,
        state: Option<&MultiAgentStateHandle>,
        options: TakeMultiAgentSnapshotOptions,
    ) -> Result<Snapshot> {
        take_snapshot(self, state, options)
    }

    /// Restore a snapshot into this swarm, replacing `state` when the snapshot carries one.
    ///
    /// # Errors
    /// Propagates [`load_snapshot`] validation failures.
    pub fn load_snapshot(&self, snapshot: &Snapshot, state: Option<&MultiAgentStateHandle>) -> Result<()> {
        load_snapshot(self, snapshot, state)
    }

    /// Run the swarm to completion with default options.
    ///
    /// # Errors
    /// Propagates node and limit failures.
    pub async fn invoke(&self, input: impl Into<MultiAgentInput>) -> Result<MultiAgentResult> {
        self.invoke_with(input.into(), MultiAgentInvokeOptions::default())
            .await
    }

    /// Stream the swarm with default options.
    pub fn stream(&self, input: impl Into<MultiAgentInput>) -> MultiAgentStream {
        self.stream_with(input.into(), MultiAgentInvokeOptions::default())
    }

    /// Initialize plugins and fire [`MultiAgentInitializedEvent`](super::MultiAgentInitializedEvent).
    ///
    /// Called automatically on the first invocation.
    ///
    /// # Errors
    /// Propagates plugin initialization failures.
    pub async fn initialize(&self) -> Result<()> {
        if *self.0.initialized.lock().expect("initialized mutex poisoned") {
            return Ok(());
        }
        let orchestrator: Arc<dyn MultiAgent> = Arc::new(self.clone());
        self.0.plugins.initialize(&orchestrator).await?;
        let mut event = super::events::MultiAgentInitializedEvent {
            orchestrator: orchestrator.clone(),
        };
        self.0.hooks.invoke_callbacks(&mut event).await?;
        *self.0.initialized.lock().expect("initialized mutex poisoned") = true;
        Ok(())
    }

    #[allow(dead_code)]
    /// The handoff schema handed to `node_id`: the other node ids, plus their descriptions.
    fn handoff_schema(&self, node_id: &str) -> JsonValue {
        let handoff_ids: Vec<&String> = self.0.nodes.keys().filter(|id| id.as_str() != node_id).collect();
        let descriptions = handoff_ids
            .iter()
            .map(
                |id| match self.0.nodes[id.as_str()].config().description.as_deref() {
                    Some(description) => format!("- {id}: {description}"),
                    None => format!("- {id}"),
                },
            )
            .collect::<Vec<_>>()
            .join("\n");

        let agent_id = if handoff_ids.is_empty() {
            json!({
                "not": {},
                "description": "No other agents available. Omit this field to end the conversation.",
            })
        } else {
            json!({
                "type": "string",
                "enum": handoff_ids,
                "description": format!(
                    "Target agent to hand off to. Omit to end the conversation.\n\nAvailable agents:\n{descriptions}"
                ),
            })
        };

        json!({
            "type": "object",
            "description": "Decide whether to hand off to another agent or produce a final response.",
            "properties": {
                "agentId": agent_id,
                "message": {
                    "type": "string",
                    "description": "Instructions for the next agent, or the final response if no handoff.",
                },
                "context": {
                    "type": "object",
                    "additionalProperties": true,
                    "description": "Structured data to pass to the next agent.",
                },
            },
            "required": ["message"],
            "additionalProperties": false,
        })
    }

    #[allow(dead_code)]
    /// Diagnostic reason when the completed-node history of this invocation looks like a loop.
    fn check_repetitive_handoff(&self, node_history: &[String]) -> Option<String> {
        let window = self.0.config.repetitive_handoff_detection_window;
        if window == 0 || node_history.len() < window {
            return None;
        }
        let recent = &node_history[node_history.len() - window..];
        let unique = recent.iter().collect::<std::collections::HashSet<_>>().len();
        (unique < self.0.config.repetitive_handoff_min_unique_agents)
            .then(|| format!("Repetitive handoff: {unique} unique nodes out of {window} recent iterations"))
    }

    #[allow(dead_code)]
    /// Errors when the step budget ran out while the last agent still wanted a handoff.
    fn check_steps(&self, steps: u64, handoff: Option<&HandoffResult>) -> Result<()> {
        let Some(max_steps) = self.0.config.max_steps else {
            return Ok(());
        };
        if handoff.is_some_and(|h| h.agent_id.is_some()) && steps >= max_steps {
            return Err(Error::other(format!(
                "max_steps=<{max_steps}> | swarm reached step limit"
            )));
        }
        Ok(())
    }
}

impl std::fmt::Debug for Swarm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Swarm")
            .field("id", &self.0.id)
            .field("nodes", &self.0.nodes.keys().collect::<Vec<_>>())
            .field("start", &self.0.start)
            .field("config", &self.0.config)
            .finish()
    }
}

#[async_trait]
impl MultiAgent for Swarm {
    fn id(&self) -> &str {
        &self.0.id
    }

    fn name(&self) -> &str {
        &self.0.name
    }

    fn description(&self) -> Option<&str> {
        self.0.description.as_deref()
    }

    fn hooks(&self) -> &HookRegistry {
        &self.0.hooks
    }

    async fn invoke_with(
        &self,
        _input: MultiAgentInput,
        _options: MultiAgentInvokeOptions,
    ) -> Result<MultiAgentResult> {
        Err(Error::other("swarm execution is not implemented yet"))
    }

    fn stream_with(&self, _input: MultiAgentInput, _options: MultiAgentInvokeOptions) -> MultiAgentStream {
        MultiAgentStream::new(Box::pin(futures::stream::once(async {
            Err(Error::other("swarm execution is not implemented yet"))
        })))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl From<Swarm> for NodeDefinition {
    fn from(swarm: Swarm) -> Self {
        let orchestrator: Arc<dyn MultiAgent> = Arc::new(swarm);
        NodeDefinition::from(orchestrator)
    }
}

/// Builder for a [`Swarm`].
#[derive(Default)]
pub struct SwarmBuilder {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    definitions: Vec<NodeDefinition>,
    entry_point: Option<String>,
    config: SwarmConfig,
    plugins: Vec<Arc<dyn MultiAgentPlugin>>,
    hooks: HookRegistry,
}

impl SwarmBuilder {
    /// Set the swarm's id. Defaults to `"swarm"`.
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the swarm's human-readable name. Defaults to the id.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set what this swarm does.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a node; its id comes from the wrapped agent or orchestrator.
    #[must_use]
    pub fn node(mut self, node: impl Into<NodeDefinition>) -> Self {
        self.definitions.push(node.into());
        self
    }

    /// Set the node that receives the initial input. Defaults to the first node added.
    #[must_use]
    pub fn entry_point(mut self, id: impl Into<String>) -> Self {
        self.entry_point = Some(id.into());
        self
    }

    /// Register a plugin.
    #[must_use]
    pub fn plugin(mut self, plugin: Arc<dyn MultiAgentPlugin>) -> Self {
        self.plugins.push(plugin);
        self
    }

    /// Register a hook callback on the swarm's registry.
    #[must_use]
    pub fn hook<E, F>(self, callback: F) -> Self
    where
        E: crate::hooks::HookEvent,
        F: Fn(&mut E) -> Result<()> + Send + Sync + 'static,
    {
        self.hooks.add_sync_callback::<E, F>(callback);
        self
    }

    /// Cap the total number of node executions, including the first.
    #[must_use]
    pub fn max_steps(mut self, max_steps: u64) -> Self {
        self.config.max_steps = Some(max_steps);
        self
    }

    /// Wall-clock ceiling for the whole invocation.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = Some(timeout);
        self
    }

    /// Fallback per-node wall-clock ceiling, used for nodes that set none.
    #[must_use]
    pub fn node_timeout(mut self, node_timeout: Duration) -> Self {
        self.config.node_timeout = Some(node_timeout);
        self
    }

    /// Window of completed nodes scanned for a handoff loop.
    #[must_use]
    pub fn repetitive_handoff_detection_window(mut self, window: usize) -> Self {
        self.config.repetitive_handoff_detection_window = window;
        self
    }

    /// Unique nodes required within the detection window.
    #[must_use]
    pub fn repetitive_handoff_min_unique_agents(mut self, minimum: usize) -> Self {
        self.config.repetitive_handoff_min_unique_agents = minimum;
        self
    }

    /// Build the swarm.
    ///
    /// # Errors
    /// Returns an error when the node list is empty, a node id is duplicated, the entry point is
    /// unknown, a limit is below 1, or the repetitive-handoff pair is inconsistent.
    pub fn build(self) -> Result<Swarm> {
        validate_config(&self.config)?;

        if self.definitions.is_empty() {
            return Err(Error::other("nodes list is empty"));
        }
        let mut nodes: IndexMap<String, Arc<dyn Node>> = IndexMap::new();
        for definition in self.definitions {
            let node = definition.into_node()?;
            let id = node.id().to_owned();
            if nodes.contains_key(&id) {
                return Err(Error::other(format!("agent_id=<{id}> | duplicate agent id")));
            }
            nodes.insert(id, node);
        }
        validate_repetitive_handoff(&self.config, nodes.len())?;

        let start = match self.entry_point {
            Some(id) => {
                if !nodes.contains_key(&id) {
                    return Err(Error::other(format!(
                        "start=<{id}> | start references unknown agent"
                    )));
                }
                id
            }
            None => nodes.keys().next().expect("non-empty").clone(),
        };

        if self.config.max_steps.is_none() && self.config.timeout.is_none() {
            tracing::warn!("swarm has no maxSteps or timeout set; execution is unbounded");
        }

        let id = self.id.unwrap_or_else(|| DEFAULT_ID.to_owned());
        let name = self.name.unwrap_or_else(|| id.clone());
        Ok(Swarm(Arc::new(SwarmInner {
            id,
            name,
            description: self.description,
            nodes,
            start,
            config: self.config,
            hooks: self.hooks,
            plugins: MultiAgentPluginRegistry::new(self.plugins),
            initialized: Mutex::new(false),
        })))
    }
}

fn validate_config(config: &SwarmConfig) -> Result<()> {
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

/// Both detection settings must be 0 or both positive, and the uniqueness threshold must fit
/// within the window and the swarm's node count.
fn validate_repetitive_handoff(config: &SwarmConfig, node_count: usize) -> Result<()> {
    let window = config.repetitive_handoff_detection_window;
    let minimum = config.repetitive_handoff_min_unique_agents;
    if (window == 0) != (minimum == 0) {
        return Err(Error::other(format!(
            "repetitive_handoff_detection_window=<{window}>, repetitive_handoff_min_unique_agents=<{minimum}> | must both be 0 or both be positive"
        )));
    }
    if minimum > window {
        return Err(Error::other(format!(
            "repetitive_handoff_min_unique_agents=<{minimum}>, repetitive_handoff_detection_window=<{window}> | minimum unique agents cannot exceed detection window"
        )));
    }
    if minimum > node_count {
        return Err(Error::other(format!(
            "repetitive_handoff_min_unique_agents=<{minimum}>, nodes=<{node_count}> | minimum unique agents cannot exceed node count"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::testing::MockMessageModel;

    fn agent(id: &str, description: Option<&str>) -> Agent {
        let mut builder = Agent::builder().model(MockMessageModel::new(vec![])).id(id);
        if let Some(description) = description {
            builder = builder.description(description);
        }
        builder.build().expect("agent builds")
    }

    fn swarm(ids: &[&str]) -> Swarm {
        let mut builder = Swarm::builder();
        for id in ids {
            builder = builder.node(agent(id, None));
        }
        builder.build().expect("swarm builds")
    }

    #[test]
    fn defaults_the_id_and_start_node() {
        let swarm = swarm(&["a", "b"]);
        assert_eq!(swarm.id(), "swarm");
        assert_eq!(swarm.name(), "swarm");
        assert_eq!(swarm.start().id(), "a");
        assert_eq!(swarm.nodes().keys().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn rejects_an_empty_node_list() {
        let error = Swarm::builder().build().expect_err("empty");
        assert_eq!(error.to_string(), "nodes list is empty");
    }

    #[test]
    fn rejects_duplicate_agent_ids() {
        let error = Swarm::builder()
            .node(agent("a", None))
            .node(agent("a", None))
            .build()
            .expect_err("duplicate");
        assert_eq!(error.to_string(), "agent_id=<a> | duplicate agent id");
    }

    #[test]
    fn rejects_an_unknown_entry_point() {
        let error = Swarm::builder()
            .node(agent("a", None))
            .entry_point("missing")
            .build()
            .expect_err("unknown start");
        assert_eq!(
            error.to_string(),
            "start=<missing> | start references unknown agent"
        );
    }

    #[test]
    fn resolves_an_explicit_entry_point() {
        let swarm = Swarm::builder()
            .node(agent("a", None))
            .node(agent("b", None))
            .entry_point("b")
            .build()
            .expect("swarm builds");
        assert_eq!(swarm.start().id(), "b");
    }

    #[test]
    fn rejects_limits_below_one() {
        for (builder, message) in [
            (
                Swarm::builder().max_steps(0),
                "max_steps=<0> | must be at least 1",
            ),
            (
                Swarm::builder().timeout(Duration::ZERO),
                "timeout=<0> | must be at least 1",
            ),
            (
                Swarm::builder().node_timeout(Duration::ZERO),
                "node_timeout=<0> | must be at least 1",
            ),
        ] {
            let error = builder.node(agent("a", None)).build().expect_err("limit");
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn rejects_a_half_configured_repetitive_handoff_pair() {
        let error = Swarm::builder()
            .node(agent("a", None))
            .repetitive_handoff_detection_window(3)
            .build()
            .expect_err("half configured");
        assert_eq!(
            error.to_string(),
            "repetitive_handoff_detection_window=<3>, repetitive_handoff_min_unique_agents=<0> | must both be 0 or both be positive"
        );
    }

    #[test]
    fn rejects_a_uniqueness_threshold_above_the_window() {
        let error = Swarm::builder()
            .node(agent("a", None))
            .node(agent("b", None))
            .node(agent("c", None))
            .repetitive_handoff_detection_window(2)
            .repetitive_handoff_min_unique_agents(3)
            .build()
            .expect_err("threshold above window");
        assert_eq!(
            error.to_string(),
            "repetitive_handoff_min_unique_agents=<3>, repetitive_handoff_detection_window=<2> | minimum unique agents cannot exceed detection window"
        );
    }

    #[test]
    fn rejects_a_uniqueness_threshold_above_the_node_count() {
        let error = Swarm::builder()
            .node(agent("a", None))
            .node(agent("b", None))
            .repetitive_handoff_detection_window(3)
            .repetitive_handoff_min_unique_agents(3)
            .build()
            .expect_err("threshold above node count");
        assert_eq!(
            error.to_string(),
            "repetitive_handoff_min_unique_agents=<3>, nodes=<2> | minimum unique agents cannot exceed node count"
        );
    }

    #[test]
    fn builds_a_handoff_schema_listing_the_other_nodes_with_descriptions() {
        let swarm = Swarm::builder()
            .node(agent("a", None))
            .node(agent("b", Some("writes things")))
            .node(agent("c", None))
            .build()
            .expect("swarm builds");
        let schema = swarm.handoff_schema("a");
        let agent_id = &schema["properties"]["agentId"];
        assert_eq!(agent_id["enum"], json!(["b", "c"]));
        assert_eq!(
            agent_id["description"],
            json!("Target agent to hand off to. Omit to end the conversation.\n\nAvailable agents:\n- b: writes things\n- c")
        );
        assert_eq!(schema["required"], json!(["message"]));
    }

    #[test]
    fn builds_a_handoff_schema_with_no_targets_for_a_single_node_swarm() {
        let swarm = swarm(&["a"]);
        let agent_id = &swarm.handoff_schema("a")["properties"]["agentId"];
        assert_eq!(
            agent_id["description"],
            json!("No other agents available. Omit this field to end the conversation.")
        );
    }

    #[test]
    fn detects_a_repetitive_handoff_loop_inside_the_window() {
        let swarm = Swarm::builder()
            .node(agent("a", None))
            .node(agent("b", None))
            .repetitive_handoff_detection_window(3)
            .repetitive_handoff_min_unique_agents(2)
            .build()
            .expect("swarm builds");
        let history: Vec<String> = ["a", "a", "a"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(
            swarm.check_repetitive_handoff(&history),
            Some("Repetitive handoff: 1 unique nodes out of 3 recent iterations".to_owned())
        );
    }

    #[test]
    fn allows_enough_unique_nodes_in_the_window() {
        let swarm = Swarm::builder()
            .node(agent("a", None))
            .node(agent("b", None))
            .repetitive_handoff_detection_window(2)
            .repetitive_handoff_min_unique_agents(2)
            .build()
            .expect("swarm builds");
        let history: Vec<String> = ["a", "b"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(swarm.check_repetitive_handoff(&history), None);
    }

    #[test]
    fn errors_when_the_step_limit_is_reached_with_a_pending_handoff() {
        let swarm = Swarm::builder()
            .node(agent("a", None))
            .node(agent("b", None))
            .max_steps(2)
            .build()
            .expect("swarm builds");
        let handoff = HandoffResult {
            agent_id: Some("b".to_owned()),
            message: "go".to_owned(),
            context: None,
        };
        let error = swarm.check_steps(2, Some(&handoff)).expect_err("step limit");
        assert_eq!(error.to_string(), "max_steps=<2> | swarm reached step limit");
        assert!(swarm.check_steps(2, None).is_ok());
    }

    #[test]
    fn serializes_a_handoff_result_like_ts() {
        let handoff = HandoffResult {
            agent_id: Some("b".to_owned()),
            message: "go".to_owned(),
            context: None,
        };
        assert_eq!(
            serde_json::to_value(&handoff).expect("serializes"),
            json!({"agentId": "b", "message": "go"})
        );
    }
}
