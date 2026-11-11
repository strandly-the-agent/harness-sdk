//! Ported from strands-ts/src/models/__tests__/model-info.test.ts,
//! strands-ts/src/models/__tests__/model-info-lookup.test.ts and
//! strands-ts/src/models/__tests__/model-info.test.node.ts

use serde_json::json;

use super::*;

fn base() -> ModelInfo {
    ModelInfo {
        capabilities: Some(ModelCapabilities {
            stateful: Some(false),
            prompt_caching: Some(PromptCaching::CachePoints),
            ..Default::default()
        }),
        limits: Some(ModelLimits {
            context_window: Some(200_000),
            ..Default::default()
        }),
    }
}

fn limits(context_window: Option<u64>) -> ModelInfo {
    ModelInfo {
        limits: Some(ModelLimits {
            context_window,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn info_json(info: &ModelInfo) -> serde_json::Value {
    serde_json::to_value(info).unwrap()
}

#[test]
fn returns_the_base_when_nothing_else_is_given() {
    assert_eq!(merge_model_info([Some(&base())]), base());
}

#[test]
fn inherits_fields_a_later_record_leaves_out_or_sets_to_undefined() {
    let unset = ModelInfo {
        capabilities: Some(ModelCapabilities::default()),
        ..Default::default()
    };
    let merged = merge_model_info([Some(&base()), Some(&limits(Some(1_000_000))), Some(&unset)]);
    assert_eq!(merged.capabilities, base().capabilities);
    assert_eq!(merged.context_window(), Some(1_000_000));
}

#[test]
fn lets_a_later_record_win_over_an_earlier_one() {
    let last = ModelInfo {
        capabilities: Some(ModelCapabilities {
            prompt_caching: Some(PromptCaching::Unsupported),
            ..Default::default()
        }),
        limits: Some(ModelLimits {
            context_window: Some(2),
            ..Default::default()
        }),
    };
    let merged = merge_model_info([Some(&base()), Some(&limits(Some(1))), None, Some(&last)]);
    assert_eq!(
        info_json(&merged)["capabilities"],
        json!({ "stateful": false, "promptCaching": "unsupported" })
    );
    assert_eq!(merged.context_window(), Some(2));
}

#[test]
fn leaves_out_sections_with_nothing_known() {
    let merged = merge_model_info([Some(&ModelInfo::default()), None, Some(&limits(None))]);
    assert_eq!(merged, ModelInfo::default());
    assert_eq!(info_json(&merged), json!({}));
}

#[test]
fn resolves_every_spelling_of_a_model_to_its_one_entry() {
    let entry = lookup_model("claude-opus-5-5").unwrap();
    assert_eq!(entry.context_window(), Some(1_000_000));
    for id in [
        "anthropic.claude-opus-5-5",
        "us.anthropic.claude-opus-5-5",
        "arn:aws:bedrock:us-west-2:123:inference-profile/global.anthropic.claude-opus-5-5",
    ] {
        assert!(std::ptr::eq(lookup_model(id).unwrap(), entry), "{id}");
    }
}

#[test]
fn resolves_a_dated_snapshot_to_the_undated_model() {
    assert!(std::ptr::eq(
        lookup_model("anthropic.claude-opus-5-5-20260401-v1:0").unwrap(),
        lookup_model("claude-opus-5-5").unwrap()
    ));
    assert!(std::ptr::eq(
        lookup_model("claude-haiku-4-5@20251001").unwrap(),
        lookup_model("claude-haiku-4-5").unwrap()
    ));
    assert_eq!(
        lookup_model("claude-3-5-haiku-20241022")
            .unwrap()
            .context_window(),
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
    assert!(std::ptr::eq(
        lookup_model("anthropic.claude-3-5-haiku-20241022-v1:0").unwrap(),
        lookup_model("claude-3-5-haiku-20241022").unwrap()
    ));
    assert_eq!(
        lookup_model("us.anthropic.claude-3-5-sonnet-20241022-v2:0")
            .unwrap()
            .context_window(),
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
        info_json(&lookup_model_info("eu.anthropic.claude-sonnet-4-6")),
        json!({
            "capabilities": { "promptCaching": "cachePoints" },
            "limits": { "contextWindow": 1_000_000, "maxOutputTokens": 128_000 },
        })
    );
    for id in [
        "anthropic.claude-opus-5-5",
        "global.anthropic.claude-fable-5-2",
        "claude-mythos-5-2",
    ] {
        assert_eq!(
            info_json(&lookup_model_info(id)),
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
            "{id}"
        );
    }
    assert_eq!(
        info_json(&lookup_model_info("amazon.nova-pro-v1:0")),
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
    let data: serde_json::Value = serde_json::from_str(MODEL_INFO_JSON).unwrap();
    assert!(data["defaults"]["limits"]["contextWindow"].is_u64());
    let models = data["models"].as_object().unwrap();
    assert_eq!(models.len(), DATA.models.len());
    for (id, entry) in models {
        assert!(
            id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && id.chars().all(|c| c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, '.' | ':' | '@' | '-')),
            "{id}"
        );
        // Unknown fields would be dropped by the typed parse, so a round trip proves there are none.
        let typed: ModelInfo = serde_json::from_value(entry.clone()).unwrap();
        assert_eq!(&info_json(&typed), entry, "{id}");
        for value in entry
            .get("limits")
            .and_then(|l| l.as_object())
            .into_iter()
            .flat_map(|l| l.values())
        {
            assert!(value.as_u64().is_some_and(|n| n > 0), "{id}");
        }
    }
}

#[test]
fn matches_shared_model_info_json_at_the_repository_root_run_npm_run_prepare_w_strands_ts_to_refresh() {
    let shared = format!("{}/../../shared/model-info.json", env!("CARGO_MANIFEST_DIR"));
    let Ok(expected) = std::fs::read_to_string(&shared) else {
        return;
    };
    assert_eq!(
        MODEL_INFO_JSON, expected,
        "strands-rs/strands/src/models/model-info.json differs from shared/model-info.json; copy it again"
    );
}

#[test]
fn server_side_summary_off_turns_the_capability_off_over_the_table() {
    let merged = merge_model_info([
        Some(&lookup_model_info("claude-opus-5-5")),
        Some(&SERVER_SIDE_SUMMARY_OFF),
    ]);
    assert_eq!(merged.capabilities().server_side_summary, Some(false));
    assert_eq!(merged.capabilities().temporary_messages, Some(true));
}

#[test]
fn strips_only_well_formed_suffixes() {
    assert_eq!(strip_version("a-v1"), "a");
    assert_eq!(strip_version("a-v1:0"), "a");
    assert_eq!(strip_version("a-1:0"), "a");
    assert_eq!(strip_version("a-v"), "a-v");
    assert_eq!(strip_version("a-1"), "a-1");
    assert_eq!(strip_version("a-v1:"), "a-v1:");
    assert_eq!(strip_date("a-20240101"), "a");
    assert_eq!(strip_date("a@20240101"), "a");
    assert_eq!(strip_date("a-2024010"), "a-2024010");
    assert_eq!(strip_date("a_20240101"), "a_20240101");
    assert_eq!(strip_date("é-20240101"), "é");
}
