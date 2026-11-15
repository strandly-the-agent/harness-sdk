//! Ported from strands-ts/src/models/openai/__tests__/responses.test.ts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::Error;
use crate::models::model::{CacheConfig, CacheKey, CacheTtl};
use crate::models::openai::client::{ApiRequest, EventStream, KeySource, OpenAIClient};
use crate::models::openai::{OpenAIApi, OpenAIApiKey, OpenAIClientConfig};
use crate::state_store::StateStore;
use crate::types::citations::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock,
};
use crate::types::json::JsonObject;
use crate::types::media::{DocumentBlock, ImageBlock, ImageFormat};
use crate::types::messages::{ContentBlock, ToolResultBlock, ToolResultContent, ToolUseBlock, UnknownBlock};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

fn object(value: JsonValue) -> JsonObject {
    let JsonValue::Object(object) = value else {
        panic!("an object");
    };
    object
}

fn config() -> OpenAIModelConfig {
    OpenAIModelConfig {
        model_id: Some("gpt-5.4".into()),
        ..Default::default()
    }
}

/// The request body of one stream (the TS `runOnce`).
fn run_once(config: OpenAIModelConfig, request: ModelRequest) -> JsonValue {
    let client = MockClient::new(empty_response());
    let model = responses_model(&client, config);
    collect_ok(&model, request);
    client.last_body()
}

fn hi() -> ModelRequest {
    super::request(vec![user("Hi")])
}

fn tool_result(id: &str, status: ToolResultStatus, content: Vec<ToolResultContent>) -> ContentBlock {
    ToolResultBlock::new(id, status, content).into()
}

fn input_of(body: &JsonValue, kind: &str) -> JsonValue {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == kind)
        .cloned()
        .unwrap_or_else(|| panic!("an input item of type {kind}"))
}

mod constructor {
    use super::*;

    fn build(
        options: OpenAIModelOptions,
        env: &[(&str, &str)],
    ) -> (Arc<Recorder>, crate::Result<OpenAIModel>) {
        let recorder = Arc::new(Recorder::default());
        let factory = recorder.factory(MockClient::new(json!([])));
        let model = OpenAIModel::with_seams(options, seams_with_env(Transport::Built(factory), env));
        (recorder, model)
    }

    fn key_of(parts: &crate::models::openai::client::HttpClientParts) -> String {
        match &parts.key {
            KeySource::Static(key) => key.clone(),
            KeySource::Setter(_) => panic!("a static key"),
        }
    }

    #[test]
    fn uses_api_key_from_constructor_parameter() {
        let (recorder, model) = build(
            OpenAIModelOptions {
                api_key: Some("sk-explicit".into()),
                config: config(),
                ..Default::default()
            },
            &[("OPENAI_API_KEY", "sk-test-env")],
        );
        model.unwrap();
        assert_eq!(key_of(&recorder.last()), "sk-explicit");
    }

    #[test]
    fn uses_api_key_from_environment_variable() {
        let (recorder, model) = build(config().into(), &[("OPENAI_API_KEY", "sk-from-env")]);
        model.unwrap();
        assert_eq!(key_of(&recorder.last()), "sk-from-env");
    }

    #[test]
    fn throws_error_when_no_api_key_is_available() {
        let (recorder, model) = build(config().into(), &[("OPENAI_API_KEY", "")]);
        let error = model.unwrap_err();
        assert!(
            error.to_string().starts_with("OpenAI API key is required"),
            "{error}"
        );
        assert!(recorder.built().is_empty());
    }

    #[test]
    fn uses_provided_client_instance_and_skips_open_ai_constructor() {
        let client = MockClient::new(json!([]));
        let model = injected(&client, OpenAIModelOptions::default());
        assert_eq!(model.api(), OpenAIApi::Responses);
    }

    #[test]
    fn does_not_require_api_key_when_client_is_provided() {
        let client = MockClient::new(json!([]));
        let model = OpenAIModel::with_seams(
            OpenAIModelOptions::default(),
            seams_with_env(Transport::Injected(client), &[("OPENAI_API_KEY", "")]),
        );
        assert!(model.is_ok());
    }

    #[test]
    fn reads_the_vendor_environment_defaults() {
        let (recorder, model) = build(
            config().into(),
            &[
                ("OPENAI_API_KEY", " sk-env "),
                ("OPENAI_BASE_URL", "https://proxy.example/v1"),
                ("OPENAI_ORG_ID", "org-1"),
                ("OPENAI_PROJECT_ID", "proj-1"),
                ("OPENAI_CUSTOM_HEADERS", "x-one: 1\nx-two: 2"),
            ],
        );
        model.unwrap();
        let parts = recorder.last();
        assert_eq!(key_of(&parts), "sk-env");
        assert_eq!(parts.base_url, "https://proxy.example/v1");
        assert_eq!(parts.max_retries, 2);
        assert_eq!(parts.timeout, std::time::Duration::from_secs(600));
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .map(|value| value.to_str().unwrap().to_owned())
        };
        assert_eq!(header("openai-organization").as_deref(), Some("org-1"));
        assert_eq!(header("openai-project").as_deref(), Some("proj-1"));
        assert_eq!(header("x-one").as_deref(), Some("1"));
        assert_eq!(header("x-two").as_deref(), Some("2"));
    }

    #[test]
    fn prefers_client_config_over_the_environment() {
        let mut default_headers = http::HeaderMap::new();
        default_headers.insert("x-one", http::HeaderValue::from_static("configured"));
        let (recorder, model) = build(
            OpenAIModelOptions {
                api_key: Some("sk-top".into()),
                client_config: OpenAIClientConfig {
                    base_url: Some("http://localhost:1234/v1".into()),
                    api_key: Some(OpenAIApiKey::new("sk-client")),
                    organization: Some("org-config".into()),
                    default_headers,
                    max_retries: Some(5),
                    ..Default::default()
                },
                config: config(),
                ..Default::default()
            },
            &[
                ("OPENAI_BASE_URL", "https://proxy.example/v1"),
                ("OPENAI_ORG_ID", "org-env"),
                ("OPENAI_CUSTOM_HEADERS", "x-one: env"),
            ],
        );
        model.unwrap();
        let parts = recorder.last();
        assert_eq!(key_of(&parts), "sk-client");
        assert_eq!(parts.base_url, "http://localhost:1234/v1");
        assert_eq!(parts.max_retries, 5);
        assert_eq!(parts.headers.get("openai-organization").unwrap(), "org-config");
        assert_eq!(parts.headers.get("x-one").unwrap(), "configured");
    }

    #[test]
    fn defaults_the_base_url_to_the_openai_api() {
        let (recorder, model) = build(config().into(), &[("OPENAI_API_KEY", "sk")]);
        model.unwrap();
        assert_eq!(recorder.last().base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn redacts_the_api_key_in_debug_output() {
        let options = OpenAIModelOptions {
            api_key: Some("sk-secret-value".into()),
            client_config: OpenAIClientConfig {
                api_key: Some("sk-other-secret".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let printed = format!("{options:?}");
        assert!(!printed.contains("secret"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }

    #[test]
    fn warns_once_when_the_model_id_defaults() {
        let client = MockClient::new(json!([]));
        let (_, logged) = warnings(|| injected(&client, OpenAIModelOptions::default()));
        assert!(
            logged
                .iter()
                .any(|message| message.starts_with("model_id=<gpt-5.4> | using default modelId")),
            "{logged:?}"
        );
    }
}

mod stateful {
    use super::*;

    fn model(stateful: Option<bool>) -> OpenAIModel {
        responses_model(
            &MockClient::new(json!([])),
            OpenAIModelConfig { stateful, ..config() },
        )
    }

    #[test]
    fn defaults_to_false() {
        assert!(!model(None).stateful());
    }

    #[test]
    fn returns_true_when_explicitly_enabled() {
        assert!(model(Some(true)).stateful());
    }

    #[test]
    fn is_declared_on_info() {
        let stateful = |model: OpenAIModel| model.model_info().capabilities.and_then(|c| c.stateful);
        assert_eq!(stateful(model(Some(true))), Some(true));
        assert_eq!(stateful(model(None)), Some(false));
    }

    #[test]
    fn is_construction_only_and_cannot_be_changed_via_update_config() {
        let model = model(Some(false));
        assert!(!model.stateful());
        let ((), logged) = warnings(|| {
            model.update_config(OpenAIModelConfig {
                stateful: Some(true),
                ..Default::default()
            });
        });
        assert!(!model.stateful());
        assert!(
            logged
                .iter()
                .any(|message| message.contains("'stateful' is construction-only")),
            "{logged:?}"
        );
    }

    #[test]
    fn turns_server_side_summary_off() {
        let summary = model(None)
            .model_info()
            .capabilities
            .and_then(|c| c.server_side_summary);
        assert_eq!(summary, Some(false));
    }
}

mod update_config_get_config {
    use super::*;

    #[test]
    fn merges_config_without_clobbering_unspecified_fields() {
        let model = responses_model(
            &MockClient::new(json!([])),
            OpenAIModelConfig {
                model_id: Some("gpt-4o".into()),
                temperature: Some(0.5),
                max_tokens: Some(1024),
                ..Default::default()
            },
        );
        model.update_config(OpenAIModelConfig {
            temperature: Some(0.9),
            ..Default::default()
        });
        let config = model.config();
        assert_eq!(config.model_id.as_deref(), Some("gpt-4o"));
        assert_eq!(config.temperature, Some(0.9));
        assert_eq!(config.max_tokens, Some(1024));
        assert_eq!(Model::get_config(&model).temperature, Some(0.9));
    }

    #[test]
    fn reports_the_default_model_id_and_its_context_window() {
        let model = responses_model(&MockClient::new(json!([])), OpenAIModelConfig::default());
        assert_eq!(model.model_id().as_deref(), Some("gpt-5.4"));
        assert_eq!(model.config().model_id, None);
        assert!(model.config().context_window_limit.is_some());
    }
}

mod managed_params_warning {
    use super::*;

    fn managed(logged: &[String]) -> Vec<&String> {
        logged
            .iter()
            .filter(|message| message.starts_with("params_key="))
            .collect()
    }

    #[test]
    fn warns_on_construction_when_params_contains_provider_managed_keys() {
        let (_, logged) = warnings(|| {
            responses_model(
                &MockClient::new(json!([])),
                OpenAIModelConfig {
                    params: Some(object(json!({ "model": "bad", "store": false }))),
                    ..config()
                },
            )
        });
        let managed = managed(&logged);
        assert_eq!(managed.len(), 2);
        assert!(managed[0].contains("'model'"));
        assert!(managed[1].contains("'store'"));
        assert_eq!(
            managed[0].as_str(),
            "params_key=<model> | 'model' is managed by the provider and will be ignored in params — use the dedicated config property instead"
        );
    }

    #[test]
    fn warns_on_update_config_when_params_contains_provider_managed_keys() {
        let model = responses_model(&MockClient::new(json!([])), config());
        let ((), logged) = warnings(|| {
            model.update_config(OpenAIModelConfig {
                params: Some(object(json!({ "stream": true }))),
                ..Default::default()
            });
        });
        let managed = managed(&logged);
        assert_eq!(managed.len(), 1);
        assert!(managed[0].contains("'stream'"));
    }

    #[test]
    fn does_not_warn_when_params_contains_only_non_managed_keys() {
        let (_, logged) = warnings(|| {
            responses_model(
                &MockClient::new(json!([])),
                OpenAIModelConfig {
                    params: Some(object(json!({ "reasoning": { "summary": "auto" } }))),
                    ..config()
                },
            )
        });
        assert_eq!(logged, Vec::<String>::new());
    }
}

mod cancellation {
    use super::*;

    /// Produces a token per pull until 20, stopping early once the stream is dropped.
    struct Producer {
        produced: Arc<AtomicUsize>,
    }

    impl OpenAIClient for Producer {
        fn stream(&self, _request: ApiRequest) -> EventStream {
            let produced = Arc::clone(&self.produced);
            Box::pin(async_stream::stream! {
                yield Ok(json!({ "type": "response.created", "response": { "id": "response-1" } }));
                while produced.load(Ordering::SeqCst) < 20 {
                    produced.fetch_add(1, Ordering::SeqCst);
                    yield Ok(json!({ "type": "response.output_text.delta", "delta": "token " }));
                    tokio::task::yield_now().await;
                }
                yield Ok(json!({ "type": "response.completed", "response": {} }));
            })
        }
    }

    #[tokio::test]
    async fn stops_an_in_flight_responses_producer_when_the_signal_aborts() {
        let produced = Arc::new(AtomicUsize::new(0));
        let client = Arc::new(Producer {
            produced: Arc::clone(&produced),
        });
        let model =
            OpenAIModel::with_seams(OpenAIModelOptions::default(), seams(Transport::Injected(client)))
                .unwrap();
        let (signal, handle) = CancelSignal::new();
        let mut stream = model.stream(ModelRequest {
            cancel_signal: Some(signal),
            ..hi()
        });
        let mut tokens_at_cancel = 0;
        let mut failure = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) if event.text_delta().is_some() && tokens_at_cancel == 0 => {
                    tokens_at_cancel = produced.load(Ordering::SeqCst);
                    handle.cancel(CancelOptions::default());
                }
                Ok(_) => {}
                Err(error) => failure = Some(error),
            }
        }
        let error = failure.expect("the stream fails");
        assert!(matches!(error, Error::Model(_)), "{error:?}");
        assert_eq!(error.to_string(), "Request was aborted.");
        assert_eq!(tokens_at_cancel, 1);
        assert_eq!(produced.load(Ordering::SeqCst), tokens_at_cancel);
    }
}

mod request_formatting {
    use super::*;

    fn with_specs(choice: Option<ToolChoice>) -> ModelRequest {
        ModelRequest {
            tool_specs: vec![ToolSpec::new("calc", "d").with_input_schema(json!({}))],
            tool_choice: choice,
            ..hi()
        }
    }

    #[test]
    fn includes_model_input_stream_and_store_false_by_default() {
        let body = run_once(OpenAIModelConfig::default(), hi());
        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert!(body["input"].is_array());
        assert_eq!(
            body,
            json!({
                "model": "gpt-5.4",
                "input": [{ "role": "user", "content": [{ "type": "input_text", "text": "Hi" }] }],
                "stream": true,
                "store": false,
            })
        );
    }

    #[test]
    fn sets_store_true_when_stateful_is_enabled() {
        let body = run_once(
            OpenAIModelConfig {
                stateful: Some(true),
                ..config()
            },
            hi(),
        );
        assert_eq!(body["store"], true);
    }

    fn with_response_id() -> ModelRequest {
        ModelRequest {
            model_state: StateStore::from_map(object(json!({ "responseId": "resp_prev" }))),
            ..hi()
        }
    }

    #[test]
    fn chains_previous_response_id_when_stateful_and_model_state_has_response_id() {
        let body = run_once(
            OpenAIModelConfig {
                stateful: Some(true),
                ..config()
            },
            with_response_id(),
        );
        assert_eq!(body["previous_response_id"], "resp_prev");
    }

    #[test]
    fn omits_previous_response_id_when_stateful_is_disabled_even_with_response_id_in_model_state() {
        let body = run_once(config(), with_response_id());
        assert!(body.get("previous_response_id").is_none());
    }

    #[test]
    fn maps_system_prompt_string_to_instructions() {
        let body = run_once(
            config(),
            ModelRequest {
                system_prompt: Some("Be helpful.".into()),
                ..hi()
            },
        );
        assert_eq!(body["instructions"], "Be helpful.");
    }

    #[test]
    fn joins_system_prompt_text_blocks_into_instructions() {
        use crate::types::messages::{CachePointBlock, SystemContentBlock, SystemPrompt};
        let body = run_once(
            config(),
            ModelRequest {
                system_prompt: Some(SystemPrompt::Blocks(vec![
                    SystemContentBlock::text("A. "),
                    CachePointBlock::new().into(),
                    SystemContentBlock::text("B."),
                ])),
                ..hi()
            },
        );
        assert_eq!(body["instructions"], "A. B.");
    }

    #[test]
    fn merges_tool_specs_with_built_in_tools_from_params() {
        let body = run_once(
            OpenAIModelConfig {
                params: Some(object(json!({ "tools": [{ "type": "web_search" }] }))),
                ..config()
            },
            ModelRequest {
                tool_specs: vec![
                    ToolSpec::new("calc", "calculator")
                        .with_input_schema(json!({ "type": "object", "properties": {} })),
                ],
                ..hi()
            },
        );
        assert_eq!(
            body["tools"],
            json!([
                { "type": "web_search" },
                {
                    "type": "function",
                    "name": "calc",
                    "description": "calculator",
                    "parameters": { "type": "object", "properties": {} },
                    "strict": null,
                },
            ])
        );
    }

    #[test]
    fn maps_tool_choice_variants() {
        assert_eq!(
            run_once(config(), with_specs(Some(ToolChoice::Auto)))["tool_choice"],
            "auto"
        );
        assert_eq!(
            run_once(config(), with_specs(Some(ToolChoice::Any)))["tool_choice"],
            "required"
        );
        assert_eq!(
            run_once(config(), with_specs(Some(ToolChoice::tool("calc"))))["tool_choice"],
            json!({ "type": "function", "name": "calc" })
        );
    }

    #[test]
    fn formats_temperature_max_tokens_max_output_tokens_and_top_p() {
        let body = run_once(
            OpenAIModelConfig {
                temperature: Some(0.3),
                max_tokens: Some(512),
                top_p: Some(0.8),
                ..config()
            },
            hi(),
        );
        assert_eq!(body["temperature"], 0.3);
        assert_eq!(body["max_output_tokens"], 512);
        assert_eq!(body["top_p"], 0.8);
    }

    fn cached(cache_config: CacheConfig) -> OpenAIModelConfig {
        OpenAIModelConfig {
            cache_config: Some(cache_config),
            ..config()
        }
    }

    fn keyed(key: CacheKey) -> CacheConfig {
        CacheConfig {
            cache_key: Some(key),
            ..Default::default()
        }
    }

    fn in_session(session_id: Option<&str>) -> ModelRequest {
        ModelRequest {
            agent_metadata: Some(AgentMetadata {
                session_id: session_id.map(str::to_owned),
            }),
            ..hi()
        }
    }

    #[test]
    fn maps_cache_config_cache_key_to_prompt_cache_key() {
        let body = run_once(cached(keyed(CacheKey::Key("tenant-42".into()))), hi());
        assert_eq!(body["prompt_cache_key"], "tenant-42");
    }

    #[test]
    fn omits_prompt_cache_key_when_cache_config_is_unset() {
        assert!(
            run_once(config(), in_session(Some("s1")))
                .get("prompt_cache_key")
                .is_none()
        );
    }

    #[test]
    fn lets_an_explicit_prompt_cache_key_in_params_win_over_cache_config() {
        let body = run_once(
            OpenAIModelConfig {
                params: Some(object(json!({ "prompt_cache_key": "explicit" }))),
                ..cached(keyed(CacheKey::Key("from-config".into())))
            },
            hi(),
        );
        assert_eq!(body["prompt_cache_key"], "explicit");
    }

    #[test]
    fn derives_prompt_cache_key_from_the_agent_metadata_session_when_cache_key_is_unset() {
        let body = run_once(cached(CacheConfig::default()), in_session(Some("s1")));
        assert_eq!(body["prompt_cache_key"], "strands-s1");
    }

    #[test]
    fn lets_a_configured_cache_key_win_over_the_agent_metadata_session() {
        let body = run_once(
            cached(keyed(CacheKey::Key("tenant-42".into()))),
            in_session(Some("s1")),
        );
        assert_eq!(body["prompt_cache_key"], "tenant-42");
    }

    #[test]
    fn treats_a_false_cache_key_as_an_opt_out_even_with_an_agent_metadata_session() {
        let body = run_once(cached(keyed(CacheKey::Disabled)), in_session(Some("s1")));
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn omits_prompt_cache_key_when_the_agent_metadata_carries_no_session() {
        let body = run_once(cached(CacheConfig::default()), in_session(None));
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn maps_retention_literal_cache_config_ttl_s_to_prompt_cache_retention() {
        for ttl in ["24h", "in_memory"] {
            let body = run_once(
                cached(CacheConfig {
                    ttl: Some(CacheTtl::from(ttl)),
                    ..Default::default()
                }),
                hi(),
            );
            assert_eq!(body["prompt_cache_retention"], ttl);
        }
    }

    #[test]
    fn ignores_a_non_retention_cache_config_ttl_and_warns() {
        let (body, logged) = warnings(|| {
            run_once(
                cached(CacheConfig {
                    ttl: Some(CacheTtl::FiveMinutes),
                    ..Default::default()
                }),
                hi(),
            )
        });
        assert!(body.get("prompt_cache_retention").is_none());
        assert!(
            logged.contains(
                &"ttl=<5m> | cacheConfig.ttl is not an openai retention value, ignoring".to_owned()
            ),
            "{logged:?}"
        );
    }

    #[test]
    fn lets_an_explicit_prompt_cache_retention_in_params_win_over_cache_config() {
        let body = run_once(
            OpenAIModelConfig {
                params: Some(object(json!({ "prompt_cache_retention": "24h" }))),
                ..cached(CacheConfig {
                    ttl: Some(CacheTtl::from("in_memory")),
                    ..Default::default()
                })
            },
            hi(),
        );
        assert_eq!(body["prompt_cache_retention"], "24h");
    }

    #[test]
    fn warns_once_that_placement_fields_have_no_effect_on_models_without_cache_points() {
        let (_, logged) = warnings(|| {
            run_once(
                cached(CacheConfig {
                    strategy: Some(crate::models::model::CacheStrategy::Anthropic),
                    ..Default::default()
                }),
                hi(),
            )
        });
        assert!(logged.contains(
            &"openai caches prefixes automatically server-side | strategy, toolsTTL, systemPromptTTL and messagesTTL have no effect".to_owned()
        ));
    }

    #[test]
    fn passes_through_extra_params_fields_to_the_request() {
        let body = run_once(
            OpenAIModelConfig {
                params: Some(object(json!({ "reasoning": { "summary": "auto" } }))),
                ..config()
            },
            hi(),
        );
        assert_eq!(body["reasoning"], json!({ "summary": "auto" }));
    }

    #[test]
    fn provider_managed_fields_in_params_are_overridden_and_cannot_take_effect() {
        let body = run_once(
            OpenAIModelConfig {
                model_id: Some("gpt-4o".into()),
                stateful: Some(true),
                params: Some(object(
                    json!({ "model": "attacker-model", "input": "hijacked", "stream": false, "store": false }),
                )),
                ..Default::default()
            },
            hi(),
        );
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], true);
        assert!(body["input"].is_array());
    }

    fn run_it() -> Message {
        user("run it")
    }

    #[test]
    fn emits_tool_use_and_tool_result_as_separate_top_level_items() {
        let messages = vec![
            run_it(),
            Message::new(
                Role::Assistant,
                vec![ToolUseBlock::new("calc", "call_1", json!({ "expr": "2+2" })).into()],
            ),
            Message::new(
                Role::User,
                vec![tool_result("call_1", ToolResultStatus::Success, vec!["4".into()])],
            ),
        ];
        let body = run_once(config(), super::super::request(messages));
        assert_eq!(
            input_of(&body, "function_call"),
            json!({ "type": "function_call", "call_id": "call_1", "name": "calc", "arguments": "{\"expr\":\"2+2\"}" })
        );
        assert_eq!(
            input_of(&body, "function_call_output"),
            json!({ "type": "function_call_output", "call_id": "call_1", "output": "4" })
        );
    }

    fn tool_output(result: ContentBlock) -> JsonValue {
        let messages = vec![user("x"), Message::new(Role::User, vec![result])];
        input_of(
            &run_once(config(), super::super::request(messages)),
            "function_call_output",
        )["output"]
            .clone()
    }

    #[test]
    fn prefixes_errored_tool_results_with_error() {
        let output = tool_output(tool_result("t1", ToolResultStatus::Error, vec!["boom".into()]));
        assert_eq!(output, "[ERROR] boom");
    }

    #[test]
    fn emits_an_array_output_with_input_image_when_a_tool_result_carries_image_bytes() {
        let output = tool_output(tool_result(
            "img_tool",
            ToolResultStatus::Success,
            vec![
                "here is the image".into(),
                ImageBlock::new(ImageFormat::Png, vec![1u8, 2, 3, 4]).into(),
            ],
        ));
        assert_eq!(
            output,
            json!([
                { "type": "input_text", "text": "here is the image" },
                { "type": "input_image", "image_url": "data:image/png;base64,AQIDBA==" },
            ])
        );
    }

    #[test]
    fn emits_an_array_output_with_input_file_when_a_tool_result_carries_a_document() {
        let output = tool_output(tool_result(
            "doc_tool",
            ToolResultStatus::Success,
            vec![DocumentBlock::new("report.pdf", "pdf", vec![5u8, 6, 7, 8]).into()],
        ));
        assert_eq!(
            output,
            json!([{ "type": "input_file", "file_data": "data:application/pdf;base64,BQYHCA==", "filename": "report.pdf" }])
        );
    }

    #[test]
    fn keeps_tool_result_output_as_a_plain_string_when_only_text_is_present() {
        let output = tool_output(tool_result(
            "text_tool",
            ToolResultStatus::Success,
            vec!["pong".into()],
        ));
        assert_eq!(output, "pong");
    }

    #[test]
    fn serializes_assistant_history_as_a_string_easy_input_message() {
        let messages = vec![user("What is 2+2?"), assistant("4"), user("What about 3+3?")];
        let body = run_once(config(), super::super::request(messages));
        assert_eq!(body["input"][1], json!({ "role": "assistant", "content": "4" }));
        assert_eq!(
            body["input"][0],
            json!({ "role": "user", "content": [{ "type": "input_text", "text": "What is 2+2?" }] })
        );
        assert_eq!(
            body["input"][2],
            json!({ "role": "user", "content": [{ "type": "input_text", "text": "What about 3+3?" }] })
        );
    }

    #[test]
    fn joins_multiple_assistant_text_blocks_with_newlines() {
        let messages = vec![
            user("list two words"),
            Message::new(
                Role::Assistant,
                vec![TextBlock::new("First.").into(), TextBlock::new("Second.").into()],
            ),
            user("thanks"),
        ];
        let body = run_once(config(), super::super::request(messages));
        assert_eq!(
            body["input"][1],
            json!({ "role": "assistant", "content": "First.\nSecond." })
        );
    }

    fn image() -> ContentBlock {
        ImageBlock::new(ImageFormat::Png, vec![1u8, 2, 3]).into()
    }

    #[test]
    fn drops_non_text_assistant_history_content_with_a_warning() {
        let messages = vec![
            user("draw something"),
            Message::new(
                Role::Assistant,
                vec![TextBlock::new("Here it is.").into(), image()],
            ),
            user("nice"),
        ];
        let (body, logged) = warnings(|| run_once(config(), super::super::request(messages)));
        assert_eq!(
            body["input"][1],
            json!({ "role": "assistant", "content": "Here it is." })
        );
        assert!(
            logged
                .iter()
                .any(|message| message.contains("no valid responses api input shape"))
        );
        assert!(
            logged
                .iter()
                .any(|message| message.contains("content_type=<input_image>"))
        );
    }

    #[test]
    fn omits_an_assistant_history_turn_whose_content_is_entirely_non_text() {
        let messages = vec![
            user("draw something"),
            Message::new(Role::Assistant, vec![image()]),
            user("nice"),
        ];
        let (body, logged) = warnings(|| run_once(config(), super::super::request(messages)));
        let assistant_items = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["role"] == "assistant")
            .count();
        assert_eq!(assistant_items, 0);
        assert!(
            logged
                .iter()
                .any(|message| message.contains("content_type=<input_image>"))
        );
    }

    #[test]
    fn serializes_user_role_citations_content_as_input_text() {
        let citation = Citation::new(
            CitationLocation::DocumentChar {
                document_index: 0,
                start: 0,
                end: 10,
            },
            "doc-0",
            vec![CitationSourceContent::new("source")],
            "Test",
        );
        let block = CitationsBlock::new(
            vec![citation],
            vec![CitationGeneratedContent::new("quoted passage")],
        );
        let body = run_once(
            config(),
            super::super::request(vec![Message::new(Role::User, vec![block.into()])]),
        );
        assert_eq!(
            body["input"][0],
            json!({ "role": "user", "content": [{ "type": "input_text", "text": "quoted passage" }] })
        );
    }

    #[test]
    fn skips_unknown_blocks_in_messages_and_tool_results_without_a_warning() {
        let unknown = UnknownBlock::new(object(json!({ "futureBlock": { "value": 1 } })));
        let messages = vec![
            Message::new(
                Role::User,
                vec![TextBlock::new("hello").into(), unknown.clone().into()],
            ),
            Message::new(
                Role::Assistant,
                vec![ToolUseBlock::new("tool", "tu1", json!({})).into()],
            ),
            Message::new(
                Role::User,
                vec![tool_result(
                    "tu1",
                    ToolResultStatus::Success,
                    vec!["done".into(), ToolResultContent::Unknown(unknown)],
                )],
            ),
        ];
        let (body, logged) = warnings(|| run_once(config(), super::super::request(messages)));
        assert_eq!(
            body["input"][0],
            json!({ "role": "user", "content": [{ "type": "input_text", "text": "hello" }] })
        );
        let last = body["input"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["type"], "function_call_output");
        assert_eq!(last["call_id"], "tu1");
        assert!(!body.to_string().contains("futureBlock"));
        assert_eq!(logged, Vec::<String>::new());
    }

    #[test]
    fn rejects_a_request_without_messages() {
        let model = responses_model(&MockClient::new(empty_response()), config());
        let error = collect_err(&model, ModelRequest::default());
        assert_eq!(error.to_string(), "At least one message is required");
    }
}

mod stream_event_mapping {
    use super::*;

    fn events(events: JsonValue) -> Vec<JsonValue> {
        let model = responses_model(&MockClient::new(events), config());
        collect_ok(&model, super::super::request(vec![user("x")]))
    }

    fn types(events: &[JsonValue]) -> Vec<String> {
        events
            .iter()
            .map(|event| event["type"].as_str().unwrap().to_owned())
            .collect()
    }

    fn delta_types(events: &[JsonValue]) -> Vec<String> {
        events
            .iter()
            .filter(|event| event["type"] == "modelContentBlockDeltaEvent")
            .map(|event| event["delta"]["type"].as_str().unwrap().to_owned())
            .collect()
    }

    fn find<'a>(events: &'a [JsonValue], kind: &str) -> &'a JsonValue {
        events
            .iter()
            .find(|event| event["type"] == kind)
            .expect("the event")
    }

    fn citation(url: &str, title: &str, cited: &str) -> JsonValue {
        json!({
            "type": "response.output_text.annotation.added",
            "annotation": { "type": "url_citation", "url": url, "title": title, "cited_text": cited },
        })
    }

    fn text(delta: &str) -> JsonValue {
        json!({ "type": "response.output_text.delta", "delta": delta })
    }

    fn created() -> JsonValue {
        json!({ "type": "response.created", "response": { "id": "r" } })
    }

    fn completed() -> JsonValue {
        json!({ "type": "response.completed", "response": {} })
    }

    #[test]
    fn resolves_stateful_once_per_stream_not_once_per_event() {
        let mut script = vec![json!({ "type": "response.created", "response": { "id": "resp_abc" } })];
        script.extend((0..50).map(|_| text("x")));
        script.push(completed());
        let model = responses_model(
            &MockClient::new(JsonValue::Array(script)),
            OpenAIModelConfig {
                stateful: Some(true),
                ..config()
            },
        );
        let state = StateStore::new();
        collect_ok(
            &model,
            ModelRequest {
                model_state: state.clone(),
                ..super::super::request(vec![user("hi")])
            },
        );
        // The adapter takes the flag at construction; each event reads no config.
        assert_eq!(state.get_value("responseId"), Some(json!("resp_abc")));
    }

    fn captured(stateful: Option<bool>) -> Option<JsonValue> {
        let state = StateStore::new();
        let model = responses_model(
            &MockClient::new(
                json!([{ "type": "response.created", "response": { "id": "resp_abc" } }, completed()]),
            ),
            OpenAIModelConfig { stateful, ..config() },
        );
        collect_ok(
            &model,
            ModelRequest {
                model_state: state.clone(),
                ..super::super::request(vec![user("hi")])
            },
        );
        state.get_value("responseId")
    }

    #[test]
    fn captures_response_id_on_response_created_when_stateful() {
        assert_eq!(captured(Some(true)), Some(json!("resp_abc")));
    }

    #[test]
    fn does_not_capture_response_id_when_stateful_is_disabled() {
        assert_eq!(captured(None), None);
    }

    #[test]
    fn emits_text_deltas_inside_a_content_block() {
        let events = events(json!([created(), text("Hello"), text(" world"), completed()]));
        assert_eq!(
            types(&events),
            [
                "modelMessageStartEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelMessageStopEvent",
            ]
        );
        let deltas: Vec<&JsonValue> = events
            .iter()
            .filter(|event| event["type"] == "modelContentBlockDeltaEvent")
            .map(|event| &event["delta"])
            .collect();
        assert_eq!(
            deltas,
            [
                &json!({ "type": "textDelta", "text": "Hello" }),
                &json!({ "type": "textDelta", "text": " world" })
            ]
        );
    }

    #[test]
    fn switches_content_blocks_between_reasoning_and_text() {
        let events = events(json!([
            created(),
            { "type": "response.reasoning_text.delta", "delta": "thinking..." },
            text("answer"),
            completed(),
        ]));
        assert_eq!(
            types(&events),
            [
                "modelMessageStartEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelMessageStopEvent",
            ]
        );
        assert_eq!(delta_types(&events), ["reasoningContentDelta", "textDelta"]);
    }

    #[test]
    fn emits_tool_call_triplet_after_stream_close_and_sets_stop_reason_tool_use() {
        let events = events(json!([
            created(),
            { "type": "response.output_item.added", "item": { "type": "function_call", "id": "item_1", "call_id": "call_1", "name": "calc" } },
            { "type": "response.function_call_arguments.delta", "item_id": "item_1", "delta": "{\"a\":" },
            { "type": "response.function_call_arguments.delta", "item_id": "item_1", "delta": "1}" },
            { "type": "response.function_call_arguments.done", "item_id": "item_1", "arguments": "{\"a\":1}" },
            completed(),
        ]));
        let start = events
            .iter()
            .find(|event| {
                event["type"] == "modelContentBlockStartEvent" && event["start"]["type"] == "toolUseStart"
            })
            .unwrap();
        assert_eq!(
            start["start"],
            json!({ "type": "toolUseStart", "name": "calc", "toolUseId": "call_1" })
        );
        let delta = events
            .iter()
            .find(|event| event["delta"]["type"] == "toolUseInputDelta")
            .unwrap();
        assert_eq!(
            delta["delta"],
            json!({ "type": "toolUseInputDelta", "input": "{\"a\":1}" })
        );
        assert_eq!(find(&events, "modelMessageStopEvent")["stopReason"], "toolUse");
    }

    #[test]
    fn skips_a_function_call_without_an_item_id() {
        let (events, logged) = warnings(|| {
            events(json!([
                created(),
                { "type": "response.output_item.added", "item": { "type": "function_call", "call_id": "call_1", "name": "calc" } },
                completed(),
            ]))
        });
        assert_eq!(
            types(&events),
            ["modelMessageStartEvent", "modelMessageStopEvent"]
        );
        assert!(logged.contains(
            &"call_id=<call_1> name=<calc> | function_call event missing item id — skipping".to_owned()
        ));
    }

    #[test]
    fn maps_response_incomplete_with_max_output_tokens_to_stop_reason_max_tokens() {
        let events = events(json!([
            created(),
            text("partial"),
            {
                "type": "response.incomplete",
                "response": {
                    "incomplete_details": { "reason": "max_output_tokens" },
                    "usage": { "input_tokens": 10, "output_tokens": 5, "total_tokens": 15 },
                },
            },
        ]));
        assert_eq!(find(&events, "modelMessageStopEvent")["stopReason"], "maxTokens");
        assert_eq!(
            find(&events, "modelMetadataEvent")["usage"],
            json!({ "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 })
        );
    }

    #[test]
    fn maps_response_incomplete_with_content_filter_to_stop_reason_content_filtered() {
        let events = events(json!([
            created(),
            text("partial"),
            { "type": "response.incomplete", "response": { "incomplete_details": { "reason": "content_filter" } } },
        ]));
        assert_eq!(
            events,
            [
                json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
                json!({ "type": "modelContentBlockStartEvent" }),
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "partial" } }),
                json!({ "type": "modelContentBlockStopEvent" }),
                json!({ "type": "modelMessageStopEvent", "stopReason": "contentFiltered" }),
            ]
        );
    }

    mod when_a_function_call_is_in_flight_at_response_incomplete {
        use super::*;

        fn truncated_call(reason: &str) -> OpenAIModel {
            responses_model(
                &MockClient::new(json!([
                    created(),
                    { "type": "response.output_item.added", "item": { "type": "function_call", "id": "item_1", "call_id": "call_1", "name": "write_file" } },
                    { "type": "response.function_call_arguments.delta", "item_id": "item_1", "delta": "{\"path\": \"notes.md\", \"content\": \"Lorem" },
                    { "type": "response.incomplete", "response": { "incomplete_details": { "reason": reason } } },
                ])),
                config(),
            )
        }

        fn tool_call_events(stop_reason: &str) -> Vec<JsonValue> {
            vec![
                json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
                json!({ "type": "modelContentBlockStartEvent", "start": { "type": "toolUseStart", "name": "write_file", "toolUseId": "call_1" } }),
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": "{\"path\": \"notes.md\", \"content\": \"Lorem" } }),
                json!({ "type": "modelContentBlockStopEvent" }),
                json!({ "type": "modelMessageStopEvent", "stopReason": stop_reason }),
            ]
        }

        #[test]
        fn reports_max_tokens_instead_of_tool_use_for_max_output_tokens() {
            let events = collect_ok(
                &truncated_call("max_output_tokens"),
                super::super::super::request(vec![user("x")]),
            );
            assert_eq!(events, tool_call_events("maxTokens"));
        }

        #[test]
        fn throws_max_tokens_error_from_stream_aggregated_for_max_output_tokens() {
            let error = aggregate(
                &truncated_call("max_output_tokens"),
                super::super::super::request(vec![user("x")]),
            )
            .unwrap_err();
            assert!(matches!(error, Error::MaxTokens(_)), "{error:?}");
        }

        #[test]
        fn keeps_tool_use_for_content_filter() {
            let events = collect_ok(
                &truncated_call("content_filter"),
                super::super::super::request(vec![user("x")]),
            );
            assert_eq!(events, tool_call_events("toolUse"));
        }
    }

    fn usage_of(details: &JsonValue) -> JsonValue {
        let events = events(json!([
            created(),
            {
                "type": "response.completed",
                "response": { "usage": { "input_tokens": 1553, "output_tokens": 42, "total_tokens": 1595, "input_tokens_details": details } },
            },
        ]));
        find(&events, "modelMetadataEvent")["usage"].clone()
    }

    #[test]
    fn plumbs_prompt_cache_reads_input_tokens_details_cached_tokens_into_cache_read_input_tokens() {
        assert_eq!(
            usage_of(&json!({ "cached_tokens": 873 })),
            json!({ "inputTokens": 1553, "outputTokens": 42, "totalTokens": 1595, "cacheReadInputTokens": 873 })
        );
    }

    #[test]
    fn omits_cache_read_input_tokens_when_there_is_no_cache_hit_cached_tokens_0_or_absent() {
        let expected = json!({ "inputTokens": 1553, "outputTokens": 42, "totalTokens": 1595 });
        assert_eq!(usage_of(&json!({ "cached_tokens": 0 })), expected);
        assert_eq!(usage_of(&json!({})), expected);
    }

    #[test]
    fn plumbs_prompt_cache_writes_input_tokens_details_cache_write_tokens_into_cache_write_input_tokens() {
        assert_eq!(
            usage_of(&json!({ "cached_tokens": 873, "cache_write_tokens": 40 })),
            json!({
                "inputTokens": 1553,
                "outputTokens": 42,
                "totalTokens": 1595,
                "cacheReadInputTokens": 873,
                "cacheWriteInputTokens": 40,
            })
        );
    }

    #[test]
    fn omits_cache_write_input_tokens_when_cache_write_tokens_is_0_or_absent() {
        assert_eq!(
            usage_of(&json!({ "cached_tokens": 873, "cache_write_tokens": 0 })),
            json!({ "inputTokens": 1553, "outputTokens": 42, "totalTokens": 1595, "cacheReadInputTokens": 873 })
        );
    }

    #[test]
    fn emits_url_citation_delta_from_response_output_text_annotation_added() {
        let events = events(json!([
            created(),
            text("The answer is here."),
            citation("https://example.com", "Example", "here"),
            completed(),
        ]));
        let delta = events
            .iter()
            .find(|event| event["delta"]["type"] == "citationsDelta")
            .unwrap();
        assert_eq!(
            delta["delta"],
            json!({
                "type": "citationsDelta",
                "citations": [{
                    "location": { "type": "web", "url": "https://example.com" },
                    "source": "https://example.com",
                    "sourceContent": [],
                    "title": "Example",
                }],
                "content": [{ "text": "here" }],
            })
        );
    }

    #[test]
    fn warns_on_an_unsupported_annotation_type() {
        let (events, logged) = warnings(|| {
            events(json!([
                created(),
                { "type": "response.output_text.annotation.added", "annotation": { "type": "file_citation" } },
                completed(),
            ]))
        });
        assert_eq!(
            types(&events),
            ["modelMessageStartEvent", "modelMessageStopEvent"]
        );
        assert!(logged.contains(
            &"annotation_type=<file_citation> | unsupported annotation type in responses api".to_owned()
        ));
    }

    #[test]
    fn closes_the_text_block_before_a_citation_producing_separate_blocks_when_stream_ends_after_citation() {
        let events = events(json!([
            created(),
            text("Before citation"),
            citation("https://example.com", "Source", "cited"),
            completed(),
        ]));
        assert_eq!(
            types(&events),
            [
                "modelMessageStartEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelMessageStopEvent",
            ]
        );
        assert_eq!(delta_types(&events), ["textDelta", "citationsDelta"]);
    }

    #[test]
    fn closes_the_text_block_before_a_citation_and_opens_a_new_text_block_after() {
        let events = events(json!([
            created(),
            text("Before "),
            citation("https://example.com", "Source", "cited"),
            text(" after"),
            completed(),
        ]));
        assert_eq!(
            types(&events),
            [
                "modelMessageStartEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelMessageStopEvent",
            ]
        );
        assert_eq!(delta_types(&events), ["textDelta", "citationsDelta", "textDelta"]);
    }

    #[test]
    fn keeps_consecutive_citations_in_the_same_block_without_extra_stop_start() {
        let events = events(json!([
            created(),
            citation("https://a.com", "A", "a"),
            citation("https://b.com", "B", "b"),
            completed(),
        ]));
        assert_eq!(
            types(&events),
            [
                "modelMessageStartEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelMessageStopEvent",
            ]
        );
    }

    #[test]
    fn handles_text_citation_text_citation_text_with_separate_blocks_each_time() {
        let events = events(json!([
            created(),
            text("intro "),
            citation("https://1.com", "1", "c1"),
            text("middle "),
            citation("https://2.com", "2", "c2"),
            text("end"),
            completed(),
        ]));
        assert_eq!(
            delta_types(&events),
            [
                "textDelta",
                "citationsDelta",
                "textDelta",
                "citationsDelta",
                "textDelta"
            ]
        );
        let count = |kind: &str| events.iter().filter(|event| event["type"] == kind).count();
        assert_eq!(count("modelContentBlockStartEvent"), 5);
        assert_eq!(count("modelContentBlockStopEvent"), 5);
    }

    #[test]
    fn aggregates_text_and_citations_into_separate_blocks() {
        let model = responses_model(
            &MockClient::new(json!([
                created(),
                text("Before "),
                citation("https://example.com", "Source", "cited"),
                completed()
            ])),
            config(),
        );
        let (blocks, _) = aggregate(&model, super::super::request(vec![user("x")])).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], json!({ "text": "Before " }));
        assert!(blocks[1].get("citations").is_some(), "{}", blocks[1]);
    }
}

mod error_mapping {
    use super::*;
    use crate::models::openai::client::ApiError;

    fn failure(events: Vec<Result<JsonValue, ApiError>>) -> Error {
        let model = responses_model(&MockClient::scripted(move |_| events.clone()), config());
        collect_err(&model, super::super::request(vec![user("x")]))
    }

    fn source_code(error: &Error) -> Option<String> {
        let mut source = std::error::Error::source(error);
        while let Some(current) = source {
            if let Some(api) = current.downcast_ref::<ApiError>() {
                return api.code.clone();
            }
            source = current.source();
        }
        None
    }

    #[test]
    fn throws_response_failed_instead_of_finalizing_an_empty_successful_response() {
        let error = failure(events_of(json!([
            { "type": "response.created", "response": { "id": "r" } },
            { "type": "response.failed", "response": { "error": { "code": "server_error", "message": "The model failed while processing the request." } } },
        ])));
        assert!(matches!(error, Error::Model(_)), "{error:?}");
        assert_eq!(
            error.to_string(),
            "The model failed while processing the request."
        );
        assert_eq!(source_code(&error).as_deref(), Some("server_error"));
    }

    #[test]
    fn wraps_a_response_failed_context_limit_as_context_window_overflow_error() {
        let error = failure(events_of(json!([
            { "type": "response.failed", "response": { "error": { "code": "invalid_prompt", "message": "prompt tokens (320666) exceed customer model maximum (278528) for model-id" } } },
        ])));
        assert!(matches!(error, Error::ContextWindowOverflow(_)), "{error:?}");
    }

    #[test]
    fn throws_the_fallback_message_when_response_failed_has_no_error_details() {
        let error = failure(events_of(
            json!([{ "type": "response.failed", "response": { "error": null } }]),
        ));
        assert_eq!(error.to_string(), "OpenAI Responses API response failed");
    }

    #[test]
    fn maps_a_responses_error_event_through_the_existing_error_classifier() {
        let error = failure(events_of(json!([
            { "type": "error", "code": "rate_limit_exceeded", "message": "Rate limit exceeded while streaming." },
        ])));
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
        assert_eq!(error.to_string(), "Rate limit exceeded while streaming.");
    }

    #[test]
    fn wraps_429_as_model_throttled_error() {
        let error = failure(vec![Err(api_status_error(429, "Too many requests"))]);
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
    }

    #[test]
    fn wraps_context_length_exceeded_as_context_window_overflow_error() {
        let error = failure(vec![Err(api_code_error(
            "context_length_exceeded",
            "This model has a maximum context length of 8k.",
        ))]);
        assert!(matches!(error, Error::ContextWindowOverflow(_)), "{error:?}");
    }

    #[test]
    fn rethrows_unknown_errors_untouched() {
        let error = failure(vec![Err(api_status_error(400, "some other failure"))]);
        assert!(matches!(error, Error::Model(_)), "{error:?}");
        assert_eq!(error.to_string(), "some other failure");
    }

    #[test]
    fn maps_transport_failures_to_model_network_error() {
        use crate::models::openai::client::ApiErrorKind;
        let error = failure(vec![Err(ApiError::new(
            ApiErrorKind::Connection,
            "Connection error.",
        ))]);
        assert!(matches!(error, Error::ModelNetwork(_)), "{error:?}");
        let error = failure(vec![Err(ApiError::new(
            ApiErrorKind::Timeout,
            "Request timed out.",
        ))]);
        assert!(matches!(error, Error::ModelNetwork(_)), "{error:?}");
    }

    #[test]
    fn does_not_map_5xx_or_408_to_throttled() {
        for status in [408, 500, 503] {
            let error = failure(vec![Err(api_status_error(
                status,
                &format!("{status} upstream failure"),
            ))]);
            assert!(matches!(error, Error::Model(_)), "{status}: {error:?}");
        }
    }
}
