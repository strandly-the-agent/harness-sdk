//! Agent state capture/restore for session snapshots.
//!
//! Temporary home for the `takeSnapshot`/`loadSnapshot` helpers of
//! `strands-ts/src/agent/snapshot.ts`; they move to `agent/` when that module is ported. Only the
//! `session` preset is implemented, and only the fields the Rust `Agent` has today (`messages`,
//! `state`, `systemPrompt`) — `modelState` and `interrupts` are not ported yet, and omitting the
//! keys makes a TS reader leave its own values untouched.

use serde_json::{Map, Value};

use super::types::{now_iso8601, Scope, Snapshot, SNAPSHOT_SCHEMA_VERSION};
use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::types::{Message, SystemPrompt};

/// Captures the agent's session state.
pub(crate) fn take_agent_snapshot(agent: &Agent) -> Snapshot {
    let mut data = Map::new();
    data.insert(
        "messages".to_owned(),
        serde_json::to_value(agent.messages()).unwrap_or(Value::Null),
    );
    data.insert("state".to_owned(), agent.state().to_json());
    data.insert(
        "systemPrompt".to_owned(),
        match agent.system_prompt() {
            Some(prompt) => serde_json::to_value(prompt).unwrap_or(Value::Null),
            None => Value::Null,
        },
    );
    Snapshot {
        scope: Scope::Agent,
        schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
        created_at: now_iso8601(),
        data,
        app_data: Map::new(),
    }
}

/// Restores a snapshot into the agent. Absent keys leave the corresponding field unchanged.
pub(crate) fn load_agent_snapshot(agent: &Agent, snapshot: &Snapshot) -> Result<()> {
    if snapshot.scope != Scope::Agent {
        return Err(Error::other(format!(
            "Expected snapshot scope 'agent', got '{}'",
            snapshot.scope
        )));
    }
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Err(Error::other(format!(
            "Unsupported snapshot schema version: {}. Current version: {SNAPSHOT_SCHEMA_VERSION}",
            snapshot.schema_version
        )));
    }

    if let Some(messages) = snapshot.data.get("messages") {
        let messages: Vec<Message> = serde_json::from_value(messages.clone())?;
        agent.set_messages(messages);
    }

    if let Some(state) = snapshot.data.get("state") {
        agent.state().load_json(state.clone());
    }

    if let Some(prompt) = snapshot.data.get("systemPrompt") {
        if prompt.is_null() {
            agent.set_system_prompt(None);
        } else {
            let prompt: SystemPrompt = serde_json::from_value(prompt.clone())?;
            agent.set_system_prompt(Some(prompt));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_agent_for_session;
    use serde_json::json;

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
