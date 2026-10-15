//! Snapshots of a multi-agent orchestrator (`strands-ts/src/multiagent/snapshot.ts`).
//!
//! Well-known keys in `data`: `orchestratorId` (validated on load) and `state` (the serialized
//! [`MultiAgentState`], absent for nested orchestrators whose execution state is ephemeral).

use serde_json::Map;

use super::multiagent::MultiAgent;
use super::state::{MultiAgentState, MultiAgentStateHandle};
use crate::error::{Error, Result};
use crate::session::types::{now_iso8601, Scope, Snapshot, SNAPSHOT_SCHEMA_VERSION};
use crate::types::JsonValue;

/// Options for [`take_snapshot`].
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct TakeMultiAgentSnapshotOptions {
    /// Application-owned data. Strands never reads or modifies it.
    pub app_data: Map<String, JsonValue>,
}

impl TakeMultiAgentSnapshotOptions {
    /// Attach application-owned data to the snapshot.
    #[must_use]
    pub fn with_app_data(mut self, app_data: Map<String, JsonValue>) -> Self {
        self.app_data = app_data;
        self
    }
}

/// Captures an orchestrator's state as a `Snapshot` (`scope: "multiAgent"`).
///
/// `state` is `None` for a nested orchestrator, whose execution state is not observable from
/// outside; the snapshot then carries the orchestrator id only.
///
/// # Errors
/// Returns an error when the state cannot be serialized.
pub fn take_snapshot(
    orchestrator: &dyn MultiAgent,
    state: Option<&MultiAgentStateHandle>,
    options: TakeMultiAgentSnapshotOptions,
) -> Result<Snapshot> {
    let mut data = Map::new();
    data.insert("orchestratorId".to_owned(), orchestrator.id().into());
    if let Some(state) = state {
        data.insert(
            "state".to_owned(),
            state.with(|state| serde_json::to_value(state))?,
        );
    }

    Ok(Snapshot {
        scope: Scope::MultiAgent,
        schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
        created_at: now_iso8601(),
        data,
        app_data: options.app_data,
    })
}

/// Restores a snapshot into `orchestrator`, replacing `state` with the snapshot's when it has one.
///
/// # Errors
/// Returns an error when the scope, schema version or orchestrator id does not match, or when the
/// stored state cannot be deserialized.
pub fn load_snapshot(
    orchestrator: &dyn MultiAgent,
    snapshot: &Snapshot,
    state: Option<&MultiAgentStateHandle>,
) -> Result<()> {
    if snapshot.scope != Scope::MultiAgent {
        return Err(Error::other(format!(
            "Expected snapshot scope 'multiAgent', got '{}'",
            snapshot.scope
        )));
    }
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Err(Error::other(format!(
            "Unsupported snapshot schema version: {}. Current version: {SNAPSHOT_SCHEMA_VERSION}",
            snapshot.schema_version
        )));
    }

    let snapshot_id = snapshot.data.get("orchestratorId").and_then(JsonValue::as_str);
    if snapshot_id != Some(orchestrator.id()) {
        return Err(Error::other(format!(
            "Snapshot orchestrator ID mismatch: expected '{}', got '{}'",
            orchestrator.id(),
            snapshot_id.unwrap_or_default()
        )));
    }

    if let (Some(state), Some(stored)) = (state, snapshot.data.get("state")) {
        let restored: MultiAgentState = serde_json::from_value(stored.clone())?;
        state.with_mut(|current| *current = restored);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::state::{MultiAgentResult, NodeResult, ResultStatus, Status};
    use crate::multiagent::test_orchestrator::TestOrchestrator;
    use serde_json::json;

    fn orchestrator(id: &str) -> std::sync::Arc<dyn MultiAgent> {
        TestOrchestrator::shared(id, vec![], MultiAgentResult::new(vec![], 1))
    }

    fn state() -> MultiAgentStateHandle {
        MultiAgentStateHandle::new(MultiAgentState::new(["a"]))
    }

    #[test]
    fn captures_orchestrator_id_serialized_state_and_app_data() {
        let orchestrator = orchestrator("my-graph");
        let state = state();
        state.with_mut(|state| {
            state.steps = 3;
            state.app.set("key", "val");
        });
        let app_data = json!({"userId": "u-1"}).as_object().unwrap().clone();

        let snapshot = take_snapshot(
            orchestrator.as_ref(),
            Some(&state),
            TakeMultiAgentSnapshotOptions::default().with_app_data(app_data.clone()),
        )
        .unwrap();

        assert_eq!(snapshot.scope, Scope::MultiAgent);
        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.data["orchestratorId"], json!("my-graph"));
        assert_eq!(snapshot.data["state"]["steps"], json!(3));
        assert_eq!(snapshot.data["state"]["app"], json!({"key": "val"}));
        assert_eq!(snapshot.app_data, app_data);
    }

    #[test]
    fn omits_state_when_no_state_is_given() {
        let orchestrator = orchestrator("nested");

        let snapshot = take_snapshot(
            orchestrator.as_ref(),
            None,
            TakeMultiAgentSnapshotOptions::default(),
        )
        .unwrap();

        assert_eq!(snapshot.data["orchestratorId"], json!("nested"));
        assert!(!snapshot.data.contains_key("state"));
        assert!(snapshot.app_data.is_empty());
    }

    #[test]
    fn restores_multi_agent_state() {
        let orchestrator = orchestrator("my-graph");
        let saved = state();
        saved.with_mut(|state| {
            state.steps = 7;
            state
                .results
                .push(NodeResult::new("a", ResultStatus::Completed, 5));
            state.node_or_insert("a").status = Status::Completed;
        });
        let snapshot = take_snapshot(
            orchestrator.as_ref(),
            Some(&saved),
            TakeMultiAgentSnapshotOptions::default(),
        )
        .unwrap();

        let restored = state();
        load_snapshot(orchestrator.as_ref(), &snapshot, Some(&restored)).unwrap();

        assert_eq!(restored.with(|state| state.steps), 7);
        assert_eq!(restored.with(|state| state.results.len()), 1);
        assert_eq!(restored.node("a").unwrap().status, Status::Completed);
    }

    #[test]
    fn does_not_modify_state_when_the_snapshot_has_no_state_data() {
        let orchestrator = orchestrator("my-graph");
        let snapshot = take_snapshot(
            orchestrator.as_ref(),
            None,
            TakeMultiAgentSnapshotOptions::default(),
        )
        .unwrap();
        let current = state();
        current.with_mut(|state| state.steps = 2);

        load_snapshot(orchestrator.as_ref(), &snapshot, Some(&current)).unwrap();

        assert_eq!(current.with(|state| state.steps), 2);
    }

    #[test]
    fn rejects_a_wrong_scope() {
        let orchestrator = orchestrator("my-graph");
        let mut snapshot = take_snapshot(
            orchestrator.as_ref(),
            None,
            TakeMultiAgentSnapshotOptions::default(),
        )
        .unwrap();
        snapshot.scope = Scope::Agent;

        let error = load_snapshot(orchestrator.as_ref(), &snapshot, None).expect_err("scope is validated");

        assert!(
            error.to_string().contains("Expected snapshot scope 'multiAgent'"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_unsupported_schema_version() {
        let orchestrator = orchestrator("my-graph");
        let mut snapshot = take_snapshot(
            orchestrator.as_ref(),
            None,
            TakeMultiAgentSnapshotOptions::default(),
        )
        .unwrap();
        snapshot.schema_version = "99.0".to_owned();

        let error = load_snapshot(orchestrator.as_ref(), &snapshot, None).expect_err("version is validated");

        assert!(
            error.to_string().contains("Unsupported snapshot schema version"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_orchestrator_id_mismatch() {
        let snapshot = take_snapshot(
            orchestrator("other-graph").as_ref(),
            None,
            TakeMultiAgentSnapshotOptions::default(),
        )
        .unwrap();

        let error = load_snapshot(orchestrator("my-graph").as_ref(), &snapshot, None)
            .expect_err("the id is validated");

        assert!(
            error.to_string().contains("Snapshot orchestrator ID mismatch"),
            "{error}"
        );
    }

    #[test]
    fn survives_a_json_round_trip() {
        let orchestrator = orchestrator("my-graph");
        let saved = state();
        saved.with_mut(|state| state.steps = 4);
        let snapshot = take_snapshot(
            orchestrator.as_ref(),
            Some(&saved),
            TakeMultiAgentSnapshotOptions::default(),
        )
        .unwrap();

        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: Snapshot = serde_json::from_str(&json).unwrap();

        let restored = state();
        load_snapshot(orchestrator.as_ref(), &parsed, Some(&restored)).unwrap();
        assert_eq!(restored.with(|state| state.steps), 4);
    }

    #[test]
    fn loads_a_ts_shaped_snapshot_fixture() {
        let orchestrator = orchestrator("my-graph");
        let fixture = json!({
            "scope": "multiAgent",
            "schemaVersion": "1.0",
            "createdAt": "2026-01-15T12:00:00.000Z",
            "data": {
                "orchestratorId": "my-graph",
                "state": {
                    "startTime": 1_736_942_400_000_u64,
                    "steps": 3,
                    "results": [],
                    "nodes": {"a": {"status": "COMPLETED", "terminus": false, "startTime": 0, "results": []}},
                    "app": {"key": "val"}
                }
            },
            "appData": {"userId": "u-1"}
        });

        let snapshot: Snapshot = serde_json::from_value(fixture).unwrap();
        let restored = state();
        load_snapshot(orchestrator.as_ref(), &snapshot, Some(&restored)).unwrap();

        assert_eq!(restored.with(|state| state.steps), 3);
        assert_eq!(restored.node("a").unwrap().status, Status::Completed);
        assert_eq!(restored.with(|state| state.app.get("key")), Some(json!("val")));
    }
}
