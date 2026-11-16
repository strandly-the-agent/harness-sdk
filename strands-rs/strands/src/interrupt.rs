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

    pub(crate) fn set_response(&mut self, response: JsonValue) {
        self.response = Some(response);
    }

    pub(crate) fn clear_response(&mut self) {
        self.response = None;
    }
}

pub(crate) mod state {
    #![cfg_attr(
        not(test),
        expect(dead_code, reason = "used by interrupt resume and snapshots")
    )]

    use indexmap::IndexMap;
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Serialize};

    use super::{Interrupt, InterruptResponse, InterruptResponseContent};
    use crate::types::json::{JsonObject, JsonValue, null_as_default};
    use crate::types::messages::{ContentBlock, Message, Role, ToolResultBlock};

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

        /// Stores the responses on their interrupts (TS `InterruptState.resume`); a no-op unless
        /// activated.
        pub(crate) fn resume(&mut self, responses: &[InterruptResponse]) -> crate::Result<()> {
            if !self.activated {
                return Ok(());
            }
            for response in responses {
                let id = response.interrupt_id();
                let interrupt = self.interrupts.get_mut(id).ok_or_else(|| {
                    crate::Error::other(format!("interrupt_id=<{id}> | no interrupt found"))
                })?;
                interrupt.set_response(response.response().clone());
            }
            self.resume_responses = Some(
                responses
                    .iter()
                    .cloned()
                    .map(InterruptResponseContent::from)
                    .collect(),
            );
            Ok(())
        }

        /// Clears the interrupts, responses, pending execution and context (TS `deactivate`).
        pub(crate) fn deactivate(&mut self) {
            *self = Self::default();
        }

        /// Restores persisted state (TS `InterruptState.fromJSON`): a pending tool execution is
        /// kept only when activated, and then it must be resumable.
        pub(crate) fn from_json(value: &JsonValue) -> crate::Result<Self> {
            let mut value = value.clone();
            let pending = value
                .as_object_mut()
                .and_then(|map| map.remove("pendingToolExecution"));
            let mut state: Self = serde_json::from_value(value).map_err(crate::Error::other)?;
            if let Some(pending) = pending.filter(|pending| state.activated && !pending.is_null()) {
                state.pending_tool_execution = Some(restore_pending(&pending)?);
            }
            Ok(state)
        }

        /// The paused batch to resume, when activated (TS `getPendingExecution`).
        pub(crate) fn pending_execution(&self) -> Option<PendingToolExecution> {
            self.pending_tool_execution.clone().filter(|_| self.activated)
        }
    }

    /// A tool batch paused by an interrupt (TS `PendingToolExecution`).
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct PendingToolExecution {
        pub(crate) assistant_message_data: Message,
        pub(crate) completed_tool_results: IndexMap<String, ToolResultEntry>,
    }

    /// TS `validatePendingToolExecution`: an assistant message with tool uses, and completed
    /// results keyed by tool use ids from that message.
    fn restore_pending(pending: &JsonValue) -> crate::Result<PendingToolExecution> {
        let message: Message = parse_restored(
            "field=<pendingToolExecution.assistantMessageData>",
            &pending["assistantMessageData"],
        )?;
        if message.role() != Role::Assistant {
            return Err(session_error(
                "field=<pendingToolExecution.assistantMessageData.role> | pending tool execution message must be an assistant message",
            ));
        }
        let tool_use_ids: Vec<&str> = message
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tool_use) => Some(tool_use.tool_use_id()),
                _ => None,
            })
            .collect();
        if tool_use_ids.is_empty() {
            return Err(session_error(
                "field=<pendingToolExecution.assistantMessageData.content> | pending tool execution message has no tool uses",
            ));
        }
        let Some(results) = pending["completedToolResults"].as_object() else {
            return Err(session_error(
                "field=<pendingToolExecution.completedToolResults> | completed tool results must be an object",
            ));
        };
        let mut completed_tool_results = IndexMap::new();
        for (tool_use_id, result) in results {
            let field =
                format!("field=<pendingToolExecution.completedToolResults>, tool_use_id=<{tool_use_id}>");
            if !tool_use_ids.contains(&tool_use_id.as_str()) {
                return Err(session_error(format!(
                    "{field} | does not match a tool use in the pending assistant message"
                )));
            }
            completed_tool_results.insert(tool_use_id.clone(), parse_restored(&field, result)?);
        }
        Ok(PendingToolExecution {
            assistant_message_data: message,
            completed_tool_results,
        })
    }

    fn parse_restored<T: DeserializeOwned>(field: &str, value: &JsonValue) -> crate::Result<T> {
        serde_json::from_value(value.clone()).map_err(|error| {
            crate::Error::other(RestoreError {
                message: format!("{field} | failed to parse restored pending tool execution"),
                source: error,
            })
        })
    }

    fn session_error(message: impl Into<String>) -> crate::Error {
        crate::Error::other(message.into())
    }

    /// A restore failure keeping the parse error as its source (TS `SessionError` with `cause`).
    #[derive(Debug, thiserror::Error)]
    #[error("{message}")]
    struct RestoreError {
        message: String,
        source: serde_json::Error,
    }

    /// A completed sibling result, written as `{"toolResult": ..}`.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct ToolResultEntry {
        pub(crate) tool_result: ToolResultBlock,
    }
}

/// Ported from strands-ts/src/__tests__/interrupt.test.ts.
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

    mod restore {
        use super::*;

        fn pending_json() -> JsonValue {
            serde_json::to_value(pending_tool_execution()).unwrap()
        }

        #[expect(clippy::needless_pass_by_value, reason = "test helper over json! literals")]
        fn restore(pending: JsonValue) -> crate::Result<InterruptStateData> {
            InterruptStateData::from_json(&json!({
                "interrupts": {}, "activated": true, "pendingToolExecution": pending,
            }))
        }

        #[test]
        fn restores_a_pending_tool_execution_a_newer_sdk_saved_with_content_block_types_this_version_does_not_know()
         {
            let saved = json!({
                "assistantMessageData": {
                    "role": "assistant",
                    "content": [
                        {"futureBlock": {"value": 1}},
                        {"toolUse": {"name": "load_tools", "toolUseId": "t-0", "input": {}}},
                        {"toolUse": {"name": "tool", "toolUseId": "t-1", "input": {}}},
                    ],
                    "trackingId": "pending-1",
                },
                "completedToolResults": {
                    "t-0": {"toolResult": {
                        "toolUseId": "t-0",
                        "status": "success",
                        "content": [{"text": "loaded"}, {"toolReference": {"name": "shell"}}],
                    }},
                },
            });
            let state = restore(saved.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&state).unwrap()["pendingToolExecution"],
                saved
            );
            let pending = state.pending_execution().unwrap();
            assert!(matches!(
                pending.assistant_message_data.content()[0],
                ContentBlock::Unknown(_)
            ));
            assert_eq!(pending.assistant_message_data.content().len(), 3);
            assert_eq!(
                pending.completed_tool_results["t-0"].tool_result.content().len(),
                2
            );
        }

        #[test]
        fn drops_a_pending_tool_execution_when_the_interrupt_state_is_not_activated() {
            let state = InterruptStateData::from_json(&json!({
                "interrupts": {}, "activated": false, "pendingToolExecution": pending_json(),
            }))
            .unwrap();
            assert_eq!(state.pending_tool_execution, None);
        }

        #[test]
        fn throws_session_error_when_the_restored_pending_tool_execution_is_malformed() {
            let pending = pending_json();
            let mut user = pending.clone();
            user["assistantMessageData"]["role"] = json!("user");
            let mut not_a_list = pending.clone();
            not_a_list["assistantMessageData"] = json!({"role": "assistant", "content": "not-a-list"});
            let no_tool_uses = json!({
                "assistantMessageData": {"role": "assistant", "content": [{"text": "hello"}]},
                "completedToolResults": {},
            });
            let mut array_results = pending.clone();
            array_results["completedToolResults"] = json!(["t-0"]);
            let mut unknown_result = pending.clone();
            unknown_result["completedToolResults"] =
                json!({"t-9": {"toolResult": {"toolUseId": "t-9", "status": "success", "content": []}}});
            let mut unparseable_result = pending;
            unparseable_result["completedToolResults"] =
                json!({"tool-a": {"toolResult": {"toolUseId": "tool-a"}}});
            let cases = [
                (user, "field=<pendingToolExecution.assistantMessageData.role>"),
                (not_a_list, "field=<pendingToolExecution.assistantMessageData>"),
                (
                    no_tool_uses,
                    "field=<pendingToolExecution.assistantMessageData.content>",
                ),
                (array_results, "field=<pendingToolExecution.completedToolResults>"),
                (
                    unknown_result,
                    "field=<pendingToolExecution.completedToolResults>, tool_use_id=<t-9>",
                ),
                (
                    unparseable_result,
                    "field=<pendingToolExecution.completedToolResults>, tool_use_id=<tool-a>",
                ),
            ];
            for (pending, message) in cases {
                let error = restore(pending).unwrap_err().to_string();
                assert!(error.starts_with(&format!("{message} | ")), "{error}");
            }
        }

        #[test]
        fn keeps_the_parse_error_as_the_cause_when_the_restored_pending_assistant_message_is_unparseable() {
            let error = restore(json!({
                "assistantMessageData": {"role": "assistant", "content": "not-a-list"},
                "completedToolResults": {},
            }))
            .unwrap_err();
            let crate::Error::Other(inner) = &error else {
                panic!("a plain error")
            };
            assert!(
                inner
                    .source()
                    .is_some_and(<dyn std::error::Error>::is::<serde_json::Error>)
            );
        }
    }

    /// The `InterruptState` and `interruptFromAgent` cases; TS `getOrCreateInterrupt` and
    /// `interruptFromAgent` are one call here, `InterruptState::interrupt`.
    mod interrupt_state {
        use super::*;
        use crate::types::agent::InterruptState;

        /// TS `getOrCreateInterrupt` with the default `hook` source; returns the stored interrupt.
        fn get_or_create(
            state: &InterruptState,
            id: &str,
            name: &str,
            reason: Option<JsonValue>,
            response: Option<JsonValue>,
        ) -> Interrupt {
            let params = InterruptParams {
                name: name.to_owned(),
                reason,
                response,
            };
            let _ = state.interrupt(id.to_owned(), params, InterruptSource::Hook);
            stored(state, id)
        }

        fn stored(state: &InterruptState, id: &str) -> Interrupt {
            state.lock().interrupts[id].clone()
        }

        fn respond(state: &InterruptState, id: &str, response: JsonValue) {
            state
                .lock()
                .interrupts
                .get_mut(id)
                .unwrap()
                .set_response(response);
        }

        fn activate(state: &InterruptState) {
            state.lock().activated = true;
        }

        fn tool_params(reason: Option<JsonValue>, response: Option<JsonValue>) -> InterruptParams {
            InterruptParams {
                name: "confirm".to_owned(),
                reason,
                response,
            }
        }

        mod get_or_create_interrupt {
            use super::*;

            #[test]
            fn creates_new_interrupt_and_stores_it() {
                let state = InterruptState::default();

                let interrupt = get_or_create(&state, "int-1", "test", Some(json!("reason")), None);

                assert_eq!(
                    serde_json::to_value(&interrupt).unwrap(),
                    json!({"id": "int-1", "name": "test", "reason": "reason", "source": "hook"})
                );
                let listed: Vec<Interrupt> = state.lock().interrupts.values().cloned().collect();
                assert_eq!(listed, [interrupt]);
            }

            #[test]
            fn returns_existing_interrupt_by_id_without_overwriting() {
                let state = InterruptState::default();
                get_or_create(&state, "int-1", "test", Some(json!("reason")), None);
                respond(&state, "int-1", json!("user response"));

                let second = get_or_create(
                    &state,
                    "int-1",
                    "different",
                    Some(json!("different reason")),
                    None,
                );

                assert_eq!(second.name(), "test");
                assert_eq!(second.reason(), Some(&json!("reason")));
                assert_eq!(second.response(), Some(&json!("user response")));
                assert_eq!(state.lock().interrupts.len(), 1);
            }

            #[test]
            fn creates_separate_interrupts_for_different_ids_with_same_name() {
                let state = InterruptState::default();
                activate(&state);
                get_or_create(
                    &state,
                    "tool:tool-1:0:confirm",
                    "confirm",
                    Some(json!("reason")),
                    None,
                );
                respond(&state, "tool:tool-1:0:confirm", json!({"approved": true}));

                let second = get_or_create(
                    &state,
                    "tool:tool-2:0:confirm",
                    "confirm",
                    Some(json!("reason")),
                    None,
                );

                assert_eq!(second.id(), "tool:tool-2:0:confirm");
                assert_eq!(second.response(), None);
                assert_eq!(state.lock().interrupts.len(), 2);
            }

            #[test]
            fn creates_interrupt_with_preemptive_response() {
                let state = InterruptState::default();

                let interrupt = get_or_create(
                    &state,
                    "int-1",
                    "confirm",
                    Some(json!("reason")),
                    Some(json!("pre-approved")),
                );

                assert_eq!(
                    serde_json::to_value(&interrupt).unwrap(),
                    json!({
                        "id": "int-1",
                        "name": "confirm",
                        "reason": "reason",
                        "response": "pre-approved",
                        "source": "hook",
                    })
                );
            }

            #[test]
            fn ignores_preemptive_response_when_interrupt_already_exists() {
                let state = InterruptState::default();
                get_or_create(&state, "int-1", "confirm", Some(json!("reason")), None);
                respond(&state, "int-1", json!("user response"));

                let second = get_or_create(
                    &state,
                    "int-1",
                    "confirm",
                    Some(json!("reason")),
                    Some(json!("preemptive")),
                );

                assert_eq!(
                    serde_json::to_value(&second).unwrap(),
                    json!({
                        "id": "int-1",
                        "name": "confirm",
                        "reason": "reason",
                        "response": "user response",
                        "source": "hook",
                    })
                );
            }
        }

        mod activate_deactivate {
            use super::*;

            #[test]
            fn deactivate_clears_all_state() {
                let state = InterruptState::default();
                get_or_create(&state, "int-1", "test", None, None);
                activate(&state);
                assert!(state.lock().activated);

                state.lock().deactivate();

                assert_eq!(*state.lock(), InterruptStateData::default());
                assert!(state.lock().interrupts.is_empty());
                assert_eq!(state.lock().resume_responses, None);
                assert!(!state.lock().activated);
            }
        }

        mod resume {
            use super::*;

            #[test]
            fn does_nothing_when_not_activated() {
                let state = InterruptState::default();
                get_or_create(&state, "int-1", "test", None, None);

                state
                    .lock()
                    .resume(&[InterruptResponse::new("int-1", json!("yes"))])
                    .unwrap();

                assert_eq!(stored(&state, "int-1").response(), None);
            }

            #[test]
            fn populates_interrupt_responses_and_stores_resume_responses_when_activated() {
                let state = InterruptState::default();
                get_or_create(&state, "int-1", "first", None, None);
                get_or_create(&state, "int-2", "second", None, None);
                activate(&state);
                let responses = [
                    InterruptResponse::new("int-1", json!("response1")),
                    InterruptResponse::new("int-2", json!({"complex": "data"})),
                ];

                state.lock().resume(&responses).unwrap();

                assert_eq!(stored(&state, "int-1").response(), Some(&json!("response1")));
                assert_eq!(
                    stored(&state, "int-2").response(),
                    Some(&json!({"complex": "data"}))
                );
                let expected: Vec<InterruptResponseContent> = responses
                    .into_iter()
                    .map(InterruptResponseContent::from)
                    .collect();
                assert_eq!(state.lock().resume_responses, Some(expected));
            }

            #[test]
            fn throws_error_for_unknown_interrupt_id() {
                let state = InterruptState::default();
                get_or_create(&state, "int-1", "test", None, None);
                activate(&state);

                let error = state
                    .lock()
                    .resume(&[InterruptResponse::new("unknown", json!("yes"))])
                    .unwrap_err();

                assert_eq!(error.to_string(), "interrupt_id=<unknown> | no interrupt found");
            }
        }

        mod get_pending_execution {
            use super::*;

            #[test]
            fn returns_undefined_when_the_interrupt_state_is_not_activated() {
                let state = InterruptState::default();
                state.lock().pending_tool_execution = Some(pending_tool_execution());

                assert_eq!(state.lock().pending_execution(), None);
            }

            #[test]
            fn returns_the_reconstructed_pending_execution_when_the_interrupt_state_is_activated() {
                let state = InterruptState::default();
                activate(&state);
                state.lock().pending_tool_execution = Some(pending_tool_execution());

                let pending = state.lock().pending_execution().unwrap();

                assert_eq!(pending, pending_tool_execution());
                assert_eq!(
                    pending.completed_tool_results.keys().collect::<Vec<_>>(),
                    ["tool-b"]
                );
            }
        }

        mod interrupt_from_agent {
            use super::*;

            #[test]
            fn returns_preemptive_response_immediately_without_throwing() {
                let state = InterruptState::default();

                let result = state.interrupt(
                    "test-id".to_owned(),
                    tool_params(Some(json!("need approval")), Some(json!("pre-approved"))),
                    InterruptSource::Tool,
                );

                assert_eq!(result.unwrap(), json!("pre-approved"));
                assert_eq!(
                    serde_json::to_value(stored(&state, "test-id")).unwrap(),
                    json!({
                        "id": "test-id",
                        "name": "confirm",
                        "reason": "need approval",
                        "response": "pre-approved",
                        "source": "tool",
                    })
                );
            }

            #[test]
            fn returns_resume_response_over_preemptive_response_for_existing_interrupt() {
                let state = InterruptState::default();
                get_or_create(&state, "test-id", "confirm", Some(json!("need approval")), None);
                respond(&state, "test-id", json!("user-provided"));

                let result = state.interrupt(
                    "test-id".to_owned(),
                    tool_params(Some(json!("need approval")), Some(json!("preemptive"))),
                    InterruptSource::Tool,
                );

                assert_eq!(result.unwrap(), json!("user-provided"));
                let interrupt = stored(&state, "test-id");
                assert_eq!(interrupt.name(), "confirm");
                assert_eq!(interrupt.reason(), Some(&json!("need approval")));
                assert_eq!(interrupt.response(), Some(&json!("user-provided")));
            }

            #[test]
            fn does_not_interrupt_when_preemptive_response_is_null() {
                let state = InterruptState::default();

                let result = state.interrupt(
                    "test-id".to_owned(),
                    tool_params(None, Some(JsonValue::Null)),
                    InterruptSource::Tool,
                );

                assert_eq!(result.unwrap(), JsonValue::Null);
                let interrupt = stored(&state, "test-id");
                assert_eq!(interrupt.name(), "confirm");
                assert_eq!(interrupt.response(), Some(&JsonValue::Null));
            }
        }
    }
}
