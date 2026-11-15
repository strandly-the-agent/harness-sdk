//! The snapshot wire shape (TS `types/snapshot.ts`).

use serde::{Deserialize, Serialize};

use crate::types::json::JsonValue;

/// The schema version written into every snapshot (TS `SNAPSHOT_SCHEMA_VERSION`).
pub const SNAPSHOT_SCHEMA_VERSION: &str = "1.0";

/// What a snapshot captures (TS `Scope`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// `"agent"`: one agent.
    Agent,
    /// `"multiAgent"`: a multi-agent orchestration.
    MultiAgent,
}

/// A point-in-time capture of agent state (TS `Snapshot`).
///
/// `created_at` is kept as the string TS writes with `Date.toISOString()`
/// (`2026-01-01T00:00:00.000Z`, millisecond precision), so it round-trips unchanged.
///
/// ```
/// use strands::types::snapshot::{Scope, Snapshot};
///
/// let snapshot = Snapshot::new(Scope::Agent, "2026-01-01T00:00:00.000Z");
/// assert_eq!(
///     serde_json::to_string(&snapshot).unwrap(),
///     r#"{"scope":"agent","schemaVersion":"1.0","createdAt":"2026-01-01T00:00:00.000Z","data":{},"appData":{}}"#
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// What the snapshot captures.
    pub scope: Scope,
    /// The schema version of the snapshot format.
    pub schema_version: String,
    /// When the snapshot was taken, in `Date.toISOString()` form.
    pub created_at: String,
    /// The framework-owned state.
    pub data: serde_json::Map<String, JsonValue>,
    /// Application-owned data; Strands never reads or changes it.
    pub app_data: serde_json::Map<String, JsonValue>,
}

impl Snapshot {
    /// Creates an empty snapshot of the current schema version.
    #[must_use]
    pub fn new(scope: Scope, created_at: impl Into<String>) -> Self {
        Self {
            scope,
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
            created_at: created_at.into(),
            data: serde_json::Map::new(),
            app_data: serde_json::Map::new(),
        }
    }

    /// Sets the framework-owned state.
    #[must_use]
    pub fn with_data(mut self, data: serde_json::Map<String, JsonValue>) -> Self {
        self.data = data;
        self
    }

    /// Sets the application-owned data.
    #[must_use]
    pub fn with_app_data(mut self, app_data: serde_json::Map<String, JsonValue>) -> Self {
        self.app_data = app_data;
        self
    }
}

#[cfg(test)]
mod tests {
    //! Snapshot wire shape (TS has no spec file for `types/snapshot.ts`).

    use serde_json::json;

    use super::*;

    #[test]
    fn snapshot_round_trips_with_the_ts_key_order_and_millisecond_timestamp() {
        let text = r#"{"scope":"multiAgent","schemaVersion":"1.0","createdAt":"2026-11-11T12:00:00.000Z","data":{"messages":[],"interrupts":null},"appData":{"k":1}}"#;
        let snapshot: Snapshot = serde_json::from_str(text).unwrap();
        assert_eq!(snapshot.scope, Scope::MultiAgent);
        assert_eq!(snapshot.data.get("interrupts"), Some(&JsonValue::Null));
        assert_eq!(serde_json::to_string(&snapshot).unwrap(), text);
        let built = Snapshot::new(Scope::MultiAgent, "2026-11-11T12:00:00.000Z")
            .with_data(
                json!({ "messages": [], "interrupts": null })
                    .as_object()
                    .cloned()
                    .unwrap(),
            )
            .with_app_data(json!({ "k": 1 }).as_object().cloned().unwrap());
        assert_eq!(built, snapshot);
    }
}
