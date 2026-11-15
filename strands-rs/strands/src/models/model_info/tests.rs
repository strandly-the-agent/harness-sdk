//! Ported from strands-ts/src/models/__tests__/model-info.test.ts,
//! strands-ts/src/models/__tests__/model-info-lookup.test.ts and
//! strands-ts/src/models/__tests__/model-info.test.node.ts

use std::ptr;

use serde_json::json;

use super::{
    DATA, MODEL_INFO_JSON, ModelCapabilities, ModelInfo, ModelLimits, PromptCaching, SERVER_SIDE_SUMMARY_OFF,
    default_context_window_limit, get_context_window_limit, lookup_model, lookup_model_info,
    merge_model_info,
};
use crate::types::json::JsonValue;

fn base() -> ModelInfo {
    ModelInfo {
        capabilities: Some(ModelCapabilities {
            stateful: Some(false),
            prompt_caching: Some(PromptCaching::CachePoints),
            ..ModelCapabilities::default()
        }),
        limits: Some(ModelLimits {
            context_window: Some(200_000),
            ..ModelLimits::default()
        }),
    }
}

fn info(value: JsonValue) -> ModelInfo {
    serde_json::from_value(value).unwrap()
}

fn same(left: Option<&ModelInfo>, right: Option<&ModelInfo>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => ptr::eq(left, right),
        _ => false,
    }
}

#[test]
fn returns_the_base_when_nothing_else_is_given() {
    assert_eq!(merge_model_info([Some(&base())]), base());
}

#[test]
fn inherits_fields_a_later_record_leaves_out_or_sets_to_undefined() {
    let merged = merge_model_info([
        Some(&base()),
        Some(&info(json!({ "limits": { "contextWindow": 1_000_000 } }))),
        Some(&info(json!({ "capabilities": {} }))),
    ]);
    assert_eq!(merged.capabilities, base().capabilities);
    assert_eq!(merged.limits.unwrap().context_window, Some(1_000_000));
}

#[test]
fn lets_a_later_record_win_over_an_earlier_one() {
    let merged = merge_model_info([
        Some(&base()),
        Some(&info(json!({ "limits": { "contextWindow": 1 } }))),
        None,
        Some(&info(
            json!({ "capabilities": { "promptCaching": "unsupported" }, "limits": { "contextWindow": 2 } }),
        )),
    ]);
    assert_eq!(
        serde_json::to_value(merged.capabilities).unwrap(),
        json!({ "stateful": false, "promptCaching": "unsupported" })
    );
    assert_eq!(merged.limits.unwrap().context_window, Some(2));
}

#[test]
fn leaves_out_sections_with_nothing_known() {
    let merged = merge_model_info([
        Some(&ModelInfo::default()),
        None,
        Some(&info(json!({ "limits": {} }))),
    ]);
    assert_eq!(merged, ModelInfo::default());
    assert_eq!(serde_json::to_value(merged).unwrap(), json!({}));
}

#[test]
fn merge_puts_a_later_record_over_this_one_with_the_merge_rules() {
    let later = info(json!({ "capabilities": { "stateful": true }, "limits": { "maxOutputTokens": 64 } }));
    let mut merged = base();
    merged.merge(later);
    assert_eq!(merged, merge_model_info([Some(&base()), Some(&later)]));
    assert_eq!(
        serde_json::to_value(merged).unwrap(),
        json!({
            "capabilities": { "stateful": true, "promptCaching": "cachePoints" },
            "limits": { "contextWindow": 200_000, "maxOutputTokens": 64 },
        })
    );

    let mut empty = ModelInfo::default();
    empty.merge(info(json!({ "limits": {} })));
    assert_eq!(empty, ModelInfo::default());
}

#[test]
fn resolves_every_spelling_of_a_model_to_its_one_entry() {
    let entry = lookup_model("claude-opus-5-5");
    assert_eq!(entry.unwrap().limits.unwrap().context_window, Some(1_000_000));
    for spelling in [
        "anthropic.claude-opus-5-5",
        "us.anthropic.claude-opus-5-5",
        "arn:aws:bedrock:us-west-2:123:inference-profile/global.anthropic.claude-opus-5-5",
    ] {
        assert!(same(lookup_model(spelling), entry), "{spelling}");
    }
}

#[test]
fn resolves_a_dated_snapshot_to_the_undated_model() {
    assert!(same(
        lookup_model("anthropic.claude-opus-5-5-20260401-v1:0"),
        lookup_model("claude-opus-5-5")
    ));
    assert!(same(
        lookup_model("claude-haiku-4-5@20251001"),
        lookup_model("claude-haiku-4-5")
    ));
    assert_eq!(
        lookup_model("claude-3-5-haiku-20241022")
            .unwrap()
            .limits
            .unwrap()
            .context_window,
        Some(200_000)
    );
}

#[test]
fn does_not_read_inherited_object_keys() {
    assert_eq!(lookup_model("toString"), None);
    assert_eq!(lookup_model("x.constructor"), None);
}

#[test]
fn ignores_a_trailing_bedrock_version_suffix() {
    assert!(same(
        lookup_model("anthropic.claude-3-5-haiku-20241022-v1:0"),
        lookup_model("claude-3-5-haiku-20241022")
    ));
    assert_eq!(
        lookup_model("us.anthropic.claude-3-5-sonnet-20241022-v2:0")
            .unwrap()
            .limits
            .unwrap()
            .context_window,
        Some(200_000)
    );
}

#[test]
fn returns_undefined_for_unknown_ids() {
    assert_eq!(lookup_model("unknown-model"), None);
    assert_eq!(lookup_model(""), None);
}

#[test]
fn returns_the_entry_facts_and_nothing_for_unknown_ids() {
    assert_eq!(
        serde_json::to_value(lookup_model_info("eu.anthropic.claude-sonnet-4-6")).unwrap(),
        json!({
            "capabilities": { "promptCaching": "cachePoints" },
            "limits": { "contextWindow": 1_000_000, "maxOutputTokens": 128_000 },
        })
    );
    for model_id in [
        "anthropic.claude-opus-5-5",
        "global.anthropic.claude-fable-5-2",
        "claude-mythos-5-2",
    ] {
        assert_eq!(
            serde_json::to_value(lookup_model_info(model_id)).unwrap(),
            json!({
                "capabilities": {
                    "promptCaching": "cachePoints",
                    "forcedToolChoice": false,
                    "nativeStructuredOutput": true,
                    "serverSideSummary": true,
                    "temporaryMessages": true,
                },
                "limits": { "contextWindow": 1_000_000, "maxOutputTokens": 128_000 },
            }),
            "{model_id}"
        );
    }
    assert_eq!(
        serde_json::to_value(lookup_model_info("amazon.nova-pro-v1:0")).unwrap(),
        json!({
            "capabilities": { "forcedToolChoice": true, "nativeStructuredOutput": false },
            "limits": { "contextWindow": 300_000 },
        })
    );
    assert_eq!(lookup_model_info("claude-future-9"), ModelInfo::default());
    assert_eq!(lookup_model_info(""), ModelInfo::default());
}

#[test]
fn keeps_get_context_window_limit_working_for_ids_the_python_table_already_knew() {
    assert_eq!(get_context_window_limit("mistral-large-latest"), Some(262_144));
    assert_eq!(get_context_window_limit("o3"), Some(200_000));
}

#[test]
fn exposes_the_context_window_default_from_the_data() {
    assert_eq!(default_context_window_limit(), 200_000);
}

#[test]
fn has_only_the_fields_model_info_defines_with_the_right_types_in_every_entry() {
    let data: JsonValue = serde_json::from_str(MODEL_INFO_JSON).unwrap();
    let mut keys: Vec<&String> = data.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["$comment", "defaults", "models"]);
    let defaults = &data["defaults"];
    assert_eq!(defaults.as_object().unwrap().len(), 1);
    assert_eq!(defaults["limits"].as_object().unwrap().len(), 1);
    assert!(defaults["limits"]["contextWindow"].is_u64());

    let models = data["models"].as_object().unwrap();
    assert_eq!(DATA.models.len(), models.len());
    for (id, entry) in models {
        let valid_id = id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:@-".contains(&b));
        assert!(valid_id, "{id}");
        let entry = entry.as_object().unwrap();
        assert!(
            entry.keys().all(|key| key == "limits" || key == "capabilities"),
            "{id}"
        );
        for (key, value) in entry
            .get("limits")
            .and_then(JsonValue::as_object)
            .into_iter()
            .flatten()
        {
            assert!(
                ["contextWindow", "maxOutputTokens"].contains(&key.as_str()),
                "{id}.limits.{key}"
            );
            assert!(value.as_u64().is_some_and(|value| value > 0), "{id}.limits.{key}");
        }
        for (key, value) in entry
            .get("capabilities")
            .and_then(JsonValue::as_object)
            .into_iter()
            .flatten()
        {
            match key.as_str() {
                "promptCaching" => assert!(
                    ["unsupported", "auto", "cachePoints"].contains(&value.as_str().unwrap_or_default()),
                    "{id}.{key}"
                ),
                "forcedToolChoice" | "nativeStructuredOutput" | "serverSideSummary" | "temporaryMessages" => {
                    assert!(value.is_boolean(), "{id}.{key}");
                }
                _ => panic!("{id}.capabilities.{key} is not a ModelCapabilities field"),
            }
        }
    }
}

#[test]
fn matches_shared_model_info_json_at_the_repository_root() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shared/model-info.json");
    let Ok(source) = std::fs::read_to_string(source) else {
        return;
    };
    assert_eq!(
        MODEL_INFO_JSON, source,
        "copy shared/model-info.json to strands-rs/strands/src/models/"
    );
}

#[test]
fn server_side_summary_off_turns_off_only_server_side_summary() {
    let merged = merge_model_info([lookup_model("claude-opus-5-5"), Some(&SERVER_SIDE_SUMMARY_OFF)]);
    let capabilities = merged.capabilities.unwrap();
    assert_eq!(capabilities.server_side_summary, Some(false));
    assert_eq!(capabilities.temporary_messages, Some(true));
}
