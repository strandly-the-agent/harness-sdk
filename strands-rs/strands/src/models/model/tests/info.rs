//! `Model.modelId`, `info`, `estimateUtilization` and server-side summary cases of
//! strands-ts/src/models/__tests__/model.test.ts and server-side-summary-capability.test.ts, and
//! the generic cache section cases of cache-portability.test.ts.

use serde_json::json;

use super::provider;
use crate::models::model::{
    BaseModelConfig, BoxStream, CacheConfig, CacheSection, CacheTTL, Model, ModelRequest, ModelSource,
    base_model_info, latest_user_index, resolve_cache_section, resolve_config_metadata, resolve_max_tokens,
};
use crate::models::model_info::{ModelCapabilities, ModelInfo, ModelLimits, PromptCaching, merge_model_info};
use crate::models::streaming::ModelStreamEvent;
use crate::testing::TestModelProvider;
use crate::types::messages::{Message, Role};

fn test_provider() -> TestModelProvider {
    provider(json!([]))
}

fn capabilities(capabilities: ModelCapabilities) -> ModelInfo {
    ModelInfo {
        capabilities: Some(capabilities),
        limits: None,
    }
}

fn context_window(context_window: u64) -> ModelInfo {
    ModelInfo {
        capabilities: None,
        limits: Some(ModelLimits {
            context_window: Some(context_window),
            max_output_tokens: None,
        }),
    }
}

fn config_with_info(model_info: ModelInfo) -> BaseModelConfig {
    BaseModelConfig {
        model_info: Some(model_info),
        ..BaseModelConfig::default()
    }
}

/// A provider that puts its own facts over the base info, with the user override over both.
struct LayeredProvider {
    inner: TestModelProvider,
    facts: ModelInfo,
}

impl Model for LayeredProvider {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        self.inner.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.inner.get_config()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.inner.update_config(config);
    }

    fn model_info(&self) -> ModelInfo {
        let config = self.get_config();
        merge_model_info([
            Some(&base_model_info(&config)),
            Some(&self.facts),
            config.model_info.as_ref(),
        ])
    }
}

/// A provider whose model id comes from somewhere other than its config.
struct RenamedProvider(TestModelProvider);

impl Model for RenamedProvider {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        self.0.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.0.get_config()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.0.update_config(config);
    }

    fn model_id(&self) -> Option<String> {
        Some("us.anthropic.claude-sonnet-4-6".to_owned())
    }
}

#[test]
fn resolves_the_shared_model_info_by_an_overridden_model_id() {
    let model = RenamedProvider(test_provider());
    assert_eq!(model.get_config().model_id.as_deref(), Some("test-model"));
    let info = model.model_info();
    assert_eq!(info.limits.unwrap().context_window, Some(1_000_000));
    assert_eq!(
        info.capabilities.unwrap().prompt_caching,
        Some(PromptCaching::CachePoints)
    );
}

#[test]
fn returns_model_id_from_model_config() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().model_id("my-model").build());
    assert_eq!(model.model_id().as_deref(), Some("my-model"));
}

#[test]
fn update_config_keeps_the_fields_an_update_leaves_out() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().max_tokens(100).build());
    assert_eq!(
        model.get_config(),
        BaseModelConfig::builder()
            .model_id("test-model")
            .max_tokens(100)
            .build()
    );
}

#[test]
fn knows_only_that_temporary_messages_are_off_for_a_provider_with_no_model_id_and_unknown_stateful_reads_as_false()
 {
    let model = test_provider();
    assert_eq!(
        serde_json::to_value(model.model_info()).unwrap(),
        json!({ "capabilities": { "temporaryMessages": false } })
    );
    assert!(!model.stateful());
}

#[test]
fn resolves_facts_from_the_shared_model_info_for_any_provider_by_model_id() {
    let model = test_provider();
    model.update_config(
        BaseModelConfig::builder()
            .model_id("us.anthropic.claude-sonnet-4-6")
            .build(),
    );
    let info = model.model_info();
    assert_eq!(info.limits.unwrap().context_window, Some(1_000_000));
    assert_eq!(
        info.capabilities.unwrap().prompt_caching,
        Some(PromptCaching::CachePoints)
    );
}

#[test]
fn leaves_temporary_messages_off_for_a_model_the_shared_data_says_takes_them() {
    let model = test_provider();
    model.update_config(
        BaseModelConfig::builder()
            .model_id("vertex.claude-opus-5-5")
            .build(),
    );
    assert_eq!(
        model.model_info().capabilities.unwrap().temporary_messages,
        Some(false)
    );
}

#[test]
fn lets_a_provider_put_its_facts_over_the_shared_model_info_with_the_user_override_over_both() {
    let model = LayeredProvider {
        inner: test_provider(),
        facts: capabilities(ModelCapabilities {
            prompt_caching: Some(PromptCaching::Unsupported),
            ..ModelCapabilities::default()
        }),
    };
    model.update_config(
        BaseModelConfig::builder()
            .model_id("claude-sonnet-4-6")
            .context_window_limit(8_000)
            .build(),
    );
    assert_eq!(
        model.model_info().capabilities.unwrap().prompt_caching,
        Some(PromptCaching::Unsupported)
    );
    assert_eq!(model.model_info().limits.unwrap().context_window, Some(8_000));
    model.update_config(config_with_info(capabilities(ModelCapabilities {
        prompt_caching: Some(PromptCaching::CachePoints),
        ..ModelCapabilities::default()
    })));
    assert_eq!(
        model.model_info().capabilities.unwrap().prompt_caching,
        Some(PromptCaching::CachePoints)
    );
}

#[test]
fn lets_the_info_override_set_stateful_and_the_stateful_getter_reads_it() {
    let model = test_provider();
    assert!(!model.stateful());
    model.update_config(config_with_info(capabilities(ModelCapabilities {
        stateful: Some(true),
        ..ModelCapabilities::default()
    })));
    assert_eq!(model.model_info().capabilities.unwrap().stateful, Some(true));
    assert!(model.stateful());
}

#[test]
fn lets_the_info_override_win_over_context_window_limit() {
    let model = test_provider();
    model.update_config(BaseModelConfig {
        context_window_limit: Some(50_000),
        model_info: Some(context_window(10_000)),
        ..BaseModelConfig::default()
    });
    assert_eq!(model.model_info().limits.unwrap().context_window, Some(10_000));
}

#[test]
fn takes_the_context_window_from_the_config() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().context_window_limit(50_000).build());
    assert_eq!(model.model_info().limits.unwrap().context_window, Some(50_000));
}

#[test]
fn reads_stateful_from_what_a_provider_puts_in_the_record() {
    let stateful = capabilities(ModelCapabilities {
        stateful: Some(true),
        ..ModelCapabilities::default()
    });
    let model = LayeredProvider {
        inner: test_provider(),
        facts: stateful,
    };
    assert!(model.stateful());
    model.update_config(config_with_info(capabilities(ModelCapabilities {
        stateful: Some(false),
        ..ModelCapabilities::default()
    })));
    assert!(!model.stateful());
}

#[test]
fn reflects_config_changes_on_the_next_read() {
    let model = test_provider();
    assert_eq!(model.model_info().limits, None);
    model.update_config(BaseModelConfig::builder().context_window_limit(4_096).build());
    assert_eq!(model.model_info().limits.unwrap().context_window, Some(4_096));
}

#[test]
fn uses_the_context_window_from_info_including_an_info_override() {
    let model = test_provider();
    model.update_config(config_with_info(context_window(10_000)));
    assert!((model.estimate_utilization(5_000) - 0.5).abs() < f64::EPSILON);
}

#[test]
fn returns_ratio_of_input_tokens_to_context_window_limit() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().context_window_limit(100_000).build());
    assert!((model.estimate_utilization(50_000) - 0.5).abs() < f64::EPSILON);
}

#[test]
fn uses_default_context_window_limit_when_context_window_limit_is_not_set() {
    let model = test_provider();
    assert!((model.estimate_utilization(100_000) - 100_000.0 / 200_000.0).abs() < f64::EPSILON);
}

#[test]
fn uses_the_default_when_the_context_window_is_zero() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().context_window_limit(0).build());
    assert!((model.estimate_utilization(100_000) - 0.5).abs() < f64::EPSILON);
}

#[test]
fn returns_value_above_1_0_when_tokens_exceed_limit() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().context_window_limit(1000).build());
    assert!(model.estimate_utilization(1500) > 1.0);
}

#[test]
fn returns_0_for_zero_tokens() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().context_window_limit(100_000).build());
    assert!(model.estimate_utilization(0).abs() < f64::EPSILON);
}

#[test]
fn a_custom_provider_with_a_listed_id_reports_server_side_summary() {
    let model = test_provider();
    model.update_config(BaseModelConfig::builder().model_id("claude-sonnet-5-5").build());
    assert_eq!(
        model.model_info().capabilities.unwrap().server_side_summary,
        Some(true)
    );
}

#[test]
fn resolve_config_metadata_fills_the_context_window_from_the_shared_info_and_override() {
    let config = BaseModelConfig::default();
    assert_eq!(
        resolve_config_metadata(&config, "claude-sonnet-4-6").context_window_limit,
        Some(1_000_000)
    );
    assert_eq!(resolve_config_metadata(&config, "unknown-model"), config);
    let explicit = BaseModelConfig::builder().context_window_limit(5).build();
    assert_eq!(resolve_config_metadata(&explicit, "claude-sonnet-4-6"), explicit);
    let overridden = config_with_info(context_window(7));
    assert_eq!(
        resolve_config_metadata(&overridden, "claude-sonnet-4-6").context_window_limit,
        Some(7)
    );
}

#[test]
fn resolve_max_tokens_prefers_the_config_then_the_model_output_ceiling() {
    assert_eq!(
        resolve_max_tokens(
            &BaseModelConfig::builder().max_tokens(10).build(),
            "claude-sonnet-4-6"
        ),
        Some(10)
    );
    assert_eq!(
        resolve_max_tokens(&BaseModelConfig::default(), "claude-sonnet-4-6"),
        Some(128_000)
    );
    assert_eq!(
        resolve_max_tokens(&BaseModelConfig::default(), "unknown-model"),
        None
    );
}

#[test]
fn latest_user_index_finds_the_last_user_message() {
    let user = Message::new(Role::User, vec!["a".into()]);
    let assistant = Message::new(Role::Assistant, vec!["b".into()]);
    assert_eq!(latest_user_index(&[]), None);
    assert_eq!(latest_user_index(std::slice::from_ref(&assistant)), None);
    assert_eq!(
        latest_user_index(&[user.clone(), assistant.clone(), user, assistant]),
        Some(2)
    );
}

#[test]
fn model_source_converts_from_models_and_ids() {
    assert!(matches!(ModelSource::from("id"), ModelSource::Id(id) if id == "id"));
    assert!(matches!(
        ModelSource::from(String::from("id")),
        ModelSource::Id(_)
    ));
    let source = ModelSource::from(test_provider());
    let ModelSource::Model(model) = &source else {
        panic!("expected a model");
    };
    assert_eq!(model.model_id().as_deref(), Some("test-model"));
    assert_eq!(format!("{source:?}"), r#"Model(Some("test-model"))"#);
}

/// The cache sections each case of cache-portability.test.ts resolves to: `(tools, messages)`,
/// `None` for a disabled section and `Some(ttl)` for an enabled one.
fn sections(config: &CacheConfig) -> (Option<Option<String>>, Option<Option<String>>) {
    let resolve = |section: Option<&CacheSection>| {
        let resolved = resolve_cache_section(section, &[config.ttl.as_ref()]);
        resolved
            .enabled
            .then(|| resolved.ttl.map(|ttl| ttl.as_str().to_owned()))
    };
    (
        resolve(config.tools_ttl.as_ref()),
        resolve(config.messages_ttl.as_ref()),
    )
}

#[test]
fn agrees_on_the_cache_sections_of_every_portable_config() {
    let one_hour = Some(Some("1h".to_owned()));
    let cases = [
        ("both sections by default", json!({}), (Some(None), Some(None))),
        (
            "a shared ttl on both sections",
            json!({ "ttl": "1h" }),
            (one_hour.clone(), one_hour.clone()),
        ),
        (
            "tool definitions only",
            json!({ "ttl": "1h", "messagesTTL": false }),
            (one_hour.clone(), None),
        ),
        (
            "conversation only",
            json!({ "ttl": "1h", "toolsTTL": false }),
            (None, one_hour.clone()),
        ),
        (
            "a per-section ttl override",
            json!({ "ttl": "1h", "messagesTTL": "5m" }),
            (one_hour, Some(Some("5m".to_owned()))),
        ),
        (
            "an empty ttl treated as unset",
            json!({ "ttl": "" }),
            (Some(None), Some(None)),
        ),
        (
            "every section disabled",
            json!({ "toolsTTL": false, "messagesTTL": false }),
            (None, None),
        ),
    ];
    for (label, config, expected) in cases {
        let config: CacheConfig = serde_json::from_value(config).unwrap();
        assert_eq!(sections(&config), expected, "{label}");
    }
}

#[test]
fn cache_config_round_trips_the_ts_shape() {
    let value = json!({
        "strategy": "anthropic",
        "ttl": "5m",
        "toolsTTL": true,
        "systemPromptTTL": "1h",
        "messagesTTL": false,
        "cacheKey": false,
    });
    let config: CacheConfig = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(config.ttl, Some(CacheTTL::FiveMinutes));
    assert_eq!(
        config.system_prompt_ttl,
        Some(CacheSection::Ttl(CacheTTL::OneHour))
    );
    assert_eq!(serde_json::to_value(&config).unwrap(), value);
    let keyed: CacheConfig = serde_json::from_value(json!({ "cacheKey": "my-key" })).unwrap();
    assert_eq!(
        keyed.cache_key,
        Some(crate::models::model::CacheKey::Key("my-key".into()))
    );
    assert!(serde_json::from_value::<CacheConfig>(json!({ "cacheKey": true })).is_err());
}
