//! Conformance: every Phase 0 fixture a type in this crate covers parses and re-serializes to the
//! exact bytes strands-ts wrote (`strands-rs/conformance/fixtures/`, compact `JSON.stringify` output).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::interrupt::state::InterruptStateData;
use crate::interrupt::{Interrupt, InterruptResponseContent};
use crate::mime::{AudioFormat, DocumentFormat, ImageFormat, VideoFormat, to_media_format, to_mime_type};
use crate::models::streaming::{Metrics, Usage};
use crate::types::json::{JsonObject, JsonValue, from_json_str, to_js_string};
use crate::types::media::bytes_from_json;
use crate::types::messages::{
    ContentBlock, LIMIT_STOP_REASONS, Message, StopReason, SystemPrompt, TextBlock, limit_stop_message,
};
use crate::types::snapshot::{SNAPSHOT_SCHEMA_VERSION, Snapshot, iso_timestamp};
use crate::types::tools::{ToolChoice, ToolSpec};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/fixtures")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(fixtures().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn parse<T: DeserializeOwned>(path: &str) -> T {
    from_json_str(&read(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn js<T: Serialize + ?Sized>(value: &T) -> String {
    to_js_string(value).unwrap()
}

/// Parses `path` as `T`, serializes it again and compares the bytes.
fn assert_round_trip<T: Serialize + DeserializeOwned>(path: &str) -> T {
    let value: T = parse(path);
    assert_eq!(js(&value), read(path), "{path}");
    value
}

/// Fixture files under `dir` (recursively), relative to the fixture root, sorted.
fn files_in(dir: &str) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|e| e == "json") {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let root = fixtures();
    let mut out = Vec::new();
    walk(&root, &root.join(dir), &mut out);
    out.sort();
    out
}

#[test]
fn content_blocks_round_trip_byte_identically() {
    let files = files_in("content_blocks");
    assert_eq!(files.len(), 31);
    for path in &files {
        assert_round_trip::<ContentBlock>(path);
    }
    let unknown: ContentBlock = parse("content_blocks/unknown.json");
    assert!(matches!(unknown, ContentBlock::Unknown(_)));
    let ContentBlock::ToolResult(result) = parse("content_blocks/tool_result_unknown_content.json") else {
        panic!("expected a tool result");
    };
    assert!(matches!(
        result.content()[1],
        crate::types::messages::ToolResultContent::Unknown(_)
    ));
}

#[test]
fn messages_round_trip_byte_identically() {
    for path in [
        "messages/assistant_every_block.json",
        "messages/user_text.json",
        "messages/with_metadata.json",
    ] {
        assert_round_trip::<Message>(path);
    }
    let every: Message = parse("messages/assistant_every_block.json");
    assert_eq!(every.content().len(), 30);
}

#[test]
fn saved_unknown_blocks_stay_in_place_and_strict_parsing_rejects_them_with_the_ts_text() {
    let message: Message = parse("messages/saved_unknown_block.input.json");
    assert_eq!(js(&message), read("messages/saved_unknown_block.json"));
    let input: JsonValue = parse("messages/saved_unknown_block.input.json");
    let error = Message::from_data(input).unwrap_err();
    let expected: JsonValue = parse("messages/strict_unknown_block_error.json");
    assert_eq!(json!({"message": error.to_string()}), expected);
}

#[test]
fn legacy_messages_without_a_tracking_id_mint_one_on_load() {
    let message: Message = parse("messages/legacy_without_tracking_id.input.json");
    let minted = uuid::Uuid::parse_str(message.tracking_id()).unwrap();
    assert_eq!(minted.get_version_num(), 7);
    let mut written: JsonValue = serde_json::to_value(&message).unwrap();
    let mut expected: JsonValue = parse("messages/legacy_without_tracking_id.json");
    written["trackingId"] = json!("minted");
    expected["trackingId"] = json!("minted");
    assert_eq!(js(&written), js(&expected));
}

#[test]
fn system_prompts_round_trip_byte_identically() {
    assert!(matches!(
        assert_round_trip::<SystemPrompt>("messages/system_prompt_string.json"),
        SystemPrompt::Text(_)
    ));
    let SystemPrompt::Blocks(blocks) =
        assert_round_trip::<SystemPrompt>("messages/system_prompt_blocks.json")
    else {
        panic!("expected blocks");
    };
    assert_eq!(blocks.len(), 3);
}

#[test]
fn base64_is_decoded_leniently_and_re_encoded_canonically() {
    let cases = [
        "canonical",
        "trailing_data_after_padding",
        "unpadded",
        "url_safe",
        "url_safe_unpadded",
        "whitespace",
    ];
    for case in cases {
        let message: Message = parse(&format!("edge_cases/base64_{case}.input.json"));
        assert_eq!(
            js(&message),
            read(&format!("edge_cases/base64_{case}.json")),
            "{case}"
        );
    }
}

#[test]
fn created_at_keeps_to_iso_string_milliseconds() {
    let fixture: JsonObject = parse("edge_cases/created_at_milliseconds.json");
    for value in fixture.values() {
        let text = value.as_str().unwrap();
        let timestamp: jiff::Timestamp = text.parse().unwrap();
        assert_eq!(iso_timestamp(timestamp), text);
    }
}

#[test]
fn numbers_in_tool_input_keep_js_formatting() {
    let block: ContentBlock = assert_round_trip("edge_cases/numbers_in_tool_input.json");
    let ContentBlock::ToolUse(tool_use) = block else {
        panic!("expected a tool use")
    };
    assert_eq!(tool_use.input()["maxSafe"], json!(9_007_199_254_740_991_u64));
    assert_eq!(js(&tool_use.input()["large"]), "1e+21");
}

#[test]
fn text_escapes_match_json_stringify() {
    assert_round_trip::<ContentBlock>("edge_cases/text_escapes.json");
}

#[test]
fn lone_surrogates_load_as_replacement_characters() {
    let blocks: Vec<ContentBlock> = parse("edge_cases/text_lone_surrogates.json");
    let texts: Vec<ContentBlock> = ["high \u{fffd} alone", "low \u{fffd} alone", "pair 😀 ok"]
        .into_iter()
        .map(|t| TextBlock::new(t).into())
        .collect();
    assert_eq!(blocks, texts);
    assert_eq!(
        js(&blocks),
        "[{\"text\":\"high \u{fffd} alone\"},{\"text\":\"low \u{fffd} alone\"},{\"text\":\"pair 😀 ok\"}]"
    );
}

#[test]
fn uint8array_index_objects_in_stream_events_read_as_bytes() {
    let event: JsonValue = parse("edge_cases/reasoning_redacted_uint8array_in_stream_event.json");
    assert_eq!(
        bytes_from_json(&event["delta"]["redactedContent"]).unwrap(),
        [0, 128, 255]
    );
}

#[test]
fn interrupts_round_trip_byte_identically() {
    let interrupts: Vec<Interrupt> = assert_round_trip("interrupts/agent_result_interrupts.json");
    assert_eq!(interrupts[0].source().as_str(), "tool");
    let responses: Vec<InterruptResponseContent> = assert_round_trip("interrupts/response_content_null.json");
    assert_eq!(responses[0].interrupt_response().response(), &JsonValue::Null);
    for path in files_in("interrupts")
        .iter()
        .filter(|p| p.starts_with("interrupts/state_"))
    {
        assert_round_trip::<InterruptStateData>(path);
    }
}

#[test]
fn a_null_response_counts_as_answered_after_the_round_trip() {
    let state: InterruptStateData = parse("interrupts/state_round_trip.json");
    let unanswered: Vec<&Interrupt> = state.unanswered().collect();
    assert_eq!(
        js(&unanswered),
        read("interrupts/unanswered_after_null_response.json")
    );
}

#[test]
fn media_format_fixtures_use_known_formats_and_round_trip() {
    for path in [
        "media_formats/audio.json",
        "media_formats/document.json",
        "media_formats/image.json",
        "media_formats/video.json",
    ] {
        let blocks: Vec<ContentBlock> = assert_round_trip(path);
        for block in &blocks {
            let known = match block {
                ContentBlock::Audio(b) => !matches!(b.format(), AudioFormat::Other(_)),
                ContentBlock::Document(b) => !matches!(b.format(), DocumentFormat::Other(_)),
                ContentBlock::Image(b) => !matches!(b.format(), ImageFormat::Other(_)),
                ContentBlock::Video(b) => !matches!(b.format(), VideoFormat::Other(_)),
                _ => false,
            };
            assert!(known, "{path}: {block:?}");
        }
    }
    let ContentBlock::Image(image) = assert_round_trip::<ContentBlock>("media_formats/unknown_format.json")
    else {
        panic!("expected an image");
    };
    assert_eq!(image.format().as_str(), "heic");
}

#[test]
fn mime_mappings_match_ts() {
    let to_mime: JsonObject = parse("media_formats/to_mime_type.json");
    for (format, mime) in &to_mime {
        assert_eq!(to_mime_type(format), mime.as_str(), "{format}");
    }
    let to_format: JsonObject = parse("media_formats/to_media_format.json");
    for (mime, format) in &to_format {
        assert_eq!(
            to_media_format(mime).as_ref().map(|f| f.as_str().to_owned()),
            format.as_str().map(str::to_owned),
            "{mime}"
        );
    }
}

/// Round-trips a snapshot and the typed parts of its agent data.
fn assert_snapshot(path: &str) {
    let snapshot: Snapshot = assert_round_trip(path);
    let data = snapshot.data();
    if let Some(messages) = data.get("messages") {
        let typed: Vec<Message> = serde_json::from_value(messages.clone()).unwrap();
        assert_eq!(js(&typed), js(messages), "{path} messages");
    }
    if let Some(prompt) = data.get("systemPrompt") {
        let typed: Option<SystemPrompt> = serde_json::from_value(prompt.clone()).unwrap();
        assert_eq!(js(&typed), js(prompt), "{path} systemPrompt");
    }
    if let Some(interrupts) = data.get("interrupts") {
        let typed: InterruptStateData = serde_json::from_value(interrupts.clone()).unwrap();
        assert_eq!(js(&typed), js(interrupts), "{path} interrupts");
    }
}

#[test]
fn snapshots_round_trip_byte_identically() {
    let files: Vec<String> = files_in("snapshots")
        .into_iter()
        .filter(|p| p != "snapshots/schema_version.json")
        .collect();
    assert_eq!(files.len(), 12);
    for path in &files {
        assert_snapshot(path);
    }
    assert_eq!(
        parse::<String>("snapshots/schema_version.json"),
        SNAPSHOT_SCHEMA_VERSION
    );
}

#[test]
fn session_snapshot_files_round_trip_byte_identically() {
    let files: Vec<String> = files_in("sessions")
        .into_iter()
        .filter(|p| p.contains("/snapshots/") && !p.ends_with("/manifest.json"))
        .collect();
    assert_eq!(files.len(), 6);
    for path in &files {
        assert_snapshot(path);
    }
}

#[test]
fn stop_reasons_round_trip_and_unknown_values_stay_other() {
    let events: Vec<JsonObject> = parse("stop_reasons/all.json");
    assert_eq!(events.len(), 17);
    let mut rebuilt = Vec::new();
    for mut event in events {
        let reason: StopReason = serde_json::from_value(event["stopReason"].clone()).unwrap();
        event.insert("stopReason".to_owned(), serde_json::to_value(&reason).unwrap());
        rebuilt.push((reason, event));
    }
    let others: Vec<&str> = rebuilt
        .iter()
        .filter(|(r, _)| matches!(r, StopReason::Other(_)))
        .map(|(r, _)| r.as_str())
        .collect();
    assert_eq!(others, ["someFutureReason"]);
    let events: Vec<&JsonObject> = rebuilt.iter().map(|(_, e)| e).collect();
    assert_eq!(js(&events), read("stop_reasons/all.json"));
    assert_eq!(
        js(&LIMIT_STOP_REASONS),
        read("stop_reasons/limit_stop_reasons.json")
    );
    assert_eq!(
        js(&limit_stop_message("my_agent", &StopReason::LimitTurns)),
        read("stop_reasons/limit_stop_message.json")
    );
}

#[test]
fn tool_specs_and_choices_round_trip_byte_identically() {
    for path in [
        "tools/function_tool_default_schema.json",
        "tools/function_tool_spec.json",
        "tools/zod_tool_spec.json",
    ] {
        assert_round_trip::<ToolSpec>(path);
    }
    assert_eq!(
        assert_round_trip::<ToolChoice>("tools/tool_choice_auto.json"),
        ToolChoice::Auto
    );
    assert_eq!(
        assert_round_trip::<ToolChoice>("tools/tool_choice_any.json"),
        ToolChoice::Any
    );
    assert!(matches!(
        assert_round_trip::<ToolChoice>("tools/tool_choice_tool.json"),
        ToolChoice::Tool { .. }
    ));
}

#[test]
fn usage_round_trips_with_integer_counters() {
    assert_eq!(assert_round_trip::<Usage>("usage/empty.json"), Usage::default());
    assert_round_trip::<Usage>("usage/accumulated.json");
    let mut event: JsonObject = parse("usage/large_integers.json");
    let usage: Usage = serde_json::from_value(event["usage"].clone()).unwrap();
    let metrics: Metrics = serde_json::from_value(event["metrics"].clone()).unwrap();
    assert_eq!(usage.total_tokens(), 9_007_199_254_740_991);
    event.insert("usage".to_owned(), serde_json::to_value(usage).unwrap());
    event.insert("metrics".to_owned(), serde_json::to_value(metrics).unwrap());
    assert_eq!(js(&event), read("usage/large_integers.json"));
}
