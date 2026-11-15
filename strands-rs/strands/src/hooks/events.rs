//! Hook event data (TS `hooks/events.ts`).

use serde::{Deserialize, Serialize};

/// Redaction a guardrail requested for the user's input (TS `Redaction`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Redaction {
    user_message: String,
}

impl Redaction {
    /// A redaction replacing the last user message with `user_message`.
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
        }
    }

    /// The text that replaces the last user message.
    pub fn user_message(&self) -> &str {
        &self.user_message
    }
}
