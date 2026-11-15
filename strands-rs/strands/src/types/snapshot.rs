//! Snapshot types (TS `types/snapshot.ts`).

use serde::{Deserialize, Serialize};

use super::json::JsonObject;

/// The snapshot schema version this SDK writes (TS `SNAPSHOT_SCHEMA_VERSION`).
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

/// A point-in-time capture of agent state (TS `Snapshot`).
///
/// `created_at` is kept as the exact string TS writes with `Date.toISOString()`
/// (`2026-11-11T12:00:00.000Z`, millisecond precision).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Snapshot {
    scope: Scope,
    schema_version: String,
    created_at: String,
    data: JsonObject,
    app_data: JsonObject,
}

impl Snapshot {
    /// A snapshot stamped with the current schema version and time.
    pub fn new(scope: Scope, data: JsonObject, app_data: JsonObject) -> Self {
        Self {
            scope,
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
            created_at: iso_timestamp(jiff::Timestamp::now()),
            data,
            app_data,
        }
    }

    /// Sets the creation time, in `toISOString()` format.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = created_at.into();
        self
    }

    /// Sets the schema version.
    #[must_use]
    pub fn with_schema_version(mut self, schema_version: impl Into<String>) -> Self {
        self.schema_version = schema_version.into();
        self
    }

    /// What the snapshot captures.
    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The schema version it was written with.
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    /// The creation time, in `toISOString()` format.
    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    /// The SDK-owned state.
    pub fn data(&self) -> &JsonObject {
        &self.data
    }

    /// Application-owned data.
    pub fn app_data(&self) -> &JsonObject {
        &self.app_data
    }
}

/// Formats a timestamp like JS `Date.prototype.toISOString()`.
pub(crate) fn iso_timestamp(timestamp: jiff::Timestamp) -> String {
    timestamp.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn iso_timestamp_keeps_milliseconds_like_to_iso_string() {
        let cases = [
            (0, "1970-01-01T00:00:00.000Z"),
            (1_767_323_045_000, "2026-01-02T03:04:05.000Z"),
            (1_794_398_643_736, "2026-11-11T12:04:03.736Z"),
            (1_794_398_643_736_999, "2026-11-11T12:04:03.736Z"),
        ];
        for (value, expected) in cases {
            let timestamp = if value > 10_000_000_000_000 {
                jiff::Timestamp::from_microsecond(value).unwrap()
            } else {
                jiff::Timestamp::from_millisecond(value).unwrap()
            };
            assert_eq!(iso_timestamp(timestamp), expected);
        }
    }

    #[test]
    fn new_stamps_the_schema_version_and_a_millisecond_time() {
        let snapshot = Snapshot::new(Scope::Agent, JsonObject::new(), JsonObject::new());
        assert_eq!(snapshot.schema_version(), SNAPSHOT_SCHEMA_VERSION);
        let created_at = snapshot.created_at();
        assert_eq!(created_at.len(), "2026-11-11T12:00:00.000Z".len());
        assert!(created_at.ends_with('Z') && created_at.as_bytes()[19] == b'.');
    }

    #[test]
    fn serializes_with_camel_case_keys_in_ts_order() {
        let snapshot = Snapshot::new(Scope::MultiAgent, JsonObject::new(), JsonObject::new())
            .with_created_at("2026-11-11T12:00:01.543Z");
        assert_eq!(
            serde_json::to_string(&snapshot).unwrap(),
            r#"{"scope":"multiAgent","schemaVersion":"1.0","createdAt":"2026-11-11T12:00:01.543Z","data":{},"appData":{}}"#
        );
        let restored: Snapshot = serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
        assert_eq!(restored, snapshot);
        assert!(serde_json::from_value::<Scope>(json!("swarm")).is_err());
    }
}
