//! Ports `strands-ts/src/telemetry/__tests__/local-trace.test.ts`.
//!
//! TS passes `{ parent, startTime }` to the constructor and an end time to `end()`. The Rust trace
//! always starts and ends at the current time and the tracer pushes a child onto its parent, so
//! the tests set `start_time` directly, move tokio's paused clock, and attach children the way
//! the tracer does.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::AgentTrace;
use crate::types::messages::{ContentBlock, Message, Role, TextBlock};

fn started_at(name: &str, epoch_ms: u64) -> AgentTrace {
    let mut trace = AgentTrace::new(name, None);
    trace.start_time = UNIX_EPOCH + Duration::from_millis(epoch_ms);
    trace
}

fn child_started_at(parent: &AgentTrace, name: &str, epoch_ms: u64) -> AgentTrace {
    let mut trace = parent.child(name);
    trace.start_time = UNIX_EPOCH + Duration::from_millis(epoch_ms);
    trace
}

fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
}

#[tokio::test]
async fn generates_a_unique_id_in_uuid_format() {
    let trace1 = AgentTrace::new("test", None);
    let trace2 = AgentTrace::new("test", None);
    assert!(is_uuid(trace1.id()), "{}", trace1.id());
    assert!(is_uuid(trace2.id()), "{}", trace2.id());
    assert_ne!(trace1.id(), trace2.id());
}

#[tokio::test]
async fn sets_name_and_defaults() {
    let trace = AgentTrace::new("Cycle 1", None);
    assert_eq!(trace.name(), "Cycle 1");
    assert_eq!(trace.parent_id(), None);
    assert_eq!(trace.end_time(), None);
    assert_eq!(trace.duration(), Duration::ZERO);
    assert!(trace.children().is_empty());
    assert_eq!(trace.metadata().len(), 0);
    assert_eq!(trace.message(), None);
}

#[tokio::test]
async fn uses_current_time_as_default_start_time() {
    let before = SystemTime::now();
    let trace = AgentTrace::new("test", None);
    let after = SystemTime::now();
    assert!(trace.start_time() >= before);
    assert!(trace.start_time() <= after);
}

#[tokio::test]
async fn adds_child_to_parent_when_parent_is_provided() {
    let mut parent = AgentTrace::new("parent", None);
    let child = parent.child("child");
    let child_id = child.id().to_owned();
    parent.children.push(child);

    assert_eq!(parent.children().len(), 1);
    assert_eq!(parent.children()[0].id(), child_id);
    assert_eq!(parent.children()[0].parent_id(), Some(parent.id()));
}

#[tokio::test]
async fn supports_multiple_children() {
    let mut parent = AgentTrace::new("parent", None);
    let child1 = parent.child("child1");
    let child2 = parent.child("child2");
    let ids = [child1.id().to_owned(), child2.id().to_owned()];
    parent.children.extend([child1, child2]);

    assert_eq!(parent.children().len(), 2);
    assert_eq!(parent.children()[0].id(), ids[0]);
    assert_eq!(parent.children()[1].id(), ids[1]);
}

#[tokio::test]
async fn sets_parent_id_to_null_when_no_parent_is_provided() {
    assert_eq!(AgentTrace::new("root", None).parent_id(), None);
}

#[tokio::test]
async fn builds_a_three_level_hierarchy() {
    let mut root = started_at("Cycle 1", 1000);
    let model = child_started_at(&root, "stream_messages", 1001);
    let tool = child_started_at(&root, "Tool: calc", 1050);
    root.children.extend([model, tool]);

    assert_eq!(root.children().len(), 2);
    assert_eq!(root.children()[0].name(), "stream_messages");
    assert_eq!(root.children()[0].parent_id(), Some(root.id()));
    assert_eq!(root.children()[1].name(), "Tool: calc");
    assert_eq!(root.children()[1].parent_id(), Some(root.id()));
}

#[tokio::test(start_paused = true)]
async fn computes_duration_from_start_time_to_end_time() {
    let mut trace = started_at("test", 1000);
    tokio::time::advance(Duration::from_millis(500)).await;
    trace.end();
    assert_eq!(trace.end_time(), Some(UNIX_EPOCH + Duration::from_millis(1500)));
    assert_eq!(trace.duration(), Duration::from_millis(500));
}

#[tokio::test]
async fn uses_current_time_when_no_end_time_is_provided() {
    let before = SystemTime::now();
    let mut trace = AgentTrace::new("test", None);
    trace.end();
    assert!(trace.end_time().expect("an end time") >= before);
    assert!(trace.duration() >= Duration::ZERO);
}

#[tokio::test]
async fn stores_cycle_metadata() {
    let mut trace = AgentTrace::new("Cycle 1", None);
    trace.metadata.insert("cycleId".to_owned(), "cycle-1".to_owned());
    assert_eq!(trace.metadata().collect::<Vec<_>>(), [("cycleId", "cycle-1")]);
}

#[tokio::test]
async fn stores_tool_metadata() {
    let mut trace = AgentTrace::new("Tool: calc", None);
    trace.metadata.insert("toolUseId".to_owned(), "tool-1".to_owned());
    trace.metadata.insert("toolName".to_owned(), "calc".to_owned());
    assert_eq!(
        trace.metadata().collect::<Vec<_>>(),
        [("toolUseId", "tool-1"), ("toolName", "calc")]
    );
    assert_eq!(trace.metadata_value("toolName"), Some("calc"));
}

#[tokio::test]
async fn stores_a_message_with_role_and_content() {
    let content = vec![ContentBlock::Text(TextBlock::new("hello"))];
    let mut trace = AgentTrace::new("stream_messages", None);
    trace.message = Some(Message::new(Role::Assistant, content.clone()));
    let message = trace.message().expect("the message");
    assert_eq!(message.role(), Role::Assistant);
    assert_eq!(message.content(), content);
}

#[tokio::test]
async fn returns_complete_data_for_a_default_trace() {
    let trace = started_at("Cycle 1", 1000);
    assert_eq!(
        serde_json::to_value(&trace).unwrap(),
        json!({
            "id": trace.id(),
            "name": "Cycle 1",
            "parentId": null,
            "startTime": 1000,
            "endTime": null,
            "duration": 0,
            "children": [],
            "metadata": {},
            "message": null,
        })
    );
}

#[tokio::test(start_paused = true)]
async fn serializes_a_hierarchy_with_children_and_metadata() {
    let mut root = started_at("Cycle 1", 1000);
    root.metadata.insert("cycleId".to_owned(), "cycle-1".to_owned());
    tokio::time::advance(Duration::from_millis(1)).await;
    let mut child = child_started_at(&root, "stream_messages", 1001);
    let child_id = child.id().to_owned();
    tokio::time::advance(Duration::from_millis(99)).await;
    child.end();
    root.children.push(child);
    tokio::time::advance(Duration::from_millis(100)).await;
    root.end();

    let json = serde_json::to_value(&root).unwrap();
    assert_eq!(json["name"], "Cycle 1");
    assert_eq!(json["metadata"]["cycleId"], "cycle-1");
    assert_eq!(json["duration"], 200);
    assert_eq!(
        json["children"],
        json!([{
            "id": child_id,
            "name": "stream_messages",
            "parentId": root.id(),
            "startTime": 1001,
            "endTime": 1100,
            "duration": 99,
            "children": [],
            "metadata": {},
            "message": null,
        }])
    );
}

#[tokio::test(start_paused = true)]
async fn serializes_tool_metadata_correctly() {
    let mut tool = started_at("Tool: calc", 1000);
    tool.metadata
        .insert("toolUseId".to_owned(), "tool-123".to_owned());
    tool.metadata.insert("toolName".to_owned(), "calc".to_owned());
    tokio::time::advance(Duration::from_millis(500)).await;
    tool.end();

    let json = serde_json::to_value(&tool).unwrap();
    assert_eq!(json["name"], "Tool: calc");
    assert_eq!(
        json["metadata"],
        json!({ "toolUseId": "tool-123", "toolName": "calc" })
    );
    assert_eq!(json["duration"], 500);
}
