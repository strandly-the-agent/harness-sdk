//! Config, cache and request helpers of `models/model.ts`.

use serde_json::json;

use super::*;
use crate::models::model_info::ModelLimits;

#[test]
fn merge_overwrites_only_the_fields_the_update_sets() {
    let mut config = BaseModelConfig {
        model_id: Some("a".into()),
        max_tokens: Some(10),
        temperature: Some(0.5),
        ..Default::default()
    };
    config.merge(BaseModelConfig {
        max_tokens: Some(20),
        top_p: Some(0.9),
        ..Default::default()
    });
    assert_eq!(
        config,
        BaseModelConfig {
            model_id: Some("a".into()),
            max_tokens: Some(20),
            temperature: Some(0.5),
            top_p: Some(0.9),
            ..Default::default()
        }
    );
}

#[test]
fn base_model_config_serializes_with_ts_keys() {
    let config = BaseModelConfig::builder()
        .model_id("m")
        .max_tokens(5)
        .context_window_limit(100)
        .build();
    assert_eq!(
        serde_json::to_value(&config).unwrap(),
        json!({ "modelId": "m", "maxTokens": 5, "contextWindowLimit": 100 })
    );
}

#[test]
fn resolve_config_metadata_fills_the_context_window_from_model_info() {
    let config = resolve_config_metadata(BaseModelConfig::default(), "claude-sonnet-4-6");
    assert_eq!(config.context_window_limit, Some(1_000_000));
    let set = BaseModelConfig {
        context_window_limit: Some(5),
        ..Default::default()
    };
    assert_eq!(
        resolve_config_metadata(set, "claude-sonnet-4-6").context_window_limit,
        Some(5)
    );
    assert_eq!(
        resolve_config_metadata(BaseModelConfig::default(), "unknown").context_window_limit,
        None
    );
}

#[test]
fn resolve_config_metadata_lets_the_info_override_win_over_the_shared_table() {
    let config = BaseModelConfig {
        model_info: Some(ModelInfo {
            limits: Some(ModelLimits {
                context_window: Some(42),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        resolve_config_metadata(config, "claude-sonnet-4-6").context_window_limit,
        Some(42)
    );
}

#[test]
fn resolve_max_tokens_prefers_the_config_then_the_known_output_limit() {
    let config = BaseModelConfig {
        max_tokens: Some(7),
        ..Default::default()
    };
    assert_eq!(resolve_max_tokens(&config, "claude-sonnet-4-6"), Some(7));
    assert_eq!(
        resolve_max_tokens(&BaseModelConfig::default(), "claude-sonnet-4-6"),
        Some(128_000)
    );
    assert_eq!(resolve_max_tokens(&BaseModelConfig::default(), "unknown"), None);
}

#[test]
fn resolve_cache_section_is_on_unless_false_and_takes_the_first_ttl() {
    let five = Some("5m");
    let hour = Some("1h");
    assert_eq!(
        resolve_cache_section(None, &[]),
        ResolvedCacheSection {
            enabled: true,
            ttl: None
        }
    );
    assert!(!resolve_cache_section(Some(&false.into()), &[hour]).enabled);
    assert_eq!(
        resolve_cache_section(Some(&true.into()), &[None, hour])
            .ttl
            .as_deref(),
        Some("1h")
    );
    assert_eq!(
        resolve_cache_section(Some(&"5m".into()), &[hour]).ttl.as_deref(),
        Some("5m")
    );
    assert_eq!(
        resolve_cache_section(Some(&"".into()), &[Some(""), five])
            .ttl
            .as_deref(),
        Some("5m")
    );
}

#[test]
fn latest_user_index_finds_the_last_user_message() {
    let user = Message::new(Role::User, vec![]);
    let assistant = Message::new(Role::Assistant, vec![]);
    assert_eq!(latest_user_index(&[]), None);
    assert_eq!(latest_user_index(std::slice::from_ref(&assistant)), None);
    assert_eq!(
        latest_user_index(&[user.clone(), assistant.clone(), user, assistant]),
        Some(2)
    );
}

// Ported from strands-ts/src/models/__tests__/model.test.ts (`Model.modelId`, `countTokens`,
// `info`, `estimateUtilization`)

use crate::models::model_info::{ModelCapabilities, PromptCaching};
use crate::models::{BoxStream, ModelStreamEvent};
use crate::test_support::{ScriptedModel, user_text};
use crate::tools::{ToolResultStatus, ToolSpec};
use crate::types::{
    Citation, CitationGeneratedContent, CitationLocation, CitationsBlock, GuardContentBlock,
    GuardContentText, GuardQualifier, InjectedBlock, ReasoningBlock, SummaryBlock, SystemContentBlock,
    TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};

fn provider() -> ScriptedModel {
    ScriptedModel::new(Vec::new())
}

fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    Message::new(role, content)
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text(TextBlock::new(text))
}

async fn count(messages: &[Message], options: CountTokensOptions) -> u64 {
    provider().count_tokens(messages, options).await.unwrap()
}

fn ceil4(text: &str) -> u64 {
    (text.encode_utf16().count() as u64).div_ceil(4)
}

#[test]
fn returns_model_id_from_model_config() {
    let provider = provider();
    provider.update_config(BaseModelConfig::builder().model_id("my-model").build());
    assert_eq!(provider.model_id().as_deref(), Some("my-model"));
}

#[tokio::test]
async fn counts_summary_text_like_a_text_block() {
    let long = "x".repeat(400);
    let summary = count(
        &[message(
            Role::Assistant,
            vec![ContentBlock::Summary(SummaryBlock::new(long.clone()))],
        )],
        CountTokensOptions::default(),
    )
    .await;
    assert_eq!(
        summary,
        count(
            &[message(Role::Assistant, vec![text(&long)])],
            CountTokensOptions::default()
        )
        .await
    );
}

#[tokio::test]
async fn estimates_text_block_tokens_using_chars_4_heuristic() {
    assert_eq!(
        count(&[user_text("Hello world")], CountTokensOptions::default()).await,
        3
    );
}

fn weather_call() -> ContentBlock {
    ContentBlock::ToolUse(ToolUseBlock::new(
        "get_weather",
        "id1",
        json!({ "city": "Seattle" }),
    ))
}

#[tokio::test]
async fn estimates_tool_use_block_tokens_name_json_input() {
    assert_eq!(
        count(
            &[message(Role::Assistant, vec![weather_call()])],
            CountTokensOptions::default()
        )
        .await,
        3 + 9
    );
}

fn tool_result(text_content: &str) -> ContentBlock {
    ContentBlock::ToolResult(ToolResultBlock::new(
        "id1",
        ToolResultStatus::Success,
        vec![ToolResultContent::Text(TextBlock::new(text_content))],
    ))
}

#[tokio::test]
async fn estimates_tool_result_block_tokens_text_items_only() {
    let result = count(
        &[message(Role::User, vec![tool_result("72°F and sunny")])],
        CountTokensOptions::default(),
    )
    .await;
    assert_eq!(result, ceil4("72°F and sunny"));
}

#[tokio::test]
async fn estimates_reasoning_block_tokens() {
    let block =
        ContentBlock::Reasoning(ReasoningBlock::new().with_text("Let me think about this step by step"));
    assert_eq!(
        count(
            &[message(Role::Assistant, vec![block])],
            CountTokensOptions::default()
        )
        .await,
        ceil4("Let me think about this step by step")
    );
}

fn guard(text: &str) -> GuardContentBlock {
    GuardContentBlock::from(GuardContentText::new(vec![GuardQualifier::Query], text))
}

#[tokio::test]
async fn estimates_guard_content_block_tokens() {
    let block = ContentBlock::GuardContent(guard("Is this safe?"));
    assert_eq!(
        count(&[message(Role::User, vec![block])], CountTokensOptions::default()).await,
        ceil4("Is this safe?")
    );
}

#[tokio::test]
async fn estimates_citations_block_tokens() {
    let citation = Citation::new(
        CitationLocation::web("https://a.com", None),
        "src",
        Vec::new(),
        "A",
    );
    let block = ContentBlock::Citations(CitationsBlock::new(
        vec![citation],
        vec![CitationGeneratedContent::new("cited text here")],
    ));
    assert_eq!(
        count(
            &[message(Role::Assistant, vec![block])],
            CountTokensOptions::default()
        )
        .await,
        ceil4("cited text here")
    );
}

fn injected(text: &str) -> ContentBlock {
    ContentBlock::Injected(InjectedBlock::new(text, "test"))
}

#[tokio::test]
async fn estimates_injected_block_tokens() {
    assert_eq!(
        count(
            &[message(Role::User, vec![injected("cwd: /repo, date: today")])],
            CountTokensOptions::default()
        )
        .await,
        ceil4("cwd: /repo, date: today")
    );
}

#[tokio::test]
async fn counts_only_the_injected_blocks_of_the_latest_user_message() {
    let messages = [
        message(Role::User, vec![injected(&"x".repeat(400))]),
        message(Role::Assistant, vec![text("ok")]),
        message(Role::User, vec![injected("cwd: /repo")]),
    ];
    assert_eq!(
        count(&messages, CountTokensOptions::default()).await,
        ceil4("ok") + ceil4("cwd: /repo")
    );
}

#[tokio::test]
async fn estimates_string_system_prompt_tokens() {
    let options = CountTokensOptions::builder()
        .system_prompt("You are a helpful assistant")
        .build();
    assert_eq!(
        count(&[user_text("Hi")], options).await,
        ceil4("You are a helpful assistant") + ceil4("Hi")
    );
}

#[tokio::test]
async fn estimates_array_system_prompt_tokens() {
    let options = CountTokensOptions::builder()
        .system_prompt(SystemPrompt::Blocks(vec![SystemContentBlock::text(
            "System instructions",
        )]))
        .build();
    assert_eq!(
        count(&[user_text("Hi")], options).await,
        ceil4("System instructions") + ceil4("Hi")
    );
}

#[tokio::test]
async fn estimates_tool_spec_tokens() {
    let spec = ToolSpec::new("get_weather", "Get weather for a city");
    let spec_json = serde_json::to_string(&spec).unwrap();
    assert_eq!(
        spec_json,
        r#"{"name":"get_weather","description":"Get weather for a city"}"#
    );
    let options = CountTokensOptions::builder().tool_specs(vec![spec]).build();
    assert_eq!(
        count(&[user_text("Hi")], options).await,
        ceil4("Hi") + (spec_json.len() as u64).div_ceil(2)
    );
}

#[tokio::test]
async fn returns_0_for_empty_messages() {
    assert_eq!(count(&[], CountTokensOptions::default()).await, 0);
}

#[tokio::test]
async fn skips_reasoning_blocks_without_text() {
    let block = ContentBlock::Reasoning(ReasoningBlock::new().with_signature("sig123"));
    assert_eq!(
        count(
            &[message(Role::Assistant, vec![block])],
            CountTokensOptions::default()
        )
        .await,
        0
    );
}

#[tokio::test]
async fn estimates_guard_content_in_array_system_prompt() {
    let options = CountTokensOptions::builder()
        .system_prompt(SystemPrompt::Blocks(vec![SystemContentBlock::GuardContent(
            guard("Guard text here"),
        )]))
        .build();
    assert_eq!(
        count(&[user_text("Hi")], options).await,
        ceil4("Guard text here") + ceil4("Hi")
    );
}

#[tokio::test]
async fn accumulates_tokens_across_multiple_messages_with_mixed_content() {
    let messages = [
        user_text("What is the weather?"),
        message(Role::Assistant, vec![weather_call()]),
        message(Role::User, vec![tool_result("72F")]),
    ];
    let options = CountTokensOptions::builder()
        .system_prompt("You are helpful")
        .build();
    let expected = ceil4("You are helpful")
        + ceil4("What is the weather?")
        + ceil4("get_weather")
        + (r#"{"city":"Seattle"}"#.len() as u64).div_ceil(2)
        + ceil4("72F");
    assert_eq!(count(&messages, options).await, expected);
}

#[tokio::test]
async fn counts_utf16_code_units_like_js_string_length() {
    assert_eq!(
        count(&[user_text("😀😀")], CountTokensOptions::default()).await,
        1
    );
    assert_eq!(
        count(&[user_text("😀😀😀")], CountTokensOptions::default()).await,
        2
    );
}

fn capabilities(capabilities: ModelCapabilities) -> ModelInfo {
    ModelInfo {
        capabilities: Some(capabilities),
        ..Default::default()
    }
}

#[test]
fn knows_only_that_temporary_messages_are_off_for_a_provider_with_no_model_id_and_unknown_stateful_reads_as_false()
 {
    let provider = provider();
    assert_eq!(
        serde_json::to_value(provider.model_info()).unwrap(),
        json!({ "capabilities": { "temporaryMessages": false } })
    );
    assert!(!provider.stateful());
}

#[test]
fn resolves_facts_from_the_shared_model_info_for_any_provider_by_model_id() {
    let provider = provider();
    provider.update_config(
        BaseModelConfig::builder()
            .model_id("us.anthropic.claude-sonnet-4-6")
            .build(),
    );
    assert_eq!(provider.model_info().context_window(), Some(1_000_000));
    assert_eq!(
        provider.model_info().capabilities().prompt_caching,
        Some(PromptCaching::CachePoints)
    );
}

#[test]
fn leaves_temporary_messages_off_for_a_model_the_shared_data_says_takes_them() {
    let provider = provider();
    provider.update_config(
        BaseModelConfig::builder()
            .model_id("vertex.claude-opus-5-5")
            .build(),
    );
    assert_eq!(
        provider.model_info().capabilities().temporary_messages,
        Some(false)
    );
}

/// A provider that puts its own facts over the default model info, the user override over both.
struct Overriding {
    inner: ScriptedModel,
    facts: ModelInfo,
}

impl Model for Overriding {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        self.inner.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.inner.get_config()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.inner.update_config(config);
    }

    fn model_info(&self) -> ModelInfo {
        let base = self.inner.model_info();
        merge_model_info([
            Some(&base),
            Some(&self.facts),
            self.get_config().model_info.as_ref(),
        ])
    }
}

#[test]
fn lets_a_provider_put_its_facts_over_the_shared_model_info_with_the_user_override_over_both() {
    let provider = Overriding {
        inner: ScriptedModel::new(Vec::new()),
        facts: capabilities(ModelCapabilities {
            prompt_caching: Some(PromptCaching::Unsupported),
            ..Default::default()
        }),
    };
    provider.update_config(
        BaseModelConfig::builder()
            .model_id("claude-sonnet-4-6")
            .context_window_limit(8_000)
            .build(),
    );
    assert_eq!(
        provider.model_info().capabilities().prompt_caching,
        Some(PromptCaching::Unsupported)
    );
    assert_eq!(provider.model_info().context_window(), Some(8_000));
    provider.update_config(BaseModelConfig {
        model_info: Some(capabilities(ModelCapabilities {
            prompt_caching: Some(PromptCaching::CachePoints),
            ..Default::default()
        })),
        ..Default::default()
    });
    assert_eq!(
        provider.model_info().capabilities().prompt_caching,
        Some(PromptCaching::CachePoints)
    );
}

#[test]
fn lets_the_info_override_set_stateful_and_the_stateful_getter_reads_it() {
    let provider = provider();
    assert!(!provider.stateful());
    provider.update_config(BaseModelConfig {
        model_info: Some(capabilities(ModelCapabilities {
            stateful: Some(true),
            ..Default::default()
        })),
        ..Default::default()
    });
    assert_eq!(provider.model_info().capabilities().stateful, Some(true));
    assert!(provider.stateful());
}

fn window(context_window: u64) -> ModelInfo {
    ModelInfo {
        limits: Some(ModelLimits {
            context_window: Some(context_window),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn lets_the_info_override_win_over_context_window_limit() {
    let provider = provider();
    provider.update_config(BaseModelConfig {
        context_window_limit: Some(50_000),
        model_info: Some(window(10_000)),
        ..Default::default()
    });
    assert_eq!(provider.model_info().context_window(), Some(10_000));
}

#[test]
fn takes_the_context_window_from_the_config() {
    let provider = provider();
    provider.update_config(BaseModelConfig::builder().context_window_limit(50_000).build());
    assert_eq!(provider.model_info().context_window(), Some(50_000));
}

#[test]
fn reads_stateful_from_what_a_provider_puts_in_the_record() {
    let provider = Overriding {
        inner: ScriptedModel::new(Vec::new()),
        facts: capabilities(ModelCapabilities {
            stateful: Some(true),
            ..Default::default()
        }),
    };
    assert!(provider.stateful());
    provider.update_config(BaseModelConfig {
        model_info: Some(capabilities(ModelCapabilities {
            stateful: Some(false),
            ..Default::default()
        })),
        ..Default::default()
    });
    assert!(!provider.stateful());
}

#[test]
fn reflects_config_changes_on_the_next_read() {
    let provider = provider();
    assert_eq!(provider.model_info().context_window(), None);
    provider.update_config(BaseModelConfig::builder().context_window_limit(4_096).build());
    assert_eq!(provider.model_info().context_window(), Some(4_096));
}

#[test]
fn uses_the_context_window_from_info_including_an_info_override() {
    let provider = provider();
    provider.update_config(BaseModelConfig {
        model_info: Some(window(10_000)),
        ..Default::default()
    });
    assert!((provider.estimate_utilization(5_000) - 0.5).abs() < f64::EPSILON);
}

#[test]
fn returns_ratio_of_input_tokens_to_context_window_limit() {
    let provider = provider();
    provider.update_config(BaseModelConfig::builder().context_window_limit(100_000).build());
    assert!((provider.estimate_utilization(50_000) - 0.5).abs() < f64::EPSILON);
}

#[test]
fn uses_default_context_window_limit_when_context_window_limit_is_not_set() {
    let (utilization, logs) = crate::test_support::capture_logs(|| provider().estimate_utilization(100_000));
    assert!((utilization - 0.5).abs() < f64::EPSILON);
    let warning = "contextWindowLimit is not set on the model, using default of 200000 for utilization estimate | set contextWindowLimit in your model config for accurate results";
    assert!(
        logs.iter().all(|log| log == &format!("WARN {warning}")),
        "{logs:?}"
    );
}

#[test]
fn returns_value_above_1_0_when_tokens_exceed_limit() {
    let provider = provider();
    provider.update_config(BaseModelConfig::builder().context_window_limit(1000).build());
    assert!(provider.estimate_utilization(1500) > 1.0);
}

#[test]
fn returns_0_for_zero_tokens() {
    let provider = provider();
    provider.update_config(BaseModelConfig::builder().context_window_limit(100_000).build());
    assert!(provider.estimate_utilization(0).abs() < f64::EPSILON);
}
