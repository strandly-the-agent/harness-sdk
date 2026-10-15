//! Snapshots: explicit capture/restore of agent state (`strands-ts/src/agent/snapshot.ts`).

use serde_json::{Map, Value};

use super::Agent;
use crate::error::{Error, Result};
use crate::interrupt::InterruptState;
use crate::session::types::{now_iso8601, Scope, Snapshot, SNAPSHOT_SCHEMA_VERSION};
use crate::types::{JsonValue, Message, SystemPrompt};

/// A field a snapshot can carry in its `data` map.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SnapshotField {
    /// The conversation (`messages`).
    Messages,
    /// Application state (`state`).
    State,
    /// The system prompt (`systemPrompt`), `null` when the agent has none.
    SystemPrompt,
    /// Model-owned state (`modelState`).
    ModelState,
    /// Interrupt state (`interrupts`), the wire form of [`InterruptState::to_json`].
    Interrupts,
}

/// Every field a snapshot can carry.
pub const ALL_SNAPSHOT_FIELDS: [SnapshotField; 5] = [
    SnapshotField::Messages,
    SnapshotField::State,
    SnapshotField::SystemPrompt,
    SnapshotField::ModelState,
    SnapshotField::Interrupts,
];

/// A named set of snapshot fields.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SnapshotPreset {
    /// Everything needed to resume a session: all of [`ALL_SNAPSHOT_FIELDS`].
    #[default]
    Session,
}

impl SnapshotPreset {
    fn fields(self) -> &'static [SnapshotField] {
        match self {
            SnapshotPreset::Session => &ALL_SNAPSHOT_FIELDS,
        }
    }
}

/// Which fields [`Agent::take_snapshot`] captures, plus application-owned data.
///
/// Defaults to the [`SnapshotPreset::Session`] preset; `include`/`exclude` adjust it in that order.
#[derive(Clone, Debug, Default)]
pub struct TakeSnapshotOptions {
    preset: Option<SnapshotPreset>,
    include: Vec<SnapshotField>,
    exclude: Vec<SnapshotField>,
    app_data: Map<String, Value>,
}

impl TakeSnapshotOptions {
    /// The starting field set (default [`SnapshotPreset::Session`]).
    #[must_use]
    pub fn preset(mut self, preset: SnapshotPreset) -> Self {
        self.preset = Some(preset);
        self
    }

    /// Adds a field to the preset's set.
    #[must_use]
    pub fn include(mut self, field: SnapshotField) -> Self {
        self.include.push(field);
        self
    }

    /// Removes a field; no error if it was never included.
    #[must_use]
    pub fn exclude(mut self, field: SnapshotField) -> Self {
        self.exclude.push(field);
        self
    }

    /// Application-owned data stored verbatim in `appData` (Strands never reads it).
    #[must_use]
    pub fn app_data(mut self, app_data: Map<String, Value>) -> Self {
        self.app_data = app_data;
        self
    }

    /// The resolved field set: preset, then `include`, then `exclude`.
    pub fn fields(&self) -> Vec<SnapshotField> {
        let mut fields: Vec<SnapshotField> = self.preset.unwrap_or_default().fields().to_vec();
        for field in &self.include {
            if !fields.contains(field) {
                fields.push(*field);
            }
        }
        fields.retain(|field| !self.exclude.contains(field));
        fields
    }
}

impl SnapshotField {
    fn key(self) -> &'static str {
        match self {
            SnapshotField::Messages => "messages",
            SnapshotField::State => "state",
            SnapshotField::SystemPrompt => "systemPrompt",
            SnapshotField::ModelState => "modelState",
            SnapshotField::Interrupts => "interrupts",
        }
    }

    fn capture(self, agent: &Agent) -> JsonValue {
        match self {
            SnapshotField::Messages => serde_json::to_value(agent.messages()).unwrap_or(Value::Null),
            SnapshotField::State => agent.state().to_json(),
            SnapshotField::SystemPrompt => match agent.system_prompt() {
                Some(prompt) => serde_json::to_value(prompt).unwrap_or(Value::Null),
                None => Value::Null,
            },
            SnapshotField::ModelState => agent.model_state().to_json(),
            SnapshotField::Interrupts => agent.interrupt_state_shared().lock().to_json(),
        }
    }
}

impl Agent {
    /// Captures the agent's state as a `Snapshot` (`scope: "agent"`).
    pub fn take_snapshot(&self, options: TakeSnapshotOptions) -> Snapshot {
        let mut data = Map::new();
        for field in options.fields() {
            data.insert(field.key().to_owned(), field.capture(self));
        }
        Snapshot {
            scope: Scope::Agent,
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
            created_at: now_iso8601(),
            data,
            app_data: options.app_data,
        }
    }

    /// Restores a snapshot into this agent.
    ///
    /// Keys absent from `data` leave the corresponding field unchanged; a key present as `null`
    /// clears it (`systemPrompt`) or is ignored (`interrupts`).
    pub fn load_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        if snapshot.scope != Scope::Agent {
            return Err(Error::other(format!(
                "Expected snapshot scope 'agent', got '{}'",
                snapshot.scope
            )));
        }
        if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(Error::other(format!(
                "Unsupported snapshot schema version: {}. Current version: {SNAPSHOT_SCHEMA_VERSION}",
                snapshot.schema_version
            )));
        }

        if let Some(messages) = snapshot.data.get("messages") {
            let messages: Vec<Message> = serde_json::from_value(messages.clone())?;
            self.with_messages(|current| {
                current.clear();
                current.extend(messages);
            });
        }

        if let Some(state) = snapshot.data.get("state") {
            self.state().load_json(state.clone());
        }

        if let Some(prompt) = snapshot.data.get("systemPrompt") {
            if prompt.is_null() {
                self.set_system_prompt(None);
            } else {
                let prompt: SystemPrompt = serde_json::from_value(prompt.clone())?;
                self.set_system_prompt(Some(prompt));
            }
        }

        if let Some(model_state) = snapshot.data.get("modelState") {
            self.model_state().load_json(model_state.clone());
        }

        if let Some(interrupts) = snapshot.data.get("interrupts") {
            if !interrupts.is_null() {
                *self.interrupt_state_shared().lock() = InterruptState::from_json(interrupts)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_agent_for_session;
    use crate::interrupt::InterruptSource;
    use crate::types::InterruptParams;
    use serde_json::json;

    #[test]
    fn exports_snapshot_constants_with_correct_values() {
        assert_eq!(SNAPSHOT_SCHEMA_VERSION, "1.0");
        assert_eq!(
            ALL_SNAPSHOT_FIELDS.map(SnapshotField::key),
            ["messages", "state", "systemPrompt", "modelState", "interrupts"]
        );
    }

    #[test]
    fn returns_session_preset_fields_when_preset_is_session() {
        let fields = TakeSnapshotOptions::default()
            .preset(SnapshotPreset::Session)
            .fields();
        assert_eq!(fields, ALL_SNAPSHOT_FIELDS.to_vec());
    }

    #[test]
    fn returns_explicit_fields_when_include_is_specified() {
        let fields = TakeSnapshotOptions::default()
            .include(SnapshotField::Messages)
            .fields();
        assert_eq!(fields, ALL_SNAPSHOT_FIELDS.to_vec());
    }

    #[test]
    fn applies_exclude_after_preset() {
        let fields = TakeSnapshotOptions::default()
            .exclude(SnapshotField::Interrupts)
            .exclude(SnapshotField::ModelState)
            .fields();
        assert_eq!(
            fields,
            vec![
                SnapshotField::Messages,
                SnapshotField::State,
                SnapshotField::SystemPrompt
            ]
        );
    }

    #[test]
    fn creates_snapshot_with_session_preset() {
        let agent = test_agent_for_session("a");
        agent.set_messages(vec![Message::user("hello")]);
        agent.state().set("k", json!("v"));
        agent.model_state().set("m", json!(1));

        let snapshot = agent.take_snapshot(TakeSnapshotOptions::default());

        assert_eq!(snapshot.scope, Scope::Agent);
        assert_eq!(snapshot.schema_version, "1.0");
        let messages = snapshot.data["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["content"], json!([{ "text": "hello" }]));
        assert_eq!(snapshot.data["state"], json!({ "k": "v" }));
        assert_eq!(snapshot.data["systemPrompt"], json!("Test prompt"));
        assert_eq!(snapshot.data["modelState"], json!({ "m": 1 }));
        assert_eq!(
            snapshot.data["interrupts"],
            json!({ "interrupts": {}, "activated": false })
        );
        assert!(snapshot.app_data.is_empty());
    }

    #[test]
    fn includes_app_data_in_snapshot() {
        let agent = test_agent_for_session("a");
        let mut app_data = Map::new();
        app_data.insert("custom".to_owned(), json!(7));

        let snapshot = agent.take_snapshot(TakeSnapshotOptions::default().app_data(app_data));

        assert_eq!(snapshot.app_data["custom"], json!(7));
    }

    #[test]
    fn excludes_specified_fields() {
        let agent = test_agent_for_session("a");

        let snapshot = agent.take_snapshot(TakeSnapshotOptions::default().exclude(SnapshotField::Messages));

        assert!(!snapshot.data.contains_key("messages"));
        assert!(snapshot.data.contains_key("state"));
    }

    #[test]
    fn throws_error_for_incompatible_schema_version() {
        let agent = test_agent_for_session("a");
        let snapshot = Snapshot {
            schema_version: "2.0".to_owned(),
            ..agent.take_snapshot(TakeSnapshotOptions::default())
        };

        assert_eq!(
            agent.load_snapshot(&snapshot).unwrap_err().to_string(),
            "Unsupported snapshot schema version: 2.0. Current version: 1.0"
        );
    }

    #[test]
    fn throws_error_for_wrong_scope() {
        let agent = test_agent_for_session("a");
        let snapshot = Snapshot {
            scope: Scope::MultiAgent,
            ..agent.take_snapshot(TakeSnapshotOptions::default())
        };

        assert_eq!(
            agent.load_snapshot(&snapshot).unwrap_err().to_string(),
            "Expected snapshot scope 'agent', got 'multiAgent'"
        );
    }

    #[test]
    fn take_snapshot_captures_state_and_load_snapshot_restores_it() {
        let source = test_agent_for_session("a");
        source.set_messages(vec![Message::assistant("hi")]);
        source.state().set("count", json!(2));
        source.model_state().set("cursor", json!("abc"));
        let snapshot = source.take_snapshot(TakeSnapshotOptions::default());

        let target = test_agent_for_session("b");
        target.load_snapshot(&snapshot).unwrap();

        assert_eq!(target.messages(), source.messages());
        assert_eq!(target.state().to_json(), json!({ "count": 2 }));
        assert_eq!(target.model_state().to_json(), json!({ "cursor": "abc" }));
        assert_eq!(target.system_prompt(), source.system_prompt());
    }

    #[test]
    fn clears_system_prompt_when_snapshot_has_null_system_prompt() {
        let agent = test_agent_for_session("a");
        let mut snapshot = agent.take_snapshot(TakeSnapshotOptions::default());
        snapshot.data.insert("systemPrompt".to_owned(), Value::Null);

        agent.load_snapshot(&snapshot).unwrap();

        assert_eq!(agent.system_prompt(), None);
    }

    #[test]
    fn leaves_fields_unchanged_when_their_keys_are_absent_from_snapshot() {
        let agent = test_agent_for_session("a");
        agent.set_messages(vec![Message::user("keep")]);
        agent.state().set("keep", json!(true));
        let snapshot = Snapshot {
            data: Map::new(),
            ..agent.take_snapshot(TakeSnapshotOptions::default())
        };

        agent.load_snapshot(&snapshot).unwrap();

        assert_eq!(agent.messages().len(), 1);
        assert_eq!(agent.messages()[0].text(), "keep");
        assert_eq!(agent.state().to_json(), json!({ "keep": true }));
        assert!(agent.system_prompt().is_some());
    }

    #[test]
    fn captures_and_restores_interrupt_state() {
        let agent = test_agent_for_session("a");
        {
            let shared = agent.interrupt_state_shared();
            let mut state = shared.lock();
            state.get_or_create(
                "tool:call-1:confirm",
                &InterruptParams::new("confirm").with_reason("ok?"),
                InterruptSource::Tool,
            );
            state.activate();
        }
        let snapshot = agent.take_snapshot(TakeSnapshotOptions::default());
        assert_eq!(snapshot.data["interrupts"]["activated"], json!(true));
        assert_eq!(
            snapshot.data["interrupts"]["interrupts"]["tool:call-1:confirm"]["name"],
            json!("confirm")
        );

        let target = test_agent_for_session("b");
        target.load_snapshot(&snapshot).unwrap();

        assert!(target.interrupt_state().is_activated());
        let restored = target.interrupt_state().interrupts();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, "tool:call-1:confirm");
        assert_eq!(restored[0].reason, Some(json!("ok?")));
        assert_eq!(restored[0].source, InterruptSource::Tool);
    }

    #[test]
    fn leaves_interrupt_state_alone_when_the_snapshot_carries_null() {
        let agent = test_agent_for_session("a");
        agent.interrupt_state_shared().lock().activate();
        let mut snapshot = agent.take_snapshot(TakeSnapshotOptions::default());
        snapshot.data.insert("interrupts".to_owned(), Value::Null);

        agent.load_snapshot(&snapshot).unwrap();

        assert!(agent.interrupt_state().is_activated());
    }

    #[test]
    fn snapshot_survives_json_round_trip() {
        let agent = test_agent_for_session("a");
        agent.set_messages(vec![Message::user("hello")]);
        let snapshot = agent.take_snapshot(TakeSnapshotOptions::default());

        let text = serde_json::to_string(&snapshot).unwrap();
        let parsed: Snapshot = serde_json::from_str(&text).unwrap();

        let target = test_agent_for_session("b");
        target.load_snapshot(&parsed).unwrap();
        assert_eq!(target.messages(), agent.messages());
        assert_eq!(parsed.created_at, snapshot.created_at);
    }
}
