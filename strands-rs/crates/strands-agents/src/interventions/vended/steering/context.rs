//! Steering context providers
//! (`strands-ts/src/vended-interventions/steering/providers/context-provider.ts`).

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Map;

use crate::agent::Agent;
use crate::error::Result;
use crate::types::JsonValue;

/// Context data a provider contributes to a steering decision.
///
/// Serializes the way TS does — the discriminator under `type`, the rest alongside it:
/// `{"type": "toolLedger", "calls": [...]}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SteeringContextData {
    /// Discriminator identifying the provider that produced the data.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Provider-specific fields, flattened next to `type`.
    #[serde(flatten)]
    pub fields: Map<String, JsonValue>,
}

impl SteeringContextData {
    /// Context data of type `type_name` with no fields yet.
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            fields: Map::new(),
        }
    }

    /// Add a field to the snapshot.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<JsonValue>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

/// A passive observer that accumulates data from agent lifecycle events.
///
/// Providers register their own hook callbacks in [`init_agent`](Self::init_agent), which the
/// owning handler calls once at registration time, and expose what they accumulated through
/// [`context`](Self::context) — read whenever a steering decision is made.
#[async_trait]
pub trait SteeringContextProvider: Send + Sync {
    /// Identifier for this provider instance.
    fn name(&self) -> &str;

    /// Subscribe to the agent's hooks. Called once, when the owning handler is registered.
    async fn init_agent(&self, agent: &Agent) -> Result<()>;

    /// Current context snapshot for steering evaluation.
    fn context(&self) -> SteeringContextData;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_data_serializes_the_type_alongside_its_fields() {
        let data = SteeringContextData::new("toolLedger").with("calls", serde_json::json!([]));
        assert_eq!(
            serde_json::to_value(&data).unwrap(),
            serde_json::json!({ "type": "toolLedger", "calls": [] })
        );
    }
}
