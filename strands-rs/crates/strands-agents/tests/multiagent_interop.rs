//! TS interop fixtures: JSON exactly as the TS SDK writes it (`strands-ts/src/multiagent/`
//! `state.ts` `toJSON()` / `snapshot.ts`), asserted to deserialize and re-serialize
//! byte-identically so a session written by one SDK can be resumed by the other.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use strands_agents::multiagent::{MultiAgentResult, MultiAgentState, NodeResult, NodeState};
use strands_agents::session::Snapshot;

/// Deserializes `fixture` into `T`, re-serializes it, and compares against the fixture
/// normalized through `serde_json` — byte-identical including key order.
fn assert_round_trips<T: DeserializeOwned + Serialize>(fixture: &str) {
    let typed: T = serde_json::from_str(fixture).expect("TS fixture deserializes");
    let actual = serde_json::to_string(&typed).expect("re-serializes");
    let expected = serde_json::to_string(&serde_json::from_str::<Value>(fixture).expect("fixture is JSON"))
        .expect("fixture re-serializes");

    assert_eq!(actual, expected);
}

/// `NodeResult.toJSON()` with every optional field present.
const NODE_RESULT: &str = r#"{
  "type": "nodeResult",
  "nodeId": "researcher",
  "status": "FAILED",
  "duration": 1234,
  "content": [{ "text": "partial findings" }],
  "error": "model timed out",
  "structuredOutput": { "answer": 42, "sources": ["a", "b"] },
  "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 }
}"#;

/// `NodeResult.toJSON()` with every optional field omitted, as TS's spread-conditionals write it.
const NODE_RESULT_MINIMAL: &str = r#"{
  "type": "nodeResult",
  "nodeId": "a",
  "status": "COMPLETED",
  "duration": 7,
  "content": []
}"#;

/// `MultiAgentResult.toJSON()`.
const MULTI_AGENT_RESULT: &str = r#"{
  "type": "multiAgentResult",
  "status": "COMPLETED",
  "results": [
    {
      "type": "nodeResult",
      "nodeId": "a",
      "status": "COMPLETED",
      "duration": 7,
      "content": [{ "text": "done" }],
      "usage": { "inputTokens": 3, "outputTokens": 4, "totalTokens": 7 }
    }
  ],
  "content": [{ "text": "done" }],
  "duration": 9,
  "usage": { "inputTokens": 3, "outputTokens": 4, "totalTokens": 7 }
}"#;

/// `NodeState` serialized state: `results` and `interrupts` are always written, even when empty.
const NODE_STATE: &str = r#"{
  "status": "COMPLETED",
  "terminus": true,
  "startTime": 1739577600000,
  "results": [
    {
      "type": "nodeResult",
      "nodeId": "a",
      "status": "COMPLETED",
      "duration": 7,
      "content": [{ "text": "done" }]
    }
  ],
  "interrupts": []
}"#;

/// `MultiAgentState` serialized state, as nested under a snapshot's `data.state`.
const MULTI_AGENT_STATE: &str = r#"{
  "startTime": 1739577600000,
  "steps": 2,
  "results": [
    {
      "type": "nodeResult",
      "nodeId": "a",
      "status": "COMPLETED",
      "duration": 7,
      "content": []
    }
  ],
  "app": { "topic": "rust" },
  "nodes": {
    "a": {
      "status": "COMPLETED",
      "terminus": false,
      "startTime": 1739577600000,
      "results": [],
      "interrupts": []
    },
    "b": {
      "status": "PENDING",
      "terminus": true,
      "startTime": 1739577600001,
      "results": [],
      "interrupts": []
    }
  }
}"#;

/// A multi-agent `Snapshot` as `takeSnapshot` writes it.
const SNAPSHOT: &str = r#"{
  "scope": "multiAgent",
  "schemaVersion": "1.0",
  "createdAt": "2025-02-15T00:00:00.000Z",
  "data": {
    "orchestratorId": "my-graph",
    "state": {
      "startTime": 1739577600000,
      "steps": 1,
      "results": [],
      "app": {},
      "nodes": {
        "a": {
          "status": "EXECUTING",
          "terminus": false,
          "startTime": 1739577600000,
          "results": [],
          "interrupts": []
        }
      }
    }
  },
  "appData": { "userId": "u-1" }
}"#;

/// A nested orchestrator's snapshot carries the id only — no `state` key.
const SNAPSHOT_WITHOUT_STATE: &str = r#"{
  "scope": "multiAgent",
  "schemaVersion": "1.0",
  "createdAt": "2025-02-15T00:00:00.000Z",
  "data": { "orchestratorId": "nested" },
  "appData": {}
}"#;

#[test]
fn node_result_round_trips() {
    assert_round_trips::<NodeResult>(NODE_RESULT);
    assert_round_trips::<NodeResult>(NODE_RESULT_MINIMAL);
}

#[test]
fn multi_agent_result_round_trips() {
    assert_round_trips::<MultiAgentResult>(MULTI_AGENT_RESULT);
}

#[test]
fn node_state_round_trips() {
    assert_round_trips::<NodeState>(NODE_STATE);
}

#[test]
fn multi_agent_state_round_trips() {
    assert_round_trips::<MultiAgentState>(MULTI_AGENT_STATE);
}

#[test]
fn snapshot_round_trips() {
    assert_round_trips::<Snapshot>(SNAPSHOT);
    assert_round_trips::<Snapshot>(SNAPSHOT_WITHOUT_STATE);
}
