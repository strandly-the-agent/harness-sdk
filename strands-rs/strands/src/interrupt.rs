//! Interrupt wire types (TS `interrupt.ts` and `types/interrupt.ts`).
//!
//! An interrupt pauses an agent so a human or the application can answer it; the answer comes back
//! as an [`InterruptResponse`] on the next invocation. The persisted interrupt state that snapshots
//! carry is crate-internal, as in TS.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::types::json::{JsonValue, present_null};

/// What raised an interrupt (TS `InterruptSource`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InterruptSource {
    /// `"tool"`: a tool called `interrupt`.
    #[serde(rename = "tool")]
    Tool,
    /// `"hook"`: a hook callback called `interrupt` (the TS default).
    #[default]
    #[serde(rename = "hook")]
    Hook,
    /// `"middleware"`: a middleware stage called `interrupt`.
    #[serde(rename = "middleware")]
    Middleware,
    /// `"multiagent-hook"`: a multi-agent hook called `interrupt`.
    #[serde(rename = "multiagent-hook")]
    MultiagentHook,
}

/// A raised interrupt (TS `Interrupt`).
///
/// `reason` and `response` distinguish a present `null` (`Some(JsonValue::Null)`) from a missing
/// value (`None`): an interrupt answered with `null` counts as answered.
///
/// ```
/// use serde_json::json;
/// use strands::interrupt::{Interrupt, InterruptSource};
///
/// let interrupt = Interrupt::new("tool:t-1:confirm", "confirm")
///     .with_reason(json!("Proceed?"))
///     .with_source(InterruptSource::Tool);
/// assert_eq!(
///     serde_json::to_value(&interrupt).unwrap(),
///     json!({ "id": "tool:t-1:confirm", "name": "confirm", "reason": "Proceed?", "source": "tool" })
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interrupt {
    /// The interrupt id, unique within an invocation (for example `tool:<toolUseId>:<name>`).
    pub id: String,
    /// The user-defined name.
    pub name: String,
    /// Why the interrupt was raised.
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub reason: Option<JsonValue>,
    /// The answer, once there is one.
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub response: Option<JsonValue>,
    /// What raised the interrupt. Data saved without it loads as [`InterruptSource::Hook`].
    #[serde(default)]
    pub source: InterruptSource,
}

impl Interrupt {
    /// Creates an unanswered interrupt raised by a hook.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            reason: None,
            response: None,
            source: InterruptSource::Hook,
        }
    }

    /// Sets the reason.
    #[must_use]
    pub fn with_reason(mut self, reason: JsonValue) -> Self {
        self.reason = Some(reason);
        self
    }

    /// Sets the response.
    #[must_use]
    pub fn with_response(mut self, response: JsonValue) -> Self {
        self.response = Some(response);
        self
    }

    /// Sets the source.
    #[must_use]
    pub fn with_source(mut self, source: InterruptSource) -> Self {
        self.source = source;
        self
    }
}

/// Parameters for raising an interrupt (TS `InterruptParams`).
///
/// ```
/// use serde_json::json;
/// use strands::interrupt::InterruptParams;
///
/// let params = InterruptParams { reason: Some(json!("Proceed?")), ..InterruptParams::new("confirm") };
/// let built = InterruptParams::builder().name("confirm").reason(json!("Proceed?")).build();
/// assert_eq!(params, built);
/// assert_eq!(InterruptParams::from("confirm").name, "confirm");
/// ```
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct InterruptParams {
    /// The interrupt name, unique within one hook callback or tool call.
    #[builder(into)]
    pub name: String,
    /// Why the interrupt is raised.
    pub reason: Option<JsonValue>,
    /// A preemptive response: when set, the interrupt is answered at once and nothing pauses.
    pub response: Option<JsonValue>,
}

impl InterruptParams {
    /// Creates parameters with only a name.
    #[must_use]
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

/// The answer to an interrupt (TS `InterruptResponse`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptResponse {
    /// The id of the interrupt this answers.
    pub interrupt_id: String,
    /// The answer; `JsonValue::Null` is a valid answer.
    #[serde(deserialize_with = "JsonValue::deserialize")]
    pub response: JsonValue,
}

impl InterruptResponse {
    /// Creates an answer to the interrupt with the given id.
    #[must_use]
    pub fn new(interrupt_id: impl Into<String>, response: JsonValue) -> Self {
        Self {
            interrupt_id: interrupt_id.into(),
            response,
        }
    }
}

/// An interrupt answer as invocation content (TS `InterruptResponseContent`), written as
/// `{"interruptResponse":{"interruptId":..,"response":..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptResponseContent {
    /// The answer.
    pub interrupt_response: InterruptResponse,
}

impl InterruptResponseContent {
    /// Wraps an answer.
    #[must_use]
    pub fn new(interrupt_response: InterruptResponse) -> Self {
        Self { interrupt_response }
    }
}

impl From<InterruptResponse> for InterruptResponseContent {
    fn from(interrupt_response: InterruptResponse) -> Self {
        Self::new(interrupt_response)
    }
}

/// The persisted interrupt state (TS `InterruptStateData`).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the interrupt runtime and snapshots")
)]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InterruptStateData {
    pub(crate) interrupts: IndexMap<String, Interrupt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resume_responses: Option<Vec<InterruptResponseContent>>,
    pub(crate) activated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_tool_execution: Option<PendingToolExecution>,
    /// TS writes `context` only when it is non-empty.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub(crate) context: serde_json::Map<String, JsonValue>,
}

/// A tool execution paused by an interrupt (TS `PendingToolExecution`).
///
/// Both fields keep the raw JSON, as TS does, so the state round-trips byte for byte (no tracking
/// id is minted into the message) and validation happens when the runtime restores it.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the interrupt runtime and snapshots")
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingToolExecution {
    pub(crate) assistant_message_data: JsonValue,
    pub(crate) completed_tool_results: JsonValue,
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/__tests__/interrupt.test.ts (the type and serde parts; the
    //! `InterruptState` runtime, `InterruptError` and `interruptFromAgent` are the interrupt runtime's).

    use serde_json::json;

    use super::*;

    fn pending_tool_execution() -> PendingToolExecution {
        PendingToolExecution {
            assistant_message_data: json!({
                "role": "assistant",
                "content": [
                    { "toolUse": { "name": "tool", "toolUseId": "t-0", "input": {} } },
                    { "toolUse": { "name": "tool", "toolUseId": "t-1", "input": {} } }
                ]
            }),
            completed_tool_results: json!({
                "t-0": { "toolResult": { "toolUseId": "t-0", "status": "success", "content": [] } }
            }),
        }
    }

    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    #[test]
    fn constructs_with_all_fields_and_supports_response_mutation() {
        let mut interrupt = Interrupt::new("int-1", "confirm_action")
            .with_reason(json!("Please confirm"))
            .with_response(json!("approved"));
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({ "id": "int-1", "name": "confirm_action", "reason": "Please confirm", "response": "approved", "source": "hook" })
        );
        interrupt.response = Some(json!("changed"));
        assert_eq!(interrupt.response, Some(json!("changed")));
    }

    #[test]
    fn round_trips_through_json_serialization_with_complex_data() {
        let original = Interrupt::new("int-1", "test")
            .with_reason(json!({ "complex": { "nested": "data" } }))
            .with_response(json!(["array", "response"]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn omits_undefined_reason_response_from_to_json() {
        let interrupt = Interrupt::new("int-1", "test");
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({ "id": "int-1", "name": "test", "source": "hook" })
        );
    }

    #[test]
    fn a_null_response_is_kept_and_a_missing_source_loads_as_hook() {
        let interrupt: Interrupt =
            serde_json::from_value(json!({ "id": "x", "name": "legacy", "response": null })).unwrap();
        assert_eq!(interrupt.response, Some(JsonValue::Null));
        assert_eq!(interrupt.source, InterruptSource::Hook);
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({ "id": "x", "name": "legacy", "response": null, "source": "hook" })
        );
    }

    #[test]
    fn round_trips_through_json_with_full_state() {
        let interrupt = Interrupt::new("int-1", "test")
            .with_reason(json!({ "complex": "reason" }))
            .with_response(json!(["array", "response"]));
        let original = InterruptStateData {
            interrupts: IndexMap::from([("int-1".to_owned(), interrupt)]),
            activated: true,
            ..InterruptStateData::default()
        };
        let value = serde_json::to_value(&original).unwrap();
        assert_eq!(
            value,
            json!({
                "interrupts": { "int-1": {
                    "id": "int-1", "name": "test", "reason": { "complex": "reason" }, "response": ["array", "response"], "source": "hook"
                } },
                "activated": true
            })
        );
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_pending_tool_execution_through_json() {
        let original = InterruptStateData {
            interrupts: IndexMap::from([("int-1".to_owned(), Interrupt::new("int-1", "test"))]),
            activated: true,
            pending_tool_execution: Some(pending_tool_execution()),
            ..InterruptStateData::default()
        };
        let restored = round_trip(&original);
        assert_eq!(restored, original);
        assert_eq!(restored.pending_tool_execution, Some(pending_tool_execution()));
    }

    #[test]
    fn deserializes_state_with_resume_responses() {
        let state: InterruptStateData = serde_json::from_value(json!({
            "interrupts": { "int-1": { "id": "int-1", "name": "test", "reason": "reason", "response": "yes" } },
            "resumeResponses": [{ "interruptResponse": { "interruptId": "int-1", "response": "yes" } }],
            "activated": true
        }))
        .unwrap();
        assert!(state.activated);
        assert_eq!(
            state.interrupts["int-1"],
            Interrupt::new("int-1", "test")
                .with_reason(json!("reason"))
                .with_response(json!("yes"))
        );
        assert_eq!(
            state.resume_responses,
            Some(vec![InterruptResponse::new("int-1", json!("yes")).into()])
        );
    }

    #[test]
    fn restores_a_pending_tool_execution_a_newer_sdk_saved_with_content_block_types_this_version_does_not_know()
     {
        let saved = json!({
            "interrupts": {},
            "activated": true,
            "pendingToolExecution": {
                "assistantMessageData": {
                    "role": "assistant",
                    "content": [
                        { "futureBlock": { "value": 1 } },
                        { "toolUse": { "name": "load_tools", "toolUseId": "t-0", "input": {} } }
                    ]
                },
                "completedToolResults": { "t-0": { "toolResult": {
                    "toolUseId": "t-0", "status": "success", "content": [{ "text": "loaded" }, { "toolReference": { "name": "shell" } }]
                } } }
            }
        });
        let state: InterruptStateData = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), saved);
    }

    #[test]
    fn context_is_written_only_when_non_empty() {
        let mut state = InterruptStateData::default();
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({ "interrupts": {}, "activated": false })
        );
        state.context.insert("interruptedTurns".into(), json!({ "t": 1 }));
        state.resume_responses = Some(vec![]);
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({ "interrupts": {}, "resumeResponses": [], "activated": false, "context": { "interruptedTurns": { "t": 1 } } })
        );
    }

    #[test]
    fn interrupt_response_content_keeps_a_null_response_and_requires_the_field() {
        let content =
            InterruptResponseContent::new(InterruptResponse::new("tool:t1:confirm", JsonValue::Null));
        let value = json!({ "interruptResponse": { "interruptId": "tool:t1:confirm", "response": null } });
        assert_eq!(serde_json::to_value(&content).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<InterruptResponseContent>(value).unwrap(),
            content
        );
        let missing = serde_json::from_value::<InterruptResponse>(json!({ "interruptId": "x" }));
        assert!(missing.is_err());
    }

    #[test]
    fn interrupt_params_convert_from_a_name() {
        assert_eq!(InterruptParams::from("approve"), InterruptParams::new("approve"));
        assert_eq!(InterruptParams::from(String::from("approve")).name, "approve");
    }

    fn assert_state_round_trips(state: &JsonValue, name: &str) -> InterruptStateData {
        let parsed: InterruptStateData = serde_json::from_str(&state.to_string()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), *state, "{name}");
        parsed
    }

    #[test]
    fn interrupt_state_matches_the_conformance_fixture() {
        let fixture: JsonValue = serde_json::from_str(include_str!(
            "../../conformance/fixtures/interrupts/interrupt_state.json"
        ))
        .unwrap();
        let before = assert_state_round_trips(&fixture["beforeResume"], "beforeResume");
        assert!(
            before
                .interrupts
                .values()
                .all(|interrupt| interrupt.response.is_none())
        );
        let after = assert_state_round_trips(&fixture["afterResume"], "afterResume");
        assert_eq!(
            after.interrupts["tool:tooluse_1:confirm"].response,
            Some(JsonValue::Null)
        );
        assert_eq!(
            after.resume_responses.unwrap()[0].interrupt_response.response,
            JsonValue::Null
        );
        assert_eq!(
            serde_json::to_value(Interrupt::new("x", "legacy")).unwrap(),
            fixture["legacyWithoutSource"]
        );
    }

    #[test]
    fn snapshot_interrupt_state_matches_the_conformance_fixtures() {
        let snapshots = [
            include_str!("../../conformance/fixtures/interrupts/hook_interrupted.snapshot.json"),
            include_str!("../../conformance/fixtures/interrupts/tool_interrupted.snapshot.json"),
            include_str!("../../conformance/fixtures/interrupts/tool_resumed.snapshot.json"),
            include_str!("../../conformance/fixtures/snapshots/take_snapshot_empty_agent.json"),
        ];
        for (index, text) in snapshots.iter().enumerate() {
            let snapshot: JsonValue = serde_json::from_str(text).unwrap();
            assert_state_round_trips(&snapshot["data"]["interrupts"], &index.to_string());
        }
        let interrupted: JsonValue = serde_json::from_str(snapshots[1]).unwrap();
        let state = assert_state_round_trips(&interrupted["data"]["interrupts"], "tool_interrupted");
        assert!(state.activated);
        assert!(state.pending_tool_execution.is_some());
    }
}
