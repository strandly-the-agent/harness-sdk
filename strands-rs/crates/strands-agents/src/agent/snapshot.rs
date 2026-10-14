//! Snapshots: explicit capture/restore of agent state (`strands-ts/src/agent/snapshot.ts`).

use serde_json::{Map, Value};

use super::Agent;
use crate::error::{Error, Result};
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
    /// Interrupt state (`interrupts`), `null` when the agent has none.
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
            SnapshotField::Messages => {
                serde_json::to_value(agent.messages()).unwrap_or(Value::Null)
            }
            SnapshotField::State => agent.state().to_json(),
            SnapshotField::SystemPrompt => match agent.system_prompt() {
                Some(prompt) => serde_json::to_value(prompt).unwrap_or(Value::Null),
                None => Value::Null,
            },
            SnapshotField::ModelState => agent.model_state().to_json(),
            // The interrupts module has not landed; an agent never has interrupt state yet.
            SnapshotField::Interrupts => Value::Null,
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

        Ok(())
    }
}
