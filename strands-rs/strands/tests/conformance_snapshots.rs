//! Conformance round-trips for the snapshot fixtures: `takeSnapshot` output, snapshots holding
//! interrupt state, and the persisted snapshot and stash bytes (`conformance/fixtures/snapshots/`,
//! `interrupts/`, `persisted/`). See `common/mod.rs` for the comparison method.
//!
//! `Snapshot.data` is framework-owned JSON; the parts Phase 1 has types for (`messages`,
//! `systemPrompt`) are parsed into those types and written back before the bytes are compared. The
//! interrupt state is crate-internal and is round-tripped by the unit tests in `interrupt.rs`.

#![expect(clippy::unwrap_used, reason = "test code")]

mod common;

use indexmap::IndexMap;
use serde_json::Value;
use strands::types::messages::{ContentBlock, Message, SystemPrompt};
use strands::types::snapshot::{SNAPSHOT_SCHEMA_VERSION, Scope, Snapshot};

use common::{COVERED, assert_compact, assert_pretty_bytes, parse};

/// Parses a snapshot and the typed parts of its data, and writes them back.
fn typed(mut snapshot: Snapshot) -> Snapshot {
    if let Some(messages) = snapshot.data.get_mut("messages") {
        *messages = common::round_trip::<Vec<Message>>(messages);
    }
    if let Some(system_prompt) = snapshot.data.get_mut("systemPrompt") {
        *system_prompt = common::round_trip::<Option<SystemPrompt>>(system_prompt);
    }
    snapshot
}

fn assert_pretty_snapshot(path: &str) -> Snapshot {
    let snapshot = typed(serde_json::from_str(&common::read(path)).unwrap());
    assert_pretty_bytes(path, &snapshot);
    snapshot
}

fn persisted(suffix: &str) -> Vec<&'static str> {
    COVERED
        .iter()
        .copied()
        .filter(|path| path.starts_with("persisted/") && path.ends_with(suffix))
        .collect()
}

#[test]
fn take_snapshot_output_round_trips() {
    let path = "snapshots/take_snapshot.json";
    let parsed: IndexMap<String, Snapshot> = serde_json::from_str(&common::read(path)).unwrap();
    let typed: IndexMap<String, Snapshot> = parsed
        .into_iter()
        .map(|(name, snapshot)| (name, typed(snapshot)))
        .collect();
    assert_pretty_bytes(path, &typed);
    let with_app_data = &typed["withAppData"];
    assert_eq!(with_app_data.app_data["label"], "checkpoint-1");
    assert_eq!(with_app_data.created_at, "2026-01-01T00:00:00.000Z");
}

#[test]
fn an_empty_agent_snapshot_keeps_its_null_fields() {
    let snapshot = assert_pretty_snapshot("snapshots/take_snapshot_empty_agent.json");
    assert_eq!(snapshot.scope, Scope::Agent);
    assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(snapshot.data["systemPrompt"], Value::Null);
    assert_eq!(snapshot.data["interrupts"]["activated"], Value::Bool(false));
}

#[test]
fn snapshots_with_interrupt_state_round_trip() {
    for path in [
        "interrupts/hook_interrupted.snapshot.json",
        "interrupts/tool_interrupted.snapshot.json",
        "interrupts/tool_resumed.snapshot.json",
    ] {
        assert_pretty_snapshot(path);
    }
}

#[test]
fn persisted_snapshots_round_trip_byte_for_byte() {
    let paths = persisted(".json");
    assert_eq!(paths.len(), 6);
    for path in paths {
        let snapshot: Snapshot = assert_compact(path);
        let text = strands::types::json::to_string(&typed(snapshot)).unwrap();
        assert_eq!(text, common::read(path), "{path}");
    }
}

#[test]
fn the_multi_agent_scope_loads() {
    let path = "persisted/session_graph/session/graph-session/scopes/multiAgent/conformance-graph/snapshots/snapshot_latest.json";
    let snapshot: Snapshot = assert_compact(path);
    assert_eq!(snapshot.scope, Scope::MultiAgent);
    let results = snapshot.data["state"]["results"].as_array().unwrap();
    for result in results {
        let _: Vec<ContentBlock> = parse(&result["content"]);
    }
}

#[test]
fn persisted_stash_record_bodies_round_trip_as_content_blocks() {
    let paths = persisted("_0");
    assert_eq!(paths.len(), 4);
    for path in paths {
        let block: ContentBlock = assert_compact(path);
        assert!(matches!(block, ContentBlock::Text(_)), "{path}");
    }
}
