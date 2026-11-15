//! Agent details a model provider can use (TS `agent/agent-metadata.ts`).

/// Agent details passed to model providers with each request (TS `AgentMetadata`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into))]
pub struct AgentMetadata {
    /// The session the agent belongs to, when it has one.
    pub session_id: Option<String>,
}
