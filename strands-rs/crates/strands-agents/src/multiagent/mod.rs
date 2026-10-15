//! Multi-agent orchestration: shared state, events, and the [`MultiAgent`] trait every
//! orchestrator (Graph, Swarm, …) implements (`strands-ts/src/multiagent/`).

pub mod events;
// Mirrors the TS file layout (`multiagent/multiagent.ts`).
#[allow(clippy::module_inception)]
pub mod multiagent;
pub mod nodes;
pub mod state;

pub use events::{
    AfterMultiAgentInvocationEvent, AfterNodeCallEvent, BeforeMultiAgentInvocationEvent, BeforeNodeCallEvent,
    MultiAgentHandoffEvent, MultiAgentInitializedEvent, MultiAgentResultEvent, MultiAgentStreamEvent,
    NodeCancelEvent, NodeInnerEvent, NodeResultEvent, NodeStreamUpdateEvent, OrchestratorRef,
};
pub use multiagent::{
    apply_orchestrator_hook_responses, drop_stale_interrupted_result, group_interrupt_responses_by_node,
    interrupted_result, record_hook_interrupt, MultiAgent, MultiAgentExt, MultiAgentInput,
    MultiAgentInvokeOptions, MultiAgentStream, UNSUPPORTED_MULTIAGENT_INPUT_MESSAGE,
};
pub use nodes::{
    run_node, AgentNode, AgentNodeOptions, MultiAgentNode, MultiAgentNodeOptions, Node, NodeConfig,
    NodeDefinition, NodeInputOptions, NodeSink, NodeType,
};
pub use state::{
    aggregate_node_usage, resolve_status, MultiAgentResult, MultiAgentState, MultiAgentStateHandle,
    NodeResult, NodeResultUpdate, NodeState, ResultStatus, Status,
};
