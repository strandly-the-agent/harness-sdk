//! The cases of `harness-ts/test/models.test.ts` that read the resolved provider config.

use serde_json::json;
use strands::models::BedrockModel;
use strands::models::anthropic::AnthropicModelConfig;
use strands::models::bedrock::{BedrockModelConfig, BedrockModelOptions};
use strands::models::model::{CacheConfig, CacheStrategy};
use strands::models::openai::OpenAIModelConfig;
use strands::types::json::JsonValue;

use super::*;
use crate::test_support::WarnRecorder;

fn env_of(vars: &'static [(&'static str, &'static str)]) -> EnvLookup {
    Arc::new(move |name| lookup(vars, name).map(str::to_owned))
}

/// Fake credentials, so building a model reads nothing from the process environment.
fn env() -> EnvLookup {
    env_of(&[
        ("ANTHROPIC_API_KEY", "test-key"),
        ("OPENAI_API_KEY", "test-key"),
        ("AWS_REGION", "us-west-2"),
    ])
}

fn options(effort: Effort, web_search: bool, caching: bool, caching_explicit: bool) -> ResolveModelOptions {
    ResolveModelOptions {
        effort,
        web_search,
        caching,
        caching_explicit,
    }
}

fn bedrock_options(spec: &str, options: ResolveModelOptions, env: &EnvLookup) -> Result<BedrockModelOptions> {
    let plan = plan(spec, options)?;
    assert_eq!(plan.provider.name, "bedrock");
    Ok(build::bedrock_options(plan.name, plan.level, plan.caching, env))
}

fn bedrock(spec: &str, options: ResolveModelOptions) -> Result<BedrockModelConfig> {
    bedrock_options(spec, options, &env()).map(|options| options.config)
}

/// The Bedrock config for `spec` at `effort`.
fn bedrock_at(spec: &str, level: Effort) -> BedrockModelConfig {
    bedrock(spec, effort(level)).unwrap()
}

fn fields(spec: &str, level: Effort) -> Option<JsonValue> {
    bedrock_at(spec, level).additional_request_fields
}

fn anthropic(spec: &str, options: ResolveModelOptions) -> Result<AnthropicModelConfig> {
    let plan = plan(spec, options)?;
    assert_eq!(plan.provider.name, "anthropic");
    let options = build::anthropic_options(plan.name, plan.level, plan.web_search, plan.caching, &env());
    assert_eq!(options.api_key.as_deref(), Some("test-key"));
    Ok(options.config)
}

/// The config of an `openai/` or `bedrock-mantle/` model.
fn openai(spec: &str, options: ResolveModelOptions) -> Result<OpenAIModelConfig> {
    let plan = plan(spec, options)?;
    let (name, level, search, caching) = (plan.name, plan.level, plan.web_search, plan.caching);
    Ok(match plan.provider.name {
        "openai" => build::openai_options(name, level, search, caching, &env()).config,
        "bedrock-mantle" => {
            let (options, mantle) = build::bedrock_mantle_options(name, level, search, caching, &env());
            assert_eq!(mantle.region.as_deref(), Some("us-west-2"));
            assert!(options.api_key.is_none());
            options.config
        }
        other => panic!("{spec} resolved to {other}"),
    })
}

fn params(config: &OpenAIModelConfig) -> JsonValue {
    config.params.clone().map_or(JsonValue::Null, JsonValue::Object)
}

fn anthropic_params(config: &AnthropicModelConfig) -> JsonValue {
    config.params.clone().map_or(JsonValue::Null, JsonValue::Object)
}

fn error(result: Result<impl std::fmt::Debug>) -> String {
    result.unwrap_err().to_string()
}

fn auto_cache() -> Option<CacheConfig> {
    Some(CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        ..Default::default()
    })
}

fn adaptive(level: &str) -> Option<JsonValue> {
    Some(json!({
        "thinking": { "type": "adaptive", "display": "summarized" },
        "output_config": { "effort": level },
    }))
}

fn extended(budget: u64) -> JsonValue {
    json!({ "thinking": { "type": "enabled", "budget_tokens": budget } })
}

fn reasoning(level: &str) -> Option<JsonValue> {
    Some(json!({ "reasoning": { "effort": level } }))
}

fn resolved(model: Option<&ModelSource>, options: ResolveModelOptions) -> ModelSource {
    resolve_model(model, DEFAULT, options, &env()).unwrap()
}

fn model_id(source: &ModelSource) -> Option<String> {
    match source {
        ModelSource::Model(model) => model.model_id(),
        other => panic!("not a model: {other:?}"),
    }
}

fn same_model(left: &ModelSource, right: &ModelSource) -> bool {
    match (left, right) {
        (ModelSource::Model(left), ModelSource::Model(right)) => Arc::ptr_eq(left, right),
        _ => false,
    }
}

fn bedrock_instance(model_id: &str) -> ModelSource {
    let model: Arc<dyn Model> = Arc::new(BedrockModel::builder().model_id(model_id).build());
    model.into()
}

mod resolve_model {
    use super::*;

    #[test]
    fn does_not_send_automatic_cache_points_to() {
        for id in [
            "anthropic.claude-3-haiku-20240307-v1:0",
            "us.anthropic.claude-3-haiku-20240307-v1:0",
        ] {
            let spec = format!("bedrock/{id}");
            let config = bedrock(&spec, options(Effort::Auto, false, true, false)).unwrap();
            assert_eq!(config.cache_config, None);
            assert_eq!(
                error(bedrock(&spec, options(Effort::Auto, false, true, true))),
                format!("Model bedrock/{id} does not support prompt caching. Pass caching: false.")
            );
        }
    }

    #[test]
    fn uses_aws_default_region_when_aws_region_is_absent_and_gives_aws_region_precedence() {
        let region = |env: EnvLookup| {
            bedrock_options(DEFAULT, effort(Effort::Auto), &env)
                .unwrap()
                .region
        };
        let default_only = env_of(&[("AWS_REGION", ""), ("AWS_DEFAULT_REGION", "us-east-2")]);
        assert_eq!(region(default_only).as_deref(), Some("us-east-2"));
        let both = env_of(&[("AWS_REGION", "us-west-2"), ("AWS_DEFAULT_REGION", "us-east-2")]);
        assert_eq!(region(both).as_deref(), Some("us-west-2"));
        assert_eq!(region(env_of(&[])), None);
    }

    #[test]
    fn resolves_undefined_to_the_default_with_thinking() {
        let model = resolved(None, effort(Effort::Auto));
        assert_eq!(
            model_id(&model).as_deref(),
            Some("global.anthropic.claude-opus-4-8")
        );
        assert_eq!(fields(DEFAULT, Effort::Auto), adaptive("high"));
    }

    #[test]
    fn treats_a_bare_string_as_a_bedrock_model_id() {
        let model = resolved(Some(&spec("my.custom.model-id")), effort(Effort::Auto));
        assert_eq!(model_id(&model).as_deref(), Some("my.custom.model-id"));
        let config = bedrock_at("my.custom.model-id", Effort::Auto);
        assert_eq!(config.model_id.as_deref(), Some("my.custom.model-id"));
    }

    #[test]
    fn uses_the_bedrock_model_id_directly() {
        let config = bedrock_at("bedrock/global.anthropic.claude-opus-4-8", Effort::Auto);
        assert_eq!(
            config.model_id.as_deref(),
            Some("global.anthropic.claude-opus-4-8")
        );
    }

    #[test]
    fn passes_a_bedrock_name_through() {
        let config = bedrock_at("bedrock/some.other.model", Effort::Auto);
        assert_eq!(config.model_id.as_deref(), Some("some.other.model"));
    }

    #[test]
    fn enables_prompt_caching_for_bedrock_when_requested() {
        let config = bedrock(DEFAULT, options(Effort::Auto, false, true, false)).unwrap();
        assert_eq!(config.cache_config, auto_cache());
    }

    #[test]
    fn enables_bedrock_caching_even_with_effort_off() {
        let config = bedrock(DEFAULT, options(Effort::Off, false, true, false)).unwrap();
        assert_eq!(config.cache_config, auto_cache());
    }

    #[test]
    fn does_not_set_cache_config_when_caching_is_off() {
        assert_eq!(bedrock_at(DEFAULT, Effort::Auto).cache_config, None);
    }

    #[test]
    fn leaves_provider_defaults_when_effort_is_off() {
        assert_eq!(fields(DEFAULT, Effort::Off), None);
    }

    #[test]
    fn applies_an_explicit_thinking_level() {
        assert_eq!(fields(DEFAULT, Effort::Low), adaptive("low"));
    }

    #[test]
    fn accepts_a_provider_specific_level() {
        assert_eq!(fields(DEFAULT, Effort::Max), adaptive("max"));
    }

    #[test]
    fn maps_openai_on_bedrock_thinking_to_reasoning_effort() {
        assert_eq!(
            fields("bedrock/us.openai.gpt-5.6-luna", Effort::Max),
            reasoning("max")
        );
    }

    #[test]
    fn uses_the_gpt_oss_bedrock_reasoning_field_and_levels() {
        assert_eq!(
            fields("bedrock/openai.gpt-oss-120b-1:0", Effort::Medium),
            Some(json!({ "reasoning_effort": "medium" }))
        );
        assert!(
            error(bedrock("bedrock/openai.gpt-oss-120b-1:0", effort(Effort::Max)))
                .contains("not supported by Bedrock model openai.gpt-oss-120b-1:0")
        );
    }

    #[test]
    fn resolves_astra_and_6_1_sol_effort_without_none_for() {
        for model_id in [
            "openai.gpt-6-astra",
            "global.openai.gpt-6-astra",
            "us.openai.gpt-6-astra",
            "us.openai.gpt-6.1-sol",
        ] {
            let spec = format!("bedrock/{model_id}");
            for (thinking, level) in [(Effort::Auto, "high"), (Effort::Low, "low"), (Effort::Max, "max")] {
                assert_eq!(fields(&spec, thinking), reasoning(level), "{spec}");
            }
            assert_eq!(fields(&spec, Effort::Off), None);
            assert!(
                error(bedrock(&spec, effort(Effort::Minimal)))
                    .ends_with("Supported levels: low, medium, high, xhigh, max (or 'auto', 'off').")
            );
        }
    }

    #[test]
    fn resolves_gpt_5_4_5_5_effort_without_max_for() {
        for model_id in [
            "openai.gpt-5.4",
            "us.openai.gpt-5.4",
            "openai.gpt-5.5",
            "us.openai.gpt-5.5",
            "global.openai.gpt-5.5",
        ] {
            let spec = format!("bedrock/{model_id}");
            for (thinking, level) in [
                (Effort::Auto, "high"),
                (Effort::Low, "low"),
                (Effort::Xhigh, "xhigh"),
                (Effort::Off, "none"),
            ] {
                assert_eq!(fields(&spec, thinking), reasoning(level), "{spec}");
            }
            for thinking in [Effort::Minimal, Effort::Max] {
                assert!(
                    error(bedrock(&spec, effort(thinking)))
                        .ends_with("Supported levels: low, medium, high, xhigh (or 'auto', 'off').")
                );
            }
        }
    }

    #[test]
    fn resolves_every_gpt_5_6_gpt_6_and_daybreak_effort_for() {
        for model_id in [
            "openai.gpt-5.6-cyber",
            "us.openai.gpt-6-sol",
            "us.openai.gpt-6.1-terra",
            "openai.gpt-daybreak-blue-5.6-sol",
            "us.openai.gpt-daybreak-blue-5.6-sol",
        ] {
            let spec = format!("bedrock/{model_id}");
            for (thinking, level) in [
                (Effort::Auto, "high"),
                (Effort::Low, "low"),
                (Effort::Max, "max"),
                (Effort::Off, "none"),
            ] {
                assert_eq!(fields(&spec, thinking), reasoning(level), "{spec}");
            }
            assert!(
                error(bedrock(&spec, effort(Effort::Minimal)))
                    .ends_with("Supported levels: low, medium, high, xhigh, max (or 'auto', 'off').")
            );
        }
    }

    #[test]
    fn uses_the_qwen_bedrock_reasoning_field_and_levels() {
        let field = |level: &str| Some(json!({ "reasoning_effort": level }));
        assert_eq!(
            fields("bedrock/qwen.qwen3-32b-v1:0", Effort::Minimal),
            field("minimal")
        );
        assert_eq!(
            fields("bedrock/us.qwen.qwen3-32b-v1:0", Effort::High),
            field("high")
        );
        assert_eq!(fields("bedrock/qwen.qwen3-32b-v1:0", Effort::Off), field("none"));
    }

    #[test]
    fn uses_the_xai_grok_bedrock_reasoning_field_and_levels() {
        let field = |level: &str| Some(json!({ "reasoning_effort": level }));
        assert_eq!(fields("bedrock/us.xai.grok-4.6", Effort::Xhigh), field("xhigh"));
        assert_eq!(fields("bedrock/us.xai.grok-4.6", Effort::Auto), field("high"));
        assert!(
            error(bedrock("bedrock/us.xai.grok-4.6", effort(Effort::Max)))
                .contains("not supported by Bedrock model us.xai.grok-4.6")
        );
    }

    #[test]
    fn does_not_attach_claude_thinking_fields_to_an_unknown_bedrock_family() {
        assert_eq!(fields("bedrock/amazon.nova-pro-v1:0", Effort::Auto), None);
    }

    /// The Google rows of the TS case are left out: Rust v0.1 has no Google provider.
    #[test]
    fn resolves_the_per_model_levels_for_with_effort() {
        let gemma = "bedrock-mantle/google.gemma-4-31b";
        let level = |effort_level| {
            params(&openai(gemma, effort(effort_level)).unwrap())["reasoning"]["effort"].clone()
        };
        assert_eq!(level(Effort::Auto), json!("high"));
        assert_eq!(level(Effort::Off), json!("none"));
        assert!(error(openai(gemma, effort(Effort::Low))).ends_with(
            "not supported by model google.gemma-4-31b. Supported levels: high (or 'auto', 'off')."
        ));
    }

    #[test]
    fn maps_openai_to_reasoning_effort() {
        let config = openai("openai/gpt-5.6-sol", effort(Effort::Auto)).unwrap();
        assert_eq!(params(&config), json!({ "reasoning": { "effort": "high" } }));
    }

    #[test]
    fn maps_anthropic_to_thinking_and_max_tokens() {
        let config = anthropic("anthropic/claude-opus-4-8", effort(Effort::Auto)).unwrap();
        assert_eq!(config.model_id.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(config.max_tokens, Some(128_000));
        assert_eq!(Some(anthropic_params(&config)), adaptive("high"));
    }

    #[test]
    fn gives_anthropic_haiku_its_own_tier_max_tokens() {
        let config = anthropic("anthropic/claude-haiku-4-5-20251001", effort(Effort::Auto)).unwrap();
        assert_eq!(config.max_tokens, Some(64_000));
    }

    #[test]
    fn falls_back_to_a_flat_max_tokens_for_an_unrecognized_anthropic_tier() {
        let config = anthropic("anthropic/claude-mythos-1", effort(Effort::Auto)).unwrap();
        assert_eq!(config.max_tokens, Some(32_000));
    }

    #[test]
    fn gives_a_bedrock_claude_model_tier_max_tokens() {
        assert_eq!(bedrock_at(DEFAULT, Effort::Auto).max_tokens, Some(128_000));
    }

    #[test]
    fn gives_a_bedrock_claude_haiku_model_its_own_tier_max_tokens() {
        let config = bedrock_at(
            "bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0",
            Effort::Auto,
        );
        assert_eq!(config.max_tokens, Some(64_000));
    }

    #[test]
    fn sends_no_thinking_block_to() {
        for model_id in [
            "us.anthropic.claude-opus-4-1-20250805-v1:0",
            "global.anthropic.claude-sonnet-4-20250514-v1:0",
            "anthropic.claude-opus-4-20250514-v1:0",
            "us.anthropic.claude-3-haiku-20240307-v1:0",
            "us.anthropic.claude-3-sonnet-20240229-v1:0",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-3.5-sonnet-20241022-v2:0",
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
        ] {
            assert_eq!(
                fields(&format!("bedrock/{model_id}"), Effort::Auto),
                None,
                "{model_id}"
            );
        }
    }

    #[test]
    fn sends_the_extended_block_to() {
        for model_id in [
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            "us.anthropic.claude-haiku-4-5-20251001-v1:0",
            "anthropic.claude-haiku-4-5-20251001-v1:0",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "global.anthropic.claude-opus-4-5-20251101-v1:0",
        ] {
            let block = fields(&format!("bedrock/{model_id}"), Effort::Auto);
            assert_eq!(block, Some(extended(16_384)), "{model_id}");
        }
    }

    #[test]
    fn maps_to_a_budget_of_that_the_api_accepts() {
        for (level, budget) in [
            (Effort::Low, 2_048),
            (Effort::Medium, 8_192),
            (Effort::High, 16_384),
            (Effort::Xhigh, 32_768),
            (Effort::Max, 49_152),
        ] {
            let config = bedrock_at("bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0", level);
            assert_eq!(config.additional_request_fields, Some(extended(budget)));
            assert!(config.max_tokens.is_some_and(|max_tokens| budget < max_tokens));
        }
    }

    #[test]
    fn gives_a_max_tokens_ceiling_of() {
        for (model_id, max_tokens) in [
            ("claude-sonnet-4-5-20250929-v1:0", 64_000),
            ("claude-sonnet-4.5", 64_000),
            ("claude-sonnet-4-6", 128_000),
            ("claude-sonnet-4.6", 128_000),
            ("claude-sonnet-5", 128_000),
            ("claude-opus-4-5-20251101-v1:0", 64_000),
            ("claude-opus-4.5", 64_000),
            ("claude-opus-4-8", 128_000),
            ("claude-opus-5", 128_000),
            ("claude-haiku-4-5-20251001-v1:0", 64_000),
            ("claude-haiku-4.5", 64_000),
            ("claude-haiku-5-5", 128_000),
        ] {
            let config = bedrock_at(&format!("bedrock/global.anthropic.{model_id}"), Effort::Auto);
            assert_eq!(config.max_tokens, Some(max_tokens), "{model_id}");
        }
    }

    #[test]
    fn clamps_a_version_led_id_against_the_anthropic_direct_fallback() {
        let config = anthropic("anthropic/claude-4-5-haiku", effort(Effort::Max)).unwrap();
        assert_eq!(config.max_tokens, Some(32_000));
        assert_eq!(anthropic_params(&config), extended(31_999));
    }

    #[test]
    fn sends_the_adaptive_block_to() {
        for model_id in [
            "global.anthropic.claude-opus-4-6-v1",
            "global.anthropic.claude-sonnet-4-6",
            "global.anthropic.claude-opus-4-7",
            "global.anthropic.claude-opus-4-8",
            "global.anthropic.claude-opus-5",
            "global.anthropic.claude-sonnet-5",
            "global.anthropic.claude-haiku-5-5",
            "au.anthropic.claude-haiku-5-5",
            "anthropic.claude-haiku-5-5",
            "global.anthropic.claude-haiku-6",
            "global.anthropic.claude-fable-5",
            "global.anthropic.claude-fable-5-1",
            "anthropic.claude-mythos-preview",
            "anthropic.claude-mythos-5-1",
        ] {
            let block = fields(&format!("bedrock/{model_id}"), Effort::Auto);
            assert_eq!(block, adaptive("high"), "{model_id}");
        }
    }

    #[test]
    fn sends_extended_thinking_params_to_an_anthropic_direct_haiku_model() {
        let config = anthropic("anthropic/claude-haiku-4-5-20251001", effort(Effort::Auto)).unwrap();
        assert_eq!(anthropic_params(&config), extended(16_384));
    }

    #[test]
    fn sends_no_thinking_params_to_an_anthropic_direct_claude_below_the_floor() {
        let config = anthropic("anthropic/claude-3-haiku-20240307", effort(Effort::Auto)).unwrap();
        assert_eq!(config.params, None);
    }

    #[test]
    fn gives_a_bedrock_claude_sonnet_model_tier_max_tokens() {
        let config = bedrock_at("bedrock/global.anthropic.claude-sonnet-5", Effort::Auto);
        assert_eq!(config.max_tokens, Some(128_000));
    }

    #[test]
    fn gives_a_bedrock_claude_fable_model_tier_max_tokens() {
        let config = bedrock_at("bedrock/global.anthropic.claude-fable-5", Effort::Auto);
        assert_eq!(config.max_tokens, Some(128_000));
    }

    #[test]
    fn does_not_force_max_tokens_on_a_non_claude_bedrock_model() {
        assert_eq!(
            bedrock_at("bedrock/amazon.nova-pro-v1:0", Effort::Auto).max_tokens,
            None
        );
    }

    #[test]
    fn does_not_force_max_tokens_on_a_legacy_claude_3_bedrock_id() {
        let config = bedrock_at("bedrock/anthropic.claude-3-haiku-20240307-v1:0", Effort::Auto);
        assert_eq!(config.max_tokens, None);
    }

    #[test]
    fn builds_bedrock_mantle_via_the_openai_provider() {
        let config = openai("bedrock-mantle/openai.gpt-oss-120b", effort(Effort::Auto)).unwrap();
        assert_eq!(config.model_id.as_deref(), Some("openai.gpt-oss-120b"));
        assert_eq!(params(&config), json!({ "reasoning": { "effort": "high" } }));
        let model = resolved(
            Some(&spec("bedrock-mantle/openai.gpt-oss-120b")),
            effort(Effort::Auto),
        );
        assert_eq!(model_id(&model).as_deref(), Some("openai.gpt-oss-120b"));
    }

    #[test]
    fn adds_the_bedrock_web_search_tool_for_bedrock_mantle_web_search() {
        let config = openai(
            "bedrock-mantle/openai.gpt-5.6-luna",
            options(Effort::Auto, true, false, false),
        )
        .unwrap();
        assert_eq!(
            params(&config),
            json!({
                "reasoning": { "effort": "high" },
                "tools": [{ "type": "web_search", "external_web_access": true }],
            })
        );
    }

    #[test]
    fn adds_the_bedrock_web_search_tool_with_effort_off() {
        let config = openai(
            "bedrock-mantle/openai.gpt-5.6-luna",
            options(Effort::Off, true, false, false),
        )
        .unwrap();
        assert_eq!(
            params(&config),
            json!({
                "reasoning": { "effort": "none" },
                "tools": [{ "type": "web_search", "external_web_access": true }],
            })
        );
    }

    #[test]
    fn adds_the_bedrock_web_search_tool_on_gpt_6() {
        let config = openai(
            "bedrock-mantle/openai.gpt-6-astra",
            options(Effort::Off, true, false, false),
        )
        .unwrap();
        assert_eq!(
            params(&config),
            json!({
                "reasoning": { "effort": "none" },
                "tools": [{ "type": "web_search", "external_web_access": true }],
            })
        );
    }

    #[test]
    fn omits_bedrock_mantle_tools_when_web_search_is_off() {
        let config = openai("bedrock-mantle/openai.gpt-5.6-luna", effort(Effort::Auto)).unwrap();
        assert_eq!(params(&config).get("tools"), None);
    }

    #[test]
    fn adds_the_openai_web_search_tool_alongside_reasoning() {
        let config = openai("openai/gpt-5.6-sol", options(Effort::High, true, false, false)).unwrap();
        assert_eq!(
            params(&config),
            json!({ "reasoning": { "effort": "high" }, "tools": [{ "type": "web_search" }] })
        );
    }

    #[test]
    fn adds_the_openai_web_search_tool_with_effort_off() {
        let config = openai("openai/gpt-5.6-sol", options(Effort::Off, true, false, false)).unwrap();
        assert_eq!(
            params(&config),
            json!({ "reasoning": { "effort": "none" }, "tools": [{ "type": "web_search" }] })
        );
    }

    #[test]
    fn omits_openai_tools_when_web_search_is_off() {
        let config = openai("openai/gpt-5.6-sol", effort(Effort::High)).unwrap();
        assert_eq!(params(&config).get("tools"), None);
    }

    #[test]
    fn adds_the_anthropic_web_search_server_tool_as_a_direct_call() {
        let config = anthropic(
            "anthropic/claude-opus-4-8",
            options(Effort::High, true, false, false),
        )
        .unwrap();
        assert_eq!(
            config.anthropic_tools,
            Some(vec![json!({
                "type": "web_search_20260318",
                "name": "web_search",
                "allowed_callers": ["direct"],
            })])
        );
        assert!(anthropic_params(&config).get("thinking").is_some());
    }

    #[test]
    fn omits_anthropic_server_tools_when_web_search_is_off() {
        let config = anthropic("anthropic/claude-opus-4-8", effort(Effort::High)).unwrap();
        assert_eq!(config.anthropic_tools, None);
    }

    #[test]
    fn never_adds_a_native_search_tool_where_the_model_has_none() {
        for id in [
            "bedrock-mantle/qwen.qwen3-32b-v1:0",
            "bedrock-mantle/openai.gpt-oss-120b-1:0",
        ] {
            let config = openai(id, options(Effort::High, true, false, false)).unwrap();
            assert_eq!(params(&config).get("tools"), None, "{id}");
        }
    }

    #[test]
    fn warns_instead_of_throwing_when_an_effort_level_is_requested_on_a_model_instance() {
        let instance = bedrock_instance("anything");
        let (model, warnings) = WarnRecorder::capture(|| resolved(Some(&instance), effort(Effort::High)));
        assert!(same_model(&model, &instance));
        assert_eq!(
            warnings,
            [
                "effort \"high\" not applied to a pre-built Model instance or ModelRouter; configure reasoning on the instance"
            ]
        );
    }

    #[test]
    fn does_not_warn_for_a_model_instance_with_default_effort_and_no_web_search_or_caching() {
        let instance = bedrock_instance("anything");
        let (model, warnings) = WarnRecorder::capture(|| resolved(Some(&instance), effort(Effort::Auto)));
        assert!(same_model(&model, &instance));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn enables_prompt_caching_for_anthropic_direct_when_requested() {
        let config = anthropic(
            "anthropic/claude-opus-4-8",
            options(Effort::Auto, false, true, false),
        )
        .unwrap();
        assert_eq!(config.cache_config, auto_cache());
    }

    #[test]
    fn does_not_set_cache_config_for_anthropic_direct_when_caching_is_off() {
        let config = anthropic("anthropic/claude-opus-4-8", effort(Effort::Auto)).unwrap();
        assert_eq!(config.cache_config, None);
    }

    #[test]
    fn configures_caching_without_changing_reasoning_or_search() {
        for (spec, model_id, search_tool) in [
            (
                "openai/gpt-5.6-sol",
                "gpt-5.6-sol",
                json!({ "type": "web_search" }),
            ),
            (
                "bedrock-mantle/openai.gpt-6-luna",
                "openai.gpt-6-luna",
                json!({ "type": "web_search", "external_web_access": true }),
            ),
        ] {
            for caching in [true, false] {
                let config = openai(spec, options(Effort::High, true, caching, false)).unwrap();
                let mut params = strands::types::json::JsonObject::new();
                params.insert("reasoning".to_owned(), json!({ "effort": "high" }));
                params.insert("tools".to_owned(), json!([search_tool]));
                let expected = OpenAIModelConfig {
                    model_id: Some(model_id.to_owned()),
                    cache_config: if caching { auto_cache() } else { None },
                    params: Some(params),
                    ..Default::default()
                };
                assert_eq!(config, expected, "{spec} caching={caching}");
            }
        }
    }

    #[test]
    fn warns_instead_of_throwing_when_caching_is_explicitly_requested_on_a_model_instance() {
        let instance = bedrock_instance("anything");
        let (model, warnings) =
            WarnRecorder::capture(|| resolved(Some(&instance), options(Effort::Auto, false, true, true)));
        assert!(same_model(&model, &instance));
        assert_eq!(
            warnings,
            [
                "prompt caching not applied to a pre-built Model instance or ModelRouter; configure it on the instance"
            ]
        );
    }

    #[test]
    fn does_not_warn_when_default_caching_hits_a_model_instance() {
        let instance = bedrock_instance("anything");
        let (model, warnings) =
            WarnRecorder::capture(|| resolved(Some(&instance), options(Effort::Auto, false, true, false)));
        assert!(same_model(&model, &instance));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn does_not_warn_when_caching_is_supported() {
        let (config, warnings) = WarnRecorder::capture(|| {
            openai(
                "bedrock-mantle/openai.gpt-oss-120b",
                options(Effort::Auto, false, true, false),
            )
        });
        assert_eq!(config.unwrap().cache_config, auto_cache());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// Building reads the API keys and the endpoint through the env seam.
    #[test]
    fn builds_each_provider_from_the_env_lookup() {
        for (spec, id) in [
            ("anthropic/claude-opus-4-8", "claude-opus-4-8"),
            ("openai/gpt-5.6-sol", "gpt-5.6-sol"),
            ("bedrock-mantle/openai.gpt-5.6-luna", "openai.gpt-5.6-luna"),
            ("us.anthropic.claude-sonnet-4-6", "us.anthropic.claude-sonnet-4-6"),
        ] {
            let model = resolved(Some(&super::spec(spec)), effort(Effort::Auto));
            assert_eq!(model_id(&model).as_deref(), Some(id));
        }
        let custom = env_of(&[
            ("ANTHROPIC_API_KEY", "test-key"),
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:1/anthropic"),
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", "http://127.0.0.1:1/v1"),
        ]);
        let anthropic = build::anthropic_options("claude-opus-4-8", None, false, false, &custom);
        assert_eq!(
            anthropic.client_config.base_url.as_deref(),
            Some("http://127.0.0.1:1/anthropic")
        );
        let openai = build::openai_options("gpt-5.6-sol", None, false, false, &custom);
        assert_eq!(
            openai.client_config.base_url.as_deref(),
            Some("http://127.0.0.1:1/v1")
        );
        assert!(openai.api_key.is_some());
    }
}
