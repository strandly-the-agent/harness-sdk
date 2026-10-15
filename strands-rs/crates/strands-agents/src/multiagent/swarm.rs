//! Swarm orchestration: agents run sequentially, each deciding via structured output whether to
//! hand off to another agent or produce the final response (`strands-ts/src/multiagent/swarm.ts`).

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::{json, Value as JsonValue};
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use super::events::{
    AfterMultiAgentInvocationEvent, AfterNodeCallEvent, BeforeMultiAgentInvocationEvent, BeforeNodeCallEvent,
    MultiAgentHandoffEvent, MultiAgentInitializedEvent, MultiAgentResultEvent, MultiAgentStreamEvent,
    NodeCancelEvent, NodeResultEvent, OrchestratorRef,
};
use super::multiagent::{
    apply_orchestrator_hook_responses, drop_stale_interrupted_result, group_interrupt_responses_by_node,
    record_hook_interrupt, MultiAgent, MultiAgentInput, MultiAgentInvokeOptions, MultiAgentStream,
};
use super::nodes::{run_node, Node, NodeDefinition, NodeInputOptions, NodeSink};
use super::plugins::{MultiAgentPlugin, MultiAgentPluginRegistry};
use super::snapshot::{load_snapshot, take_snapshot, TakeMultiAgentSnapshotOptions};
use super::state::{
    now_ms, MultiAgentResult, MultiAgentState, MultiAgentStateHandle, NodeResult, ResultStatus, Status,
};
use crate::agent::InvocationState;
use crate::error::{Error, Result};
use crate::hooks::{Cancel, HookEvent, HookRegistry};
use crate::session::types::Snapshot;
use crate::types::{ContentBlock, InterruptResponseContent};

/// Default swarm id, matching TS `id ?? 'swarm'`.
const DEFAULT_ID: &str = "swarm";

/// Reason reported when a hook cancels a node without giving one.
const DEFAULT_CANCEL_MESSAGE: &str = "node cancelled by hook";

/// Buffer of the channel the invocation task writes its events to.
const EVENT_BUFFER: usize = 64;

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
    pending_interrupt_state: Mutex<Option<MultiAgentStateHandle>>,
}

/// What one invocation of the loop carries into every node run.
struct Invocation {
    orchestrator: OrchestratorRef,
    state: MultiAgentStateHandle,
    invocation_state: InvocationState,
    cancel: Option<CancellationToken>,
    /// When the swarm's wall-clock budget runs out, if it has one.
    deadline: Option<Instant>,
}

impl Invocation {
    fn after_node(&self, node_id: &str, error: Option<String>) -> AfterNodeCallEvent {
        AfterNodeCallEvent {
            orchestrator: self.orchestrator.clone(),
            state: self.state.clone(),
            node_id: node_id.to_owned(),
            invocation_state: self.invocation_state.clone(),
            error,
        }
    }

    fn node_result(&self, node: &dyn Node, result: NodeResult) -> NodeResultEvent {
        NodeResultEvent {
            node_id: node.id().to_owned(),
            node_type: node.node_type(),
            state: self.state.clone(),
            result,
            invocation_state: self.invocation_state.clone(),
        }
    }
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
        self.invoke_with(input, MultiAgentInvokeOptions::default()).await
    }

    /// [`Swarm::invoke`] with options.
    ///
    /// # Errors
    /// Propagates node and limit failures.
    pub async fn invoke_with(
        &self,
        input: impl Into<MultiAgentInput>,
        options: MultiAgentInvokeOptions,
    ) -> Result<MultiAgentResult> {
        // Inline, not spawned: the loop runs in the caller's task, so a panic in a hook or a node
        // propagates to the caller instead of ending the stream without a result.
        let (tx, mut rx) = mpsc::channel(EVENT_BUFFER);
        let drain = async move { while rx.recv().await.is_some() {} };
        let (result, ()) = tokio::join!(self.run(input.into(), options, tx), drain);
        result
    }

    /// Stream the swarm with default options.
    ///
    /// # Panics
    /// Panics if called outside a tokio runtime (it spawns the invocation task).
    pub fn stream(&self, input: impl Into<MultiAgentInput>) -> MultiAgentStream {
        self.stream_with(input, MultiAgentInvokeOptions::default())
    }

    /// [`Swarm::stream`] with options.
    ///
    /// The loop runs in a spawned task, so cleanup events are emitted even when the consumer stops
    /// reading. Dropping the stream ends the invocation.
    ///
    /// # Panics
    /// Panics if called outside a tokio runtime (it spawns the invocation task).
    pub fn stream_with(
        &self,
        input: impl Into<MultiAgentInput>,
        options: MultiAgentInvokeOptions,
    ) -> MultiAgentStream {
        let (tx, mut rx) = mpsc::channel(EVENT_BUFFER);
        let swarm = self.clone();
        let input = input.into();
        tokio::spawn(async move {
            if let Err(error) = swarm.run(input, options, tx.clone()).await {
                let _ = tx.send(Err(error)).await;
            }
        });
        MultiAgentStream::new(Box::pin(async_stream::stream! {
            while let Some(event) = rx.recv().await {
                yield event;
            }
        }))
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
        let mut event = MultiAgentInitializedEvent {
            orchestrator: orchestrator.clone(),
        };
        self.0.hooks.invoke_callbacks(&mut event).await?;
        *self.0.initialized.lock().expect("initialized mutex poisoned") = true;
        Ok(())
    }

    /// One invocation, from `initialize` to the terminal result event (TS `Swarm._stream`).
    ///
    /// Takes `tx` by value so the channel closes when this future is dropped or completes.
    async fn run(
        &self,
        input: MultiAgentInput,
        options: MultiAgentInvokeOptions,
        tx: NodeSink,
    ) -> Result<MultiAgentResult> {
        let tx = &tx;
        self.initialize().await?;

        // Reuse the state of a prior INTERRUPTED run so `invoke(responses)` resumes on the same
        // instance without a session manager.
        let pending = self.pending_interrupt_state().take();
        let mut run = Invocation {
            orchestrator: Arc::new(self.clone()),
            state: pending.unwrap_or_else(|| {
                MultiAgentStateHandle::new(MultiAgentState::new(self.0.nodes.keys().cloned()))
            }),
            // Shared by reference across every node, so mutations in one are visible to the next.
            invocation_state: options.invocation_state.unwrap_or_default(),
            cancel: options.cancel,
            deadline: None,
        };

        // A session manager (or any plugin) may restore `state.results` from this hook.
        self.emit(
            tx,
            BeforeMultiAgentInvocationEvent {
                orchestrator: run.orchestrator.clone(),
                state: run.state.clone(),
                invocation_state: run.invocation_state.clone(),
            },
            MultiAgentStreamEvent::BeforeMultiAgentInvocation,
        )
        .await?;

        // Routing runs before the guarded section, as in TS, so a resume that cannot be routed
        // never reaches the after-invocation hook.
        let routed = self.route(&input, &run)?;

        // Armed after the before-invocation hook and routing, as TS does (`swarm.ts:331-335`), so a
        // slow session restore does not eat the budget. The timer starts fresh per invocation:
        // human response time between resumes is not deducted.
        run.deadline = deadline_after(self.0.config.timeout);

        let outcome = self.execute(&input, routed, &run, tx).await;

        // TS emits this from a `finally`, so it runs on the error path too — and a hook that fails
        // there replaces the in-flight error, as a JS `finally` does.
        self.emit(
            tx,
            AfterMultiAgentInvocationEvent {
                orchestrator: run.orchestrator.clone(),
                state: run.state.clone(),
                invocation_state: run.invocation_state.clone(),
            },
            MultiAgentStreamEvent::AfterMultiAgentInvocation,
        )
        .await?;
        let result = outcome?;

        self.emit(
            tx,
            MultiAgentResultEvent {
                result: result.clone(),
                invocation_state: run.invocation_state.clone(),
            },
            MultiAgentStreamEvent::MultiAgentResult,
        )
        .await?;
        Ok(result)
    }

    /// The node this invocation starts from, its input, and the handoff it is resuming, if any.
    ///
    /// Resume input routes straight to the interrupted node, bypassing handoff-derived resume. On a
    /// fresh run the input is stashed for replay if a hook gate pauses before the node runs.
    fn route(
        &self,
        input: &MultiAgentInput,
        run: &Invocation,
    ) -> Result<(Arc<dyn Node>, MultiAgentInput, Option<HandoffResult>)> {
        if let Some(responses) = input.resume_responses() {
            let grouped = run
                .state
                .with(|state| group_interrupt_responses_by_node(responses, state))?;
            let (node, next_input) = self.route_resume(grouped, &run.state)?;
            return Ok((node, next_input, None));
        }

        run.state
            .with_mut(|state| state.pending_input = Some(input.clone()));
        match self.find_resume_node(&run.state) {
            Some((node, handoff)) => Ok((node, input.clone(), Some(handoff))),
            None => Ok((self.start().clone(), input.clone(), None)),
        }
    }

    /// The step loop: handoffs, limits and the aggregate result.
    async fn execute(
        &self,
        input: &MultiAgentInput,
        routed: (Arc<dyn Node>, MultiAgentInput, Option<HandoffResult>),
        run: &Invocation,
        tx: &NodeSink,
    ) -> Result<MultiAgentResult> {
        let (mut node, mut next_input, mut handoff) = routed;

        let mut repetitive_handoff = None;
        let mut history: Vec<String> = Vec::new();
        loop {
            if self
                .0
                .config
                .max_steps
                .is_some_and(|max| run.state.steps() >= max)
            {
                break;
            }
            repetitive_handoff = self.check_repetitive_handoff(&history);
            if let Some(reason) = &repetitive_handoff {
                tracing::debug!("reason=<{reason}> | stopping swarm execution");
                break;
            }
            if self.budget_spent(run) {
                return Err(self.timeout_error(None));
            }
            if run.cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
                return Err(Error::other(format!(
                    "swarm_id=<{}> | swarm cancelled by external signal",
                    self.0.id
                )));
            }
            run.state.with_mut(|state| state.steps += 1);

            let result = self
                .stream_node(&node, next_input, handoff.as_ref(), run, tx)
                .await?;
            // The first step may consume routed resume responses; later nodes see the original input.
            next_input = input.clone();
            handoff = result
                .structured_output
                .as_ref()
                .and_then(|output| serde_json::from_value(output.clone()).ok());
            if result.status == ResultStatus::Completed {
                history.push(result.node_id.clone());
            }

            if self.budget_spent(run) {
                return Err(self.timeout_error(Some(node.id())));
            }
            if matches!(result.status, ResultStatus::Failed | ResultStatus::Interrupted) {
                break;
            }
            let Some(target_id) = handoff.as_ref().and_then(|handoff| handoff.agent_id.clone()) else {
                break;
            };
            // TS's zod enum rejects an unknown target inside the agent; the Rust schema is advisory,
            // so the orchestrator rejects it here instead of routing nowhere.
            let Some(target) = self.0.nodes.get(&target_id).cloned() else {
                return Err(Error::other(format!(
                    "handoff_target=<{target_id}>, swarm_id=<{}> | handoff target is not a node in the swarm",
                    self.0.id
                )));
            };
            self.emit(
                tx,
                MultiAgentHandoffEvent {
                    source: node.id().to_owned(),
                    targets: vec![target.id().to_owned()],
                    state: run.state.clone(),
                    invocation_state: run.invocation_state.clone(),
                },
                MultiAgentStreamEvent::MultiAgentHandoff,
            )
            .await?;
            tracing::debug!(source = node.id(), target = target.id(), "swarm handoff");
            node = target;
        }

        self.check_steps(run.state.steps(), handoff.as_ref())?;

        let content = self.resolve_content(&run.state);
        let duration = now_ms().saturating_sub(run.state.with(|state| state.start_time));
        let mut result = MultiAgentResult::new(run.state.results(), duration).with_content(content);
        if let Some(reason) = repetitive_handoff {
            result = result.with_status(ResultStatus::Failed).with_error(reason);
        }

        // Stash the state on interrupt so a same-instance resume has it; otherwise start fresh.
        if result.status == ResultStatus::Interrupted {
            *self.pending_interrupt_state() = Some(run.state.clone());
        } else {
            *self.pending_interrupt_state() = None;
            run.state.with_mut(|state| state.pending_input = None);
        }
        Ok(result)
    }

    /// Runs one node: the hook gate, the node itself, and the events around it (TS `_streamNode`).
    async fn stream_node(
        &self,
        node: &Arc<dyn Node>,
        input: MultiAgentInput,
        handoff: Option<&HandoffResult>,
        run: &Invocation,
        tx: &NodeSink,
    ) -> Result<NodeResult> {
        let node_id = node.id().to_owned();
        let mut before = BeforeNodeCallEvent {
            orchestrator: run.orchestrator.clone(),
            state: run.state.clone(),
            node_id: node_id.clone(),
            invocation_state: run.invocation_state.clone(),
            cancel: Cancel::No,
        };
        if let Err(error) = self.0.hooks.invoke_callbacks(&mut before).await {
            if !error.is_control_flow() {
                return Err(error);
            }
            // A hook-raised interrupt pauses the run before the node executes; it still terminates
            // with the event pair every node run ends on.
            let result = run.state.with_mut(|state| {
                let result = record_hook_interrupt(&node_id, state.node_or_insert(&node_id));
                state.results.push(result.clone());
                result
            });
            self.send(tx, MultiAgentStreamEvent::BeforeNodeCall(before))
                .await?;
            self.emit(
                tx,
                run.node_result(node.as_ref(), result.clone()),
                MultiAgentStreamEvent::NodeResult,
            )
            .await?;
            self.emit(
                tx,
                run.after_node(&node_id, None),
                MultiAgentStreamEvent::AfterNodeCall,
            )
            .await?;
            return Ok(result);
        }
        let cancel = before.cancel.clone();
        self.send(tx, MultiAgentStreamEvent::BeforeNodeCall(before))
            .await?;

        if let Some(message) = cancel.message_or(DEFAULT_CANCEL_MESSAGE) {
            // The cancel path skips `run_node`, so do its INTERRUPTED cleanup here.
            let result = run.state.with_mut(|state| {
                drop_stale_interrupted_result(&node_id, state);
                let result = NodeResult::new(&node_id, ResultStatus::Cancelled, 0);
                let node_state = state.node_or_insert(&node_id);
                node_state.status = Status::Cancelled;
                node_state.results.push(result.clone());
                state.results.push(result.clone());
                result
            });
            self.emit(
                tx,
                NodeCancelEvent {
                    node_id: node_id.clone(),
                    state: run.state.clone(),
                    message: message.to_owned(),
                    invocation_state: run.invocation_state.clone(),
                },
                MultiAgentStreamEvent::NodeCancel,
            )
            .await?;
            self.emit(
                tx,
                run.after_node(&node_id, None),
                MultiAgentStreamEvent::AfterNodeCall,
            )
            .await?;
            return Ok(result);
        }

        // TS wraps the node run in a try/catch that reports the error on `AfterNodeCallEvent`
        // before rethrowing, so observers always see the node terminate.
        match self.drive_node(node, input, handoff, run, tx).await {
            Ok(result) => Ok(result),
            Err(error) => {
                self.emit(
                    tx,
                    run.after_node(node.id(), Some(error.to_string())),
                    MultiAgentStreamEvent::AfterNodeCall,
                )
                .await?;
                Err(error)
            }
        }
    }

    /// Runs the node itself: its timeout, its event stream, and its result (TS `_streamNode`'s
    /// `try` block).
    async fn drive_node(
        &self,
        node: &Arc<dyn Node>,
        input: MultiAgentInput,
        handoff: Option<&HandoffResult>,
        run: &Invocation,
        tx: &NodeSink,
    ) -> Result<NodeResult> {
        let node_id = node.id().to_owned();
        let node_timeout = node.timeout().or(self.0.config.node_timeout);
        let options = NodeInputOptions::default()
            .with_structured_output_schema(self.handoff_schema(&node_id))
            .with_invocation_state(run.invocation_state.clone())
            // The swarm budget and the external signal compose with the node's own, so a hung node
            // is aborted by whichever expires first.
            .with_cancel(CancellationToken::new());
        let token = options.cancel.clone().expect("cancel token set above");

        // Fixed when the node starts: a node that streams events must not push its own deadline out.
        let node_deadline = deadline_after(node_timeout);
        let (node_tx, mut node_rx) = mpsc::channel(EVENT_BUFFER);
        let execution = run_node(
            node.as_ref(),
            resolve_node_input(input, handoff),
            &run.state,
            &options,
            &node_tx,
        );
        tokio::pin!(execution);

        let mut node_expired = false;
        let mut swarm_expired = false;
        let mut cancelled = false;
        let result = loop {
            tokio::select! {
                biased;
                Some(event) = node_rx.recv() => self.forward(tx, event?).await?,
                () = wait_until(node_deadline), if !node_expired => {
                    node_expired = true;
                    token.cancel();
                }
                () = wait_until(run.deadline), if !swarm_expired => {
                    swarm_expired = true;
                    token.cancel();
                }
                () = wait_cancelled(run.cancel.as_ref()), if !cancelled => {
                    cancelled = true;
                    token.cancel();
                }
                result = &mut execution => break result,
            }
        };
        while let Ok(event) = node_rx.try_recv() {
            self.forward(tx, event?).await?;
        }

        if node_expired {
            return Err(Error::other(format!(
                "node_timeout=<{}>, node_id=<{node_id}>, swarm_id=<{}> | node exceeded wall-clock budget",
                node_timeout.unwrap_or_default().as_millis(),
                self.0.id
            )));
        }

        // `run_node` records the result on the node; the orchestrator owns the aggregate list.
        run.state.with_mut(|state| state.results.push(result.clone()));
        self.emit(
            tx,
            run.after_node(&node_id, None),
            MultiAgentStreamEvent::AfterNodeCall,
        )
        .await?;
        Ok(result)
    }

    /// Routes a resume invocation to the one node that was left `INTERRUPTED`.
    fn route_resume(
        &self,
        grouped: IndexMap<String, Vec<InterruptResponseContent>>,
        state: &MultiAgentStateHandle,
    ) -> Result<(Arc<dyn Node>, MultiAgentInput)> {
        // A swarm runs one node at a time, so at most one can be INTERRUPTED. Assert it so a future
        // change that produces several surfaces loudly instead of silently taking the first.
        if grouped.len() > 1 {
            let nodes = grouped.keys().cloned().collect::<Vec<_>>().join(",");
            return Err(Error::other(format!(
                "swarm_id=<{}>, interrupted_nodes=<{nodes}> | swarm cannot have multiple interrupted nodes simultaneously",
                self.0.id
            )));
        }
        let Some((node_id, responses)) = grouped.into_iter().next() else {
            return Err(Error::other(format!(
                "swarm_id=<{}> | no interrupt responses to route",
                self.0.id
            )));
        };
        let Some(node) = self.0.nodes.get(&node_id).cloned() else {
            return Err(Error::other(format!(
                "node_id=<{node_id}>, swarm_id=<{}> | resume response targets a node missing from the swarm; topology changed between save and resume?",
                self.0.id
            )));
        };
        // Orchestrator hooks consume the responses that match their own interrupts; the rest go to
        // the child agent. If the hooks consumed them all, replay the original invocation input.
        let forwarded = state.with_mut(|state| {
            state
                .node_mut(&node_id)
                .map(|node_state| apply_orchestrator_hook_responses(node_state, responses))
        });
        let Some(forwarded) = forwarded else {
            return Err(Error::other(format!(
                "node_id=<{node_id}>, swarm_id=<{}> | routed interrupt response targets a node missing from state; topology changed between save and resume?",
                self.0.id
            )));
        };
        let input = if forwarded.is_empty() {
            state
                .with(|state| state.pending_input.clone())
                .unwrap_or_default()
        } else {
            MultiAgentInput::InterruptResponses(forwarded)
        };
        Ok((node, input))
    }

    /// The node a restored state left pending: the handoff target the previous run never ran.
    ///
    /// `None` when there is nothing to resume, which starts from the entry point.
    fn find_resume_node(&self, state: &MultiAgentStateHandle) -> Option<(Arc<dyn Node>, HandoffResult)> {
        let last = state.with(|state| state.results.last().cloned())?;
        let handoff: HandoffResult = serde_json::from_value(last.structured_output?).ok()?;
        let agent_id = handoff.agent_id.clone()?;
        let Some(node) = self.0.nodes.get(&agent_id).cloned() else {
            tracing::warn!("node_id=<{agent_id}> | resume target not found in swarm, starting fresh");
            return None;
        };
        tracing::debug!(
            node_id = node.id(),
            prior_steps = state.steps(),
            "resuming swarm from restored state"
        );
        Some((node, handoff))
    }

    /// The swarm's final content: the last node's handoff message, or its content when it had none.
    ///
    /// Marks the node that produced it as the terminus.
    fn resolve_content(&self, state: &MultiAgentStateHandle) -> Vec<ContentBlock> {
        state.with_mut(|state| {
            let Some(last) = state.results.last().cloned() else {
                return Vec::new();
            };
            if let Some(node) = state.node_mut(&last.node_id) {
                node.terminus = true;
            }
            let handoff: Option<HandoffResult> = last
                .structured_output
                .as_ref()
                .and_then(|output| serde_json::from_value(output.clone()).ok());
            match handoff {
                Some(handoff) if !handoff.message.is_empty() => vec![ContentBlock::text(handoff.message)],
                _ => last.content,
            }
        })
    }

    /// Dispatches `event` to the swarm's hooks, then yields it on the stream.
    async fn emit<E, F>(&self, tx: &NodeSink, mut event: E, wrap: F) -> Result<()>
    where
        E: HookEvent,
        F: FnOnce(E) -> MultiAgentStreamEvent,
    {
        self.0.hooks.invoke_callbacks(&mut event).await?;
        self.send(tx, wrap(event)).await
    }

    /// Yields `event` without dispatching it; a closed stream ends the invocation.
    async fn send(&self, tx: &NodeSink, event: MultiAgentStreamEvent) -> Result<()> {
        tx.send(Ok(event)).await.map_err(|_| Error::Cancelled)
    }

    /// Forwards a node's event, dispatching it to the swarm's hooks first (TS `_emit`s every
    /// `HookableEvent` a node yields, which is every multi-agent event).
    async fn forward(&self, tx: &NodeSink, event: MultiAgentStreamEvent) -> Result<()> {
        use MultiAgentStreamEvent as Event;
        match event {
            Event::NodeStreamUpdate(event) => self.emit(tx, event, Event::NodeStreamUpdate).await,
            Event::NodeResult(event) => self.emit(tx, event, Event::NodeResult).await,
            Event::NodeCancel(event) => self.emit(tx, event, Event::NodeCancel).await,
            Event::BeforeNodeCall(event) => self.emit(tx, event, Event::BeforeNodeCall).await,
            Event::AfterNodeCall(event) => self.emit(tx, event, Event::AfterNodeCall).await,
            Event::MultiAgentHandoff(event) => self.emit(tx, event, Event::MultiAgentHandoff).await,
            Event::MultiAgentResult(event) => self.emit(tx, event, Event::MultiAgentResult).await,
            Event::BeforeMultiAgentInvocation(event) => {
                self.emit(tx, event, Event::BeforeMultiAgentInvocation).await
            }
            Event::AfterMultiAgentInvocation(event) => {
                self.emit(tx, event, Event::AfterMultiAgentInvocation).await
            }
        }
    }

    fn pending_interrupt_state(&self) -> std::sync::MutexGuard<'_, Option<MultiAgentStateHandle>> {
        self.0
            .pending_interrupt_state
            .lock()
            .expect("pending interrupt state mutex poisoned")
    }

    fn budget_spent(&self, run: &Invocation) -> bool {
        run.deadline.is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn timeout_error(&self, node_id: Option<&str>) -> Error {
        let timeout = self.0.config.timeout.unwrap_or_default().as_millis();
        match node_id {
            Some(node_id) => Error::other(format!(
                "timeout=<{timeout}>, swarm_id=<{}>, node_id=<{node_id}> | swarm exceeded wall-clock budget during node execution",
                self.0.id
            )),
            None => Error::other(format!(
                "timeout=<{timeout}>, swarm_id=<{}> | swarm exceeded wall-clock budget",
                self.0.id
            )),
        }
    }

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

/// The input for the next node after a handoff: the handoff message, plus its context as a
/// pretty-printed JSON text block. Without a handoff (initial or resume run) the input passes
/// through untouched, so resume responses reach the child agent directly.
fn resolve_node_input(input: MultiAgentInput, handoff: Option<&HandoffResult>) -> MultiAgentInput {
    let Some(handoff) = handoff else {
        return input;
    };
    let mut blocks = vec![ContentBlock::text(handoff.message.clone())];
    if let Some(context) = &handoff.context {
        let context = serde_json::to_string_pretty(context).unwrap_or_default();
        blocks.push(ContentBlock::text(format!("Context:\n{context}")));
    }
    MultiAgentInput::Blocks(blocks)
}

/// The deadline `timeout` from now, or `None` for no deadline. A timeout that overflows the clock
/// — [`Duration::MAX`], the documented per-node opt-out — is one with no deadline.
fn deadline_after(timeout: Option<Duration>) -> Option<Instant> {
    timeout.and_then(|timeout| Instant::now().checked_add(timeout))
}

/// Never completes when there is no deadline, so an unbounded run never wakes up for one.
async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Never completes when the invocation has no external cancellation.
async fn wait_cancelled(cancel: Option<&CancellationToken>) {
    match cancel {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending().await,
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
        input: MultiAgentInput,
        options: MultiAgentInvokeOptions,
    ) -> Result<MultiAgentResult> {
        Swarm::invoke_with(self, input, options).await
    }

    fn stream_with(&self, input: MultiAgentInput, options: MultiAgentInvokeOptions) -> MultiAgentStream {
        Swarm::stream_with(self, input, options)
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
            pending_interrupt_state: Mutex::new(None),
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::StreamExt;

    use crate::agent::Agent;
    use crate::multiagent::{AgentNodeOptions, NodeConfig, NodeResultUpdate, NodeType};
    use crate::testing::{MockMessageModel, MockResponse};
    use crate::types::{Message, Role, StopReason, ToolUseBlock};

    fn agent(id: &str, description: Option<&str>) -> Agent {
        let mut builder = Agent::builder().model(MockMessageModel::new(vec![])).id(id);
        if let Some(description) = description {
            builder = builder.description(description);
        }
        builder.build().expect("agent builds")
    }

    /// One `strands_structured_output` call carrying `handoff`, as a swarm agent's model reply.
    fn handoff_turn(handoff: JsonValue) -> MockResponse {
        MockResponse::with_stop_reason(
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock::new(
                    "strands_structured_output",
                    "tool-1",
                    handoff,
                ))],
            ),
            StopReason::ToolUse,
        )
    }

    /// Agent that decides `handoff` every turn.
    fn handoff_agent(id: &str, handoff: JsonValue) -> Agent {
        handoff_sequence_agent(id, vec![handoff])
    }

    /// Agent that ends the swarm with `message` as the final response.
    fn final_agent(id: &str, message: &str) -> Agent {
        handoff_agent(id, json!({ "message": message }))
    }

    /// Agent that walks a finite sequence of handoff decisions.
    fn handoff_sequence_agent(id: &str, handoffs: Vec<JsonValue>) -> Agent {
        Agent::builder()
            .model(MockMessageModel::new(
                handoffs.into_iter().map(handoff_turn).collect(),
            ))
            .id(id)
            .description(format!("Agent {id}"))
            .build()
            .expect("agent builds")
    }

    /// Agent whose model takes `delay` to answer and honours cancellation while it waits.
    fn slow_agent(id: &str, delay: Duration) -> Agent {
        slow_agent_handoff(id, delay, json!({ "message": "slow but done" }))
    }

    fn slow_agent_handoff(id: &str, delay: Duration, handoff: JsonValue) -> Agent {
        Agent::builder()
            .model(SlowModel {
                inner: MockMessageModel::new(vec![handoff_turn(handoff)]),
                delay,
                per_event: false,
            })
            .id(id)
            .build()
            .expect("agent builds")
    }

    /// Agent whose model pauses `delay` before *every* stream event, so the node keeps emitting
    /// events for longer than a short node timeout.
    fn chatty_agent(id: &str, delay: Duration) -> Agent {
        Agent::builder()
            .model(SlowModel {
                inner: MockMessageModel::new(vec![handoff_turn(json!({ "message": "done" }))]),
                delay,
                per_event: true,
            })
            .id(id)
            .build()
            .expect("agent builds")
    }

    /// Wraps a mock model with a delay, standing in for TS's `createCancellableAgent`.
    struct SlowModel {
        inner: MockMessageModel,
        delay: Duration,
        /// Pause before every event instead of once before the first.
        per_event: bool,
    }

    #[async_trait]
    impl crate::models::Model for SlowModel {
        fn model_id(&self) -> Option<String> {
            self.inner.model_id()
        }

        fn stream<'a>(
            &'a self,
            messages: &'a [Message],
            options: &'a crate::models::StreamOptions,
        ) -> futures::stream::BoxStream<'a, Result<crate::models::ModelStreamEvent>> {
            Box::pin(async_stream::stream! {
                if !self.per_event {
                    tokio::time::sleep(self.delay).await;
                }
                let mut inner = self.inner.stream(messages, options);
                while let Some(event) = inner.next().await {
                    if self.per_event {
                        tokio::time::sleep(self.delay).await;
                    }
                    yield event;
                }
            })
        }
    }

    /// Stands in for the TS `SessionManager` plugin, which is not ported yet: saves the
    /// orchestration state after every run and restores it into the next one. Like TS, it restores
    /// at most once per orchestrator (`SessionManager._multiAgentRestoredIds`).
    #[derive(Default)]
    struct Session {
        saved: Mutex<Option<Snapshot>>,
    }

    impl Session {
        fn shared() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// Registers the save/restore hooks a session manager would register. Each call is one
        /// manager instance over the same storage, so each restores at most once.
        fn install(self: &Arc<Self>, builder: SwarmBuilder) -> SwarmBuilder {
            let restore = self.clone();
            let save = self.clone();
            let restored = Arc::new(Mutex::new(false));
            builder
                .hook::<BeforeMultiAgentInvocationEvent, _>(move |event| {
                    let mut restored = restored.lock().expect("not poisoned");
                    if *restored {
                        return Ok(());
                    }
                    *restored = true;
                    let saved = restore.saved.lock().expect("not poisoned").clone();
                    if let Some(snapshot) = saved {
                        load_snapshot(event.orchestrator.as_ref(), &snapshot, Some(&event.state))?;
                    }
                    Ok(())
                })
                .hook::<AfterMultiAgentInvocationEvent, _>(move |event| {
                    let snapshot = take_snapshot(
                        event.orchestrator.as_ref(),
                        Some(&event.state),
                        TakeMultiAgentSnapshotOptions::default(),
                    )?;
                    *save.saved.lock().expect("not poisoned") = Some(snapshot);
                    Ok(())
                })
        }
    }

    /// Tool that interrupts for approval the first time and returns the answer on resume.
    fn approve_tool() -> Arc<crate::tools::FunctionTool> {
        Arc::new(
            crate::tools::FunctionTool::builder("confirm_tool")
                .description("Interrupts for approval")
                .input_schema(json!({ "type": "object", "properties": {} }))
                .handler(|_input, ctx| async move {
                    let answer: String =
                        ctx.interrupt(crate::types::InterruptParams::new("confirm").with_reason("approve?"))?;
                    Ok(json!(format!("approved: {answer}")))
                }),
        )
    }

    /// Assistant turn calling `confirm_tool`, which interrupts.
    fn interrupting_turn() -> MockResponse {
        MockResponse::with_stop_reason(
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock::new(
                    "confirm_tool",
                    "tool-int",
                    json!({}),
                ))],
            ),
            StopReason::ToolUse,
        )
    }

    /// Node that writes an event the built-in nodes never emit, to prove the swarm dispatches its
    /// hooks for everything a node forwards.
    struct CancelEmittingNode {
        id: String,
        config: NodeConfig,
    }

    #[async_trait]
    impl Node for CancelEmittingNode {
        fn id(&self) -> &str {
            &self.id
        }

        fn node_type(&self) -> NodeType {
            NodeType::from("custom")
        }

        fn config(&self) -> &NodeConfig {
            &self.config
        }

        async fn handle(
            &self,
            _input: MultiAgentInput,
            state: &MultiAgentStateHandle,
            options: &NodeInputOptions,
            events: &NodeSink,
        ) -> Result<NodeResultUpdate> {
            let _ = events
                .send(Ok(MultiAgentStreamEvent::NodeCancel(NodeCancelEvent {
                    node_id: self.id.clone(),
                    state: state.clone(),
                    message: "from the node".to_owned(),
                    invocation_state: options.invocation_state.clone(),
                })))
                .await;
            Ok(NodeResultUpdate::default().with_content(vec![ContentBlock::text("done")]))
        }
    }

    fn node_ids(result: &MultiAgentResult) -> Vec<&str> {
        result.results.iter().map(|r| r.node_id.as_str()).collect()
    }

    /// Drains a swarm stream into its event type names plus the terminal result.
    async fn collect(stream: MultiAgentStream) -> (Vec<String>, Option<MultiAgentResult>) {
        let mut names = Vec::new();
        let mut result = None;
        let mut stream = stream;
        while let Some(event) = stream.next().await {
            let event = event.expect("stream event");
            names.push(event.type_name().to_owned());
            if let Some(aggregate) = event.result() {
                result = Some(aggregate.clone());
            }
        }
        (names, result)
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

    // ---- execution loop (`swarm.test.ts` `invoke` / `stream`) ----------------

    #[tokio::test]
    async fn returns_a_completed_result_with_content_and_duration() {
        let swarm = Swarm::builder()
            .node(final_agent("a", "final answer"))
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("hello").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(result.content, vec![ContentBlock::text("final answer")]);
        assert_eq!(node_ids(&result), ["a"]);
        assert_eq!(
            result.results[0].structured_output,
            Some(json!({ "message": "final answer" }))
        );
    }

    #[tokio::test]
    async fn hands_off_to_the_target_agent() {
        let swarm = Swarm::builder()
            .node(handoff_agent(
                "a",
                json!({ "agentId": "b", "message": "please handle this" }),
            ))
            .node(final_agent("b", "done by b"))
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a", "b"]);
        assert_eq!(result.content, vec![ContentBlock::text("done by b")]);
    }

    #[tokio::test]
    async fn chains_handoffs_across_agents() {
        let swarm = Swarm::builder()
            .node(handoff_agent(
                "a",
                json!({ "agentId": "b", "message": "go to b" }),
            ))
            .node(handoff_agent(
                "b",
                json!({ "agentId": "c", "message": "go to c" }),
            ))
            .node(final_agent("c", "final from c"))
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn passes_the_handoff_context_as_a_pretty_json_text_block() {
        let model = MockMessageModel::shared(vec![handoff_turn(json!({ "message": "done" }))]);
        let swarm = Swarm::builder()
            .node(handoff_agent(
                "a",
                json!({ "agentId": "b", "message": "handle this", "context": { "key": "value" } }),
            ))
            .node(
                Agent::builder()
                    .model(model.clone())
                    .id("b")
                    .build()
                    .expect("agent builds"),
            )
            .build()
            .expect("swarm builds");

        swarm.invoke("start").await.expect("swarm runs");

        let (messages, _) = model.calls().remove(0);
        let input = messages[0].text();
        assert!(input.contains("handle this"), "{input}");
        assert!(input.contains("Context:\n{\n  \"key\": \"value\"\n}"), "{input}");
    }

    #[tokio::test]
    async fn hands_each_agent_a_schema_that_excludes_itself() {
        let model_a = MockMessageModel::shared(vec![handoff_turn(json!({ "message": "from a" }))]);
        let swarm = Swarm::builder()
            .node(
                Agent::builder()
                    .model(model_a.clone())
                    .id("a")
                    .build()
                    .expect("agent builds"),
            )
            .node(final_agent("b", "done"))
            .build()
            .expect("swarm builds");

        swarm.invoke("start").await.expect("swarm runs");

        let (_, options) = model_a.calls().remove(0);
        let spec = options
            .tool_specs
            .iter()
            .find(|spec| spec.name == "strands_structured_output")
            .expect("structured output tool offered");
        let schema = spec.input_schema.as_ref().expect("schema present");
        assert_eq!(schema["properties"]["agentId"]["enum"], json!(["b"]));
    }

    #[tokio::test]
    async fn errors_when_the_step_limit_is_exceeded() {
        let swarm = Swarm::builder()
            .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
            .node(final_agent("b", "done"))
            .max_steps(1)
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("start").await.expect_err("step limit");
        assert_eq!(error.to_string(), "max_steps=<1> | swarm reached step limit");
    }

    #[tokio::test]
    async fn completes_when_it_uses_exactly_the_step_budget() {
        let swarm = Swarm::builder()
            .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
            .node(final_agent("b", "done by b"))
            .max_steps(2)
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a", "b"]);
    }

    #[tokio::test]
    async fn fails_the_aggregate_when_recent_handoffs_repeat() {
        let swarm = Swarm::builder()
            .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
            .node(handoff_agent("b", json!({ "agentId": "a", "message": "to a" })))
            .node(final_agent("c", "unused"))
            .repetitive_handoff_detection_window(4)
            .repetitive_handoff_min_unique_agents(3)
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Failed);
        assert_eq!(
            result.error.as_deref(),
            Some("Repetitive handoff: 2 unique nodes out of 4 recent iterations")
        );
        assert_eq!(node_ids(&result), ["a", "b", "a", "b"]);
        // Detection is an aggregate failure; each node execution completed.
        assert!(result
            .results
            .iter()
            .all(|result| result.status == ResultStatus::Completed));
    }

    #[tokio::test]
    async fn checks_the_trailing_window_after_earlier_diverse_handoffs() {
        let swarm = Swarm::builder()
            .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
            .node(handoff_sequence_agent(
                "b",
                vec![
                    json!({ "agentId": "c", "message": "to c" }),
                    json!({ "agentId": "a", "message": "to a" }),
                ],
            ))
            .node(handoff_agent("c", json!({ "agentId": "a", "message": "to a" })))
            .repetitive_handoff_detection_window(3)
            .repetitive_handoff_min_unique_agents(3)
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Failed);
        assert_eq!(
            result.error.as_deref(),
            Some("Repetitive handoff: 2 unique nodes out of 3 recent iterations")
        );
        assert_eq!(node_ids(&result), ["a", "b", "c", "a", "b", "a"]);
    }

    #[tokio::test]
    async fn continues_when_the_window_has_enough_unique_nodes() {
        let swarm = Swarm::builder()
            .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
            .node(handoff_agent("b", json!({ "agentId": "c", "message": "to c" })))
            .node(final_agent("c", "done"))
            .repetitive_handoff_detection_window(2)
            .repetitive_handoff_min_unique_agents(2)
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn errors_when_a_node_exceeds_the_node_timeout() {
        let swarm = Swarm::builder()
            .node(slow_agent("slow", Duration::from_millis(400)))
            .node_timeout(Duration::from_millis(20))
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("go").await.expect_err("node timeout");
        assert_eq!(
            error.to_string(),
            "node_timeout=<20>, node_id=<slow>, swarm_id=<swarm> | node exceeded wall-clock budget"
        );
    }

    #[tokio::test]
    async fn a_streaming_node_does_not_refresh_its_own_timeout() {
        let swarm = Swarm::builder()
            .node(chatty_agent("slow", Duration::from_millis(8)))
            .node_timeout(Duration::from_millis(20))
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("go").await.expect_err("node timeout");
        assert!(
            error.to_string().starts_with("node_timeout=<20>, node_id=<slow>"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn prefers_a_nodes_own_timeout_over_the_swarm_default() {
        let swarm = Swarm::builder()
            .node(
                AgentNodeOptions::new(slow_agent("slow", Duration::from_millis(400)))
                    .with_timeout(Duration::from_millis(15)),
            )
            .node_timeout(Duration::from_secs(10))
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("go").await.expect_err("node timeout");
        assert!(
            error.to_string().starts_with("node_timeout=<15>, node_id=<slow>"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_node_timeout_of_duration_max_opts_out_of_the_swarm_default() {
        let swarm = Swarm::builder()
            .node(
                AgentNodeOptions::new(slow_agent("slow", Duration::from_millis(30)))
                    .with_timeout(Duration::MAX),
            )
            .node_timeout(Duration::from_millis(10))
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("go").await.expect("swarm runs");
        assert_eq!(result.status, ResultStatus::Completed);
    }

    #[tokio::test]
    async fn a_swarm_timeout_of_duration_max_is_unbounded() {
        let swarm = Swarm::builder()
            .node(final_agent("a", "done"))
            .timeout(Duration::MAX)
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("go").await.expect("swarm runs");
        assert_eq!(result.status, ResultStatus::Completed);
    }

    #[tokio::test]
    async fn runs_without_a_node_timeout() {
        let swarm = Swarm::builder()
            .node(slow_agent("a", Duration::from_millis(20)))
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("go").await.expect("swarm runs");
        assert_eq!(result.status, ResultStatus::Completed);
    }

    #[tokio::test]
    async fn errors_when_the_swarm_budget_runs_out_between_steps() {
        let swarm = Swarm::builder()
            .node(slow_agent_handoff(
                "a",
                Duration::from_millis(40),
                json!({ "agentId": "b", "message": "to b" }),
            ))
            .node(slow_agent("b", Duration::from_millis(40)))
            .timeout(Duration::from_millis(20))
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("go").await.expect_err("swarm timeout");
        assert_eq!(
            error.to_string(),
            "timeout=<20>, swarm_id=<swarm>, node_id=<a> | swarm exceeded wall-clock budget during node execution"
        );
    }

    #[tokio::test]
    async fn aborts_an_in_flight_node_when_the_swarm_budget_runs_out() {
        let swarm = Swarm::builder()
            .node(slow_agent("slow", Duration::from_secs(30)))
            .timeout(Duration::from_millis(20))
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("go").await.expect_err("swarm timeout");
        assert!(
            error.to_string().starts_with("timeout=<20>, swarm_id=<swarm>"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn arms_the_swarm_budget_after_the_before_invocation_hook() {
        let swarm = Swarm::builder()
            .node(final_agent("a", "done"))
            .timeout(Duration::from_millis(500))
            .build()
            .expect("swarm builds");
        // Longer than the budget: under TS's ordering the node still gets the full 500 ms.
        swarm
            .hooks()
            .add_callback_boxed::<BeforeMultiAgentInvocationEvent, _>(|_event| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    Ok(())
                })
            });

        let result = swarm.invoke("go").await.expect("swarm runs");
        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a"]);
    }

    #[tokio::test]
    async fn stops_at_the_loop_top_when_cancelled_externally() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let swarm = Swarm::builder()
            .node(final_agent("a", "hi"))
            .build()
            .expect("swarm builds");

        let error = swarm
            .invoke_with("go", MultiAgentInvokeOptions::default().with_cancel(cancel))
            .await
            .expect_err("cancelled");
        assert_eq!(
            error.to_string(),
            "swarm_id=<swarm> | swarm cancelled by external signal"
        );
    }

    #[tokio::test]
    async fn propagates_a_panicking_hook_out_of_invoke() {
        let task = tokio::spawn(async {
            let swarm = Swarm::builder()
                .node(final_agent("a", "done"))
                .hook::<BeforeMultiAgentInvocationEvent, _>(|_event| panic!("hook exploded"))
                .build()
                .expect("swarm builds");
            swarm.invoke("go").await
        });

        let error = task.await.expect_err("the panic reaches the caller");
        assert!(error.is_panic(), "{error}");
    }

    #[tokio::test]
    async fn fails_the_result_when_an_agent_errors() {
        let agent = Agent::builder()
            .model(MockMessageModel::new(vec![MockResponse::Error(Error::other(
                "agent exploded",
            ))]))
            .id("a")
            .build()
            .expect("agent builds");
        let swarm = Swarm::builder().node(agent).build().expect("swarm builds");

        let result = swarm.invoke("go").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Failed);
        assert_eq!(node_ids(&result), ["a"]);
        assert_eq!(result.results[0].status, ResultStatus::Failed);
    }

    #[tokio::test]
    async fn initializes_once_across_invocations() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let swarm = Swarm::builder()
            .node(handoff_sequence_agent(
                "a",
                vec![json!({ "message": "one" }), json!({ "message": "two" })],
            ))
            .hook::<MultiAgentInitializedEvent, _>(move |_event| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .build()
            .expect("swarm builds");

        swarm.invoke("first").await.expect("swarm runs");
        swarm.invoke("second").await.expect("swarm runs");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn leaves_the_agent_observably_unchanged() {
        let agent = final_agent("a", "reply");
        let before = agent.messages();
        let swarm = Swarm::builder()
            .node(agent.clone())
            .build()
            .expect("swarm builds");

        swarm.invoke("hello").await.expect("swarm runs");

        assert_eq!(agent.messages(), before);
    }

    #[tokio::test]
    async fn keeps_a_plugin_initialization_failure_fail_closed() {
        struct FailingPlugin;

        #[async_trait]
        impl MultiAgentPlugin for FailingPlugin {
            fn name(&self) -> &str {
                "failing-plugin"
            }

            async fn init_multi_agent(&self, _orchestrator: &OrchestratorRef) -> Result<()> {
                Err(Error::other("plugin initialization failed"))
            }
        }

        let swarm = Swarm::builder()
            .node(final_agent("a", "hi"))
            .plugin(Arc::new(FailingPlugin))
            .build()
            .expect("swarm builds");

        for _ in 0..2 {
            let error = swarm.initialize().await.expect_err("plugin fails");
            assert!(
                error.to_string().contains("plugin initialization failed"),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn streams_the_lifecycle_events_in_order() {
        let swarm = Swarm::builder()
            .node(final_agent("a", "reply"))
            .build()
            .expect("swarm builds");

        let (events, result) = collect(swarm.stream("go")).await;
        let result = result.expect("terminal result");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a"]);
        let types: Vec<&str> = events
            .iter()
            .map(String::as_str)
            .filter(|name| *name != "nodeStreamUpdateEvent")
            .collect();
        assert_eq!(
            types,
            [
                "beforeMultiAgentInvocationEvent",
                "beforeNodeCallEvent",
                "nodeResultEvent",
                "afterNodeCallEvent",
                "afterMultiAgentInvocationEvent",
                "multiAgentResultEvent",
            ]
        );
        // The agent's own events arrive between the node's before and result events.
        let updates = events.iter().position(|name| name == "nodeStreamUpdateEvent");
        assert!(updates.is_some_and(|at| at > 1), "{events:?}");
    }

    #[tokio::test]
    async fn streams_a_handoff_event_between_agents() {
        let handoffs = Arc::new(Mutex::new(Vec::new()));
        let recorded = handoffs.clone();
        let swarm = Swarm::builder()
            .node(handoff_agent("a", json!({ "agentId": "b", "message": "go" })))
            .node(final_agent("b", "done"))
            .hook::<MultiAgentHandoffEvent, _>(move |event| {
                recorded
                    .lock()
                    .expect("not poisoned")
                    .push((event.source.clone(), event.targets.clone()));
                Ok(())
            })
            .build()
            .expect("swarm builds");

        let (events, _) = collect(swarm.stream("start")).await;

        assert_eq!(
            events
                .iter()
                .filter(|name| *name == "multiAgentHandoffEvent")
                .count(),
            1
        );
        assert_eq!(
            *handoffs.lock().expect("not poisoned"),
            [("a".to_owned(), vec!["b".to_owned()])]
        );
    }

    #[tokio::test]
    async fn cancels_a_node_from_a_hook_with_the_default_message() {
        let swarm = Swarm::builder()
            .node(final_agent("a", "hi"))
            .hook::<BeforeNodeCallEvent, _>(|event| {
                event.cancel = Cancel::Yes;
                Ok(())
            })
            .build()
            .expect("swarm builds");

        let messages = Arc::new(Mutex::new(Vec::new()));
        let recorded = messages.clone();
        swarm
            .hooks()
            .add_sync_callback::<NodeCancelEvent, _>(move |event| {
                recorded
                    .lock()
                    .expect("not poisoned")
                    .push((event.node_id.clone(), event.message.clone()));
                Ok(())
            });

        let (events, result) = collect(swarm.stream("go")).await;
        let result = result.expect("terminal result");

        assert_eq!(result.status, ResultStatus::Cancelled);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].status, ResultStatus::Cancelled);
        assert_eq!(result.results[0].duration, 0);
        assert!(events.contains(&"nodeCancelEvent".to_owned()), "{events:?}");
        assert_eq!(
            *messages.lock().expect("not poisoned"),
            [("a".to_owned(), "node cancelled by hook".to_owned())]
        );
    }

    #[tokio::test]
    async fn cancels_a_node_from_a_hook_with_a_custom_message() {
        let swarm = Swarm::builder()
            .node(final_agent("a", "hi"))
            .hook::<BeforeNodeCallEvent, _>(|event| {
                event.cancel = Cancel::from("agent not ready");
                Ok(())
            })
            .build()
            .expect("swarm builds");

        let messages = Arc::new(Mutex::new(Vec::new()));
        let recorded = messages.clone();
        swarm
            .hooks()
            .add_sync_callback::<NodeCancelEvent, _>(move |event| {
                recorded.lock().expect("not poisoned").push(event.message.clone());
                Ok(())
            });

        let result = swarm.invoke("go").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Cancelled);
        assert_eq!(
            *messages.lock().expect("not poisoned"),
            ["agent not ready".to_owned()]
        );
    }

    // ---- resume from restored state (`swarm.test.ts` "resume with session manager") ---------

    #[tokio::test]
    async fn resumes_from_the_pending_handoff_target_after_a_crash() {
        let session = Session::shared();
        let resume_swarm = |max_steps: Option<u64>| {
            let mut builder = Swarm::builder()
                .id("my-swarm")
                .node(handoff_agent(
                    "a",
                    json!({ "agentId": "b", "message": "go to b" }),
                ))
                .node(final_agent("b", "done by b"));
            if let Some(max_steps) = max_steps {
                builder = builder.max_steps(max_steps);
            }
            session.install(builder).build().expect("swarm builds")
        };

        let error = resume_swarm(Some(1))
            .invoke("start")
            .await
            .expect_err("step limit");
        assert_eq!(error.to_string(), "max_steps=<1> | swarm reached step limit");

        let result = resume_swarm(None).invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a", "b"]);
    }

    #[tokio::test]
    async fn carries_forward_the_step_count_from_the_previous_invocation() {
        let session = Session::shared();
        let resume_swarm = |max_steps: u64| {
            session
                .install(
                    Swarm::builder()
                        .id("my-swarm")
                        .node(handoff_agent(
                            "a",
                            json!({ "agentId": "b", "message": "go to b" }),
                        ))
                        .node(final_agent("b", "done by b"))
                        .max_steps(max_steps),
                )
                .build()
                .expect("swarm builds")
        };

        resume_swarm(1).invoke("start").await.expect_err("step limit");
        let result = resume_swarm(2).invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a", "b"]);
        assert_eq!(result.results.len(), 2, "the restored step count is spent");
    }

    #[tokio::test]
    async fn restores_only_once_so_a_completed_run_starts_fresh() {
        let session = Session::shared();
        let swarm = session
            .install(
                Swarm::builder()
                    .id("my-swarm")
                    .node(handoff_sequence_agent(
                        "a",
                        vec![json!({ "message": "all done" }), json!({ "message": "again" })],
                    ))
                    .node(final_agent("b", "done by b")),
            )
            .build()
            .expect("swarm builds");

        let first = swarm.invoke("start").await.expect("swarm runs");
        assert_eq!(node_ids(&first), ["a"]);

        let second = swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(second.status, ResultStatus::Completed);
        assert_eq!(node_ids(&second), ["a"]);
    }

    #[tokio::test]
    async fn passes_the_last_handoff_context_to_the_resumed_node() {
        let session = Session::shared();
        let handoff = json!({
            "agentId": "b",
            "message": "write this up",
            "context": { "research": "quantum computing basics" },
        });

        let crashed = session
            .install(
                Swarm::builder()
                    .id("my-swarm")
                    .node(handoff_agent("a", handoff.clone()))
                    .node(final_agent("b", "done"))
                    .max_steps(1),
            )
            .build()
            .expect("swarm builds");
        crashed.invoke("start").await.expect_err("step limit");

        let model_b = MockMessageModel::shared(vec![handoff_turn(json!({ "message": "done" }))]);
        let resumed = session
            .install(
                Swarm::builder()
                    .id("my-swarm")
                    .node(handoff_agent("a", handoff))
                    .node(
                        Agent::builder()
                            .model(model_b.clone())
                            .id("b")
                            .build()
                            .expect("agent builds"),
                    ),
            )
            .build()
            .expect("swarm builds");
        resumed.invoke("start").await.expect("swarm runs");

        let (messages, _) = model_b.calls().remove(0);
        let input = messages[0].text();
        assert!(input.contains("write this up"), "{input}");
        assert!(input.contains("quantum computing basics"), "{input}");
    }

    #[tokio::test]
    async fn starts_fresh_when_the_resume_target_left_the_swarm() {
        let session = Session::shared();
        let crashed = session
            .install(
                Swarm::builder()
                    .id("my-swarm")
                    .node(handoff_agent(
                        "a",
                        json!({ "agentId": "b", "message": "go to b" }),
                    ))
                    .node(final_agent("b", "done by b"))
                    .max_steps(1),
            )
            .build()
            .expect("swarm builds");
        crashed.invoke("start").await.expect_err("step limit");

        // Same session, but b is gone and c is new.
        let reconfigured = session
            .install(
                Swarm::builder()
                    .id("my-swarm")
                    .node(final_agent("a", "fresh start"))
                    .node(final_agent("c", "done by c")),
            )
            .build()
            .expect("swarm builds");

        let result = reconfigured.invoke("start").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(node_ids(&result), ["a", "a"]);
    }

    #[tokio::test]
    async fn scopes_repetitive_handoff_detection_to_the_current_invocation() {
        let session = Session::shared();
        let prior = session
            .install(
                Swarm::builder()
                    .id("my-swarm")
                    .node(handoff_sequence_agent(
                        "a",
                        vec![
                            json!({ "agentId": "b", "message": "to b" }),
                            json!({ "agentId": "b", "message": "to b again" }),
                        ],
                    ))
                    .node(handoff_sequence_agent(
                        "b",
                        vec![
                            json!({ "agentId": "a", "message": "to a" }),
                            json!({ "message": "prior run complete" }),
                        ],
                    ))
                    .node(final_agent("c", "unused")),
            )
            .build()
            .expect("swarm builds");
        let prior_result = prior.invoke("start").await.expect("swarm runs");
        assert_eq!(node_ids(&prior_result), ["a", "b", "a", "b"]);

        let resumed = session
            .install(
                Swarm::builder()
                    .id("my-swarm")
                    .node(final_agent("a", "a"))
                    .node(final_agent("b", "b"))
                    .node(final_agent("c", "fresh result"))
                    .entry_point("c")
                    .repetitive_handoff_detection_window(4)
                    .repetitive_handoff_min_unique_agents(3),
            )
            .build()
            .expect("swarm builds");

        let result = resumed.invoke("new request").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(result.error, None);
        assert_eq!(node_ids(&result), ["a", "b", "a", "b", "c"]);
    }

    // ---- invocation state (`swarm.invocation-state.test.ts`) --------------------------------

    #[tokio::test]
    async fn forwards_one_invocation_state_to_every_node() {
        let agent_a = handoff_agent("a", json!({ "agentId": "b", "message": "to b" }));
        agent_a
            .hooks()
            .add_sync_callback::<crate::hooks::BeforeModelCallEvent, _>(|event| {
                event.invocation_state.insert("touched_by_a", true);
                Ok(())
            });

        let observed = Arc::new(Mutex::new(Vec::new()));
        let recorded = observed.clone();
        let agent_b = final_agent("b", "done");
        agent_b
            .hooks()
            .add_sync_callback::<crate::hooks::BeforeModelCallEvent, _>(move |event| {
                recorded.lock().expect("not poisoned").push((
                    event.invocation_state.clone(),
                    event.invocation_state.contains("touched_by_a"),
                ));
                Ok(())
            });

        let swarm = Swarm::builder()
            .node(agent_a)
            .node(agent_b)
            .build()
            .expect("swarm builds");

        let state = InvocationState::new();
        state.insert("request_id", "r-1".to_owned());
        swarm
            .invoke_with(
                "hello",
                MultiAgentInvokeOptions::default().with_invocation_state(state.clone()),
            )
            .await
            .expect("swarm runs");

        let observed = observed.lock().expect("not poisoned");
        let (seen, saw_mutation) = observed.first().expect("node b ran");
        // Every node shares the caller's state, so b sees a's write.
        assert!(seen.ptr_eq(&state));
        assert!(*saw_mutation);
        assert!(*state.get::<bool>("touched_by_a").expect("flag"));
    }

    #[tokio::test]
    async fn defaults_the_invocation_state_to_an_empty_map() {
        let observed = Arc::new(Mutex::new(None));
        let recorded = observed.clone();
        let agent = final_agent("a", "done");
        agent
            .hooks()
            .add_sync_callback::<crate::hooks::BeforeModelCallEvent, _>(move |event| {
                *recorded.lock().expect("not poisoned") = Some(event.invocation_state.clone());
                Ok(())
            });
        let swarm = Swarm::builder().node(agent).build().expect("swarm builds");

        swarm.invoke("hello").await.expect("swarm runs");

        let observed = observed.lock().expect("not poisoned");
        assert_eq!(observed.as_ref().expect("node ran").keys(), Vec::<String>::new());
    }

    // ---- interrupts (`interrupts.test.ts`, swarm half) --------------------------------------

    #[tokio::test]
    async fn interrupts_inside_an_agent_and_resumes_from_a_restored_session() {
        let session = Session::shared();
        let interrupted_swarm = session
            .install(
                Swarm::builder().id("my-swarm").node(
                    Agent::builder()
                        .model(MockMessageModel::new(vec![interrupting_turn()]))
                        .id("a")
                        .tool(approve_tool())
                        .build()
                        .expect("agent builds"),
                ),
            )
            .build()
            .expect("swarm builds");

        let interrupted = interrupted_swarm.invoke("start").await.expect("swarm runs");
        assert_eq!(interrupted.status, ResultStatus::Interrupted);
        assert_eq!(interrupted.interrupts.len(), 1);

        // A fresh instance resumes from the session, like a cross-process resume. The swarm routes
        // handoffs through structured output, so the final turn ends the run.
        let resumed_swarm = session
            .install(
                Swarm::builder().id("my-swarm").node(
                    Agent::builder()
                        .model(MockMessageModel::new(vec![handoff_turn(
                            json!({ "message": "all done" }),
                        )]))
                        .id("a")
                        .tool(approve_tool())
                        .build()
                        .expect("agent builds"),
                ),
            )
            .build()
            .expect("swarm builds");

        let result = resumed_swarm
            .invoke(vec![InterruptResponseContent::new(
                &interrupted.interrupts[0].id,
                json!("ok"),
            )])
            .await
            .expect("swarm resumes");

        assert_eq!(result.status, ResultStatus::Completed);
        assert!(result.interrupts.is_empty());
    }

    #[tokio::test]
    async fn resumes_an_interrupted_run_on_the_same_instance_without_a_session() {
        let swarm = Swarm::builder()
            .node(
                Agent::builder()
                    .model(MockMessageModel::new(vec![
                        interrupting_turn(),
                        handoff_turn(json!({ "message": "all done" })),
                    ]))
                    .id("a")
                    .tool(approve_tool())
                    .build()
                    .expect("agent builds"),
            )
            .build()
            .expect("swarm builds");

        let interrupted = swarm.invoke("start").await.expect("swarm runs");
        assert_eq!(interrupted.status, ResultStatus::Interrupted);

        let result = swarm
            .invoke(vec![InterruptResponseContent::new(
                &interrupted.interrupts[0].id,
                json!("ok"),
            )])
            .await
            .expect("swarm resumes");

        assert_eq!(result.status, ResultStatus::Completed);
        // The stale INTERRUPTED result is replaced, not appended to.
        assert_eq!(node_ids(&result), ["a"]);
    }

    #[tokio::test]
    async fn repetitive_handoff_detection_does_not_swallow_a_restored_interrupt_response() {
        let session = Session::shared();
        let detection = |builder: SwarmBuilder| {
            builder
                .id("my-swarm")
                .entry_point("a")
                .repetitive_handoff_detection_window(4)
                .repetitive_handoff_min_unique_agents(3)
        };

        let interrupted_swarm = session
            .install(detection(
                Swarm::builder()
                    .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
                    .node(
                        Agent::builder()
                            .model(MockMessageModel::new(vec![
                                handoff_turn(json!({ "agentId": "a", "message": "to a" })),
                                interrupting_turn(),
                            ]))
                            .id("b")
                            .tool(approve_tool())
                            .build()
                            .expect("agent builds"),
                    )
                    .node(final_agent("c", "unused")),
            ))
            .build()
            .expect("swarm builds");

        let interrupted = interrupted_swarm.invoke("start").await.expect("swarm runs");

        assert_eq!(interrupted.status, ResultStatus::Interrupted);
        assert_eq!(
            interrupted
                .results
                .iter()
                .map(|result| (result.node_id.as_str(), result.status))
                .collect::<Vec<_>>(),
            [
                ("a", ResultStatus::Completed),
                ("b", ResultStatus::Completed),
                ("a", ResultStatus::Completed),
                ("b", ResultStatus::Interrupted),
            ]
        );

        let resumed_swarm = session
            .install(detection(
                Swarm::builder()
                    .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
                    .node(
                        Agent::builder()
                            .model(MockMessageModel::new(vec![handoff_turn(
                                json!({ "message": "all done" }),
                            )]))
                            .id("b")
                            .tool(approve_tool())
                            .build()
                            .expect("agent builds"),
                    )
                    .node(final_agent("c", "unused")),
            ))
            .build()
            .expect("swarm builds");

        let result = resumed_swarm
            .invoke(vec![InterruptResponseContent::new(
                &interrupted.interrupts[0].id,
                json!("yes"),
            )])
            .await
            .expect("swarm resumes");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(result.error, None);
        assert!(result.interrupts.is_empty());
        assert_eq!(node_ids(&result), ["a", "b", "a", "b"]);
    }

    #[tokio::test]
    async fn gates_a_node_with_a_before_node_call_interrupt_and_resumes() {
        let session = Session::shared();
        let gated = |builder: SwarmBuilder| {
            builder.id("my-swarm").hook::<BeforeNodeCallEvent, _>(|event| {
                let _: JsonValue =
                    event.interrupt(crate::types::InterruptParams::new("gate").with_reason("approve?"))?;
                Ok(())
            })
        };

        let interrupted_swarm = session
            .install(gated(Swarm::builder().node(final_agent("a", "ran"))))
            .build()
            .expect("swarm builds");

        let interrupted = interrupted_swarm.invoke("begin").await.expect("swarm runs");
        assert_eq!(interrupted.status, ResultStatus::Interrupted);
        assert_eq!(
            interrupted.interrupts[0].source,
            crate::interrupt::InterruptSource::MultiagentHook
        );

        let resumed_swarm = session
            .install(gated(Swarm::builder().node(final_agent("a", "ran"))))
            .build()
            .expect("swarm builds");

        let result = resumed_swarm
            .invoke(vec![InterruptResponseContent::new(
                &interrupted.interrupts[0].id,
                json!("approved"),
            )])
            .await
            .expect("swarm resumes");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(result.content, vec![ContentBlock::text("ran")]);
    }

    #[tokio::test]
    async fn a_hook_gated_node_still_emits_its_result_and_after_events() {
        let swarm = Swarm::builder()
            .node(final_agent("a", "never runs"))
            .hook::<BeforeNodeCallEvent, _>(|event| {
                let _: JsonValue = event.interrupt(crate::types::InterruptParams::new("gate"))?;
                Ok(())
            })
            .build()
            .expect("swarm builds");

        let (events, result) = collect(swarm.stream("hi")).await;

        assert_eq!(result.expect("terminal result").status, ResultStatus::Interrupted);
        let position = |name: &str| events.iter().position(|event| event == name);
        assert!(position("beforeNodeCallEvent") < position("nodeResultEvent"));
        assert!(position("nodeResultEvent") < position("afterNodeCallEvent"));
        assert!(position("afterNodeCallEvent").is_some(), "{events:?}");
    }

    // ---- hook and stream fidelity (`swarm.ts` `_stream` / `_streamNode` branches) ------------

    #[tokio::test]
    async fn a_failing_node_event_hook_still_reports_on_the_after_node_event() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let recorded = errors.clone();
        let swarm = Swarm::builder()
            .node(final_agent("a", "done"))
            .hook::<NodeResultEvent, _>(|_event| Err(Error::other("hook exploded")))
            .hook::<AfterNodeCallEvent, _>(move |event| {
                recorded.lock().expect("not poisoned").push(event.error.clone());
                Ok(())
            })
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("go").await.expect_err("hook error propagates");

        assert_eq!(error.to_string(), "hook exploded");
        assert_eq!(
            *errors.lock().expect("not poisoned"),
            [Some("hook exploded".to_owned())]
        );
    }

    #[tokio::test]
    async fn an_unroutable_resume_fails_before_the_after_invocation_hook() {
        let fired = Arc::new(Mutex::new(Vec::new()));
        let before = fired.clone();
        let after = fired.clone();
        let swarm = Swarm::builder()
            .node(final_agent("a", "done"))
            .hook::<BeforeMultiAgentInvocationEvent, _>(move |_event| {
                before.lock().expect("not poisoned").push("before");
                Ok(())
            })
            .hook::<AfterMultiAgentInvocationEvent, _>(move |_event| {
                after.lock().expect("not poisoned").push("after");
                Ok(())
            })
            .build()
            .expect("swarm builds");

        let error = swarm
            .invoke(vec![InterruptResponseContent::new("missing", json!(true))])
            .await
            .expect_err("nothing to resume");

        assert_eq!(
            error.to_string(),
            "interrupt_id=<missing> | no node found with matching interrupt"
        );
        // TS routes before its `try`, so the run never reaches the after-invocation hook.
        assert_eq!(*fired.lock().expect("not poisoned"), ["before"]);
    }

    #[tokio::test]
    async fn a_failing_after_invocation_hook_replaces_the_runs_own_error() {
        let swarm = Swarm::builder()
            .node(handoff_agent("a", json!({ "agentId": "b", "message": "to b" })))
            .node(final_agent("b", "done"))
            .max_steps(1)
            .hook::<AfterMultiAgentInvocationEvent, _>(|_event| Err(Error::other("save failed")))
            .build()
            .expect("swarm builds");

        let error = swarm.invoke("start").await.expect_err("hook error wins");

        assert_eq!(error.to_string(), "save failed");
    }

    #[tokio::test]
    async fn dispatches_hooks_for_every_event_a_node_forwards() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let recorded = messages.clone();
        let node: Arc<dyn Node> = Arc::new(CancelEmittingNode {
            id: "custom".to_owned(),
            config: NodeConfig::default(),
        });
        let swarm = Swarm::builder()
            .node(node)
            .hook::<NodeCancelEvent, _>(move |event| {
                recorded
                    .lock()
                    .expect("not poisoned")
                    .push((event.node_id.clone(), event.message.clone()));
                Ok(())
            })
            .build()
            .expect("swarm builds");

        let result = swarm.invoke("go").await.expect("swarm runs");

        assert_eq!(result.status, ResultStatus::Completed);
        assert_eq!(
            *messages.lock().expect("not poisoned"),
            [("custom".to_owned(), "from the node".to_owned())]
        );
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
