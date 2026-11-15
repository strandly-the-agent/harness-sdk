//! Agent metadata passed to a model on every request (TS `agent/agent-metadata.ts`).

use serde::{Deserialize, Serialize};

/// Read-only view of agent metadata passed to a model with each request (TS `AgentMetadata`).
///
/// The agent rebuilds it for every request, so one model shared across agents sees each agent's
/// own identity.
///
/// ```
/// use strands::agent::agent_metadata::AgentMetadata;
///
/// let metadata = AgentMetadata::new().with_session_id("session-1");
/// assert_eq!(metadata.session_id.as_deref(), Some("session-1"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMetadata {
    /// The agent's persisted session id; present only when a session manager is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl AgentMetadata {
    /// Metadata with no fields set.
    #[must_use]
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
