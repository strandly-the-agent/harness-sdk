//! Persisted session shapes (`strands-ts/src/types/snapshot.ts`, `src/session/types.ts`).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::types::JsonValue;

/// Current schema version of the snapshot format.
pub const SNAPSHOT_SCHEMA_VERSION: &str = "1.0";

/// Context a snapshot was taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// A single agent.
    Agent,
    /// A multi-agent orchestrator (graph or swarm).
    MultiAgent,
}

impl Scope {
    /// Wire name (`"agent"` / `"multiAgent"`), also the on-disk directory name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Agent => "agent",
            Scope::MultiAgent => "multiAgent",
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Point-in-time capture of agent or orchestrator state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Scope identifying the snapshot context.
    pub scope: Scope,
    /// Schema version string for forward compatibility.
    pub schema_version: String,
    /// ISO 8601 timestamp of when the snapshot was created.
    pub created_at: String,
    /// Framework-owned state data.
    pub data: Map<String, JsonValue>,
    /// Application-owned data. Strands never reads or modifies it.
    pub app_data: Map<String, JsonValue>,
}

/// Snapshot metadata stored alongside snapshots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotManifest {
    /// Schema version for forward/backward compatibility.
    pub schema_version: String,
    /// ISO 8601 timestamp of the last manifest update.
    pub updated_at: String,
}

/// The current time as an ISO 8601 / RFC 3339 string with milliseconds (`Date.toISOString()`).
pub(crate) fn now_iso8601() -> String {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    iso8601_from_unix_millis(since_epoch.as_millis() as i64)
}

fn iso8601_from_unix_millis(millis: i64) -> String {
    let (days, time_ms) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let (year, month, day) = civil_from_days(days);
    let (ms, secs) = (time_ms % 1000, time_ms / 1000);
    let (hour, minute, second) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

/// Days since the Unix epoch to a civil date, after Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snapshot_serializes_with_ts_field_names() {
        let snapshot = Snapshot {
            scope: Scope::Agent,
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
            created_at: "2024-01-01T00:00:00.000Z".to_owned(),
            data: json!({ "messages": [] }).as_object().unwrap().clone(),
            app_data: Map::new(),
        };
        assert_eq!(
            serde_json::to_string(&snapshot).unwrap(),
            r#"{"scope":"agent","schemaVersion":"1.0","createdAt":"2024-01-01T00:00:00.000Z","data":{"messages":[]},"appData":{}}"#
        );
    }

    #[test]
    fn manifest_serializes_with_ts_field_names() {
        let manifest = SnapshotManifest {
            schema_version: "1.0".to_owned(),
            updated_at: "2024-01-01T00:00:00.000Z".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&manifest).unwrap(),
            r#"{"schemaVersion":"1.0","updatedAt":"2024-01-01T00:00:00.000Z"}"#
        );
    }

    #[test]
    fn multi_agent_scope_round_trips() {
        assert_eq!(
            serde_json::to_string(&Scope::MultiAgent).unwrap(),
            "\"multiAgent\""
        );
        assert_eq!(
            serde_json::from_str::<Scope>("\"multiAgent\"").unwrap(),
            Scope::MultiAgent
        );
    }

    #[test]
    fn iso8601_matches_date_to_iso_string() {
        assert_eq!(iso8601_from_unix_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso8601_from_unix_millis(1_704_067_200_000),
            "2024-01-01T00:00:00.000Z"
        );
        assert_eq!(
            iso8601_from_unix_millis(1_760_358_896_789),
            "2025-10-13T12:34:56.789Z"
        );
        assert_eq!(
            iso8601_from_unix_millis(951_782_400_000),
            "2000-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn now_is_well_formed() {
        let now = now_iso8601();
        assert_eq!(now.len(), 24, "{now}");
        assert!(now.ends_with('Z'), "{now}");
    }
}
