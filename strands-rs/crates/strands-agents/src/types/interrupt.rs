//! Interrupt input types (`strands-ts/src/types/interrupt.ts`).

use serde::{Deserialize, Serialize};

use crate::types::JsonValue;

/// What a raiser asks for when it interrupts (`InterruptParams`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InterruptParams {
    /// Unique within one hook callback or tool execution.
    pub name: String,
    /// Why input is needed; surfaced to the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<JsonValue>,
    /// Preemptive response: returns immediately instead of halting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<JsonValue>,
}

impl InterruptParams {
    /// Params with just a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            reason: None,
            response: None,
        }
    }

    /// Attach a reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<JsonValue>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Attach a preemptive response (the raise returns it instead of halting).
    #[must_use]
    pub fn with_response(mut self, response: impl Into<JsonValue>) -> Self {
        self.response = Some(response.into());
        self
    }
}

/// One answer to one interrupt (`InterruptResponse`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InterruptResponse {
    /// Id of the interrupt being answered.
    pub interrupt_id: String,
    /// The answer handed back to the raiser.
    pub response: JsonValue,
}

impl InterruptResponse {
    /// An answer for `interrupt_id`.
    pub fn new(interrupt_id: impl Into<String>, response: impl Into<JsonValue>) -> Self {
        Self {
            interrupt_id: interrupt_id.into(),
            response: response.into(),
        }
    }
}

/// Resume input block: wire shape `{ "interruptResponse": { "interruptId", "response" } }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InterruptResponseContent {
    /// The answer carried by this block.
    pub interrupt_response: InterruptResponse,
}

impl InterruptResponseContent {
    /// Block answering `interrupt_id` with `response`.
    pub fn new(interrupt_id: impl Into<String>, response: impl Into<JsonValue>) -> Self {
        Self {
            interrupt_response: InterruptResponse::new(interrupt_id, response),
        }
    }
}

impl From<InterruptResponse> for InterruptResponseContent {
    fn from(interrupt_response: InterruptResponse) -> Self {
        Self { interrupt_response }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_content_wire_shape_matches_ts() {
        let block = InterruptResponseContent::new("tool:1:approve", serde_json::json!(true));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            serde_json::json!({ "interruptResponse": { "interruptId": "tool:1:approve", "response": true } })
        );
        let parsed: InterruptResponseContent = serde_json::from_value(
            serde_json::json!({ "interruptResponse": { "interruptId": "x", "response": 1 } }),
        )
        .unwrap();
        assert_eq!(parsed.interrupt_response.interrupt_id, "x");
    }

    #[test]
    fn params_omit_absent_reason_and_response() {
        assert_eq!(
            serde_json::to_value(InterruptParams::new("approve")).unwrap(),
            serde_json::json!({ "name": "approve" })
        );
    }
}
