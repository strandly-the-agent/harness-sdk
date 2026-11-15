//! Interrupts (TS `src/interrupt.ts` and `types/interrupt.ts`).
//!
//! A tool or hook raises an interrupt to pause the agent for a human answer; the caller resumes by
//! invoking again with [`InterruptResponse`]s. Interrupt state is persisted in snapshots.

use serde::{Deserialize, Deserializer, Serialize};

use crate::types::json::{JsonValue, present_null};

/// Parameters for raising an interrupt (TS `InterruptParams`).
#[derive(Debug, Clone, PartialEq, Default, bon::Builder)]
#[builder(on(String, into))]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct InterruptParams {
    /// The interrupt name, unique within the tool or hook that raises it.
    pub name: String,
    /// Why the interrupt was raised, shown to the caller.
    pub reason: Option<JsonValue>,
    /// A preemptive response; when set, the interrupt returns it instead of pausing.
    pub response: Option<JsonValue>,
}

impl InterruptParams {
    /// Parameters with just a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
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

/// The caller's answer to an interrupt (TS `InterruptResponse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InterruptResponse {
    interrupt_id: String,
    response: JsonValue,
}

impl InterruptResponse {
    /// An answer to the interrupt with this id; `JsonValue::Null` is a valid answer.
    pub fn new(interrupt_id: impl Into<String>, response: JsonValue) -> Self {
        Self {
            interrupt_id: interrupt_id.into(),
            response,
        }
    }

    /// The id of the interrupt this answers.
    pub fn interrupt_id(&self) -> &str {
        &self.interrupt_id
    }

    /// The answer.
    pub fn response(&self) -> &JsonValue {
        &self.response
    }
}

/// An interrupt response as invocation content, `{"interruptResponse": ..}`
/// (TS `InterruptResponseContent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InterruptResponseContent {
    interrupt_response: InterruptResponse,
}

impl InterruptResponseContent {
    /// Wraps a response.
    pub fn new(interrupt_response: InterruptResponse) -> Self {
        Self { interrupt_response }
    }

    /// The wrapped response.
    pub fn interrupt_response(&self) -> &InterruptResponse {
        &self.interrupt_response
    }
}

impl From<InterruptResponse> for InterruptResponseContent {
    fn from(interrupt_response: InterruptResponse) -> Self {
        Self::new(interrupt_response)
    }
}

open_enum! {
    /// What raised an interrupt (TS `InterruptSource`).
    pub enum InterruptSource / UnknownInterruptSource {
        /// A tool.
        Tool => "tool",
        /// A hook.
        Hook => "hook",
        /// A middleware.
        Middleware => "middleware",
        /// A multi-agent hook.
        MultiagentHook => "multiagent-hook",
    }
}

/// A raised interrupt (TS `Interrupt`).
///
/// A present `null` `reason` or `response` is kept as `Some(JsonValue::Null)`: a `null` response
/// means "answered with null", not "unanswered".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Interrupt {
    id: String,
    name: String,
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    reason: Option<JsonValue>,
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    response: Option<JsonValue>,
    #[serde(default = "default_source", deserialize_with = "source_or_hook")]
    source: InterruptSource,
}

fn default_source() -> InterruptSource {
    InterruptSource::Hook
}

/// TS reads a missing or `null` source as `hook` (`data.source ?? 'hook'`).
fn source_or_hook<'de, D: Deserializer<'de>>(deserializer: D) -> Result<InterruptSource, D::Error> {
    Option::deserialize(deserializer).map(|source| source.unwrap_or_else(default_source))
}

impl Interrupt {
    /// An interrupt raised by a hook (the TS default source).
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            reason: None,
            response: None,
            source: default_source(),
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

    /// Sets what raised the interrupt.
    #[must_use]
    pub fn with_source(mut self, source: InterruptSource) -> Self {
        self.source = source;
        self
    }

    /// The interrupt id (for example `tool:<toolUseId>:<name>`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The interrupt name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Why it was raised.
    pub fn reason(&self) -> Option<&JsonValue> {
        self.reason.as_ref()
    }

    /// The answer, once given.
    pub fn response(&self) -> Option<&JsonValue> {
        self.response.as_ref()
    }

    /// What raised it.
    pub fn source(&self) -> &InterruptSource {
        &self.source
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "used by interrupt resume"))]
    pub(crate) fn set_response(&mut self, response: JsonValue) {
        self.response = Some(response);
    }
}

pub(crate) mod state {
    #![cfg_attr(
        not(test),
        expect(dead_code, reason = "used by interrupt resume and snapshots")
    )]

    use indexmap::IndexMap;
    use serde::{Deserialize, Serialize};

    use super::{Interrupt, InterruptResponseContent};
    use crate::types::json::{JsonObject, null_as_default};
    use crate::types::messages::{Message, ToolResultBlock};

    /// Persisted interrupt state (TS `InterruptStateData`), the `interrupts` entry of a snapshot.
    #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct InterruptStateData {
        pub(crate) interrupts: IndexMap<String, Interrupt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub(crate) resume_responses: Option<Vec<InterruptResponseContent>>,
        pub(crate) activated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub(crate) pending_tool_execution: Option<PendingToolExecution>,
        #[serde(
            default,
            deserialize_with = "null_as_default",
            skip_serializing_if = "JsonObject::is_empty"
        )]
        pub(crate) context: JsonObject,
    }

    impl InterruptStateData {
        /// Interrupts with no answer yet; a `null` answer counts as answered.
        pub(crate) fn unanswered(&self) -> impl Iterator<Item = &Interrupt> {
            self.interrupts
                .values()
                .filter(|interrupt| interrupt.response().is_none())
        }
    }

    /// A tool batch paused by an interrupt (TS `PendingToolExecution`).
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct PendingToolExecution {
        pub(crate) assistant_message_data: Message,
        pub(crate) completed_tool_results: IndexMap<String, ToolResultEntry>,
    }

    /// A completed sibling result, written as `{"toolResult": ..}`.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct ToolResultEntry {
        pub(crate) tool_result: ToolResultBlock,
    }
}

/// Ported from strands-ts/src/__tests__/interrupt.test.ts (the type and serialization parts).
#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use serde_json::json;

    use super::state::{InterruptStateData, PendingToolExecution, ToolResultEntry};
    use super::*;
    use crate::types::messages::{
        ContentBlock, Message, Role, ToolResultBlock, ToolResultContent, ToolUseBlock,
    };
    use crate::types::tools::ToolResultStatus;

    mod interrupt {
        use super::*;

        #[test]
        fn constructs_with_all_fields_and_supports_response_mutation() {
            let mut interrupt = Interrupt::new("int-1", "confirm_action")
                .with_reason(json!("Please confirm"))
                .with_response(json!("approved"));
            assert_eq!(
                serde_json::to_value(&interrupt).unwrap(),
                json!({"id": "int-1", "name": "confirm_action", "reason": "Please confirm", "response": "approved", "source": "hook"})
            );
            interrupt.set_response(json!("changed"));
            assert_eq!(interrupt.response(), Some(&json!("changed")));
        }

        #[test]
        fn round_trips_through_json_serialization_with_complex_data() {
            let original = Interrupt::new("int-1", "test")
                .with_reason(json!({"complex": {"nested": "data"}}))
                .with_response(json!(["array", "response"]));
            let text = serde_json::to_string(&original).unwrap();
            assert_eq!(serde_json::from_str::<Interrupt>(&text).unwrap(), original);
        }

        #[test]
        fn omits_undefined_reason_response_from_to_json() {
            let json = serde_json::to_value(Interrupt::new("int-1", "test")).unwrap();
            assert_eq!(json, json!({"id": "int-1", "name": "test", "source": "hook"}));
        }
    }

    fn pending_tool_execution() -> PendingToolExecution {
        let assistant = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::from(ToolUseBlock::new("a", "tool-a", json!({}))),
                ToolUseBlock::new("b", "tool-b", json!({"x": 1})).into(),
            ],
        )
        .with_tracking_id("pending-1");
        let result = ToolResultBlock::new(
            "tool-b",
            ToolResultStatus::Success,
            vec![ToolResultContent::from("done")],
        );
        PendingToolExecution {
            assistant_message_data: assistant,
            completed_tool_results: IndexMap::from([(
                "tool-b".to_owned(),
                ToolResultEntry { tool_result: result },
            )]),
        }
    }

    mod serialization {
        use super::*;

        fn round_trip(state: &InterruptStateData) -> InterruptStateData {
            serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap()
        }

        #[test]
        fn round_trips_through_json_with_full_state() {
            let interrupt = Interrupt::new("int-1", "test")
                .with_reason(json!({"complex": "reason"}))
                .with_response(json!(["array", "response"]));
            let original = InterruptStateData {
                interrupts: IndexMap::from([("int-1".to_owned(), interrupt)]),
                activated: true,
                ..InterruptStateData::default()
            };
            let restored = round_trip(&original);
            assert_eq!(
                serde_json::to_value(&restored).unwrap(),
                serde_json::to_value(&original).unwrap()
            );
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
            assert_eq!(
                serde_json::to_value(&restored).unwrap(),
                serde_json::to_value(&original).unwrap()
            );
            assert_eq!(restored.pending_tool_execution, original.pending_tool_execution);
        }

        #[test]
        fn deserializes_state_with_resume_responses() {
            let state: InterruptStateData = serde_json::from_value(json!({
                "interrupts": {"int-1": {"id": "int-1", "name": "test", "reason": "reason", "response": "yes"}},
                "resumeResponses": [{"interruptResponse": {"interruptId": "int-1", "response": "yes"}}],
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
    }

    #[test]
    fn a_null_response_is_kept_and_counts_as_answered() {
        let state: InterruptStateData = serde_json::from_value(json!({
            "interrupts": {
                "a": {"id": "a", "name": "a", "response": null, "source": "tool"},
                "b": {"id": "b", "name": "b", "source": "tool"}
            },
            "activated": true
        }))
        .unwrap();
        assert_eq!(state.interrupts["a"].response(), Some(&JsonValue::Null));
        let unanswered: Vec<&str> = state.unanswered().map(Interrupt::id).collect();
        assert_eq!(unanswered, ["b"]);
        assert_eq!(
            serde_json::to_value(&state.interrupts["a"]).unwrap(),
            json!({"id": "a", "name": "a", "response": null, "source": "tool"})
        );
    }

    #[test]
    fn a_null_source_loads_as_hook() {
        let interrupt: Interrupt =
            serde_json::from_value(json!({"id": "a", "name": "a", "source": null})).unwrap();
        assert_eq!(interrupt, Interrupt::new("a", "a"));
        assert_eq!(interrupt.source(), &InterruptSource::Hook);
    }

    #[test]
    fn a_null_context_loads_as_empty() {
        let state: InterruptStateData = serde_json::from_value(json!({
            "interrupts": {},
            "activated": false,
            "resumeResponses": null,
            "pendingToolExecution": null,
            "context": null
        }))
        .unwrap();
        assert_eq!(state, InterruptStateData::default());
    }

    #[test]
    fn a_fresh_state_writes_interrupts_and_activated_only() {
        assert_eq!(
            serde_json::to_string(&InterruptStateData::default()).unwrap(),
            r#"{"interrupts":{},"activated":false}"#
        );
    }

    #[test]
    fn interrupt_params_build_from_a_name() {
        assert_eq!(InterruptParams::from("confirm"), InterruptParams::new("confirm"));
        let params = InterruptParams {
            reason: Some(json!("why")),
            ..InterruptParams::from("confirm".to_owned())
        };
        assert_eq!(params.name, "confirm");
        assert_eq!(params.response, None);
    }

    #[test]
    fn interrupt_params_build_with_the_builder() {
        let params = InterruptParams::builder()
            .name("confirm")
            .reason(json!("why"))
            .build();
        assert_eq!(
            params,
            InterruptParams {
                reason: Some(json!("why")),
                ..InterruptParams::new("confirm")
            }
        );
    }

    #[test]
    fn interrupt_response_content_wraps_the_response() {
        let content =
            InterruptResponseContent::from(InterruptResponse::new("tool:t1:confirm", JsonValue::Null));
        let wire = json!({"interruptResponse": {"interruptId": "tool:t1:confirm", "response": null}});
        assert_eq!(serde_json::to_value(&content).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<InterruptResponseContent>(wire).unwrap(),
            content
        );
        assert_eq!(
            InterruptSource::from("multiagent-hook"),
            InterruptSource::MultiagentHook
        );
    }
}
