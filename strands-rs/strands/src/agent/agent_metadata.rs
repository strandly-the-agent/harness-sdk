//! Agent metadata passed to models (TS `agent/agent-metadata.ts`).

/// Read-only view of the invoking agent, passed to a model with each request (TS `AgentMetadata`).
///
/// The agent rebuilds it for every request, so one model shared by several agents sees each
/// agent's own identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AgentMetadata {
    /// The agent's persisted session id; present only when a session manager is attached.
    pub session_id: Option<String>,
}

impl AgentMetadata {
    /// Metadata with nothing set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the session id.
    #[must_use]
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}
