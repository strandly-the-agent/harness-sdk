//! Ported from strands-ts/src/models/__tests__/model-info.test.ts, model-info-lookup.test.ts and
//! model-info.test.node.ts

use std::path::Path;

use serde_json::json;

use super::*;
use crate::types::json::JsonValue;

fn info(value: JsonValue) -> ModelInfo {
    serde_json::from_value(value).unwrap()
}

fn same(a: Option<&ModelInfo>, b: Option<&ModelInfo>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::eq(a, b),
        _ => false,
    }
}

mod merge_model_info {
    use super::*;

    fn base() -> ModelInfo {
        info(json!({
            "capabilities": {"stateful": false, "promptCaching": "cachePoints"},
            "limits": {"contextWindow": 200_000}
        }))
    }

    #[test]
    fn returns_the_base_when_nothing_else_is_given() {
        assert_eq!(merge_model_info([Some(&base())]), base());
    }

    #[test]
    fn inherits_fields_a_later_record_leaves_out_or_sets_to_undefined() {
        let merged = merge_model_info([
            Some(&base()),
            Some(&info(json!({"limits": {"contextWindow": 1_000_000}}))),
            Some(&info(json!({"capabilities": {}}))),
        ]);
        assert_eq!(merged.capabilities, base().capabilities);
        assert_eq!(merged.context_window(), Some(1_000_000));
    }

    #[test]
    fn lets_a_later_record_win_over_an_earlier_one() {
        let merged = merge_model_info([
            Some(&base()),
            Some(&info(json!({"limits": {"contextWindow": 1}}))),
            None,
            Some(&info(json!({
                "capabilities": {"promptCaching": "unsupported"},
                "limits": {"contextWindow": 2}
            }))),
        ]);
        assert_eq!(
            serde_json::to_value(&merged.capabilities).unwrap(),
            json!({"stateful": false, "promptCaching": "unsupported"})
        );
        assert_eq!(merged.context_window(), Some(2));
    }

    #[test]
    fn leaves_out_sections_with_nothing_known() {
        let merged = merge_model_info([
            Some(&ModelInfo::default()),
            None,
            Some(&info(json!({"limits": {}}))),
        ]);
        assert_eq!(serde_json::to_value(merged).unwrap(), json!({}));
    }
}

mod shared_model_info_data {
    use super::*;

    #[test]
    fn resolves_every_spelling_of_a_model_to_its_one_entry() {
        let entry = lookup_model("claude-opus-5-5");
        assert_eq!(entry.and_then(ModelInfo::context_window), Some(1_000_000));
        for id in [
            "anthropic.claude-opus-5-5",
            "us.anthropic.claude-opus-5-5",
            "arn:aws:bedrock:us-west-2:123:inference-profile/global.anthropic.claude-opus-5-5",
        ] {
            assert!(same(lookup_model(id), entry), "{id}");
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
            lookup_model("claude-3-5-haiku-20241022").and_then(ModelInfo::context_window),
            Some(200_000)
        );
    }

    #[test]
    fn does_not_read_inherited_object_keys() {
        assert!(lookup_model("toString").is_none());
        assert!(lookup_model("x.constructor").is_none());
    }

    #[test]
    fn ignores_a_trailing_bedrock_version_suffix() {
        assert!(same(
            lookup_model("anthropic.claude-3-5-haiku-20241022-v1:0"),
            lookup_model("claude-3-5-haiku-20241022")
        ));
        assert_eq!(
            lookup_model("us.anthropic.claude-3-5-sonnet-20241022-v2:0").and_then(ModelInfo::context_window),
            Some(200_000)
        );
    }

    #[test]
    fn returns_undefined_for_unknown_ids() {
        assert!(lookup_model("unknown-model").is_none());
        assert!(lookup_model("").is_none());
    }

    #[test]
    fn returns_the_entry_facts_and_nothing_for_unknown_ids() {
        let facts = |id: &str| serde_json::to_value(lookup_model_info(id)).unwrap();
        assert_eq!(
            facts("eu.anthropic.claude-sonnet-4-6"),
            json!({
                "capabilities": {"promptCaching": "cachePoints"},
                "limits": {"contextWindow": 1_000_000, "maxOutputTokens": 128_000}
            })
        );
        for id in [
            "anthropic.claude-opus-5-5",
            "global.anthropic.claude-fable-5-2",
            "claude-mythos-5-2",
        ] {
            assert_eq!(
                facts(id),
                json!({
                    "capabilities": {
                        "promptCaching": "cachePoints",
                        "forcedToolChoice": false,
                        "nativeStructuredOutput": true,
                        "serverSideSummary": true,
                        "temporaryMessages": true
                    },
                    "limits": {"contextWindow": 1_000_000, "maxOutputTokens": 128_000}
                }),
                "{id}"
            );
        }
        assert_eq!(
            facts("amazon.nova-pro-v1:0"),
            json!({
                "capabilities": {"forcedToolChoice": true, "nativeStructuredOutput": false},
                "limits": {"contextWindow": 300_000}
            })
        );
        assert_eq!(facts("claude-future-9"), json!({}));
        assert_eq!(facts(""), json!({}));
    }

    #[test]
    fn keeps_getcontextwindowlimit_working_for_ids_the_python_table_already_knew() {
        assert_eq!(get_context_window_limit("mistral-large-latest"), Some(262_144));
        assert_eq!(get_context_window_limit("o3"), Some(200_000));
    }

    #[test]
    fn exposes_the_context_window_default_from_the_data() {
        assert_eq!(DEFAULT_CONTEXT_WINDOW_LIMIT, 200_000);
        let data: ModelInfoData = serde_json::from_str(MODEL_INFO_JSON).unwrap();
        assert_eq!(data.defaults.context_window(), Some(DEFAULT_CONTEXT_WINDOW_LIMIT));
    }

    #[test]
    fn has_only_the_fields_modelinfo_defines_with_the_right_types_in_every_entry() {
        let raw: JsonValue = serde_json::from_str(MODEL_INFO_JSON).unwrap();
        let mut keys: Vec<_> = raw.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["$comment", "defaults", "models"]);
        assert_eq!(
            raw["defaults"],
            json!({"limits": {"contextWindow": DEFAULT_CONTEXT_WINDOW_LIMIT}})
        );
        let models = raw["models"].as_object().unwrap();
        assert_eq!(models.len(), DATA.models.len());
        for (id, entry) in models {
            let first = id.bytes().next().unwrap();
            assert!(first.is_ascii_lowercase() || first.is_ascii_digit(), "{id}");
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:@-".contains(&b)),
                "{id}"
            );
            let parsed: ModelInfo = serde_json::from_value(entry.clone()).unwrap();
            assert_eq!(&serde_json::to_value(&parsed).unwrap(), entry, "{id}");
            if let Some(caching) = parsed.capabilities.and_then(|c| c.prompt_caching) {
                assert!(!matches!(caching, PromptCaching::Other(_)), "{id}");
            }
            for value in parsed
                .limits
                .iter()
                .flat_map(|l| [l.context_window, l.max_output_tokens])
            {
                assert_ne!(value, Some(0), "{id}");
            }
        }
    }
}

mod generated_model_info_copy {
    use super::*;

    #[test]
    fn matches_shared_model_info_json_at_the_repository_root() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shared/model-info.json");
        let Ok(source) = std::fs::read_to_string(source) else {
            return;
        };
        assert_eq!(
            MODEL_INFO_JSON, source,
            "copy shared/model-info.json to strands-rs/strands/src/models/model-info.json"
        );
    }
}
