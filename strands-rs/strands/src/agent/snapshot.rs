//! Snapshot options (TS `agent/snapshot.ts`, the types).

use serde::{Deserialize, Serialize};

use crate::types::json::JsonValue;

/// A part of agent state a snapshot can hold (TS `SnapshotField`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotField {
    /// `"messages"`: the conversation.
    Messages,
    /// `"state"`: the app state.
    State,
    /// `"systemPrompt"`: the system prompt.
    SystemPrompt,
    /// `"modelState"`: the model state.
    ModelState,
    /// `"interrupts"`: the interrupt state.
    Interrupts,
}

/// A named set of snapshot fields (TS `SnapshotPreset`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotPreset {
    /// `"session"`: every field, as session persistence needs.
    Session,
}

/// Options for taking a snapshot (TS `TakeSnapshotOptions`).
///
/// The fields start from `preset` (or nothing), add `include`, then drop `exclude`.
///
/// ```
/// use strands::agent::snapshot::{SnapshotField, SnapshotPreset, TakeSnapshotOptions};
///
/// let options = TakeSnapshotOptions {
///     preset: Some(SnapshotPreset::Session),
///     exclude: Some(vec![SnapshotField::Messages]),
///     ..TakeSnapshotOptions::default()
/// };
/// assert_eq!(options, TakeSnapshotOptions::builder().preset(SnapshotPreset::Session).exclude(vec![SnapshotField::Messages]).build());
/// ```
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct TakeSnapshotOptions {
    /// The starting set of fields.
    pub preset: Option<SnapshotPreset>,
    /// Fields to add to the preset.
    pub include: Option<Vec<SnapshotField>>,
    /// Fields to leave out, applied last.
    pub exclude: Option<Vec<SnapshotField>>,
    /// Application-owned data to store in the snapshot.
    pub app_data: Option<serde_json::Map<String, JsonValue>>,
}
