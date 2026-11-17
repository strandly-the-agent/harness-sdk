//! The cases of `harness-ts/test/agent.test.ts` that read the provider model `create_harness`
//! builds from a model string (TS `agent.model.getConfig()`).
//!
//! The typed model is behind `Arc<dyn Model>` once built, so each case records what
//! `create_harness` resolved the model with (`Seams::on_resolve_model`) and derives the
//! provider config from that with the functions the resolver builds the model from.

use std::sync::Mutex;

use serde_json::json;
use strands::models::anthropic::AnthropicModelConfig;
use strands::models::bedrock::BedrockModelConfig;
use strands::models::model::{CacheConfig, CacheStrategy};
use strands::models::openai::OpenAIModelConfig;
use tracing::Level;

use super::*;
use crate::defaults::DEFAULT_MODEL;
use crate::env::EnvLookup;
use crate::logging::warned;
use crate::models::{build, plan};
use crate::test_support::WarnRecorder;

#[cfg(feature = "mcp")]
const EXA_WARNING: &str = "web_search is opted into Exa (exa.ai), a third-party service: every search query leaves your environment and is subject to Exa's privacy policy (https://exa.ai/privacy-policy).";
const INSTANCE_CACHING_WARNING: &str =
    "prompt caching not applied to a pre-built Model instance or ModelRouter; configure it on the instance";

/// A dummy key for every provider and a region, so building a model reads nothing else.
pub(super) fn env() -> EnvLookup {
    Arc::new(|name| match name {
        "AWS_REGION" => Some("us-west-2".to_owned()),
        _ => name.ends_with("_API_KEY").then(|| "test-key".to_owned()),
    })
}

pub(super) fn model(spec: &str) -> HarnessAgentOptions {
    HarnessAgentOptions {
        model: Some(ModelSource::Id(spec.to_owned())),
        ..Default::default()
    }
}

/// Builds the agent and returns it with what its model was resolved with.
async fn resolved(options: HarnessAgentOptions) -> (Agent, ResolveModelOptions) {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(None));
    let seams = Seams {
        base_dir: Some(dir.path().to_owned()),
        env: env(),
        on_resolve_model: Some(Arc::new({
            let seen = Arc::clone(&seen);
            move |options| *seen.lock().unwrap() = Some(options)
        })),
    };
    let agent = create_harness_with(options, seams).await.unwrap();
    let options = seen.lock().unwrap().take().expect("the model was resolved");
    (agent, options)
}

/// [`resolved`] on a runtime of this thread, with the events of `level` logged meanwhile.
fn resolved_logging(level: Level, options: HarnessAgentOptions) -> (Agent, ResolveModelOptions, Vec<String>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ((agent, options), logged) =
        WarnRecorder::capture_level(level, || runtime.block_on(resolved(options)));
    (agent, options, logged)
}

fn model_id(agent: &Agent) -> Option<String> {
    agent.model().get_config().model_id
}

/// The config of the Bedrock model `agent` holds, built from `spec` with `options`.
fn bedrock_config(agent: &Agent, spec: &str, options: ResolveModelOptions) -> BedrockModelConfig {
    let plan = plan(spec, options).unwrap();
    assert_eq!(plan.provider.name, "bedrock");
    let config = build::bedrock_options(plan.name, plan.level, plan.caching, &env()).config;
    assert_eq!(model_id(agent), config.model_id);
    config
}

fn anthropic_config(agent: &Agent, spec: &str, options: ResolveModelOptions) -> AnthropicModelConfig {
    let plan = plan(spec, options).unwrap();
    assert_eq!(plan.provider.name, "anthropic");
    let config =
        build::anthropic_options(plan.name, plan.level, plan.web_search, plan.caching, &env()).config;
    assert_eq!(model_id(agent), config.model_id);
    config
}

/// The config of an `openai/` or `bedrock-mantle/` model.
pub(super) fn openai_config(agent: &Agent, spec: &str, options: ResolveModelOptions) -> OpenAIModelConfig {
    let plan = plan(spec, options).unwrap();
    let (name, level, search, caching) = (plan.name, plan.level, plan.web_search, plan.caching);
    let config = match plan.provider.name {
        "openai" => build::openai_options(name, level, search, caching, &env()).config,
        "bedrock-mantle" => {
            build::bedrock_mantle_options(name, level, search, caching, &env())
                .0
                .config
        }
        other => panic!("{spec} resolved to {other}"),
    };
    assert_eq!(model_id(agent), config.model_id);
    config
}

pub(super) fn params(config: &OpenAIModelConfig) -> JsonValue {
    config.params.clone().map_or(JsonValue::Null, JsonValue::Object)
}

fn auto_cache() -> Option<CacheConfig> {
    Some(CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        ..Default::default()
    })
}

#[tokio::test]
async fn enables_web_search_on_a_supported_provider_from_the_default_set() {
    let spec = "openai/gpt-5.6-sol";
    let (agent, options) = resolved(model(spec)).await;
    let config = openai_config(&agent, spec, options);
    assert_eq!(params(&config)["tools"], json!([{ "type": "web_search" }]));
}

#[test]
fn logs_info_not_a_warning_and_builds_when_default_web_search_hits_the_default_bedrock_model() {
    let (agent, _, info) = resolved_logging(Level::INFO, HarnessAgentOptions::default());
    assert!(!has_tool(&agent, "web_search"));
    let line = info
        .iter()
        .find(|line| line.contains("has no built-in web search"))
        .expect("an info line");
    assert!(line.ends_with("web-access/#web_search"), "{line}");

    let (_, _, warnings) = resolved_logging(Level::WARN, HarnessAgentOptions::default());
    assert!(
        !warnings
            .iter()
            .any(|line| line.contains("has no built-in web search")),
        "{warnings:?}"
    );
}

#[cfg(feature = "mcp")]
#[test]
fn builds_the_exa_tool_for_the_fallback_and_warns_about_the_third_party() {
    let (agent, _, warnings) = resolved_logging(
        Level::WARN,
        HarnessAgentOptions {
            builtin_tools: Some(BuiltinToolsOption::Config(BuiltinToolsConfig {
                web_search: Some(WebSearchSetting::Exa),
                ..Default::default()
            })),
            ..Default::default()
        },
    );
    assert!(has_tool(&agent, "web_search"));
    assert_eq!(warnings, [EXA_WARNING]);
    assert!(
        !warnings
            .iter()
            .any(|line| line.contains("has no built-in web search")),
        "{warnings:?}"
    );
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn lets_an_explicit_exa_selection_win_over_native_search() {
    let spec = "openai/gpt-5.6-sol";
    let (agent, options) = resolved(HarnessAgentOptions {
        builtin_tools: Some(BuiltinToolsOption::Config(BuiltinToolsConfig {
            web_search: Some(WebSearchSetting::Exa),
            ..Default::default()
        })),
        ..model(spec)
    })
    .await;
    assert!(has_tool(&agent, "web_search"));
    assert_eq!(params(&openai_config(&agent, spec, options)).get("tools"), None);
}

#[tokio::test]
async fn enables_anthropic_native_web_search_from_the_default_set() {
    let spec = "anthropic/claude-opus-4-8";
    let (agent, options) = resolved(model(spec)).await;
    assert!(!has_tool(&agent, "web_search"));
    assert_eq!(
        anthropic_config(&agent, spec, options).anthropic_tools,
        Some(vec![json!({
            "type": "web_search_20260318",
            "name": "web_search",
            "allowed_callers": ["direct"],
        })])
    );
}

#[test]
fn only_offers_bedrock_web_search_on_gpt_5_and_gpt_6_mantle_models() {
    let spec = "bedrock-mantle/openai.gpt-5.6-luna";
    let (agent, options, _) = resolved_logging(Level::INFO, model(spec));
    assert!(!has_tool(&agent, "web_search"));
    assert_eq!(
        params(&openai_config(&agent, spec, options))["tools"],
        json!([{ "type": "web_search", "external_web_access": true }])
    );

    let spec = "bedrock-mantle/openai.gpt-oss-120b-1:0";
    let (agent, options, info) = resolved_logging(Level::INFO, model(spec));
    assert!(!has_tool(&agent, "web_search"));
    assert_eq!(params(&openai_config(&agent, spec, options)).get("tools"), None);
    assert!(
        info.iter()
            .any(|line| line.contains("has no built-in web search")),
        "{info:?}"
    );
}

#[tokio::test]
async fn enables_caching_by_default_on_bedrock() {
    let (agent, options) = resolved(HarnessAgentOptions::default()).await;
    assert_eq!(
        bedrock_config(&agent, DEFAULT_MODEL, options).cache_config,
        auto_cache()
    );
}

#[tokio::test]
async fn enables_caching_by_default_on_anthropic_direct() {
    let spec = "anthropic/claude-opus-4-8";
    let (agent, options) = resolved(model(spec)).await;
    assert_eq!(anthropic_config(&agent, spec, options).cache_config, auto_cache());
}

// TS runs five values; Rust's typed option has three (`undefined` and `'auto'` are `Auto`,
// `false` and `null` are `Off`).
#[tokio::test]
async fn configures_caching() {
    for spec in ["openai/gpt-5.6-sol", "bedrock-mantle/openai.gpt-6-luna"] {
        for caching in [CachingOption::Auto, CachingOption::On, CachingOption::Off] {
            let (agent, options) = resolved(HarnessAgentOptions {
                caching,
                session: SessionOption::Off,
                memory: MemoryOption::Off,
                skills: SkillsOption::Off,
                builtin_tools: names(&[]),
                ..model(spec)
            })
            .await;
            let expected = if caching == CachingOption::Off {
                None
            } else {
                auto_cache()
            };
            assert_eq!(
                openai_config(&agent, spec, options).cache_config,
                expected,
                "{spec} {caching:?}"
            );
        }
    }
}

#[test]
fn builds_silently_with_default_caching_on_a_model_instance() {
    let (_, _, warnings) = resolved_logging(Level::WARN, mock_options());
    assert!(
        !warnings
            .iter()
            .any(|line| line.contains("prompt caching not applied")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn warns_when_caching_is_explicitly_requested_on_a_model_instance() {
    resolved(HarnessAgentOptions {
        caching: CachingOption::On,
        ..mock_options()
    })
    .await;
    // Logged once per process, by this build or the next case's.
    assert!(warned(INSTANCE_CACHING_WARNING));
}

// TS passes `'auto'` explicitly; Rust's `Auto` is the default, so the explicit request is `On`.
#[tokio::test]
async fn warns_instead_of_throwing_when_caching_is_explicitly_enabled_on_a_model_instance() {
    let instance: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    let (agent, _) = resolved(HarnessAgentOptions {
        model: Some(ModelSource::Model(Arc::clone(&instance))),
        caching: CachingOption::On,
        ..Default::default()
    })
    .await;
    assert!(Arc::ptr_eq(agent.model(), &instance));
    assert!(warned(INSTANCE_CACHING_WARNING));
}

#[tokio::test]
async fn leaves_no_cache_config_when_caching_is_disabled() {
    let (agent, options) = resolved(HarnessAgentOptions {
        caching: CachingOption::Off,
        ..Default::default()
    })
    .await;
    assert_eq!(bedrock_config(&agent, DEFAULT_MODEL, options).cache_config, None);
}

#[tokio::test]
async fn uses_the_recommended_level_when_effort_is_auto() {
    let (agent, options) = resolved(HarnessAgentOptions {
        effort: Effort::Auto,
        ..Default::default()
    })
    .await;
    let fields = bedrock_config(&agent, DEFAULT_MODEL, options)
        .additional_request_fields
        .expect("reasoning fields");
    assert_eq!(fields["output_config"]["effort"], "high");
}

#[tokio::test]
async fn sends_no_reasoning_fields_when_effort_is_off() {
    let (agent, options) = resolved(HarnessAgentOptions {
        effort: Effort::Off,
        ..Default::default()
    })
    .await;
    assert_eq!(
        bedrock_config(&agent, DEFAULT_MODEL, options).additional_request_fields,
        None
    );
}
