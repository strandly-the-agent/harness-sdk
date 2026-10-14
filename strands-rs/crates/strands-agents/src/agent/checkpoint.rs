//! Checkpoints: pause-point markers at cycle boundaries
//! (`strands-ts/src/experimental/checkpoint.ts`).
//!
//! A checkpoint carries **no** conversation state — durability is the session manager's job.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Agent, AgentResult, InvocationState, InvokeInput};
use crate::error::{Error, Result};
use crate::types::{JsonValue, Message, StopReason};

/// Schema version of the checkpoint wire format.
pub const CHECKPOINT_SCHEMA_VERSION: &str = "1.0";

const CHECKPOINTING_DISABLED_MESSAGE: &str = "Received a checkpointResume block but the agent was created with checkpointing: false. Pass checkpointing: true when constructing the Agent.";

/// Where in a cycle the loop paused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckpointPosition {
    /// The model returned tool use; the tools have not run yet.
    AfterModel,
    /// The tools finished; the next model call has not happened yet.
    AfterTools,
}

/// Wire form of a [`Checkpoint`]; unknown keys are warned about, not rejected.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointData {
    /// Boundary the loop paused at.
    pub position: Option<CheckpointPosition>,
    /// 0-based cycle index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_index: Option<u64>,
    /// Schema version; must match [`CHECKPOINT_SCHEMA_VERSION`] on load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    /// Keys the current version does not know about.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A pause point at a cycle boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checkpoint {
    /// Boundary the loop paused at.
    pub position: CheckpointPosition,
    /// 0-based cycle index.
    pub cycle_index: u64,
}

/// Resume payload: the entire input of the resuming invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointResumeContent {
    /// The checkpoint to resume from.
    pub checkpoint: Checkpoint,
}

impl Checkpoint {
    /// A checkpoint at `position` of cycle `cycle_index`.
    pub fn new(position: CheckpointPosition, cycle_index: u64) -> Self {
        Self {
            position,
            cycle_index,
        }
    }

    /// Schema version of this checkpoint (never taken from input, so it cannot be forged).
    pub fn schema_version(&self) -> &'static str {
        CHECKPOINT_SCHEMA_VERSION
    }

    /// Wire form: `{ position, cycleIndex, schemaVersion }`.
    pub fn to_json(&self) -> JsonValue {
        serde_json::to_value(self).expect("Checkpoint is serializable")
    }

    /// Parses a checkpoint, rejecting a schema-version mismatch and warning on unknown keys.
    pub fn from_json(value: JsonValue) -> Result<Self> {
        let data: CheckpointData = serde_json::from_value(value)?;
        Self::try_from(data)
    }
}

impl TryFrom<CheckpointData> for Checkpoint {
    type Error = Error;

    fn try_from(data: CheckpointData) -> Result<Self> {
        let version = data.schema_version.unwrap_or_default();
        if version != CHECKPOINT_SCHEMA_VERSION {
            return Err(Error::other(format!(
                "Checkpoints with schema version {} are not compatible with current version {CHECKPOINT_SCHEMA_VERSION}.",
                serde_json::to_string(&version).unwrap_or_else(|_| "\"\"".to_owned())
            )));
        }
        if !data.extra.is_empty() {
            let keys: Vec<&str> = data.extra.keys().map(String::as_str).collect();
            tracing::warn!(
                "unknown_keys=<{}> | ignoring unknown fields in checkpoint data",
                keys.join(", ")
            );
        }
        let position = data
            .position
            .ok_or_else(|| Error::other("The checkpoint data is missing its required \"position\" key."))?;
        Ok(Self::new(position, data.cycle_index.unwrap_or(0)))
    }
}

impl Serialize for Checkpoint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        CheckpointData {
            position: Some(self.position),
            cycle_index: Some(self.cycle_index),
            schema_version: Some(CHECKPOINT_SCHEMA_VERSION.to_owned()),
            extra: Map::new(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Checkpoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let data = CheckpointData::deserialize(deserializer)?;
        Checkpoint::try_from(data).map_err(serde::de::Error::custom)
    }
}

/// The checkpoint a `CheckpointResume` input resumes from, if any.
///
/// Errors when the agent was not built with `checkpointing(true)`.
pub(crate) fn resume_checkpoint(agent: &Agent, input: &InvokeInput) -> Result<Option<Checkpoint>> {
    let InvokeInput::CheckpointResume(content) = input else {
        return Ok(None);
    };
    if !agent.inner.checkpointing {
        return Err(Error::other(CHECKPOINTING_DISABLED_MESSAGE));
    }
    Ok(Some(content.checkpoint.clone()))
}

/// Cycle index a resumed invocation starts at: `afterTools` means the cycle finished, so the next
/// `afterModel` checkpoint belongs to the following cycle.
pub(crate) fn resume_cycle_index(checkpoint: &Checkpoint) -> u64 {
    match checkpoint.position {
        CheckpointPosition::AfterTools => checkpoint.cycle_index + 1,
        CheckpointPosition::AfterModel => checkpoint.cycle_index,
    }
}

fn checkpoint_result(
    state: &InvocationState,
    last_message: Message,
    position: CheckpointPosition,
    cycle_index: u64,
) -> AgentResult {
    AgentResult {
        stop_reason: StopReason::Checkpoint,
        last_message,
        invocation_state: state.clone(),
        structured_output: None,
        interrupts: Vec::new(),
        checkpoint: Some(Checkpoint::new(position, cycle_index)),
    }
}

/// The `afterModel` boundary: pause unless we are the invocation resuming from it, in which case
/// the model was re-invoked solely to regenerate the un-persisted assistant message.
pub(crate) fn after_model_checkpoint(
    agent: &Agent,
    state: &InvocationState,
    message: &Message,
    cycle_index: u64,
    resume_position: &mut Option<CheckpointPosition>,
) -> Option<AgentResult> {
    if !agent.inner.checkpointing {
        return None;
    }
    if resume_position.take() == Some(CheckpointPosition::AfterModel) {
        return None;
    }
    Some(checkpoint_result(
        state,
        message.clone(),
        CheckpointPosition::AfterModel,
        cycle_index,
    ))
}

/// The `afterTools` boundary: only fires when the loop would otherwise continue, and cancel wins.
pub(crate) fn after_tools_checkpoint(
    agent: &Agent,
    state: &InvocationState,
    message: &Message,
    cycle_index: u64,
    cancelled: bool,
) -> Option<AgentResult> {
    if !agent.inner.checkpointing || cancelled {
        return None;
    }
    Some(checkpoint_result(
        state,
        message.clone(),
        CheckpointPosition::AfterTools,
        cycle_index,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_through_to_json_from_json() {
        let checkpoint = Checkpoint::new(CheckpointPosition::AfterTools, 3);

        let json = checkpoint.to_json();
        assert_eq!(
            json,
            json!({ "position": "afterTools", "cycleIndex": 3, "schemaVersion": "1.0" })
        );
        assert_eq!(Checkpoint::from_json(json).unwrap(), checkpoint);
    }

    #[test]
    fn always_sets_schema_version_to_the_current_constant() {
        let json = Checkpoint::new(CheckpointPosition::AfterModel, 0).to_json();
        assert_eq!(json["schemaVersion"], json!(CHECKPOINT_SCHEMA_VERSION));
    }

    #[test]
    fn defaults_cycle_index_to_0() {
        let checkpoint =
            Checkpoint::from_json(json!({ "position": "afterModel", "schemaVersion": "1.0" })).unwrap();
        assert_eq!(checkpoint.cycle_index, 0);
    }

    #[test]
    fn errors_on_schema_version_mismatch() {
        let error =
            Checkpoint::from_json(json!({ "position": "afterModel", "schemaVersion": "9.9" })).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Checkpoints with schema version \"9.9\" are not compatible with current version 1.0."
        );
    }

    #[test]
    fn errors_when_schema_version_is_missing() {
        let error = Checkpoint::from_json(json!({ "position": "afterModel" })).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Checkpoints with schema version \"\" are not compatible with current version 1.0."
        );
    }

    #[test]
    fn ignores_unknown_fields() {
        let checkpoint = Checkpoint::from_json(json!({
            "position": "afterTools",
            "cycleIndex": 1,
            "schemaVersion": "1.0",
            "mystery": true
        }))
        .unwrap();

        assert_eq!(checkpoint, Checkpoint::new(CheckpointPosition::AfterTools, 1));
    }

    #[test]
    fn after_tools_resume_starts_at_the_next_cycle() {
        assert_eq!(
            resume_cycle_index(&Checkpoint::new(CheckpointPosition::AfterTools, 2)),
            3
        );
        assert_eq!(
            resume_cycle_index(&Checkpoint::new(CheckpointPosition::AfterModel, 2)),
            2
        );
    }
}
