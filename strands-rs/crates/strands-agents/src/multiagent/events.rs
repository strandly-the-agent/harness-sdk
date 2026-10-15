//! Multi-agent hook and stream events (`strands-ts/src/multiagent/events.ts`).

use std::sync::Arc;

use super::multiagent::MultiAgent;
use super::nodes::NodeType;
use super::state::{MultiAgentResult, MultiAgentStateHandle, NodeResult};
use crate::agent::{AgentStreamEvent, InvocationState};
use crate::error::{Error, Result};
use crate::hooks::{Cancel, HookEvent};
use crate::interrupt::{Interrupt, InterruptSource};
use crate::types::{InterruptParams, JsonValue};

/// Handle to the orchestrator an event was raised by.
pub type OrchestratorRef = Arc<dyn MultiAgent>;

/// A multi-agent orchestrator finished initialization.
#[derive(Clone)]
pub struct MultiAgentInitializedEvent {
    /// The orchestrator.
    pub orchestrator: OrchestratorRef,
}
impl HookEvent for MultiAgentInitializedEvent {}

/// An orchestrator invocation is starting.
#[derive(Clone)]
pub struct BeforeMultiAgentInvocationEvent {
    /// The orchestrator.
    pub orchestrator: OrchestratorRef,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
}
impl HookEvent for BeforeMultiAgentInvocationEvent {}

/// An orchestrator invocation finished.
#[derive(Clone)]
pub struct AfterMultiAgentInvocationEvent {
    /// The orchestrator.
    pub orchestrator: OrchestratorRef,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
}
impl HookEvent for AfterMultiAgentInvocationEvent {
    fn reverse_callbacks() -> bool {
        true
    }
}

/// A node is about to execute. Set `cancel` to skip it.
#[derive(Clone)]
pub struct BeforeNodeCallEvent {
    /// The orchestrator.
    pub orchestrator: OrchestratorRef,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// Id of the node about to run.
    pub node_id: String,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
    /// Cancel this node's execution.
    pub cancel: Cancel,
}
impl HookEvent for BeforeNodeCallEvent {}

impl BeforeNodeCallEvent {
    /// Raises an orchestrator-level interrupt that pauses the run before this node executes.
    ///
    /// Returns the stored response once a resume has answered it; otherwise returns
    /// [`Error::Interrupt`] and the orchestrator produces an `INTERRUPTED` result. The interrupt
    /// is stored on the target node's state, so resuming routes through the same machinery as a
    /// child-agent interrupt.
    pub fn interrupt(&self, params: InterruptParams) -> Result<JsonValue> {
        let id = format!("multiagent-hook:beforeNodeCall:{}:{}", self.node_id, params.name);
        let interrupt = self.state.with_mut(|state| {
            let node_id = self.node_id.clone();
            let Some(node) = state.node_mut(&node_id) else {
                return Err(Error::other(format!(
                    "node_id=<{node_id}> | node state not found"
                )));
            };
            if let Some(existing) = node.interrupts.iter().find(|i| i.id == id) {
                return Ok(existing.clone());
            }
            let mut interrupt = Interrupt::new(&id, &params.name, InterruptSource::MultiagentHook);
            interrupt.reason = params.reason.clone();
            interrupt.response = params.response.clone();
            node.interrupts.push(interrupt.clone());
            Ok(interrupt)
        })?;
        match interrupt.response.clone() {
            Some(response) => Ok(response),
            None => Err(Error::Interrupt(vec![interrupt])),
        }
    }
}

/// A node finished executing (successfully, cancelled, or with an error).
#[derive(Clone)]
pub struct AfterNodeCallEvent {
    /// The orchestrator.
    pub orchestrator: OrchestratorRef,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// Id of the node that ran.
    pub node_id: String,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
    /// Failure message, when the node raised an error.
    pub error: Option<String>,
}
impl HookEvent for AfterNodeCallEvent {
    fn reverse_callbacks() -> bool {
        true
    }
}

/// An inner streaming event from a node, tagged with its origin (TS
/// `NodeStreamUpdateInnerEvent`).
#[derive(Clone)]
pub enum NodeInnerEvent {
    /// The node wraps an agent.
    Agent(Box<AgentStreamEvent>),
    /// The node wraps a nested orchestrator.
    MultiAgent(Box<MultiAgentStreamEvent>),
    /// The node wraps a custom runnable; its events are opaque here.
    Custom(JsonValue),
}

impl NodeInnerEvent {
    /// The TS `source` discriminator for this inner event.
    pub fn source(&self) -> &'static str {
        match self {
            NodeInnerEvent::Agent(_) => "agent",
            NodeInnerEvent::MultiAgent(_) => "multiAgent",
            NodeInnerEvent::Custom(_) => "custom",
        }
    }
}

/// A node forwarded an inner streaming event, tagged with the node's identity.
#[derive(Clone)]
pub struct NodeStreamUpdateEvent {
    /// Id of the streaming node.
    pub node_id: String,
    /// What the node wraps.
    pub node_type: NodeType,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// The inner event.
    pub inner: NodeInnerEvent,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
}
impl HookEvent for NodeStreamUpdateEvent {}

/// A node produced its result.
#[derive(Clone)]
pub struct NodeResultEvent {
    /// Id of the node.
    pub node_id: String,
    /// What the node wraps.
    pub node_type: NodeType,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// The node's result.
    pub result: NodeResult,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
}
impl HookEvent for NodeResultEvent {}

/// Execution transitioned from one node to one or more others.
#[derive(Clone)]
pub struct MultiAgentHandoffEvent {
    /// Id of the node handing off.
    pub source: String,
    /// Ids of the nodes receiving control.
    pub targets: Vec<String>,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
}
impl HookEvent for MultiAgentHandoffEvent {}

/// A node was skipped via [`BeforeNodeCallEvent::cancel`].
#[derive(Clone)]
pub struct NodeCancelEvent {
    /// Id of the cancelled node.
    pub node_id: String,
    /// Per-invocation orchestration state.
    pub state: MultiAgentStateHandle,
    /// Why the node was cancelled.
    pub message: String,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
}
impl HookEvent for NodeCancelEvent {}

/// The invocation's aggregate result; always the last event in the stream.
#[derive(Clone)]
pub struct MultiAgentResultEvent {
    /// The aggregate result.
    pub result: MultiAgentResult,
    /// Shared invocation state forwarded to every node.
    pub invocation_state: InvocationState,
}
impl HookEvent for MultiAgentResultEvent {}

/// Union of all multi-agent streaming events, like
/// [`AgentStreamEvent`](crate::agent::AgentStreamEvent).
#[derive(Clone)]
pub enum MultiAgentStreamEvent {
    /// Before the invocation starts.
    BeforeMultiAgentInvocation(BeforeMultiAgentInvocationEvent),
    /// After the invocation finishes.
    AfterMultiAgentInvocation(AfterMultiAgentInvocationEvent),
    /// Before a node runs.
    BeforeNodeCall(BeforeNodeCallEvent),
    /// After a node runs.
    AfterNodeCall(AfterNodeCallEvent),
    /// An inner event forwarded from a node.
    NodeStreamUpdate(NodeStreamUpdateEvent),
    /// A node's result.
    NodeResult(NodeResultEvent),
    /// A node was cancelled by a hook.
    NodeCancel(NodeCancelEvent),
    /// Execution handed off between nodes.
    MultiAgentHandoff(MultiAgentHandoffEvent),
    /// The invocation's aggregate result; always the last event.
    MultiAgentResult(MultiAgentResultEvent),
}

impl MultiAgentStreamEvent {
    /// Short event name, matching the TS `type` discriminator.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::BeforeMultiAgentInvocation(_) => "beforeMultiAgentInvocationEvent",
            Self::AfterMultiAgentInvocation(_) => "afterMultiAgentInvocationEvent",
            Self::BeforeNodeCall(_) => "beforeNodeCallEvent",
            Self::AfterNodeCall(_) => "afterNodeCallEvent",
            Self::NodeStreamUpdate(_) => "nodeStreamUpdateEvent",
            Self::NodeResult(_) => "nodeResultEvent",
            Self::NodeCancel(_) => "nodeCancelEvent",
            Self::MultiAgentHandoff(_) => "multiAgentHandoffEvent",
            Self::MultiAgentResult(_) => "multiAgentResultEvent",
        }
    }

    /// Id of the node this event concerns, when it concerns one.
    pub fn node_id(&self) -> Option<&str> {
        match self {
            Self::BeforeNodeCall(e) => Some(&e.node_id),
            Self::AfterNodeCall(e) => Some(&e.node_id),
            Self::NodeStreamUpdate(e) => Some(&e.node_id),
            Self::NodeResult(e) => Some(&e.node_id),
            Self::NodeCancel(e) => Some(&e.node_id),
            _ => None,
        }
    }

    /// The aggregate result, when this is the `MultiAgentResult` event.
    pub fn result(&self) -> Option<&MultiAgentResult> {
        match self {
            Self::MultiAgentResult(e) => Some(&e.result),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookRegistry;
    use crate::multiagent::multiagent::{MultiAgentInput, MultiAgentInvokeOptions, MultiAgentStream};
    use crate::multiagent::state::{MultiAgentState, ResultStatus, Status};
    use futures::future::BoxFuture;
    use serde_json::json;

    struct TestOrchestrator {
        hooks: HookRegistry,
    }

    impl MultiAgent for TestOrchestrator {
        fn id(&self) -> &str {
            "test-orchestrator"
        }

        fn name(&self) -> &str {
            "test"
        }

        fn hooks(&self) -> &HookRegistry {
            &self.hooks
        }

        fn invoke_with<'a>(
            &'a self,
            _input: MultiAgentInput,
            _options: MultiAgentInvokeOptions,
        ) -> BoxFuture<'a, Result<MultiAgentResult>> {
            Box::pin(async { Ok(MultiAgentResult::new(vec![], 0)) })
        }

        fn stream_with(
            &self,
            _input: MultiAgentInput,
            _options: MultiAgentInvokeOptions,
        ) -> MultiAgentStream {
            MultiAgentStream::new(Box::pin(futures::stream::empty()))
        }
    }

    fn orchestrator() -> OrchestratorRef {
        Arc::new(TestOrchestrator {
            hooks: HookRegistry::new(),
        })
    }

    fn handle() -> MultiAgentStateHandle {
        MultiAgentStateHandle::new(MultiAgentState::new(["a"]))
    }

    fn before_node_call() -> BeforeNodeCallEvent {
        BeforeNodeCallEvent {
            orchestrator: orchestrator(),
            state: handle(),
            node_id: "a".to_owned(),
            invocation_state: InvocationState::default(),
            cancel: Cancel::No,
        }
    }

    #[test]
    fn creates_events_with_their_properties() {
        let event = MultiAgentInitializedEvent {
            orchestrator: orchestrator(),
        };
        assert_eq!(event.orchestrator.id(), "test-orchestrator");

        let before = before_node_call();
        assert_eq!(before.node_id, "a");
        assert!(!before.cancel.is_cancelled());

        let after = AfterNodeCallEvent {
            orchestrator: orchestrator(),
            state: handle(),
            node_id: "a".to_owned(),
            invocation_state: InvocationState::default(),
            error: Some("boom".to_owned()),
        };
        assert_eq!(after.error.as_deref(), Some("boom"));

        let handoff = MultiAgentHandoffEvent {
            source: "a".to_owned(),
            targets: vec!["b".to_owned()],
            state: handle(),
            invocation_state: InvocationState::default(),
        };
        assert_eq!(handoff.targets, ["b"]);

        let cancel = NodeCancelEvent {
            node_id: "a".to_owned(),
            state: handle(),
            message: "skipped".to_owned(),
            invocation_state: InvocationState::default(),
        };
        assert_eq!(cancel.message, "skipped");
    }

    #[test]
    fn only_after_events_reverse_their_callbacks() {
        assert!(!MultiAgentInitializedEvent::reverse_callbacks());
        assert!(!BeforeMultiAgentInvocationEvent::reverse_callbacks());
        assert!(AfterMultiAgentInvocationEvent::reverse_callbacks());
        assert!(!BeforeNodeCallEvent::reverse_callbacks());
        assert!(AfterNodeCallEvent::reverse_callbacks());
        assert!(!NodeStreamUpdateEvent::reverse_callbacks());
        assert!(!NodeResultEvent::reverse_callbacks());
        assert!(!MultiAgentHandoffEvent::reverse_callbacks());
        assert!(!NodeCancelEvent::reverse_callbacks());
        assert!(!MultiAgentResultEvent::reverse_callbacks());
    }

    #[test]
    fn cancel_accepts_true_and_a_message() {
        let mut event = before_node_call();
        event.cancel = Cancel::from(true);
        assert_eq!(event.cancel.message_or("default"), Some("default"));
        event.cancel = Cancel::from("no capacity");
        assert_eq!(event.cancel.message_or("default"), Some("no capacity"));
    }

    #[test]
    fn stream_event_type_names_match_ts() {
        let state = handle();
        let invocation_state = InvocationState::default();
        let events = vec![
            MultiAgentStreamEvent::BeforeMultiAgentInvocation(BeforeMultiAgentInvocationEvent {
                orchestrator: orchestrator(),
                state: state.clone(),
                invocation_state: invocation_state.clone(),
            }),
            MultiAgentStreamEvent::AfterMultiAgentInvocation(AfterMultiAgentInvocationEvent {
                orchestrator: orchestrator(),
                state: state.clone(),
                invocation_state: invocation_state.clone(),
            }),
            MultiAgentStreamEvent::BeforeNodeCall(before_node_call()),
            MultiAgentStreamEvent::AfterNodeCall(AfterNodeCallEvent {
                orchestrator: orchestrator(),
                state: state.clone(),
                node_id: "a".to_owned(),
                invocation_state: invocation_state.clone(),
                error: None,
            }),
            MultiAgentStreamEvent::NodeStreamUpdate(NodeStreamUpdateEvent {
                node_id: "a".to_owned(),
                node_type: NodeType::Agent,
                state: state.clone(),
                inner: NodeInnerEvent::Custom(json!({"type": "custom"})),
                invocation_state: invocation_state.clone(),
            }),
            MultiAgentStreamEvent::NodeResult(NodeResultEvent {
                node_id: "a".to_owned(),
                node_type: NodeType::Agent,
                state: state.clone(),
                result: NodeResult::new("a", ResultStatus::Completed, 1),
                invocation_state: invocation_state.clone(),
            }),
            MultiAgentStreamEvent::NodeCancel(NodeCancelEvent {
                node_id: "a".to_owned(),
                state: state.clone(),
                message: "skipped".to_owned(),
                invocation_state: invocation_state.clone(),
            }),
            MultiAgentStreamEvent::MultiAgentHandoff(MultiAgentHandoffEvent {
                source: "a".to_owned(),
                targets: vec!["b".to_owned()],
                state,
                invocation_state: invocation_state.clone(),
            }),
            MultiAgentStreamEvent::MultiAgentResult(MultiAgentResultEvent {
                result: MultiAgentResult::new(vec![], 0),
                invocation_state,
            }),
        ];
        let names: Vec<&str> = events.iter().map(MultiAgentStreamEvent::type_name).collect();
        assert_eq!(
            names,
            [
                "beforeMultiAgentInvocationEvent",
                "afterMultiAgentInvocationEvent",
                "beforeNodeCallEvent",
                "afterNodeCallEvent",
                "nodeStreamUpdateEvent",
                "nodeResultEvent",
                "nodeCancelEvent",
                "multiAgentHandoffEvent",
                "multiAgentResultEvent",
            ]
        );
        assert_eq!(events[2].node_id(), Some("a"));
        assert!(events[0].node_id().is_none());
        assert!(events[8].result().is_some());
        assert!(events[0].result().is_none());
    }

    #[test]
    fn inner_event_sources_match_ts() {
        assert_eq!(NodeInnerEvent::Custom(json!({})).source(), "custom");
        assert_eq!(
            NodeInnerEvent::MultiAgent(Box::new(MultiAgentStreamEvent::MultiAgentResult(
                MultiAgentResultEvent {
                    result: MultiAgentResult::new(vec![], 0),
                    invocation_state: InvocationState::default(),
                }
            )))
            .source(),
            "multiAgent"
        );
    }

    #[test]
    fn hook_interrupt_halts_then_returns_the_stored_response() {
        let event = before_node_call();
        let err = event.interrupt(InterruptParams::new("approve")).unwrap_err();
        let Error::Interrupt(interrupts) = &err else {
            panic!("expected interrupt, got {err:?}");
        };
        assert_eq!(interrupts[0].id, "multiagent-hook:beforeNodeCall:a:approve");
        assert_eq!(interrupts[0].source, InterruptSource::MultiagentHook);
        assert_eq!(event.state.node("a").unwrap().interrupts.len(), 1);

        event.state.with_mut(|state| {
            state.node_mut("a").unwrap().interrupts[0].response = Some(json!(true));
        });
        assert_eq!(
            event.interrupt(InterruptParams::new("approve")).unwrap(),
            json!(true)
        );
    }

    #[test]
    fn hook_interrupt_with_a_preemptive_response_does_not_halt() {
        let event = before_node_call();
        let response = event
            .interrupt(InterruptParams::new("approve").with_response(json!("yes")))
            .unwrap();
        assert_eq!(response, json!("yes"));
    }

    #[test]
    fn hook_interrupt_fails_when_the_node_is_not_tracked() {
        let mut event = before_node_call();
        event.node_id = "missing".to_owned();
        let err = event.interrupt(InterruptParams::new("approve")).unwrap_err();
        assert_eq!(err.to_string(), "node_id=<missing> | node state not found");
    }

    #[test]
    fn node_states_start_pending() {
        assert_eq!(handle().node("a").unwrap().status, Status::Pending);
    }
}
