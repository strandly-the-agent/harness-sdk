//! Snapshots of agent or orchestrator state (TS `types/snapshot.ts`).

use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::types::json::JsonValue;

/// The snapshot format version this SDK writes (TS `SNAPSHOT_SCHEMA_VERSION`).
pub const SNAPSHOT_SCHEMA_VERSION: &str = "1.0";

/// What a snapshot captures (TS `Scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum Scope {
    /// A single agent.
    Agent,
    /// A multi-agent orchestrator.
    MultiAgent,
}

/// A point-in-time capture of state (TS `Snapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Snapshot {
    /// What the snapshot captures.
    pub scope: Scope,
    /// The snapshot format version.
    pub schema_version: String,
    /// When it was taken, as an ISO 8601 UTC timestamp with milliseconds
    /// (`2026-11-11T12:00:00.000Z`).
    pub created_at: String,
    /// The captured state, owned by the SDK.
    pub data: Map<String, JsonValue>,
    /// Application data stored alongside.
    pub app_data: Map<String, JsonValue>,
}

impl Snapshot {
    /// A snapshot taken now, at the current schema version, with no app data.
    pub fn new(scope: Scope, data: Map<String, JsonValue>) -> Self {
        Self {
            scope,
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
            created_at: iso_timestamp(jiff::Timestamp::now()),
            data,
            app_data: Map::new(),
        }
    }

    /// Sets the app data.
    #[must_use]
    pub fn with_app_data(mut self, app_data: Map<String, JsonValue>) -> Self {
        self.app_data = app_data;
        self
    }

    /// Sets the creation timestamp.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = created_at.into();
        self
    }
}

/// Formats a timestamp like JS `Date.prototype.toISOString` (millisecond precision, `Z`).
pub(crate) fn iso_timestamp(timestamp: jiff::Timestamp) -> String {
    timestamp.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_use_the_to_iso_string_format() {
        let timestamp: jiff::Timestamp = "2026-11-11T12:00:00Z".parse().unwrap();
        assert_eq!(iso_timestamp(timestamp), "2026-11-11T12:00:00.000Z");
        let timestamp: jiff::Timestamp = "2026-11-11T12:00:00.123456Z".parse().unwrap();
        assert_eq!(iso_timestamp(timestamp), "2026-11-11T12:00:00.123Z");
    }

    #[test]
    fn new_snapshots_use_the_current_schema_version() {
        let snapshot = Snapshot::new(Scope::Agent, Map::new());
        assert_eq!(snapshot.schema_version, "1.0");
        assert_eq!(snapshot.created_at.len(), "2026-11-11T12:00:00.000Z".len());
        assert!(snapshot.created_at.ends_with('Z'));
        assert_eq!(
            serde_json::to_string(&snapshot.with_created_at("2026-11-11T12:00:00.000Z")).unwrap(),
            r#"{"scope":"agent","schemaVersion":"1.0","createdAt":"2026-11-11T12:00:00.000Z","data":{},"appData":{}}"#
        );
    }
}
