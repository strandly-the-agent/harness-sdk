//! Taking and loading agent snapshots (TS `agent/snapshot.ts`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::interrupt::state::InterruptStateData;
use crate::types::agent::LocalAgent;
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{Message, SystemPrompt};
use crate::types::snapshot::{SNAPSHOT_SCHEMA_VERSION, Scope, Snapshot};

/// A part of the agent's state a snapshot can capture (TS `SnapshotField`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum SnapshotField {
    /// The conversation history.
    Messages,
    /// The app state.
    State,
    /// The system prompt (`null` when the agent has none).
    SystemPrompt,
    /// The model state.
    ModelState,
    /// The interrupt state.
    Interrupts,
}

impl SnapshotField {
    /// Every field, in TS order (TS `ALL_SNAPSHOT_FIELDS`).
    pub const ALL: [Self; 5] = [
        Self::Messages,
        Self::State,
        Self::SystemPrompt,
        Self::ModelState,
        Self::Interrupts,
    ];

    /// The TS field name, which is also the key in [`Snapshot::data`].
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::State => "state",
            Self::SystemPrompt => "systemPrompt",
            Self::ModelState => "modelState",
            Self::Interrupts => "interrupts",
        }
    }
}

impl fmt::Display for SnapshotField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SnapshotField {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|field| field.as_str() == name)
            .ok_or_else(|| {
                let valid: Vec<&str> = Self::ALL.iter().map(|field| field.as_str()).collect();
                Error::other(format!(
                    "Invalid snapshot field: {name}. Valid fields are: {}",
                    valid.join(", ")
                ))
            })
    }
}

/// A named starting set of fields (TS `SnapshotPreset`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum SnapshotPreset {
    /// Everything a session restores: all five fields.
    Session,
}

impl SnapshotPreset {
    /// Every preset (the keys of TS `SNAPSHOT_PRESETS`).
    pub const ALL: [Self; 1] = [Self::Session];

    /// The TS preset name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
        }
    }

    /// The fields the preset captures (TS `SNAPSHOT_PRESETS[preset]`).
    pub fn fields(self) -> &'static [SnapshotField] {
        match self {
            Self::Session => &SnapshotField::ALL,
        }
    }
}

impl fmt::Display for SnapshotPreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SnapshotPreset {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.as_str() == name)
            .ok_or_else(|| {
                let valid: Vec<&str> = Self::ALL.iter().map(|preset| preset.as_str()).collect();
                Error::other(format!(
                    "Invalid preset: {name}. Valid presets are: {}",
                    valid.join(", ")
                ))
            })
    }
}

/// What [`Agent::take_snapshot`] captures (TS `TakeSnapshotOptions`): the preset's fields, plus
/// `include`, minus `exclude`.
///
/// ```
/// use strands::agent::{SnapshotField, SnapshotPreset, TakeSnapshotOptions};
///
/// let options = TakeSnapshotOptions::builder()
///     .preset(SnapshotPreset::Session)
///     .exclude(vec![SnapshotField::Interrupts])
///     .build();
/// assert!(options.include.is_empty());
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct TakeSnapshotOptions {
    /// The starting set of fields; none when unset.
    pub preset: Option<SnapshotPreset>,
    /// Fields added to the preset's.
    #[builder(default)]
    pub include: Vec<SnapshotField>,
    /// Fields removed after `include` is applied.
    #[builder(default)]
    pub exclude: Vec<SnapshotField>,
    /// Application-owned data stored as is in [`Snapshot::app_data`].
    #[builder(default)]
    pub app_data: JsonObject,
}

impl TakeSnapshotOptions {
    /// The `session` preset (TS `{ preset: 'session' }`).
    pub fn session() -> Self {
        Self {
            preset: Some(SnapshotPreset::Session),
            ..Self::default()
        }
    }
}

/// TS `resolveSnapshotFields`: the preset, plus `include`, minus `exclude`, in TS order.
pub(crate) fn resolve_snapshot_fields(options: &TakeSnapshotOptions) -> Result<Vec<SnapshotField>> {
    let preset = options.preset.map_or(&[][..], SnapshotPreset::fields);
    let fields: Vec<SnapshotField> = preset
        .iter()
        .chain(&options.include)
        .fold(Vec::new(), |mut fields, field| {
            if !fields.contains(field) {
                fields.push(*field);
            }
            fields
        })
        .into_iter()
        .filter(|field| !options.exclude.contains(field))
        .collect();
    if fields.is_empty() {
        return Err(Error::other(
            "No fields to include in snapshot. Specify a preset or include fields.",
        ));
    }
    Ok(fields)
}

/// The parsed parts of a snapshot's data; parsing happens before anything is applied.
#[derive(Default)]
struct Restore {
    messages: Option<Vec<Message>>,
    state: Option<JsonValue>,
    system_prompt: Option<Option<SystemPrompt>>,
    model_state: Option<JsonValue>,
    interrupts: Option<InterruptStateData>,
}

impl Restore {
    fn parse(data: &JsonObject) -> Result<Self> {
        let messages = data
            .get("messages")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(Error::other)?;
        let system_prompt = data
            .get("systemPrompt")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(Error::other)?;
        let interrupts = data
            .get("interrupts")
            .filter(|value| !value.is_null())
            .map(InterruptStateData::from_json)
            .transpose()?;
        Ok(Self {
            messages,
            state: data.get("state").cloned(),
            system_prompt,
            model_state: data.get("modelState").cloned(),
            interrupts,
        })
    }
}

impl LocalAgent {
    /// Captures the selected parts of the agent's state (TS `takeSnapshot`).
    ///
    /// # Errors
    ///
    /// When the options select no field.
    pub fn take_snapshot(&self, options: &TakeSnapshotOptions) -> Result<Snapshot> {
        let fields = resolve_snapshot_fields(options)?;
        let mut data = JsonObject::new();
        for field in SnapshotField::ALL
            .into_iter()
            .filter(|field| fields.contains(field))
        {
            let value = match field {
                SnapshotField::Messages => serde_json::to_value(self.messages()),
                SnapshotField::State => Ok(JsonValue::Object(self.app_state().get_all())),
                SnapshotField::SystemPrompt => serde_json::to_value(self.system_prompt()),
                SnapshotField::ModelState => Ok(JsonValue::Object(self.model_state().get_all())),
                SnapshotField::Interrupts => serde_json::to_value(&*self.interrupt_state().lock()),
            }
            .map_err(Error::other)?;
            data.insert(field.as_str().to_owned(), value);
        }
        Ok(Snapshot::new(Scope::Agent, data, options.app_data.clone()))
    }

    /// Restores the fields present in `snapshot.data`, leaving the others unchanged (TS
    /// `loadSnapshot`). A `null` system prompt clears it; a `null` interrupt state is ignored.
    ///
    /// # Errors
    ///
    /// When the scope isn't `agent`, the schema version isn't [`SNAPSHOT_SCHEMA_VERSION`], or
    /// a field doesn't parse; nothing is restored then.
    pub fn load_snapshot(&mut self, snapshot: &Snapshot) -> Result<()> {
        if snapshot.scope() != Scope::Agent {
            let scope = serde_json::to_value(snapshot.scope()).map_err(Error::other)?;
            let scope = scope.as_str().unwrap_or_default().to_owned();
            return Err(Error::other(format!(
                "Expected snapshot scope 'agent', got '{scope}'"
            )));
        }
        if snapshot.schema_version() != SNAPSHOT_SCHEMA_VERSION {
            return Err(Error::other(format!(
                "Unsupported snapshot schema version: {}. Current version: {SNAPSHOT_SCHEMA_VERSION}",
                snapshot.schema_version()
            )));
        }
        let restore = Restore::parse(snapshot.data())?;
        if let Some(messages) = restore.messages {
            self.set_messages(messages);
        }
        if let Some(state) = restore.state {
            self.app_state().load_json(state);
        }
        if let Some(system_prompt) = restore.system_prompt {
            self.set_system_prompt(system_prompt);
        }
        if let Some(model_state) = restore.model_state {
            self.model_state().load_json(model_state);
        }
        if let Some(interrupts) = restore.interrupts {
            *self.interrupt_state().lock() = interrupts;
        }
        Ok(())
    }
}

impl Agent {
    /// Captures the selected parts of the agent's state (TS `takeSnapshot`).
    ///
    /// ```
    /// use strands::agent::TakeSnapshotOptions;
    /// use strands::testing::MockMessageModel;
    /// use strands::Agent;
    ///
    /// let mut agent = Agent::builder().model(MockMessageModel::new()).build()?;
    /// agent.app_state().set("counter", 42)?;
    /// let checkpoint = agent.take_snapshot(&TakeSnapshotOptions::session())?;
    ///
    /// agent.app_state().clear();
    /// agent.load_snapshot(&checkpoint)?;
    /// assert_eq!(agent.app_state().get::<u32>("counter")?, Some(42));
    /// # Ok::<(), strands::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// When the options select no field.
    pub fn take_snapshot(&self, options: &TakeSnapshotOptions) -> Result<Snapshot> {
        self.local.take_snapshot(options)
    }

    /// Restores the fields present in `snapshot.data`, leaving the others unchanged (TS
    /// `loadSnapshot`).
    ///
    /// # Errors
    ///
    /// When the scope isn't `agent`, the schema version isn't [`SNAPSHOT_SCHEMA_VERSION`], or
    /// a field doesn't parse; nothing is restored then.
    pub fn load_snapshot(&mut self, snapshot: &Snapshot) -> Result<()> {
        let background = self.background_tasks.clone().filter(|_| self.initialized);
        if let Some(background) = &background {
            background.assert_can_load_snapshot()?;
        }
        self.local.load_snapshot(snapshot)?;
        // A dropped stream's cancel message left history with the messages just replaced.
        if let Some(run) = &mut self.abandoned
            && snapshot.data().contains_key("messages")
        {
            run.clear_repair();
        }
        if let Some(background) = background
            && snapshot.data().contains_key("state")
        {
            background.load_app_state();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
