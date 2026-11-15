//! Conformance: every Phase 0 fixture a type in this crate covers parses and re-serializes to the
//! exact bytes strands-ts wrote (`strands-rs/conformance/fixtures/`, compact `JSON.stringify` output).

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::interrupt::state::InterruptStateData;
use crate::interrupt::{Interrupt, InterruptResponseContent};
use crate::mime::{AudioFormat, DocumentFormat, ImageFormat, VideoFormat, to_media_format, to_mime_type};
use crate::models::streaming::{Metrics, Usage};
use crate::types::agent::AgentStreamEvent;
use crate::types::json::{JsonObject, JsonValue, from_json_str, to_js_string};
use crate::types::media::bytes_from_json;
use crate::types::messages::{
    ContentBlock, LIMIT_STOP_REASONS, Message, Role, StopReason, SystemPrompt, TextBlock, ToolUseBlock,
    limit_stop_message,
};
use crate::types::snapshot::{SNAPSHOT_SCHEMA_VERSION, Scope, Snapshot, iso_timestamp};
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
fn nesting_deeper_than_the_serde_json_limit_round_trips() {
    assert_round_trip::<ContentBlock>("edge_cases/deep_nesting.json");
}

#[test]
fn a_snapshot_with_200_levels_of_tool_input_reads_back_what_rust_wrote() {
    let mut input = json!({"leaf": true});
    for _ in 1..200 {
        input = json!({"nested": input});
    }
    let message = Message::new(
        Role::Assistant,
        vec![ToolUseBlock::new("deep", "tooluse_deep", input).into()],
    );
    let mut data = JsonObject::new();
    data.insert("messages".into(), serde_json::to_value(vec![&message]).unwrap());
    let written = js(&Snapshot::new(Scope::Agent, data, JsonObject::new()));
    let snapshot: Snapshot = from_json_str(&written).unwrap();
    assert_eq!(js(&snapshot), written);
    let messages: Vec<Message> = serde_json::from_value(snapshot.data()["messages"].clone()).unwrap();
    assert_eq!(messages, [message]);
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

/// Loads every agent-scope snapshot fixture into a fresh agent and takes it again with the same
/// fields: the result is byte-identical. Snapshots with a context-manager `stash` wait for
/// Phase 6.
#[test]
fn agent_snapshot_fixtures_survive_load_snapshot_and_take_snapshot() {
    use crate::agent::{Agent, SnapshotField, TakeSnapshotOptions};

    let files: Vec<String> = files_in("snapshots")
        .into_iter()
        .chain(files_in("sessions"))
        .filter(|path| path.contains("snapshot") && !path.ends_with("manifest.json"))
        .filter(|path| parse::<JsonValue>(path)["scope"] == "agent")
        .filter(|path| parse::<JsonValue>(path)["data"].get("stash").is_none())
        .collect();
    assert_eq!(files.len(), 13, "{files:?}");
    for path in &files {
        let fixture: Snapshot = parse(path);
        let mut agent = Agent::builder()
            .model(crate::testing::MockMessageModel::new())
            .build()
            .unwrap();
        agent.load_snapshot(&fixture).unwrap();
        let options = TakeSnapshotOptions {
            include: fixture
                .data()
                .keys()
                .map(|key| key.parse::<SnapshotField>().unwrap())
                .collect(),
            app_data: fixture.app_data().clone(),
            ..TakeSnapshotOptions::default()
        };
        let taken = agent
            .take_snapshot(&options)
            .unwrap()
            .with_created_at(fixture.created_at());
        assert_eq!(js(&taken), read(path), "{path}");
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

#[test]
fn model_stream_events_round_trip_byte_identically() {
    use crate::models::streaming::{ContentBlockDelta, ModelStreamEvent};

    let files = files_in("model_stream_events");
    assert_eq!(files.len(), 20);
    for path in &files {
        let event: ModelStreamEvent = assert_round_trip(path);
        let written: JsonValue = parse(path);
        assert_eq!(written["type"], event.event_type(), "{path}");
    }
    let ModelStreamEvent::BlockDelta(event) = parse("model_stream_events/delta_reasoning_redacted.json")
    else {
        panic!("expected a delta");
    };
    let ContentBlockDelta::ReasoningContent(delta) = event.delta() else {
        panic!("expected a reasoning delta");
    };
    assert_eq!(delta.redacted_content(), Some(&[0, 1, 2, 255][..]));
    let ModelStreamEvent::MessageStop(stop) =
        parse("model_stream_events/message_stop_additional_fields_null.json")
    else {
        panic!("expected a message stop");
    };
    assert_eq!(stop.additional_model_response_fields(), Some(&JsonValue::Null));
}

#[test]
fn model_requests_carry_the_fields_the_ts_model_received() {
    use crate::models::model::ModelRequest;

    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Recorded {
        messages: Vec<Message>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_specs: Vec<ToolSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<SystemPrompt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_choice: Option<ToolChoice>,
    }

    for path in files_in("model_requests") {
        let recorded: Vec<Recorded> = assert_round_trip(&path);
        for call in recorded {
            let request = ModelRequest {
                messages: call.messages,
                tool_specs: call.tool_specs,
                system_prompt: call.system_prompt,
                tool_choice: call.tool_choice,
                ..Default::default()
            };
            assert!(!request.messages.is_empty(), "{path}");
        }
    }
}

/// Builds the live hook event a stream event fixture entry was serialized from, and returns its
/// stream record.
fn stream_event_from(entry: &JsonValue, agent: &mut crate::types::agent::LocalAgent) -> AgentStreamEvent {
    use std::sync::Arc;

    use crate::hooks::{
        AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent,
        BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent,
        ContentBlockEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent, ModelStopData,
        ModelStreamUpdateEvent, Redaction, ToolResultEvent, ToolStreamUpdateEvent,
    };
    use crate::tools::ToolStreamEvent;
    use crate::types::agent::{AgentResult, InvocationState};
    use crate::types::messages::ToolResultBlock;
    use crate::types::tools::ToolUse;

    fn field<T: DeserializeOwned>(entry: &JsonValue, key: &str) -> T {
        serde_json::from_value(entry[key].clone()).unwrap_or_else(|e| panic!("{key}: {e}"))
    }
    let state = InvocationState::new;
    let tool_result = |entry: &JsonValue| -> ToolResultBlock {
        serde_json::from_value(entry["result"]["toolResult"].clone()).unwrap()
    };
    let model = Arc::clone(agent.model());
    match entry["type"].as_str().unwrap() {
        "modelStreamUpdateEvent" => AgentStreamEvent::ModelStreamUpdate(
            (&ModelStreamUpdateEvent::new(agent, field(entry, "event"), state())).into(),
        ),
        "contentBlockEvent" => AgentStreamEvent::ContentBlock(
            (&ContentBlockEvent::new(agent, field(entry, "contentBlock"), state())).into(),
        ),
        "modelMessageEvent" => AgentStreamEvent::ModelMessage(
            (&ModelMessageEvent::new(
                agent,
                field(entry, "message"),
                field(entry, "stopReason"),
                state(),
            ))
                .into(),
        ),
        "toolStreamUpdateEvent" => {
            let event = match entry["event"].get("data") {
                Some(data) => ToolStreamEvent::new(data.clone()),
                None => ToolStreamEvent::default(),
            };
            AgentStreamEvent::ToolStreamUpdate(
                (&ToolStreamUpdateEvent::new(agent.tool_agent(), event, state())).into(),
            )
        }
        "toolResultEvent" => AgentStreamEvent::ToolResult(
            (&ToolResultEvent::new(agent.tool_agent(), tool_result(entry), state())).into(),
        ),
        "beforeInvocationEvent" => AgentStreamEvent::BeforeInvocation(
            (&BeforeInvocationEvent::new(agent, state(), Vec::new())).into(),
        ),
        "afterInvocationEvent" => {
            AgentStreamEvent::AfterInvocation((&AfterInvocationEvent::new(agent, state())).into())
        }
        "beforeModelCallEvent" => {
            let projected = entry
                .get("projectedInputTokens")
                .map(|tokens| tokens.as_u64().unwrap());
            AgentStreamEvent::BeforeModelCall(
                (&BeforeModelCallEvent::new(agent, model, state(), projected)).into(),
            )
        }
        "afterModelCallEvent" => {
            let attempt_count =
                NonZeroU32::new(u32::try_from(entry["attemptCount"].as_u64().unwrap()).unwrap()).unwrap();
            let error = entry
                .get("error")
                .map(|error| crate::Error::other(error["message"].as_str().unwrap().to_owned()));
            let mut event = AfterModelCallEvent::new(agent, model, state(), attempt_count);
            if let Some(stop_data) = entry.get("stopData") {
                let mut data =
                    ModelStopData::new(field(stop_data, "message"), field(stop_data, "stopReason"));
                if let Some(redaction) = stop_data.get("redaction") {
                    data = data.with_redaction(Redaction::new(redaction["userMessage"].as_str().unwrap()));
                }
                event = event.with_stop_data(data);
            }
            if let Some(error) = &error {
                event = event.with_error(error);
            }
            AgentStreamEvent::AfterModelCall((&event).into())
        }
        "beforeToolsEvent" => AgentStreamEvent::BeforeTools(
            (&BeforeToolsEvent::new(agent, field(entry, "message"), state())).into(),
        ),
        "afterToolsEvent" => AgentStreamEvent::AfterTools(
            (&AfterToolsEvent::new(agent, field(entry, "message"), state())).into(),
        ),
        "beforeToolCallEvent" => {
            let tool_use: ToolUse = field(entry, "toolUse");
            AgentStreamEvent::BeforeToolCall(
                (&BeforeToolCallEvent::new(agent.tool_agent(), tool_use, None, state())).into(),
            )
        }
        "afterToolCallEvent" => {
            let error = entry.get("error").map(|error| {
                let error: Arc<dyn std::error::Error + Send + Sync> = Arc::new(std::io::Error::other(
                    error["message"].as_str().unwrap().to_owned(),
                ));
                error
            });
            let event = AfterToolCallEvent::new(
                agent.tool_agent(),
                field(entry, "toolUse"),
                None,
                tool_result(entry),
                state(),
            )
            .with_error(error);
            AgentStreamEvent::AfterToolCall((&event).into())
        }
        "messageAddedEvent" => AgentStreamEvent::MessageAdded(
            (&MessageAddedEvent::new(agent, field(entry, "message"), state())).into(),
        ),
        "interruptEvent" => AgentStreamEvent::Interrupt(
            (&InterruptEvent::new(agent, field(entry, "interrupt"), state())).into(),
        ),
        "agentResultEvent" => {
            let result = &entry["result"];
            let mut agent_result =
                AgentResult::new(field(result, "stopReason"), field(result, "lastMessage"), state());
            if let Some(output) = result.get("structuredOutput") {
                agent_result = agent_result.with_structured_output(output.clone());
            }
            AgentStreamEvent::AgentResult(agent_result)
        }
        other => panic!("unexpected event type {other}"),
    }
}

fn test_agent() -> crate::types::agent::LocalAgent {
    crate::types::agent::LocalAgent::new(std::sync::Arc::new(crate::testing::MockMessageModel::new()))
}

#[test]
fn agent_stream_events_serialize_byte_identically() {
    let mut agent = test_agent();
    let files: Vec<String> = files_in("agent_stream_events")
        .into_iter()
        .filter(|path| !path.ends_with("_types.json"))
        .collect();
    assert_eq!(files.len(), 4);
    let mut seen = std::collections::BTreeSet::new();
    for path in &files {
        let entries: Vec<JsonValue> = parse(path);
        let events: Vec<AgentStreamEvent> = entries
            .iter()
            .map(|entry| stream_event_from(entry, &mut agent))
            .collect();
        assert_eq!(js(&events), read(path), "{path}");
        seen.extend(
            entries
                .iter()
                .map(|entry| entry["type"].as_str().unwrap().to_owned()),
        );
    }
    assert_eq!(seen.len(), 16, "every stream event type is covered: {seen:?}");

    let types: Vec<String> = parse("agent_stream_events/tool_run_types.json");
    let entries: Vec<JsonValue> = parse("agent_stream_events/tool_run.json");
    let written: Vec<&str> = entries
        .iter()
        .map(|entry| entry["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, written);
}

#[test]
fn hook_events_serialize_byte_identically() {
    use crate::hooks::InitializedEvent;

    let mut agent = test_agent();
    assert_eq!(
        js(&InitializedEvent::new(&mut agent)),
        read("hook_events/initialized.json")
    );
    let files: Vec<String> = files_in("hook_events")
        .into_iter()
        .filter(|path| !path.ends_with("initialized.json") && !path.ends_with("tool_run_order.json"))
        .collect();
    assert_eq!(files.len(), 6);
    for path in &files {
        let entry: JsonValue = parse(path);
        assert_eq!(js(&stream_event_from(&entry, &mut agent)), read(path), "{path}");
    }
}

/// Replays scripted model turns, one per call.
struct ScriptedModel {
    turns: std::sync::Mutex<std::collections::VecDeque<Vec<crate::models::streaming::ModelStreamEvent>>>,
}

impl crate::models::model::Model for ScriptedModel {
    fn stream(
        &self,
        _request: crate::models::model::ModelRequest,
    ) -> crate::models::model::BoxStream<'static, crate::Result<crate::models::streaming::ModelStreamEvent>>
    {
        let turn = self.turns.lock().unwrap().pop_front().expect("a scripted turn");
        Box::pin(futures::stream::iter(turn.into_iter().map(Ok)))
    }

    fn get_config(&self) -> crate::models::model::BaseModelConfig {
        crate::models::model::BaseModelConfig::default()
    }

    fn update_config(&self, _config: crate::models::model::BaseModelConfig) {}
}

fn without_tracking_ids(value: &mut JsonValue) {
    match value {
        JsonValue::Object(map) => {
            map.remove("trackingId");
            map.values_mut().for_each(without_tracking_ids);
        }
        JsonValue::Array(items) => items.iter_mut().for_each(without_tracking_ids),
        _ => {}
    }
}

/// The TS `toolRun` scenario (`conformance/generate.ts`) through the real agent loop: the stream
/// matches `agent_stream_events/tool_run.json` event for event (tracking ids aside) and the hooks
/// fire in `hook_events/tool_run_order.json` order.
#[tokio::test(flavor = "current_thread")]
async fn the_agent_loop_reproduces_the_ts_tool_run() {
    use futures::StreamExt;

    use crate::agent::Agent;
    use crate::testing::HookRecorder;
    use crate::tools::{
        FunctionTool, Tool, ToolContext, ToolError, ToolOutcome, ToolStream, ToolStreamEvent,
    };
    use crate::types::messages::ToolResultBlock;

    struct MediaTool(ToolResultBlock);

    impl Tool for MediaTool {
        fn name(&self) -> &str {
            "fetch_media"
        }
        fn description(&self) -> &str {
            "Returns mixed media."
        }
        fn tool_spec(&self) -> ToolSpec {
            ToolSpec::new("fetch_media", "Returns mixed media.").with_input_schema(
                json!({"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}),
            )
        }
        fn stream(&self, _context: ToolContext) -> ToolStream {
            let result = self.0.clone();
            ToolStream::new(futures::stream::iter([
                ToolStreamEvent::new(json!({"progress": 0.5, "stage": "download"})).into(),
                ToolStreamEvent::new(json!("almost done")).into(),
                ToolOutcome::Result(result).into(),
            ]))
        }
    }

    let entries: Vec<JsonValue> = parse("agent_stream_events/tool_run.json");
    let mut turns = std::collections::VecDeque::new();
    let mut turn = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| entry["type"] == "modelStreamUpdateEvent")
    {
        let event: crate::models::streaming::ModelStreamEvent =
            serde_json::from_value(entry["event"].clone()).unwrap();
        let ends_turn = entry["event"]["type"] == "modelMetadataEvent";
        turn.push(event);
        if ends_turn {
            turns.push_back(std::mem::take(&mut turn));
        }
    }
    let media_result: ToolResultBlock = entries
        .iter()
        .find(|entry| entry["type"] == "afterToolCallEvent" && entry["toolUse"]["name"] == "fetch_media")
        .map(|entry| serde_json::from_value(entry["result"]["toolResult"].clone()).unwrap())
        .unwrap();

    let weather = FunctionTool::new(
        "get_weather",
        "Get the weather for a city.",
        |input: JsonValue, _ctx| async move {
            Ok::<_, ToolError>(json!({"city": input["city"], "forecast": "sunny", "high": 21}))
        },
    )
    .with_input_schema(json!({
        "type": "object",
        "properties": {"city": {"type": "string", "description": "City name"}},
        "required": ["city"],
        "additionalProperties": false
    }));
    let flaky = FunctionTool::new(
        "flaky_tool",
        "Always fails.",
        |_input: JsonValue, _ctx| async move { Err::<JsonValue, _>(ToolError::msg("flaky_tool exploded")) },
    );
    let mut app_state = JsonObject::new();
    app_state.insert("user".into(), json!({"id": "u-1", "tier": "pro"}));
    app_state.insert("counter".into(), json!(3));
    let system_prompt: SystemPrompt = serde_json::from_value(
        json!([{"text": "You report the weather."}, {"cachePoint": {"cacheType": "default"}}]),
    )
    .unwrap();

    let mut agent = Agent::builder()
        .name("weather_agent")
        .id("weather_agent")
        .model(ScriptedModel { turns: turns.into() })
        .tools(crate::tool_list![weather, MediaTool(media_result), flaky])
        .system_prompt(system_prompt)
        .app_state(app_state)
        .build()
        .unwrap();
    let recorder = HookRecorder::new();
    recorder.register(agent.hooks());

    let mut written = Vec::new();
    {
        let mut stream = agent.stream("What is the weather in Seattle?");
        while let Some(event) = stream.next().await {
            let mut event = serde_json::to_value(event.unwrap()).unwrap();
            without_tracking_ids(&mut event);
            written.push(event);
        }
    }
    let mut expected = entries;
    expected.iter_mut().for_each(without_tracking_ids);
    // Tools run on their own tasks, so only each tool's own event order is TS's (section 8.8);
    // everything outside the batch matches exactly.
    let batch = |events: &[JsonValue]| {
        let start = events
            .iter()
            .position(|e| e["type"] == "beforeToolsEvent")
            .unwrap();
        let end = events
            .iter()
            .position(|e| e["type"] == "afterToolsEvent")
            .unwrap();
        (start, end)
    };
    let per_tool = |events: &[JsonValue]| {
        let mut by_tool: std::collections::BTreeMap<String, Vec<JsonValue>> = Default::default();
        for event in events {
            let id = event["toolUse"]["toolUseId"]
                .as_str()
                .or_else(|| event["result"]["toolResult"]["toolUseId"].as_str())
                .unwrap_or("tooluse_run_2"); // the one streaming tool
            by_tool.entry(id.to_owned()).or_default().push(event.clone());
        }
        by_tool
    };
    let (start, end) = batch(&written);
    assert_eq!((start, end), batch(&expected));
    assert_eq!(written.len(), expected.len());
    assert_eq!(written[..=start], expected[..=start]);
    assert_eq!(written[end..], expected[end..]);
    assert_eq!(
        per_tool(&written[start + 1..end]),
        per_tool(&expected[start + 1..end])
    );

    let order: Vec<String> = parse("hook_events/tool_run_order.json");
    let fired = recorder.events();
    let types: Vec<&str> = written
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(fired[0], "initializedEvent");
    assert_eq!(fired[1..], types[..], "hooks fire in stream order");
    let (start, end) = (start + 1, end + 1);
    assert_eq!(fired[..=start], order[..=start]);
    assert_eq!(fired[end..], order[end..]);
    let mut fired_batch: Vec<&str> = fired[start + 1..end].to_vec();
    let mut order_batch: Vec<&str> = order[start + 1..end].iter().map(String::as_str).collect();
    fired_batch.sort_unstable();
    order_batch.sort_unstable();
    assert_eq!(fired_batch, order_batch);

    use crate::agent::{SnapshotField, SnapshotPreset, TakeSnapshotOptions};
    let session = TakeSnapshotOptions::session();
    assert_snapshot_matches(
        &agent.take_snapshot(&session).unwrap(),
        "snapshots/agent_session_preset.json",
    );
    let mut app_data = JsonObject::new();
    app_data.insert("userId".into(), json!("u-123"));
    app_data.insert("tags".into(), json!(["a"]));
    let subset = TakeSnapshotOptions {
        include: vec![SnapshotField::Messages, SnapshotField::State],
        app_data,
        ..TakeSnapshotOptions::default()
    };
    assert_snapshot_matches(
        &agent.take_snapshot(&subset).unwrap(),
        "snapshots/agent_include_subset.json",
    );
    let exclude = TakeSnapshotOptions {
        preset: Some(SnapshotPreset::Session),
        exclude: vec![SnapshotField::Interrupts, SnapshotField::Messages],
        ..TakeSnapshotOptions::default()
    };
    assert_snapshot_matches(
        &agent.take_snapshot(&exclude).unwrap(),
        "snapshots/agent_exclude_interrupts.json",
    );

    let mut bare = Agent::builder()
        .id("bare_agent")
        .model(ScriptedModel {
            turns: Default::default(),
        })
        .build()
        .unwrap();
    assert_snapshot_matches(
        &bare.take_snapshot(&session).unwrap(),
        "snapshots/agent_empty.json",
    );
    bare.set_system_prompt(Some("A plain string prompt.".into()));
    let prompt = TakeSnapshotOptions {
        include: vec![SnapshotField::SystemPrompt, SnapshotField::ModelState],
        ..TakeSnapshotOptions::default()
    };
    assert_snapshot_matches(
        &bare.take_snapshot(&prompt).unwrap(),
        "snapshots/agent_system_prompt_string.json",
    );
}

/// Compares a snapshot the Rust agent took with the TS fixture, creation times and message
/// tracking ids aside.
fn assert_snapshot_matches(snapshot: &Snapshot, path: &str) {
    let normalize = |mut value: JsonValue| {
        value["createdAt"] = json!("");
        normalize_turns(&mut value);
        without_tracking_ids(&mut value);
        js(&value)
    };
    assert_eq!(
        normalize(serde_json::to_value(snapshot).unwrap()),
        normalize(parse(path)),
        "{path}"
    );
}

/// Clears the creation time of child turns stored in interrupt context.
fn normalize_turns(value: &mut JsonValue) {
    if let Some(turns) = value["data"]["interrupts"]["context"]["subAgentInterruptedTurns"].as_object_mut() {
        for turn in turns.values_mut() {
            turn["createdAt"] = json!("");
        }
    }
}

/// TS `textTurn` and `toolUseTurn` from `conformance/generate.ts`.
fn text_turn(text: &str) -> Vec<crate::models::streaming::ModelStreamEvent> {
    scripted_turn(&json!([
        {"type": "modelMessageStartEvent", "role": "assistant"},
        {"type": "modelContentBlockStartEvent"},
        {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": text}},
        {"type": "modelContentBlockStopEvent"},
        {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
        {"type": "modelMetadataEvent", "usage": {"inputTokens": 12, "outputTokens": 5, "totalTokens": 17}, "metrics": {"latencyMs": 21}},
    ]))
}

fn tool_use_turn(uses: &[(&str, &str, JsonValue)]) -> Vec<crate::models::streaming::ModelStreamEvent> {
    let mut events = vec![json!({"type": "modelMessageStartEvent", "role": "assistant"})];
    for (name, tool_use_id, input) in uses {
        events.extend([
            json!({"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "name": name, "toolUseId": tool_use_id}}),
            json!({"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": input.to_string()}}),
            json!({"type": "modelContentBlockStopEvent"}),
        ]);
    }
    events.extend([
        json!({"type": "modelMessageStopEvent", "stopReason": "toolUse"}),
        json!({"type": "modelMetadataEvent", "usage": {"inputTokens": 30, "outputTokens": 14, "totalTokens": 44}, "metrics": {"latencyMs": 40, "timeToFirstByteMs": 9}}),
    ]);
    scripted_turn(&JsonValue::Array(events))
}

fn scripted_turn(events: &JsonValue) -> Vec<crate::models::streaming::ModelStreamEvent> {
    serde_json::from_value(events.clone()).unwrap()
}

fn scripted(turns: Vec<Vec<crate::models::streaming::ModelStreamEvent>>) -> ScriptedModel {
    ScriptedModel {
        turns: std::sync::Mutex::new(turns.into()),
    }
}

/// The TS `interruptRun` scenario: the snapshots while the interrupt is pending, at
/// `BeforeInvocationEvent` of the resume with a `null` answer, and after the resumed run.
#[tokio::test]
async fn the_agent_loop_reproduces_the_ts_interrupt_run_snapshots() {
    use crate::agent::{Agent, SnapshotField, TakeSnapshotOptions};
    use crate::hooks::BeforeInvocationEvent;
    use crate::interrupt::{InterruptParams, InterruptResponse};
    use crate::tools::{FunctionTool, ToolContext, ToolError};

    let confirm = FunctionTool::new(
        "confirm_tool",
        "Asks for confirmation.",
        |_input: JsonValue, ctx: ToolContext| async move {
            let answer = ctx.interrupt(InterruptParams {
                name: "confirm".into(),
                reason: Some(json!({"question": "Proceed?", "risk": 2})),
                ..Default::default()
            })?;
            Ok::<_, ToolError>(json!({"answer": answer}))
        },
    );
    let weather = FunctionTool::new(
        "get_weather",
        "Get the weather for a city.",
        |input: JsonValue, _ctx| async move {
            Ok::<_, ToolError>(json!({"city": input["city"], "forecast": "sunny", "high": 21}))
        },
    );
    let model = scripted(vec![
        tool_use_turn(&[
            ("confirm_tool", "tooluse_confirm_1", json!({})),
            ("get_weather", "tooluse_sibling_1", json!({"city": "Rome"})),
        ]),
        text_turn("Confirmed."),
    ]);
    let mut agent = Agent::builder()
        .id("interrupt_agent")
        .model(model)
        .tools(crate::tool_list![confirm, weather])
        .build()
        .unwrap();
    let first = agent.invoke("Do the risky thing").await.unwrap();
    let session = TakeSnapshotOptions::session();
    assert_snapshot_matches(
        &agent.take_snapshot(&session).unwrap(),
        "snapshots/agent_pending_interrupt.json",
    );

    let resumed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let slot = std::sync::Arc::clone(&resumed);
    agent.add_hook(move |event: &mut BeforeInvocationEvent<'_>| {
        let interrupts = TakeSnapshotOptions {
            include: vec![SnapshotField::Interrupts],
            ..TakeSnapshotOptions::default()
        };
        slot.lock()
            .unwrap()
            .get_or_insert_with(|| event.agent().take_snapshot(&interrupts).unwrap());
    });
    let responses: Vec<InterruptResponse> = first
        .interrupts()
        .iter()
        .map(|interrupt| InterruptResponse::new(interrupt.id(), JsonValue::Null))
        .collect();
    agent.invoke(responses).await.unwrap();
    let resumed = resumed.lock().unwrap().take().unwrap();
    assert_snapshot_matches(&resumed, "snapshots/agent_resuming_interrupt_null_response.json");
    assert_snapshot_matches(
        &agent.take_snapshot(&session).unwrap(),
        "snapshots/agent_after_interrupt_resolved.json",
    );
}

/// The TS `subAgentInterruptRun` scenario: the child's paused turn is a session snapshot in
/// the parent's interrupt context, before and at the start of the resume.
#[tokio::test]
async fn the_agent_loop_reproduces_the_ts_sub_agent_interrupt_snapshots() {
    use crate::agent::{Agent, SnapshotField, TakeSnapshotOptions};
    use crate::hooks::BeforeInvocationEvent;
    use crate::interrupt::{InterruptParams, InterruptResponse};
    use crate::tools::{FunctionTool, ToolContext, ToolError};

    let confirm = FunctionTool::new(
        "confirm_tool",
        "Asks for confirmation.",
        |_input: JsonValue, ctx: ToolContext| async move {
            let answer = ctx.interrupt(InterruptParams {
                name: "approve".into(),
                reason: Some(json!("Child needs approval")),
                ..Default::default()
            })?;
            Ok::<_, ToolError>(json!({"answer": answer}))
        },
    );
    let child = Agent::builder()
        .name("child_agent")
        .id("child_agent")
        .description("A child agent that asks for approval.")
        .model(scripted(vec![
            tool_use_turn(&[("confirm_tool", "tooluse_child_1", json!({}))]),
            text_turn("Child approved."),
        ]))
        .tools([confirm])
        .build()
        .unwrap();
    let mut parent = Agent::builder()
        .id("parent_agent")
        .model(scripted(vec![
            tool_use_turn(&[(
                "child_agent",
                "tooluse_parent_1",
                json!({"input": "Please approve"}),
            )]),
            text_turn("Parent done."),
        ]))
        .tools([child])
        .build()
        .unwrap();
    let interrupts = TakeSnapshotOptions {
        include: vec![SnapshotField::Interrupts],
        ..TakeSnapshotOptions::default()
    };
    let first = parent.invoke("Delegate").await.unwrap();
    assert_snapshot_matches(
        &parent.take_snapshot(&interrupts).unwrap(),
        "snapshots/agent_sub_agent_interrupt_context.json",
    );

    let resumed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let slot = std::sync::Arc::clone(&resumed);
    parent.add_hook(move |event: &mut BeforeInvocationEvent<'_>| {
        slot.lock()
            .unwrap()
            .get_or_insert_with(|| event.agent().take_snapshot(&interrupts).unwrap());
    });
    let responses: Vec<InterruptResponse> = first
        .interrupts()
        .iter()
        .map(|interrupt| InterruptResponse::new(interrupt.id(), json!({"approved": true})))
        .collect();
    let result = parent.invoke(responses).await.unwrap();
    assert_eq!(result.to_string(), "Parent done.");
    // TS `InterruptState.toJSON` shares `context` with the live state, so the fixture shows the
    // turn the resumed tool deleted afterwards (ts-bugs.md); Rust's snapshot is a copy.
    let mut resumed = serde_json::to_value(resumed.lock().unwrap().take().unwrap()).unwrap();
    let turns = &mut resumed["data"]["interrupts"]["context"]["subAgentInterruptedTurns"];
    assert!(turns.get("tooluse_parent_1").is_some());
    *turns = json!({});
    assert_snapshot_matches(
        &serde_json::from_value(resumed).unwrap(),
        "snapshots/agent_sub_agent_resume_with_context.json",
    );
}
