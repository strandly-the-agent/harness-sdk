//! The [`MultiAgent`] trait, its invocation types, and the shared interrupt helpers every
//! orchestrator uses (`strands-ts/src/multiagent/multiagent.ts`).

use std::collections::BTreeMap;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::future::BoxFuture;
use futures::stream::{BoxStream, Stream};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::events::MultiAgentStreamEvent;
use super::state::{now_ms, MultiAgentResult, MultiAgentState, NodeResult, NodeState, ResultStatus, Status};
use crate::agent::{InvocationState, InvokeInput};
use crate::error::{Error, Result};
use crate::hooks::HookRegistry;
use crate::interrupt::InterruptSource;
use crate::types::{ContentBlock, InterruptResponseContent};

/// Input accepted by a multi-agent orchestrator.
///
/// Excludes whole messages and checkpoint resumes (TS `Exclude<InvokeArgs, ..>`): orchestrators
/// route content blocks between nodes rather than replaying raw conversation history.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MultiAgentInput {
    /// No new input.
    #[default]
    None,
    /// A text prompt.
    Text(String),
    /// Content blocks.
    Blocks(Vec<ContentBlock>),
    /// Answers to the interrupts an earlier invocation stopped on; resumes that run.
    InterruptResponses(Vec<InterruptResponseContent>),
}

impl From<&str> for MultiAgentInput {
    fn from(text: &str) -> Self {
        MultiAgentInput::Text(text.to_owned())
    }
}

impl From<String> for MultiAgentInput {
    fn from(text: String) -> Self {
        MultiAgentInput::Text(text)
    }
}

impl From<Vec<ContentBlock>> for MultiAgentInput {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        MultiAgentInput::Blocks(blocks)
    }
}

impl From<Vec<InterruptResponseContent>> for MultiAgentInput {
    fn from(responses: Vec<InterruptResponseContent>) -> Self {
        MultiAgentInput::InterruptResponses(responses)
    }
}

/// Rejected by [`MultiAgentInput::try_from`] for inputs an orchestrator cannot route.
pub const UNSUPPORTED_MULTIAGENT_INPUT_MESSAGE: &str =
    "Multi-agent orchestrators accept text, content blocks or interrupt responses only";

impl TryFrom<InvokeInput> for MultiAgentInput {
    type Error = Error;

    fn try_from(input: InvokeInput) -> Result<Self> {
        match input {
            InvokeInput::None => Ok(MultiAgentInput::None),
            InvokeInput::Text(text) => Ok(MultiAgentInput::Text(text)),
            InvokeInput::Blocks(blocks) => Ok(MultiAgentInput::Blocks(blocks)),
            InvokeInput::InterruptResponses(responses) => Ok(MultiAgentInput::InterruptResponses(responses)),
            InvokeInput::Messages(_) | InvokeInput::CheckpointResume(_) => {
                Err(Error::other(UNSUPPORTED_MULTIAGENT_INPUT_MESSAGE))
            }
        }
    }
}

impl From<MultiAgentInput> for InvokeInput {
    fn from(input: MultiAgentInput) -> Self {
        match input {
            MultiAgentInput::None => InvokeInput::None,
            MultiAgentInput::Text(text) => InvokeInput::Text(text),
            MultiAgentInput::Blocks(blocks) => InvokeInput::Blocks(blocks),
            MultiAgentInput::InterruptResponses(responses) => InvokeInput::InterruptResponses(responses),
        }
    }
}

impl MultiAgentInput {
    /// The interrupt responses this input resumes with, if it is a resume.
    pub fn resume_responses(&self) -> Option<&[InterruptResponseContent]> {
        match self {
            MultiAgentInput::InterruptResponses(responses) if !responses.is_empty() => Some(responses),
            _ => None,
        }
    }
}

/// Options for a single multi-agent orchestrator invocation, mirroring
/// [`InvokeOptions`](crate::agent::InvokeOptions).
#[derive(Clone, Debug, Default)]
pub struct MultiAgentInvokeOptions {
    /// Per-invocation state forwarded to every node's child agent. Mutable — one node's
    /// hooks/tools can read state written by a previous node.
    pub invocation_state: Option<InvocationState>,
    /// External cancellation, forwarded to every node and nested orchestrator.
    pub cancel: Option<CancellationToken>,
}

impl MultiAgentInvokeOptions {
    /// Set the shared per-invocation state.
    #[must_use]
    pub fn with_invocation_state(mut self, state: InvocationState) -> Self {
        self.invocation_state = Some(state);
        self
    }

    /// Set external cancellation.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// A multi-agent orchestrator that can be invoked, streamed, and composed as a node inside
/// another orchestrator.
///
/// Object-safe: implementors are used as `Arc<dyn MultiAgent>`. The generic
/// `invoke(impl Into<MultiAgentInput>)` conveniences live on [`MultiAgentExt`].
pub trait MultiAgent: Send + Sync {
    /// Unique identifier for this orchestrator.
    fn id(&self) -> &str;

    /// Human-readable name.
    fn name(&self) -> &str;

    /// Description of what this orchestrator does, when it has one.
    fn description(&self) -> Option<&str> {
        None
    }

    /// The orchestrator's hook registry.
    fn hooks(&self) -> &HookRegistry;

    /// Run to completion and return the aggregate result.
    fn invoke_with<'a>(
        &'a self,
        input: MultiAgentInput,
        options: MultiAgentInvokeOptions,
    ) -> BoxFuture<'a, Result<MultiAgentResult>>;

    /// Run and stream events as they occur; the last event carries the aggregate result.
    fn stream_with(&self, input: MultiAgentInput, options: MultiAgentInvokeOptions) -> MultiAgentStream;
}

/// `impl Into<MultiAgentInput>` conveniences for every [`MultiAgent`], including `dyn MultiAgent`.
pub trait MultiAgentExt: MultiAgent {
    /// Run to completion with default options.
    fn invoke(&self, input: impl Into<MultiAgentInput>) -> BoxFuture<'_, Result<MultiAgentResult>> {
        self.invoke_with(input.into(), MultiAgentInvokeOptions::default())
    }

    /// Stream with default options.
    fn stream(&self, input: impl Into<MultiAgentInput>) -> MultiAgentStream {
        self.stream_with(input.into(), MultiAgentInvokeOptions::default())
    }
}

impl<T: MultiAgent + ?Sized> MultiAgentExt for T {}

/// Stream of [`MultiAgentStreamEvent`]s from one invocation, like
/// [`AgentStream`](crate::agent::AgentStream).
pub struct MultiAgentStream {
    inner: BoxStream<'static, Result<MultiAgentStreamEvent>>,
}

impl MultiAgentStream {
    /// Wraps an event stream.
    pub fn new(inner: BoxStream<'static, Result<MultiAgentStreamEvent>>) -> Self {
        Self { inner }
    }

    /// The underlying boxed stream.
    pub fn into_inner(self) -> BoxStream<'static, Result<MultiAgentStreamEvent>> {
        self.inner
    }
}

impl Stream for MultiAgentStream {
    type Item = Result<MultiAgentStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Groups a flat list of interrupt responses by the node that raised each interrupt.
///
/// Ids are globally unique, so each response maps to exactly one node. A response for a nested
/// orchestrator matches the wrapping node and is forwarded as-is for recursive grouping.
pub fn group_interrupt_responses_by_node(
    responses: &[InterruptResponseContent],
    state: &MultiAgentState,
) -> Result<BTreeMap<String, Vec<InterruptResponseContent>>> {
    let mut grouped: BTreeMap<String, Vec<InterruptResponseContent>> = BTreeMap::new();
    for response in responses {
        let id = &response.interrupt_response.interrupt_id;
        let target = state
            .nodes()
            .iter()
            .find(|(_, node)| node.interrupts.iter().any(|i| &i.id == id))
            .map(|(node_id, _)| node_id.clone());
        let Some(target) = target else {
            return Err(Error::other(format!(
                "interrupt_id=<{id}> | no node found with matching interrupt"
            )));
        };
        grouped.entry(target).or_default().push(response.clone());
    }
    Ok(grouped)
}

/// Removes a stale `INTERRUPTED` result for `node_id` from both the node's history and the
/// orchestrator aggregate so a fresh result replaces it cleanly. No-op unless the node is
/// `INTERRUPTED`.
pub fn drop_stale_interrupted_result(node_id: &str, state: &mut MultiAgentState) {
    let Some(node) = state.node_mut(node_id) else {
        return;
    };
    if node.status != Status::Interrupted {
        return;
    }
    if node.results.last().map(|r| r.status) == Some(ResultStatus::Interrupted) {
        node.results.pop();
    }
    if let Some(idx) = state
        .results
        .iter()
        .position(|r| r.node_id == node_id && r.status == ResultStatus::Interrupted)
    {
        state.results.remove(idx);
    }
}

/// Records a hook-raised interrupt on a node that had not started executing: builds the
/// `INTERRUPTED` result, transitions the node, and appends the result to its history.
pub fn record_hook_interrupt(node_id: &str, node_state: &mut NodeState) -> NodeResult {
    let result = NodeResult::new(
        node_id,
        ResultStatus::Interrupted,
        now_ms().saturating_sub(node_state.start_time),
    )
    .with_interrupts(node_state.interrupts.clone());
    node_state.status = Status::Interrupted;
    node_state.results.push(result.clone());
    result
}

/// Builds the `INTERRUPTED` aggregate result for a paused run: the results collected so far plus
/// every unanswered interrupt still pending on a node.
pub fn interrupted_result(state: &MultiAgentState, content: Vec<ContentBlock>) -> MultiAgentResult {
    let interrupts = state
        .nodes()
        .values()
        .filter(|node| node.status == Status::Interrupted)
        .flat_map(|node| node.interrupts.iter().filter(|i| i.response.is_none()).cloned())
        .collect();
    MultiAgentResult::new(state.results.clone(), now_ms().saturating_sub(state.start_time))
        .with_status(ResultStatus::Interrupted)
        .with_content(content)
        .with_interrupts(interrupts)
}

/// Applies interrupt responses to a node's own orchestrator-hook interrupts and returns the
/// remaining responses — those bound for the child agent's interrupts.
///
/// Hook interrupts ([`InterruptSource::MultiagentHook`]) are stored on the node state directly;
/// the hook re-runs on resume and reads the stored response. Agent-level interrupts flow to the
/// child agent as resume input instead.
pub fn apply_orchestrator_hook_responses(
    node_state: &mut NodeState,
    responses: Vec<InterruptResponseContent>,
) -> Vec<InterruptResponseContent> {
    let mut forwarded = Vec::new();
    for response in responses {
        let local = node_state.interrupts.iter_mut().find(|i| {
            i.id == response.interrupt_response.interrupt_id && i.source == InterruptSource::MultiagentHook
        });
        match local {
            Some(interrupt) => interrupt.response = Some(response.interrupt_response.response.clone()),
            None => forwarded.push(response),
        }
    }
    forwarded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupt::Interrupt;
    use serde_json::json;

    fn hook_interrupt(id: &str) -> Interrupt {
        Interrupt::new(id, "approve", InterruptSource::MultiagentHook)
    }

    fn agent_interrupt(id: &str) -> Interrupt {
        Interrupt::new(id, "approve", InterruptSource::Tool)
    }

    #[test]
    fn converts_from_str_string_blocks_and_responses() {
        assert_eq!(
            MultiAgentInput::from("hi"),
            MultiAgentInput::Text("hi".to_owned())
        );
        assert_eq!(
            MultiAgentInput::from("hi".to_owned()),
            MultiAgentInput::Text("hi".to_owned())
        );
        assert_eq!(
            MultiAgentInput::from(vec![ContentBlock::text("hi")]),
            MultiAgentInput::Blocks(vec![ContentBlock::text("hi")])
        );
        let responses = vec![InterruptResponseContent::new("i1", json!(true))];
        assert_eq!(
            MultiAgentInput::from(responses.clone()),
            MultiAgentInput::InterruptResponses(responses)
        );
    }

    #[test]
    fn try_from_invoke_input_rejects_messages_and_checkpoint_resume() {
        let err = MultiAgentInput::try_from(InvokeInput::Messages(vec![])).unwrap_err();
        assert_eq!(err.to_string(), UNSUPPORTED_MULTIAGENT_INPUT_MESSAGE);
        assert_eq!(
            MultiAgentInput::try_from(InvokeInput::Text("hi".to_owned())).unwrap(),
            MultiAgentInput::Text("hi".to_owned())
        );
        assert_eq!(
            MultiAgentInput::try_from(InvokeInput::None).unwrap(),
            MultiAgentInput::None
        );
    }

    #[test]
    fn detects_resume_input_and_ignores_fresh_input() {
        let responses = vec![InterruptResponseContent::new("i1", json!(true))];
        let input = MultiAgentInput::InterruptResponses(responses.clone());
        assert_eq!(input.resume_responses(), Some(responses.as_slice()));
        assert!(MultiAgentInput::Text("hi".to_owned())
            .resume_responses()
            .is_none());
        assert!(MultiAgentInput::None.resume_responses().is_none());
        assert!(MultiAgentInput::InterruptResponses(vec![])
            .resume_responses()
            .is_none());
    }

    #[test]
    fn serializes_input_like_the_ts_wire_shape() {
        assert_eq!(
            serde_json::to_value(MultiAgentInput::Text("hi".to_owned())).unwrap(),
            json!("hi")
        );
        assert_eq!(
            serde_json::to_value(MultiAgentInput::Blocks(vec![ContentBlock::text("hi")])).unwrap(),
            json!([{"text": "hi"}])
        );
        assert_eq!(serde_json::to_value(MultiAgentInput::None).unwrap(), json!(null));
    }

    #[test]
    fn groups_responses_by_the_node_whose_interrupts_match_each_id() {
        let mut state = MultiAgentState::new(["a", "b"]);
        state.node_mut("a").unwrap().interrupts = vec![hook_interrupt("i1")];
        state.node_mut("b").unwrap().interrupts = vec![hook_interrupt("i2")];
        let grouped = group_interrupt_responses_by_node(
            &[
                InterruptResponseContent::new("i2", json!(false)),
                InterruptResponseContent::new("i1", json!(true)),
            ],
            &state,
        )
        .unwrap();
        assert_eq!(grouped["a"].len(), 1);
        assert_eq!(grouped["a"][0].interrupt_response.interrupt_id, "i1");
        assert_eq!(grouped["b"][0].interrupt_response.interrupt_id, "i2");
    }

    #[test]
    fn grouping_fails_when_a_response_id_matches_no_node_interrupt() {
        let state = MultiAgentState::new(["a"]);
        let err =
            group_interrupt_responses_by_node(&[InterruptResponseContent::new("nope", json!(true))], &state)
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            "interrupt_id=<nope> | no node found with matching interrupt"
        );
    }

    #[test]
    fn grouping_returns_an_empty_map_for_empty_responses() {
        let state = MultiAgentState::new(["a"]);
        assert!(group_interrupt_responses_by_node(&[], &state).unwrap().is_empty());
    }

    #[test]
    fn applies_hook_responses_and_forwards_the_rest() {
        let mut node = NodeState::new();
        node.interrupts = vec![hook_interrupt("hook1"), agent_interrupt("tool1")];
        let forwarded = apply_orchestrator_hook_responses(
            &mut node,
            vec![
                InterruptResponseContent::new("hook1", json!("yes")),
                InterruptResponseContent::new("tool1", json!("no")),
            ],
        );
        assert_eq!(node.interrupts[0].response, Some(json!("yes")));
        assert_eq!(node.interrupts[1].response, None);
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].interrupt_response.interrupt_id, "tool1");
    }

    #[test]
    fn records_a_hook_interrupt_on_a_node_that_never_ran() {
        let mut node = NodeState::new();
        node.interrupts = vec![hook_interrupt("hook1")];
        let result = record_hook_interrupt("a", &mut node);
        assert_eq!(result.status, ResultStatus::Interrupted);
        assert_eq!(result.interrupts.len(), 1);
        assert_eq!(node.status, Status::Interrupted);
        assert_eq!(node.results.len(), 1);
    }

    #[test]
    fn drops_the_stale_interrupted_result_from_node_and_aggregate() {
        let mut state = MultiAgentState::new(["a"]);
        let stale = NodeResult::new("a", ResultStatus::Interrupted, 1);
        state.results.push(stale.clone());
        let node = state.node_mut("a").unwrap();
        node.status = Status::Interrupted;
        node.results.push(stale);
        drop_stale_interrupted_result("a", &mut state);
        assert!(state.node("a").unwrap().results.is_empty());
        assert!(state.results.is_empty());
    }

    #[test]
    fn drop_stale_interrupted_result_is_a_no_op_for_a_running_node() {
        let mut state = MultiAgentState::new(["a"]);
        state
            .results
            .push(NodeResult::new("a", ResultStatus::Completed, 1));
        state.node_mut("a").unwrap().status = Status::Executing;
        drop_stale_interrupted_result("a", &mut state);
        assert_eq!(state.results.len(), 1);
        drop_stale_interrupted_result("missing", &mut state);
        assert_eq!(state.results.len(), 1);
    }

    #[test]
    fn interrupted_result_collects_unanswered_interrupts() {
        let mut state = MultiAgentState::new(["a", "b"]);
        state
            .results
            .push(NodeResult::new("a", ResultStatus::Interrupted, 1));
        let node = state.node_mut("a").unwrap();
        node.status = Status::Interrupted;
        node.interrupts = vec![
            hook_interrupt("open"),
            hook_interrupt("answered").with_reason(json!("why")),
        ];
        node.interrupts[1].response = Some(json!(true));
        let result = interrupted_result(&state, vec![ContentBlock::text("paused")]);
        assert_eq!(result.status, ResultStatus::Interrupted);
        assert_eq!(result.interrupts.len(), 1);
        assert_eq!(result.interrupts[0].id, "open");
        assert_eq!(result.to_string(), "paused");
    }
}
