//! Helpers shared by the conformance round-trip tests.
//!
//! Every fixture is parsed through the saved-data serde path (`serde_json::from_str` on the fixture
//! text), serialized back and compared with the bytes TS wrote:
//!
//! - compact fixtures (`persisted/**`, `types/compact_message.json`) are compared byte for byte with
//!   [`strands::types::json::to_string`];
//! - pretty fixtures are re-serialized with [`strands::types::json::to_string_pretty`] plus the
//!   trailing newline and compared byte for byte too. That printer matches `JSON.stringify(v, null,
//!   2)` (indentation, escapes and JavaScript number formatting), so key order is checked as well.

#![allow(dead_code, reason = "each test crate uses a subset of the helpers")]

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Fixtures that a Rust test in this crate round-trips (paths relative to `fixtures/`).
pub(crate) const COVERED: &[&str] = &[
    "events/model_stream_events.json",
    "interrupts/hook_interrupted.snapshot.json",
    "interrupts/interrupt_state.json",
    "interrupts/tool_interrupted.snapshot.json",
    "interrupts/tool_resumed.snapshot.json",
    "persisted/session_agent/session/conformance-session/scopes/agent/main-agent/snapshots/immutable_history/snapshot_019b76da-a800-71a7-85b2-4531183ebcd2.json",
    "persisted/session_agent/session/conformance-session/scopes/agent/main-agent/snapshots/immutable_history/snapshot_019b76da-a800-71a7-85b2-4aae2bc8222f.json",
    "persisted/session_agent/session/conformance-session/scopes/agent/main-agent/snapshots/snapshot_latest.json",
    "persisted/session_graph/session/graph-session/scopes/multiAgent/conformance-graph/snapshots/snapshot_latest.json",
    "persisted/stash_in_memory/context/stash-session/scopes/agent/stash-agent/00000000-0000-4000-8000-00000000005c_0",
    "persisted/stash_in_memory/context/stash-session/scopes/agent/stash-agent/00000000-0000-4000-8000-000000000060_0",
    "persisted/stash_in_memory/session/stash-session/scopes/agent/stash-agent/snapshots/snapshot_latest.json",
    "persisted/stash_local_file/context/stash-session/scopes/agent/stash-agent/00000000-0000-4000-8000-000000000061_0",
    "persisted/stash_local_file/context/stash-session/scopes/agent/stash-agent/00000000-0000-4000-8000-000000000065_0",
    "persisted/stash_local_file/session/stash-session/scopes/agent/stash-agent/snapshots/snapshot_latest.json",
    "snapshots/take_snapshot.json",
    "snapshots/take_snapshot_empty_agent.json",
    "types/base64_lenient.json",
    "types/compact_message.json",
    "types/content_blocks.json",
    "types/interrupt_response_content.json",
    "types/json_edge_cases.json",
    "types/media_formats.json",
    "types/messages.json",
    "types/metrics_full.json",
    "types/mime.json",
    "types/saved_data_round_trip.json",
    "types/stop_reasons.json",
    "types/strict_parse_errors.json",
    "types/system_prompts.json",
    "types/tool_choice.json",
    "types/usage_metrics.json",
];

/// Fixtures whose types arrive in a later phase.
pub(crate) const DEFERRED: &[&str] = &[
    "agent/interrupt_hook.stream.json",
    "agent/interrupt_hook_resumed.stream.json",
    "agent/interrupt_tool.stream.json",
    "agent/interrupt_tool_resumed.stream.json",
    "agent/model_error.error.json",
    "agent/model_error.stream.json",
    "agent/redaction.stream.json",
    "agent/structured_output.model_calls.json",
    "agent/structured_output.stream.json",
    "agent/text_turn.metrics.json",
    "agent/text_turn.stream.json",
    "agent/tool_turn.model_calls.json",
    "agent/tool_turn.stream.json",
    "hooks/hook_events.json",
    "hooks/text_turn.order.json",
    "hooks/tool_turn.order.json",
    "persisted/session_agent.keys.json",
    "persisted/session_agent/session/conformance-session/scopes/agent/main-agent/snapshots/manifest.json",
    "schemas/tool_input_schemas.json",
    "schemas/tool_specs.json",
    "schemas/zod_to_json_schema.json",
    "snapshots/multi_agent_result.json",
    "snapshots/take_snapshot_errors.json",
    "stash/stash_records.json",
    "telemetry/agent_run.default.json",
    "telemetry/agent_run.latest_experimental.json",
    "telemetry/memory_run.default.json",
];

/// The conformance fixture directory.
pub(crate) fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/fixtures")
}

/// Reads a fixture as text.
pub(crate) fn read(path: &str) -> String {
    let full = fixtures_dir().join(path);
    std::fs::read_to_string(&full)
        .map_err(|error| format!("{}: {error}", full.display()))
        .unwrap()
}

/// Parses a pretty fixture holding named cases into its ordered case map.
pub(crate) fn cases(path: &str) -> Map<String, Value> {
    serde_json::from_str(&read(path)).unwrap()
}

/// Parses `value`'s JSON text as `T` and serializes it back.
pub(crate) fn round_trip<T: DeserializeOwned + Serialize>(value: &Value) -> Value {
    let parsed: T = parse(value);
    serde_json::to_value(&parsed).unwrap()
}

/// Parses `value`'s JSON text as `T`, the way a load from storage does.
pub(crate) fn parse<T: DeserializeOwned>(value: &Value) -> T {
    let text = serde_json::to_string(value).unwrap();
    serde_json::from_str(&text)
        .map_err(|error| format!("{error}: {text}"))
        .unwrap()
}

/// Asserts that `value` pretty-prints to exactly the bytes of the pretty fixture at `path`.
pub(crate) fn assert_pretty_bytes(path: &str, value: &impl Serialize) {
    let written = strands::types::json::to_string_pretty(value).unwrap() + "\n";
    assert_eq!(written, read(path), "{path}");
}

/// Round-trips a pretty fixture whose cases are all of type `T`, parsing the file text directly.
pub(crate) fn assert_pretty_cases<T: DeserializeOwned + Serialize>(path: &str) {
    let parsed: IndexMap<String, T> = serde_json::from_str(&read(path)).unwrap();
    assert_pretty_bytes(path, &parsed);
}

/// Round-trips a pretty fixture holding one value of type `T`.
pub(crate) fn assert_pretty_value<T: DeserializeOwned + Serialize>(path: &str) {
    let parsed: T = serde_json::from_str(&read(path)).unwrap();
    assert_pretty_bytes(path, &parsed);
}

/// Round-trips a compact fixture of type `T` and compares the bytes.
pub(crate) fn assert_compact<T: DeserializeOwned + Serialize>(path: &str) -> T {
    let text = read(path);
    let parsed: T = serde_json::from_str(&text).unwrap();
    assert_eq!(strands::types::json::to_string(&parsed).unwrap(), text, "{path}");
    parsed
}

/// Every fixture file under `fixtures/` except `INDEX.md`, as paths relative to it.
pub(crate) fn all_fixtures() -> Vec<String> {
    let root = fixtures_dir();
    let mut pending = vec![root.clone()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if relative != "INDEX.md" {
                    files.push(relative);
                }
            }
        }
    }
    files.sort();
    files
}
