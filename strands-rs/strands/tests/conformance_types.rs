//! Conformance round-trips for the message, content block, media, tool and usage fixtures
//! (`conformance/fixtures/types/`). See `common/mod.rs` for the comparison method.

#![expect(clippy::unwrap_used, reason = "test code")]

mod common;

use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use strands::interrupt::InterruptResponseContent;
use strands::models::streaming::{Metrics, Usage};
use strands::types::citations::CitationLocation;
use strands::types::media::{AudioFormat, DocumentFormat, ImageFormat, ImageSource, VideoFormat};
use strands::types::messages::{
    ContentBlock, Message, ReasoningBlock, StopReason, SystemPrompt, ToolResultContent,
};
use strands::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

use common::{COVERED, DEFERRED, assert_compact, assert_pretty_bytes, assert_pretty_cases, cases, parse};

#[test]
fn every_fixture_is_covered_or_deferred() {
    let mut listed: Vec<String> = COVERED
        .iter()
        .chain(DEFERRED)
        .map(|path| (*path).to_owned())
        .collect();
    listed.sort();
    let unique = listed.len();
    listed.dedup();
    assert_eq!(listed.len(), unique, "a fixture is listed twice");
    assert_eq!(common::all_fixtures(), listed);
}

#[test]
fn content_blocks_round_trip() {
    assert_pretty_cases::<ContentBlock>("types/content_blocks.json");
    let unknown: ContentBlock = parse(&cases("types/content_blocks.json")["unknown"]);
    assert!(matches!(unknown, ContentBlock::Unknown(_)));
}

#[test]
fn compact_message_round_trips_byte_for_byte() {
    let message: Message = assert_compact("types/compact_message.json");
    assert_eq!(message.tracking_id(), "tracking-fixed-1");
}

#[test]
fn messages_round_trip() {
    assert_pretty_cases::<Message>("types/messages.json");
}

#[test]
fn legacy_messages_without_a_tracking_id_gain_a_minted_one() {
    let expected: Message = parse(&cases("types/messages.json")["legacyWithoutTrackingId"]);
    let loaded: Message =
        serde_json::from_value(json!({ "role": "user", "content": [{ "text": "legacy" }] })).unwrap();
    let minted = uuid::Uuid::parse_str(loaded.tracking_id()).unwrap();
    assert_eq!(minted.get_version_num(), 4);
    assert_eq!(loaded.with_tracking_id(expected.tracking_id()), expected);
}

#[test]
fn media_formats_round_trip_and_every_listed_format_is_known() {
    let path = "types/media_formats.json";
    let parsed: IndexMap<String, IndexMap<String, ContentBlock>> =
        serde_json::from_str(&common::read(path)).unwrap();
    assert_pretty_bytes(path, &parsed);
    for (kind, blocks) in &parsed {
        for format in blocks.keys() {
            let unknown = match kind.as_str() {
                "audio" => matches!(AudioFormat::from(format.as_str()), AudioFormat::Other(_)),
                "image" => matches!(ImageFormat::from(format.as_str()), ImageFormat::Other(_)),
                "video" => matches!(VideoFormat::from(format.as_str()), VideoFormat::Other(_)),
                _ => matches!(DocumentFormat::from(format.as_str()), DocumentFormat::Other(_)),
            };
            assert!(!unknown, "{kind} format {format} loaded as Other");
        }
    }
}

#[test]
fn bedrock_citations_keep_the_key_order_the_adapter_built() {
    assert_pretty_cases::<Message>("types/bedrock_citations.json");
}

#[test]
fn saved_data_keeps_unknown_blocks_formats_locations_and_statuses() {
    let path = "types/saved_data_round_trip.json";
    for (name, case) in cases(path) {
        let message: Message = parse(&case["input"]);
        assert_eq!(
            strands::types::json::to_string(&message).unwrap(),
            serde_json::to_string(&case["output"]).unwrap(),
            "{name}"
        );
    }
    let all = cases(path);
    let unknown_block: Message = parse(&all["unknownContentBlock"]["input"]);
    assert!(matches!(unknown_block.content[0], ContentBlock::Unknown(_)));
    let unknown_content: Message = parse(&all["unknownToolResultContent"]["input"]);
    let ContentBlock::ToolResult(result) = &unknown_content.content[0] else {
        panic!("not a tool result")
    };
    assert!(matches!(result.content[0], ToolResultContent::Unknown(_)));
    let heic: Message = parse(&all["unknownMediaFormat"]["input"]);
    let ContentBlock::Image(image) = &heic.content[0] else {
        panic!("not an image")
    };
    assert!(matches!(&image.format, ImageFormat::Other(other) if other.as_str() == "heic"));
    let open: Message = parse(&all["unknownCitationLocationAndStatus"]["input"]);
    let ContentBlock::Citations(citations) = &open.content[0] else {
        panic!("not citations")
    };
    assert!(matches!(
        citations.citations[0].location,
        CitationLocation::Other(_)
    ));
    let ContentBlock::ToolResult(partial) = &open.content[1] else {
        panic!("not a tool result")
    };
    assert!(matches!(&partial.status, ToolResultStatus::Other(other) if other.as_str() == "partial"));
}

#[test]
fn stop_reason_other_never_holds_a_known_value() {
    let all: Vec<String> = parse(&cases("types/stop_reasons.json")["all"]);
    assert_eq!(all.len(), 16);
    for known in &all {
        let reason = StopReason::from(known.as_str());
        assert!(!matches!(reason, StopReason::Other(_)), "{known}");
        let loaded: StopReason = serde_json::from_value(json!(known)).unwrap();
        assert_eq!(loaded, reason);
        assert_eq!(serde_json::to_value(&loaded).unwrap(), json!(known));
    }
    let future = StopReason::from("someFutureReason");
    assert!(matches!(&future, StopReason::Other(other) if other.as_str() == "someFutureReason"));
}

#[test]
fn strict_parsers_fail_with_the_ts_texts() {
    let texts = cases("types/strict_parse_errors.json");
    let text = |name: &str| texts[name].as_str().unwrap().to_owned();
    let png = "iVBORw0KGgo=";
    let errors = [
        (
            "unknownContentBlock",
            ContentBlock::from_data(json!({ "futureBlock": {} })).unwrap_err(),
        ),
        (
            "unknownToolResultContent",
            ToolResultContent::from_data(json!({ "futureContent": {} })).unwrap_err(),
        ),
        (
            "unknownToolResultContentInBlock",
            ContentBlock::from_data(json!({
                "toolResult": { "toolUseId": "x", "status": "success", "content": [{ "futureContent": {} }] }
            }))
            .unwrap_err(),
        ),
        (
            "unknownMessageBlock",
            Message::from_message_data(json!({ "role": "user", "content": [{ "futureBlock": {} }] }))
                .unwrap_err(),
        ),
        (
            "guardContentEmpty",
            ContentBlock::from_data(json!({ "guardContent": {} })).unwrap_err(),
        ),
        (
            "guardContentBoth",
            ContentBlock::from_data(json!({ "guardContent": {
                "text": { "text": "a", "qualifiers": [] },
                "image": { "format": "png", "source": { "bytes": png } }
            } }))
            .unwrap_err(),
        ),
    ];
    for (name, error) in errors {
        assert_eq!(error.to_string(), text(name), "{name}");
    }
    let system = serde_json::from_str::<SystemPrompt>(r#"[{"futureBlock":{}}]"#).unwrap_err();
    assert_eq!(system.to_string(), text("unknownSystemContentBlock"));
}

#[test]
fn system_prompts_round_trip() {
    assert_pretty_cases::<SystemPrompt>("types/system_prompts.json");
}

#[test]
fn tool_specs_round_trip() {
    assert_pretty_cases::<ToolSpec>("schemas/tool_specs.json");
}

#[test]
fn tool_choice_round_trips_with_empty_object_variants() {
    let path = "types/tool_choice.json";
    assert_pretty_cases::<ToolChoice>(path);
    let all = cases(path);
    assert_eq!(parse::<ToolChoice>(&all["auto"]), ToolChoice::auto());
    assert_eq!(parse::<ToolChoice>(&all["any"]), ToolChoice::any());
    assert_eq!(parse::<ToolChoice>(&all["tool"]), ToolChoice::tool("get_weather"));
}

#[test]
fn usage_and_metrics_round_trip_without_gaining_a_fraction() {
    let path = "types/usage_metrics.json";
    let mut written = Map::new();
    for (name, case) in cases(path) {
        let value = match name.as_str() {
            "metricsMinimal" => common::round_trip::<Metrics>(&case),
            "totalPromptTokens" => case,
            _ => common::round_trip::<Usage>(&case),
        };
        written.insert(name, value);
    }
    assert_pretty_bytes(path, &written);
    let max: Usage = parse(&cases(path)["usageMaxSafeInteger"]);
    assert_eq!(max.input_tokens, 9_007_199_254_740_991);
    common::assert_pretty_value::<Metrics>("types/metrics_full.json");
}

#[test]
fn integers_never_gain_a_fraction() {
    let usage: Usage = serde_json::from_str(r#"{"inputTokens":1,"outputTokens":2,"totalTokens":3}"#).unwrap();
    assert_eq!(
        strands::types::json::to_string(&usage).unwrap(),
        r#"{"inputTokens":1,"outputTokens":2,"totalTokens":3}"#
    );
    assert!(
        serde_json::from_str::<Usage>(r#"{"inputTokens":1.5,"outputTokens":2,"totalTokens":3}"#).is_err()
    );
}

#[test]
fn interrupt_response_content_round_trips_and_keeps_a_null_response() {
    let path = "types/interrupt_response_content.json";
    assert_pretty_cases::<InterruptResponseContent>(path);
    let null: InterruptResponseContent = parse(&cases(path)["null"]);
    assert_eq!(null.interrupt_response.response, Value::Null);
}

#[test]
fn lenient_base64_matches_every_node_case() {
    let path = "types/base64_lenient.json";
    let all = cases(path);
    for (name, case) in all["decode"].as_object().unwrap() {
        let input = json!({ "image": { "format": "png", "source": { "bytes": case["input"] } } });
        let block: ContentBlock = parse(&input);
        let ContentBlock::Image(image) = &block else {
            panic!("not an image")
        };
        let ImageSource::Bytes(bytes) = &image.source else {
            panic!("not bytes")
        };
        let decoded: Vec<u8> = serde_json::from_value(case["decoded"].clone()).unwrap();
        assert_eq!(bytes, &decoded, "{name}");
        let written = serde_json::to_value(&block).unwrap();
        assert_eq!(written["image"]["source"]["bytes"], case["canonical"], "{name}");
    }
    for name in ["imageFromNonCanonical", "reasoningFromUnpadded"] {
        let written = common::round_trip::<ContentBlock>(&all[name]["input"]);
        assert_eq!(written, all[name]["output"], "{name}");
    }
}

#[test]
fn persisted_bytes_accept_index_keyed_uint8_array_objects() {
    let block: ContentBlock =
        serde_json::from_str(r#"{"reasoning":{"redactedContent":{"0":1,"1":2}}}"#).unwrap();
    assert_eq!(
        block,
        ContentBlock::Reasoning(ReasoningBlock::new().with_redacted_content(vec![1, 2]))
    );
    assert_eq!(
        strands::types::json::to_string(&block).unwrap(),
        r#"{"reasoning":{"redactedContent":"AQI="}}"#
    );
    assert!(serde_json::from_str::<ContentBlock>(r#"{"reasoning":{"redactedContent":{"1":2}}}"#).is_err());
}
