//! Interrupt request and response types (TS `types/interrupt.ts`).

use serde::{Deserialize, Serialize};

use crate::types::json::JsonValue;

/// What a tool, hook or middleware passes to raise an interrupt (TS `InterruptParams`).
///
/// `response` answers the interrupt up front: when set, raising it returns the response instead
/// of pausing the agent.
///
/// ```
/// use serde_json::json;
/// use strands::types::InterruptParams;
///
/// let params = InterruptParams::builder()
///     .name("confirm_delete")
///     .reason(json!("Delete report.pdf?"))
///     .build();
/// assert_eq!(params.name, "confirm_delete");
///
/// let by_name: InterruptParams = "confirm_delete".into();
/// assert_eq!(by_name.reason, None);
/// ```
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct InterruptParams {
    /// Names the interrupt; the caller sees it and answers by id.
    #[builder(into)]
    pub name: String,
    /// Why the interrupt was raised, for the caller.
    pub reason: Option<JsonValue>,
    /// A preemptive response.
    pub response: Option<JsonValue>,
}

impl InterruptParams {
    /// Params with only a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            reason: None,
            response: None,
        }
    }
}

impl From<&str> for InterruptParams {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for InterruptParams {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

/// A caller's answer to an interrupt (TS `InterruptResponse`).
///
/// ```
/// use serde_json::json;
/// use strands::InterruptResponse;
///
/// let response = InterruptResponse::new("tool:tooluse-1:confirm_delete", json!(true));
/// assert_eq!(
///     serde_json::to_value(&response)?,
///     json!({ "interruptId": "tool:tooluse-1:confirm_delete", "response": true })
/// );
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InterruptResponse {
    /// The id of the interrupt being answered.
    pub interrupt_id: String,
    /// The answer. `null` is an answer.
    pub response: JsonValue,
}

impl InterruptResponse {
    /// An answer to interrupt `interrupt_id`.
    pub fn new(interrupt_id: impl Into<String>, response: JsonValue) -> Self {
        Self {
            interrupt_id: interrupt_id.into(),
            response,
        }
    }
}

/// An interrupt response wrapped for agent input (TS `InterruptResponseContent`).
///
/// Serializes as `{"interruptResponse": {"interruptId": .., "response": ..}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InterruptResponseContent {
    /// The response.
    pub interrupt_response: InterruptResponse,
}

impl InterruptResponseContent {
    /// Wraps `interrupt_response`.
    pub fn new(interrupt_response: InterruptResponse) -> Self {
        Self { interrupt_response }
    }
}

impl From<InterruptResponse> for InterruptResponseContent {
    fn from(interrupt_response: InterruptResponse) -> Self {
        Self::new(interrupt_response)
    }
}
