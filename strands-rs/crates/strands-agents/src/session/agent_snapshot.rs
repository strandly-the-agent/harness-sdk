//! Agent state capture/restore for session snapshots.
//!
//! Thin wrappers over `agent::snapshot` (`strands-ts/src/agent/snapshot.ts`) with the `session`
//! preset, kept so the session manager has a stable crate-internal entry point.

use super::types::Snapshot;
use crate::agent::{Agent, TakeSnapshotOptions};
use crate::error::Result;

/// Captures the agent's session state (the `session` snapshot preset).
pub(crate) fn take_agent_snapshot(agent: &Agent) -> Snapshot {
    agent.take_snapshot(TakeSnapshotOptions::default())
}

/// Restores a snapshot into the agent. Absent keys leave the corresponding field unchanged.
pub(crate) fn load_agent_snapshot(agent: &Agent, snapshot: &Snapshot) -> Result<()> {
    agent.load_snapshot(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_agent_for_session;
    use crate::session::types::Scope;
    use crate::types::Message;
    use serde_json::{json, Map, Value};

    #[test]
    fn captures_messages_state_and_system_prompt() {
        let agent = test_agent_for_session("test-agent");
        agent.set_messages(vec![Message::user("hello")]);
        agent.state().set("k", json!("v"));

        let snapshot = take_agent_snapshot(&agent);

        assert_eq!(snapshot.scope, Scope::Agent);
        assert_eq!(snapshot.schema_version, "1.0");
        let messages = snapshot.data["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["content"], json!([{ "text": "hello" }]));
        assert_eq!(snapshot.data["state"], json!({ "k": "v" }));
        assert_eq!(snapshot.data["systemPrompt"], json!("Test prompt"));
        assert!(snapshot.app_data.is_empty());
    }

    #[test]
    fn round_trips_through_load() {
        let source = test_agent_for_session("a");
        source.set_messages(vec![Message::assistant("hi")]);
        source.state().set("count", json!(2));
        let snapshot = take_agent_snapshot(&source);

        let target = test_agent_for_session("b");
        load_agent_snapshot(&target, &snapshot).unwrap();

        assert_eq!(target.messages(), source.messages());
        assert_eq!(target.state().to_json(), json!({ "count": 2 }));
        assert_eq!(target.system_prompt(), source.system_prompt());
    }

    #[test]
    fn null_system_prompt_clears_it() {
        let agent = test_agent_for_session("a");
        let mut snapshot = take_agent_snapshot(&agent);
        snapshot.data.insert("systemPrompt".to_owned(), Value::Null);

        load_agent_snapshot(&agent, &snapshot).unwrap();

        assert_eq!(agent.system_prompt(), None);
    }

    #[test]
    fn absent_keys_leave_fields_unchanged() {
        let agent = test_agent_for_session("a");
        agent.set_messages(vec![Message::user("keep")]);
        let snapshot = Snapshot {
            data: Map::new(),
            ..take_agent_snapshot(&agent)
        };

        load_agent_snapshot(&agent, &snapshot).unwrap();

        assert_eq!(agent.messages().len(), 1);
        assert!(agent.system_prompt().is_some());
    }

    #[test]
    fn rejects_wrong_scope_and_schema_version() {
        let agent = test_agent_for_session("a");
        let snapshot = take_agent_snapshot(&agent);

        let wrong_scope = Snapshot {
            scope: Scope::MultiAgent,
            ..snapshot.clone()
        };
        assert_eq!(
            load_agent_snapshot(&agent, &wrong_scope).unwrap_err().to_string(),
            "Expected snapshot scope 'agent', got 'multiAgent'"
        );

        let wrong_version = Snapshot {
            schema_version: "2.0".to_owned(),
            ..snapshot
        };
        assert_eq!(
            load_agent_snapshot(&agent, &wrong_version)
                .unwrap_err()
                .to_string(),
            "Unsupported snapshot schema version: 2.0. Current version: 1.0"
        );
    }
}
