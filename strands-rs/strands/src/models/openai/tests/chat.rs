//! Ported from strands-ts/src/models/openai/__tests__/chat.test.ts.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::models::model::{CacheConfig, CacheKey, CacheSection, CacheStrategy, CacheTtl};
use crate::models::openai::client::{ApiRequest, EventStream, KeySource, OpenAIClient};
use crate::models::openai::{OpenAIApi, OpenAIApiKey, OpenAIClientConfig};
use crate::types::json::JsonObject;
use crate::types::media::{DocumentBlock, ImageBlock, ImageFormat, ImageSource, VideoBlock};
use crate::types::messages::{
    CachePointBlock, ContentBlock, GuardContentImage, GuardContentText, GuardImageFormat, GuardImageSource,
    GuardQualifier, ReasoningBlock, SystemContentBlock, SystemPrompt, ToolResultBlock, ToolResultContent,
    ToolUseBlock, UnknownBlock,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

fn object(value: JsonValue) -> JsonObject {
    let JsonValue::Object(object) = value else {
        panic!("an object");
    };
    object
}

fn chat_options(config: OpenAIModelConfig) -> OpenAIModelOptions {
    OpenAIModelOptions {
        api: OpenAIApi::Chat,
        config,
        ..Default::default()
    }
}

fn gpt() -> OpenAIModelConfig {
    OpenAIModelConfig {
        model_id: Some("gpt-5.4".into()),
        ..Default::default()
    }
}

fn chat_model(client: &Arc<MockClient>, config: OpenAIModelConfig) -> OpenAIModel {
    injected(client, chat_options(config))
}

/// The TS `createMockClientWithCapture` stream.
fn capture_client() -> Arc<MockClient> {
    MockClient::new(json!([
        { "choices": [{ "delta": { "role": "assistant" }, "index": 0 }] },
        { "choices": [{ "finish_reason": "stop", "delta": {}, "index": 0 }] },
    ]))
}

fn hi() -> ModelRequest {
    super::request(vec![user("Hi")])
}

/// The request body of one stream.
fn captured(config: OpenAIModelConfig, request: ModelRequest) -> JsonValue {
    let client = capture_client();
    collect_ok(&chat_model(&client, config), request);
    client.last_body()
}

fn chat_events(chunks: JsonValue) -> Vec<JsonValue> {
    collect_ok(&chat_model(&MockClient::new(chunks), gpt()), hi())
}

fn role_chunk() -> JsonValue {
    json!({ "choices": [{ "delta": { "role": "assistant" }, "index": 0 }] })
}

fn finish_chunk(reason: &str) -> JsonValue {
    json!({ "choices": [{ "finish_reason": reason, "delta": {}, "index": 0 }] })
}

fn text_chunk(text: &str) -> JsonValue {
    json!({ "choices": [{ "delta": { "content": text }, "index": 0 }] })
}

#[expect(clippy::needless_pass_by_value, reason = "call sites pass json! literals")]
fn tool_chunk(calls: JsonValue) -> JsonValue {
    json!({ "choices": [{ "delta": { "tool_calls": calls }, "index": 0 }] })
}

fn find<'a>(events: &'a [JsonValue], kind: &str) -> &'a JsonValue {
    events
        .iter()
        .find(|event| event["type"] == kind)
        .expect("the event")
}

fn count(events: &[JsonValue], kind: &str) -> usize {
    events.iter().filter(|event| event["type"] == kind).count()
}

fn tool_result(id: &str, status: ToolResultStatus, content: Vec<ToolResultContent>) -> ContentBlock {
    ToolResultBlock::new(id, status, content).into()
}

mod constructor {
    use super::*;

    fn build(
        options: OpenAIModelOptions,
        env: &[(&str, &str)],
    ) -> (Arc<Recorder>, crate::Result<OpenAIModel>) {
        let recorder = Arc::new(Recorder::default());
        let factory = recorder.factory(capture_client());
        let model = OpenAIModel::with_seams(options, seams_with_env(Transport::Built(factory), env));
        (recorder, model)
    }

    fn keyed(api_key: OpenAIApiKey) -> OpenAIModelOptions {
        OpenAIModelOptions {
            api_key: Some(api_key),
            ..chat_options(gpt())
        }
    }

    fn static_key(recorder: &Recorder) -> String {
        match recorder.last().key {
            KeySource::Static(key) => key,
            KeySource::Setter(_) => panic!("a static key"),
        }
    }

    #[test]
    fn creates_an_instance_with_required_model_id() {
        let (_, model) = build(chat_options(gpt()), &[("OPENAI_API_KEY", "sk-test-env")]);
        let model = model.unwrap();
        assert_eq!(model.api(), OpenAIApi::Chat);
        assert_eq!(model.config().model_id.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn uses_custom_model_id() {
        let (_, model) = build(
            chat_options(OpenAIModelConfig {
                model_id: Some("gpt-3.5-turbo".into()),
                ..Default::default()
            }),
            &[("OPENAI_API_KEY", "sk-test-env")],
        );
        assert_eq!(model.unwrap().config().model_id.as_deref(), Some("gpt-3.5-turbo"));
    }

    #[test]
    fn warns_when_model_id_is_not_explicitly_set() {
        let (_, logged) = warnings(|| build(keyed("sk-test".into()), &[]).1.map(|_| ()).ok());
        assert!(
            logged
                .iter()
                .all(|message| !message.contains("using default modelId"))
        );
        let (_, logged) = warnings(|| {
            build(
                OpenAIModelOptions {
                    api_key: Some("sk-test".into()),
                    ..chat_options(OpenAIModelConfig::default())
                },
                &[],
            )
        });
        assert!(
            logged
                .iter()
                .any(|message| message.contains("using default modelId")),
            "{logged:?}"
        );
    }

    #[test]
    fn does_not_warn_when_model_id_is_explicitly_set() {
        let (_, logged) = warnings(|| build(keyed("sk-test".into()), &[]).1.map(|_| ()).ok());
        assert!(
            !logged
                .iter()
                .any(|message| message.contains("using default modelId"))
        );
    }

    #[test]
    fn uses_api_key_from_constructor_parameter() {
        let (recorder, model) = build(keyed("sk-explicit".into()), &[]);
        model.unwrap();
        assert_eq!(static_key(&recorder), "sk-explicit");
    }

    #[test]
    fn uses_api_key_from_environment_variable() {
        let (recorder, model) = build(chat_options(gpt()), &[("OPENAI_API_KEY", "sk-from-env")]);
        model.unwrap();
        assert_eq!(static_key(&recorder), "sk-from-env");
    }

    #[test]
    fn explicit_api_key_takes_precedence_over_environment_variable() {
        let (recorder, model) = build(keyed("sk-explicit".into()), &[("OPENAI_API_KEY", "sk-from-env")]);
        model.unwrap();
        assert_eq!(static_key(&recorder), "sk-explicit");
    }

    #[test]
    fn throws_error_when_no_api_key_is_available() {
        let (_, model) = build(chat_options(gpt()), &[("OPENAI_API_KEY", "")]);
        assert_eq!(
            model.unwrap_err().to_string(),
            "OpenAI API key is required. Provide it via the 'api_key' option (string or function) or set the OPENAI_API_KEY environment variable."
        );
    }

    #[test]
    fn uses_custom_client_configuration() {
        let (recorder, model) = build(
            OpenAIModelOptions {
                client_config: OpenAIClientConfig {
                    timeout: Some(std::time::Duration::from_secs(30)),
                    ..Default::default()
                },
                ..keyed("sk-test".into())
            },
            &[],
        );
        model.unwrap();
        assert_eq!(recorder.last().timeout, std::time::Duration::from_secs(30));
    }

    #[test]
    fn uses_provided_client_instance() {
        let model = injected(&capture_client(), chat_options(OpenAIModelConfig::default()));
        assert_eq!(model.api(), OpenAIApi::Chat);
    }

    #[test]
    fn provided_client_takes_precedence_over_api_key_and_client_config() {
        let client = capture_client();
        let model = injected(
            &client,
            OpenAIModelOptions {
                client_config: OpenAIClientConfig {
                    timeout: Some(std::time::Duration::from_secs(30)),
                    ..Default::default()
                },
                ..keyed("sk-test".into())
            },
        );
        collect_ok(&model, hi());
        assert_eq!(client.requests().len(), 1);
    }

    #[test]
    fn does_not_require_api_key_when_client_is_provided() {
        let model = OpenAIModel::with_seams(
            chat_options(OpenAIModelConfig::default()),
            seams_with_env(Transport::Injected(capture_client()), &[("OPENAI_API_KEY", "")]),
        );
        assert!(model.is_ok());
    }

    fn setter(token: &'static str) -> OpenAIApiKey {
        OpenAIApiKey::from_setter(Arc::new(move || Box::pin(async move { Ok(token.to_owned()) })))
    }

    #[test]
    fn accepts_function_based_api_key() {
        let (recorder, model) = build(keyed(setter("sk-dynamic")), &[]);
        model.unwrap();
        let KeySource::Setter(setter) = recorder.last().key else {
            panic!("the setter is passed to the client");
        };
        assert_eq!(futures::executor::block_on(setter()).unwrap(), "sk-dynamic");
    }

    #[test]
    fn accepts_async_function_based_api_key() {
        let key = OpenAIApiKey::from_setter(Arc::new(|| {
            Box::pin(async {
                futures::future::ready(()).await;
                Ok("sk-async-key".to_owned())
            })
        }));
        let (recorder, model) = build(keyed(key), &[]);
        model.unwrap();
        let KeySource::Setter(setter) = recorder.last().key else {
            panic!("the setter is passed to the client");
        };
        assert_eq!(futures::executor::block_on(setter()).unwrap(), "sk-async-key");
    }
}

mod update_config {
    use super::*;

    fn model(config: OpenAIModelConfig) -> OpenAIModel {
        chat_model(&capture_client(), config)
    }

    #[test]
    fn merges_new_config_with_existing_config() {
        let model = model(OpenAIModelConfig {
            temperature: Some(0.5),
            ..gpt()
        });
        model.update_config(OpenAIModelConfig {
            model_id: Some("gpt-5.4".into()),
            temperature: Some(0.8),
            max_tokens: Some(2048),
            ..Default::default()
        });
        assert_eq!(
            model.config(),
            OpenAIModelConfig {
                model_id: Some("gpt-5.4".into()),
                temperature: Some(0.8),
                max_tokens: Some(2048),
                context_window_limit: Some(1_050_000),
                ..Default::default()
            }
        );
    }

    #[test]
    fn preserves_fields_not_included_in_the_update() {
        let model = model(OpenAIModelConfig {
            model_id: Some("gpt-3.5-turbo".into()),
            temperature: Some(0.5),
            max_tokens: Some(1024),
            ..Default::default()
        });
        model.update_config(OpenAIModelConfig {
            model_id: Some("gpt-3.5-turbo".into()),
            temperature: Some(0.8),
            ..Default::default()
        });
        assert_eq!(
            model.config(),
            OpenAIModelConfig {
                model_id: Some("gpt-3.5-turbo".into()),
                temperature: Some(0.8),
                max_tokens: Some(1024),
                ..Default::default()
            }
        );
    }

    #[test]
    fn re_resolves_context_window_limit_when_model_id_changes_and_it_was_auto_resolved() {
        let model = model(OpenAIModelConfig::default());
        assert_eq!(model.config().context_window_limit, Some(1_050_000));
        model.update_config(OpenAIModelConfig {
            model_id: Some("gpt-4o".into()),
            ..Default::default()
        });
        assert_eq!(model.config().context_window_limit, Some(128_000));
    }

    #[test]
    fn preserves_explicit_context_window_limit_when_model_id_changes() {
        let model = model(OpenAIModelConfig {
            context_window_limit: Some(50_000),
            ..Default::default()
        });
        assert_eq!(model.config().context_window_limit, Some(50_000));
        model.update_config(OpenAIModelConfig {
            model_id: Some("gpt-4o".into()),
            ..Default::default()
        });
        assert_eq!(model.config().context_window_limit, Some(50_000));
    }
}

mod get_config {
    use super::*;

    fn config_of(config: OpenAIModelConfig) -> OpenAIModelConfig {
        chat_model(&capture_client(), config).config()
    }

    fn with(model_id: Option<&str>, context_window_limit: Option<u64>) -> OpenAIModelConfig {
        OpenAIModelConfig {
            model_id: model_id.map(str::to_owned),
            context_window_limit,
            ..Default::default()
        }
    }

    #[test]
    fn returns_the_current_configuration() {
        assert_eq!(
            config_of(OpenAIModelConfig {
                max_tokens: Some(1024),
                temperature: Some(0.7),
                ..gpt()
            }),
            OpenAIModelConfig {
                max_tokens: Some(1024),
                temperature: Some(0.7),
                context_window_limit: Some(1_050_000),
                ..gpt()
            }
        );
    }

    #[test]
    fn includes_context_window_limit_in_config_when_provided() {
        assert_eq!(
            config_of(with(Some("gpt-4o"), Some(128_000))),
            with(Some("gpt-4o"), Some(128_000))
        );
    }

    #[test]
    fn auto_populates_context_window_limit_from_model_id_lookup() {
        assert_eq!(
            config_of(with(Some("gpt-4o"), None)),
            with(Some("gpt-4o"), Some(128_000))
        );
    }

    #[test]
    fn auto_populates_context_window_limit_for_default_model_id() {
        assert_eq!(config_of(with(None, None)), with(None, Some(1_050_000)));
    }

    #[test]
    fn does_not_override_explicit_context_window_limit() {
        assert_eq!(
            config_of(with(Some("gpt-4o"), Some(50_000))),
            with(Some("gpt-4o"), Some(50_000))
        );
    }

    #[test]
    fn leaves_context_window_limit_undefined_for_unknown_model_ids() {
        assert_eq!(
            config_of(with(Some("unknown-model"), None)),
            with(Some("unknown-model"), None)
        );
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
            chat_model(
                &capture_client(),
                OpenAIModelConfig {
                    params: Some(object(json!({ "model": "bad", "stream": false }))),
                    ..gpt()
                },
            )
        });
        let managed = managed(&logged);
        assert_eq!(managed.len(), 2);
        assert!(managed[0].contains("'model'"));
        assert!(managed[1].contains("'stream'"));
    }

    #[test]
    fn warns_on_update_config_when_params_contains_provider_managed_keys() {
        let model = chat_model(&capture_client(), gpt());
        let ((), logged) = warnings(|| {
            model.update_config(OpenAIModelConfig {
                params: Some(object(json!({ "stream_options": { "include_usage": false } }))),
                ..Default::default()
            });
        });
        let managed = managed(&logged);
        assert_eq!(managed.len(), 1);
        assert!(managed[0].contains("'stream_options'"));
    }

    #[test]
    fn does_not_warn_when_params_contains_only_non_managed_keys() {
        let (_, logged) = warnings(|| {
            chat_model(
                &capture_client(),
                OpenAIModelConfig {
                    params: Some(object(json!({ "seed": 42 }))),
                    ..gpt()
                },
            )
        });
        assert_eq!(logged, Vec::<String>::new());
    }

    #[test]
    fn provider_managed_fields_in_params_are_overridden_and_cannot_take_effect() {
        let body = captured(
            OpenAIModelConfig {
                params: Some(object(json!({
                    "model": "attacker-model",
                    "messages": [{ "role": "user", "content": "hijacked" }],
                    "stream": false,
                    "stream_options": { "include_usage": false },
                }))),
                ..gpt()
            },
            hi(),
        );
        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"], json!({ "include_usage": true }));
        assert_eq!(
            body["messages"][0],
            json!({ "role": "user", "content": [{ "type": "text", "text": "Hi" }] })
        );
    }
}

mod stream {
    use super::*;

    mod cancellation {
        use super::*;

        struct Producer {
            produced: Arc<AtomicUsize>,
        }

        impl OpenAIClient for Producer {
            fn stream(&self, _request: ApiRequest) -> EventStream {
                let produced = Arc::clone(&self.produced);
                Box::pin(async_stream::stream! {
                    yield Ok(role_chunk());
                    while produced.load(Ordering::SeqCst) < 20 {
                        produced.fetch_add(1, Ordering::SeqCst);
                        yield Ok(text_chunk("token "));
                        tokio::task::yield_now().await;
                    }
                    yield Ok(finish_chunk("stop"));
                })
            }
        }

        #[tokio::test]
        async fn stops_an_in_flight_chat_completions_producer_when_the_signal_aborts() {
            let produced = Arc::new(AtomicUsize::new(0));
            let client = Arc::new(Producer {
                produced: Arc::clone(&produced),
            });
            let model = OpenAIModel::with_seams(
                chat_options(OpenAIModelConfig::default()),
                seams(Transport::Injected(client)),
            )
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
            assert_eq!(
                failure.expect("the stream fails").to_string(),
                "Request was aborted."
            );
            assert_eq!(tokens_at_cancel, 1);
            assert_eq!(produced.load(Ordering::SeqCst), tokens_at_cancel);
        }
    }

    mod validation {
        use super::*;

        fn failure(config: OpenAIModelConfig, request: ModelRequest) -> String {
            collect_err(&chat_model(&MockClient::new(json!([])), config), request).to_string()
        }

        #[test]
        fn throws_error_when_messages_array_is_empty() {
            assert_eq!(
                failure(gpt(), ModelRequest::default()),
                "At least one message is required"
            );
        }

        #[test]
        fn validates_system_prompt_is_not_empty() {
            let client = MockClient::new(json!([
                { "choices": [{ "delta": { "role": "assistant", "content": "Hello" }, "index": 0 }] },
                finish_chunk("stop"),
            ]));
            let events = collect_ok(
                &chat_model(&client, gpt()),
                ModelRequest {
                    system_prompt: Some("   ".into()),
                    ..hi()
                },
            );
            assert_eq!(events[0]["type"], "modelMessageStartEvent");
            assert_eq!(client.last_body()["messages"].as_array().unwrap().len(), 1);
        }

        #[test]
        fn throws_error_for_streaming_with_n_1() {
            let config = OpenAIModelConfig {
                params: Some(object(json!({ "n": 2 }))),
                ..gpt()
            };
            assert_eq!(failure(config, hi()), "Streaming with n > 1 is not supported");
        }

        #[test]
        fn throws_error_for_tool_spec_without_name_or_description() {
            let request = ModelRequest {
                tool_specs: vec![ToolSpec::new("", "test").with_input_schema(json!({}))],
                ..hi()
            };
            assert_eq!(
                failure(gpt(), request),
                "Tool specification must have both name and description"
            );
        }

        #[test]
        fn throws_error_for_empty_tool_result_content() {
            let request = super::super::super::request(vec![Message::new(
                Role::User,
                vec![tool_result("tool-123", ToolResultStatus::Success, Vec::new())],
            )]);
            assert_eq!(
                failure(gpt(), request),
                "Tool result for toolUseId \"tool-123\" has empty content. OpenAI requires tool messages to have non-empty content."
            );
        }

        #[test]
        fn handles_tool_result_with_error_status() {
            let client = MockClient::new(json!([
                { "choices": [{ "delta": { "role": "assistant", "content": "Ok" }, "index": 0 }] },
                finish_chunk("stop"),
            ]));
            let messages = vec![
                user("Run tool"),
                Message::new(
                    Role::Assistant,
                    vec![ToolUseBlock::new("calculator", "tool-123", json!({ "expr": "invalid" })).into()],
                ),
                Message::new(
                    Role::User,
                    vec![tool_result(
                        "tool-123",
                        ToolResultStatus::Error,
                        vec!["Division by zero".into()],
                    )],
                ),
            ];
            let events = collect_ok(
                &chat_model(&client, gpt()),
                super::super::super::request(messages),
            );
            assert_eq!(events[0]["type"], "modelMessageStartEvent");
            assert_eq!(
                client.last_body()["messages"][2],
                json!({ "role": "tool", "tool_call_id": "tool-123", "content": "[ERROR] Division by zero" })
            );
        }
    }

    mod basic_streaming {
        use super::*;

        #[test]
        fn yields_correct_event_sequence_for_simple_text_response() {
            let events = chat_events(json!([
                role_chunk(),
                text_chunk("Hello"),
                text_chunk(" world"),
                finish_chunk("stop")
            ]));
            assert_eq!(
                events,
                [
                    json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
                    json!({ "type": "modelContentBlockStartEvent" }),
                    json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hello" } }),
                    json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": " world" } }),
                    json!({ "type": "modelContentBlockStopEvent" }),
                    json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" }),
                ]
            );
        }
    }

    #[expect(clippy::needless_pass_by_value, reason = "call sites pass json! literals")]
    fn usage_after_stop(usage: JsonValue) -> JsonValue {
        let events =
            chat_events(json!([role_chunk(), finish_chunk("stop"), { "choices": [], "usage": usage }]));
        find(&events, "modelMetadataEvent").clone()
    }

    #[test]
    fn emits_model_metadata_event_with_usage_information() {
        assert_eq!(
            usage_after_stop(json!({ "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 })),
            json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } })
        );
    }

    #[expect(clippy::needless_pass_by_value, reason = "call sites pass json! literals")]
    fn with_details(details: JsonValue) -> JsonValue {
        usage_after_stop(json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "prompt_tokens_details": details,
        }))["usage"]
            .clone()
    }

    #[test]
    fn surfaces_cache_read_input_tokens_from_prompt_tokens_details() {
        assert_eq!(
            with_details(json!({ "cached_tokens": 64 })),
            json!({ "inputTokens": 100, "outputTokens": 20, "totalTokens": 120, "cacheReadInputTokens": 64 })
        );
    }

    #[test]
    fn omits_cache_read_input_tokens_when_cached_tokens_is_zero_or_absent() {
        let expected = json!({ "inputTokens": 100, "outputTokens": 20, "totalTokens": 120 });
        assert_eq!(with_details(json!({ "cached_tokens": 0 })), expected);
        assert_eq!(with_details(json!({})), expected);
    }

    #[test]
    fn surfaces_cache_write_input_tokens_from_prompt_tokens_details() {
        assert_eq!(
            with_details(json!({ "cached_tokens": 64, "cache_write_tokens": 40 })),
            json!({
                "inputTokens": 100,
                "outputTokens": 20,
                "totalTokens": 120,
                "cacheReadInputTokens": 64,
                "cacheWriteInputTokens": 40,
            })
        );
    }

    #[test]
    fn omits_cache_write_input_tokens_when_cache_write_tokens_is_zero_or_absent() {
        assert_eq!(
            with_details(json!({ "cached_tokens": 64, "cache_write_tokens": 0 })),
            json!({ "inputTokens": 100, "outputTokens": 20, "totalTokens": 120, "cacheReadInputTokens": 64 })
        );
    }

    #[test]
    fn handles_usage_with_undefined_properties() {
        assert_eq!(
            usage_after_stop(json!({})),
            json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 0, "outputTokens": 0, "totalTokens": 0 } })
        );
    }

    #[test]
    fn emits_usage_that_arrives_on_the_finish_reason_chunk() {
        let events = chat_events(json!([
            role_chunk(),
            text_chunk("Hi"),
            {
                "choices": [{ "finish_reason": "stop", "delta": { "content": "" }, "index": 0 }],
                "usage": { "prompt_tokens": 8, "completion_tokens": 6, "total_tokens": 14 },
            },
        ]));
        assert_eq!(
            events,
            [
                json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
                json!({ "type": "modelContentBlockStartEvent" }),
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hi" } }),
                json!({ "type": "modelContentBlockStopEvent" }),
                json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 8, "outputTokens": 6, "totalTokens": 14 } }),
                json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" }),
            ]
        );
    }

    #[test]
    fn filters_out_empty_string_content_deltas() {
        let events = chat_events(json!([
            role_chunk(),
            text_chunk(""),
            text_chunk("Hello"),
            finish_chunk("stop")
        ]));
        let deltas: Vec<&JsonValue> = events
            .iter()
            .filter(|event| event["type"] == "modelContentBlockDeltaEvent")
            .collect();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["delta"]["text"], "Hello");
    }

    #[test]
    fn prevents_duplicate_message_start_events() {
        let events = chat_events(json!([
            role_chunk(),
            { "choices": [{ "delta": { "role": "assistant", "content": "Hello" }, "index": 0 }] },
            finish_chunk("stop"),
        ]));
        assert_eq!(count(&events, "modelMessageStartEvent"), 1);
    }
}

mod tool_calling {
    use super::*;

    #[test]
    fn handles_tool_use_request_with_content_block_start_and_content_block_stop_events() {
        let events = chat_events(json!([
            role_chunk(),
            tool_chunk(
                json!([{ "index": 0, "id": "call_123", "type": "function", "function": { "name": "calculator", "arguments": "" } }])
            ),
            tool_chunk(json!([{ "index": 0, "function": { "arguments": "{\"expr" } }])),
            tool_chunk(json!([{ "index": 0, "function": { "arguments": "\":\"2+2\"}" } }])),
            finish_chunk("tool_calls"),
        ]));
        assert_eq!(
            events,
            [
                json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
                json!({ "type": "modelContentBlockStartEvent", "start": { "type": "toolUseStart", "name": "calculator", "toolUseId": "call_123" } }),
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": "{\"expr" } }),
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": "\":\"2+2\"}" } }),
                json!({ "type": "modelContentBlockStopEvent" }),
                json!({ "type": "modelMessageStopEvent", "stopReason": "toolUse" }),
            ]
        );
    }

    #[test]
    fn handles_multiple_tool_calls() {
        let events = chat_events(json!([
            role_chunk(),
            tool_chunk(
                json!([{ "index": 0, "id": "call_1", "type": "function", "function": { "name": "tool1", "arguments": "{}" } }])
            ),
            tool_chunk(
                json!([{ "index": 1, "id": "call_2", "type": "function", "function": { "name": "tool2", "arguments": "{}" } }])
            ),
            finish_chunk("tool_calls"),
        ]));
        assert_eq!(count(&events, "modelContentBlockStopEvent"), 2);
    }

    #[test]
    fn emits_three_interleaved_parallel_tool_calls_in_index_order() {
        let call = |index: u64, id: &str, name: &str| json!({ "index": index, "id": id, "type": "function", "function": { "name": name, "arguments": "" } });
        let args =
            |index: u64, arguments: &str| json!({ "index": index, "function": { "arguments": arguments } });
        let chunks = json!([
            role_chunk(),
            tool_chunk(json!([call(0, "call_a", "alpha")])),
            tool_chunk(json!([call(2, "call_c", "gamma"), args(0, "{\"a\":")])),
            tool_chunk(json!([call(1, "call_b", "beta"), args(2, "{\"c\":")])),
            tool_chunk(json!([args(1, "{\"b\":2}"), args(0, "1}")])),
            tool_chunk(json!([args(2, "3}")])),
            finish_chunk("tool_calls"),
        ]);
        let events = chat_events(chunks.clone());
        let tool = |name: &str, id: &str, deltas: &[&str]| {
            let mut events = vec![
                json!({ "type": "modelContentBlockStartEvent", "start": { "type": "toolUseStart", "name": name, "toolUseId": id } }),
            ];
            events.extend(deltas.iter().map(|input| {
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": input } })
            }));
            events.push(json!({ "type": "modelContentBlockStopEvent" }));
            events
        };
        let mut expected = vec![json!({ "type": "modelMessageStartEvent", "role": "assistant" })];
        expected.extend(tool("alpha", "call_a", &["{\"a\":", "1}"]));
        expected.extend(tool("beta", "call_b", &["{\"b\":2}"]));
        expected.extend(tool("gamma", "call_c", &["{\"c\":", "3}"]));
        expected.push(json!({ "type": "modelMessageStopEvent", "stopReason": "toolUse" }));
        assert_eq!(events, expected);

        let model = chat_model(&MockClient::new(chunks), gpt());
        let (blocks, result) = aggregate(&model, hi()).unwrap();
        assert_eq!(
            blocks,
            [
                json!({ "toolUse": { "name": "alpha", "toolUseId": "call_a", "input": { "a": 1 } } }),
                json!({ "toolUse": { "name": "beta", "toolUseId": "call_b", "input": { "b": 2 } } }),
                json!({ "toolUse": { "name": "gamma", "toolUseId": "call_c", "input": { "c": 3 } } }),
            ]
        );
        assert_eq!(result.stop_reason().as_str(), "toolUse");
    }

    #[test]
    fn skips_tool_calls_with_invalid_index() {
        let (events, logged) = warnings(|| {
            chat_events(json!([
                role_chunk(),
                tool_chunk(
                    json!([{ "id": "call_123", "type": "function", "function": { "name": "tool", "arguments": "{}" } }])
                ),
                finish_chunk("stop"),
            ]))
        });
        assert_eq!(count(&events, "modelContentBlockStartEvent"), 0);
        assert_eq!(count(&events, "modelContentBlockDeltaEvent"), 0);
        assert!(!events.is_empty());
        assert!(
            logged
                .iter()
                .any(|message| message.ends_with("| received tool call with invalid index"))
        );
    }

    #[test]
    fn tool_argument_deltas_can_be_reassembled_into_valid_json() {
        let mut chunks = vec![
            role_chunk(),
            tool_chunk(
                json!([{ "index": 0, "id": "call_123", "type": "function", "function": { "name": "calculator", "arguments": "" } }]),
            ),
        ];
        for part in ["{\"", "x\":", "10,", "\"y\":", "20}"] {
            chunks.push(tool_chunk(
                json!([{ "index": 0, "function": { "arguments": part } }]),
            ));
        }
        chunks.push(finish_chunk("tool_calls"));
        let events = chat_events(JsonValue::Array(chunks));
        let reassembled: String = events
            .iter()
            .filter(|event| event["delta"]["type"] == "toolUseInputDelta")
            .map(|event| event["delta"]["input"].as_str().unwrap())
            .collect();
        assert_eq!(
            serde_json::from_str::<JsonValue>(&reassembled).unwrap(),
            json!({ "x": 10, "y": 20 })
        );
    }

    #[test]
    fn handles_messages_with_both_text_and_tool_calls() {
        let events = chat_events(json!([
            role_chunk(),
            text_chunk("Let me calculate "),
            text_chunk("that for you."),
            tool_chunk(
                json!([{ "index": 0, "id": "call_123", "type": "function", "function": { "name": "calculator", "arguments": "{\"expr\":\"2+2\"}" } }])
            ),
            finish_chunk("tool_calls"),
        ]));
        assert_eq!(events[0]["type"], "modelMessageStartEvent");
        assert_eq!(events[1]["type"], "modelContentBlockStartEvent");
        assert_eq!(
            events[2]["delta"],
            json!({ "type": "textDelta", "text": "Let me calculate " })
        );
        // The text block closes before the tool block opens.
        assert_eq!(events[4]["type"], "modelContentBlockStopEvent");
        assert_eq!(events[5]["start"]["type"], "toolUseStart");
        assert_eq!(count(&events, "modelContentBlockStopEvent"), 2);
    }
}

mod stop_reasons {
    use super::*;

    #[test]
    fn maps_open_ai_stop_reasons_to_sdk_stop_reasons() {
        for (openai, sdk) in [
            ("stop", "endTurn"),
            ("tool_calls", "toolUse"),
            ("length", "maxTokens"),
            ("content_filter", "contentFiltered"),
        ] {
            let events = chat_events(json!([role_chunk(), finish_chunk(openai)]));
            assert_eq!(
                find(&events, "modelMessageStopEvent")["stopReason"],
                sdk,
                "{openai}"
            );
        }
    }

    #[test]
    fn handles_unknown_stop_reasons_with_warning() {
        let (events, logged) =
            warnings(|| chat_events(json!([role_chunk(), finish_chunk("new_unknown_reason")])));
        assert_eq!(
            find(&events, "modelMessageStopEvent")["stopReason"],
            "newUnknownReason"
        );
        assert!(logged.contains(
            &"finish_reason=<new_unknown_reason>, fallback=<newUnknownReason> | unknown openai stop reason, using camelCase conversion as fallback"
                .to_owned()
        ));
    }
}

mod api_request_formatting {
    use super::*;

    #[test]
    fn formats_api_request_correctly_with_all_options() {
        let client = capture_client();
        let model = chat_model(
            &client,
            OpenAIModelConfig {
                temperature: Some(0.7),
                max_tokens: Some(1000),
                ..gpt()
            },
        );
        collect_ok(
            &model,
            ModelRequest {
                system_prompt: Some("You are a helpful assistant".into()),
                tool_specs: vec![
                    ToolSpec::new("calculator", "Calculate expressions").with_input_schema(
                        json!({ "type": "object", "properties": { "expr": { "type": "string" } } }),
                    ),
                ],
                tool_choice: Some(ToolChoice::Auto),
                ..hi()
            },
        );
        assert_eq!(client.requests().len(), 1);
        assert_eq!(client.requests()[0].path, "/chat/completions");
        assert_eq!(
            client.last_body(),
            json!({
                "model": "gpt-5.4",
                "stream": true,
                "stream_options": { "include_usage": true },
                "temperature": 0.7,
                "max_completion_tokens": 1000,
                "messages": [
                    { "role": "system", "content": "You are a helpful assistant" },
                    { "role": "user", "content": [{ "type": "text", "text": "Hi" }] },
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "calculator",
                        "description": "Calculate expressions",
                        "parameters": { "type": "object", "properties": { "expr": { "type": "string" } } },
                    },
                }],
                "tool_choice": "auto",
            })
        );
    }

    #[test]
    fn maps_tool_choice_any_and_tool() {
        let with = |choice| ModelRequest {
            tool_specs: vec![ToolSpec::new("calc", "d")],
            tool_choice: Some(choice),
            ..hi()
        };
        assert_eq!(captured(gpt(), with(ToolChoice::Any))["tool_choice"], "required");
        assert_eq!(
            captured(gpt(), with(ToolChoice::tool("calc")))["tool_choice"],
            json!({ "type": "function", "function": { "name": "calc" } })
        );
    }

    #[test]
    fn formats_top_p_and_the_penalties() {
        let body = captured(
            OpenAIModelConfig {
                top_p: Some(0.9),
                frequency_penalty: Some(0.5),
                presence_penalty: Some(-0.5),
                ..gpt()
            },
            hi(),
        );
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["frequency_penalty"], 0.5);
        assert_eq!(body["presence_penalty"], -0.5);
    }

    fn cached(cache_config: CacheConfig) -> OpenAIModelConfig {
        OpenAIModelConfig {
            cache_config: Some(cache_config),
            ..gpt()
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
        assert_eq!(
            captured(cached(keyed(CacheKey::Key("tenant-42".into()))), hi())["prompt_cache_key"],
            "tenant-42"
        );
    }

    #[test]
    fn omits_prompt_cache_key_when_cache_config_is_unset() {
        assert!(
            captured(gpt(), in_session(Some("s1")))
                .get("prompt_cache_key")
                .is_none()
        );
    }

    #[test]
    fn lets_an_explicit_prompt_cache_key_in_params_win_over_cache_config() {
        let body = captured(
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
        assert_eq!(
            captured(cached(CacheConfig::default()), in_session(Some("s1")))["prompt_cache_key"],
            "strands-s1"
        );
    }

    #[test]
    fn lets_a_configured_cache_key_win_over_the_agent_metadata_session() {
        let body = captured(
            cached(keyed(CacheKey::Key("tenant-42".into()))),
            in_session(Some("s1")),
        );
        assert_eq!(body["prompt_cache_key"], "tenant-42");
    }

    #[test]
    fn treats_a_false_cache_key_as_an_opt_out_even_with_an_agent_metadata_session() {
        let body = captured(cached(keyed(CacheKey::Disabled)), in_session(Some("s1")));
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn omits_prompt_cache_key_when_the_agent_metadata_carries_no_session() {
        assert!(
            captured(cached(CacheConfig::default()), in_session(None))
                .get("prompt_cache_key")
                .is_none()
        );
    }

    fn ttl(value: &str) -> CacheConfig {
        CacheConfig {
            ttl: Some(CacheTtl::from(value)),
            ..Default::default()
        }
    }

    #[test]
    fn maps_retention_literal_cache_config_ttl_s_to_prompt_cache_retention() {
        for value in ["24h", "in_memory"] {
            assert_eq!(
                captured(cached(ttl(value)), hi())["prompt_cache_retention"],
                value
            );
        }
    }

    #[test]
    fn ignores_a_non_retention_cache_config_ttl_and_warns_once() {
        let (body, logged) = warnings(|| captured(cached(ttl("5m")), hi()));
        assert!(body.get("prompt_cache_retention").is_none());
        assert!(
            logged.contains(
                &"ttl=<5m> | cacheConfig.ttl is not an openai retention value, ignoring".to_owned()
            )
        );
    }

    #[test]
    fn lets_an_explicit_prompt_cache_retention_in_params_win_over_cache_config() {
        let body = captured(
            OpenAIModelConfig {
                params: Some(object(json!({ "prompt_cache_retention": "24h" }))),
                ..cached(ttl("in_memory"))
            },
            hi(),
        );
        assert_eq!(body["prompt_cache_retention"], "24h");
    }

    #[test]
    fn does_not_warn_about_an_unsupported_ttl_when_an_explicit_prompt_cache_retention_already_wins() {
        let (body, logged) = warnings(|| {
            captured(
                OpenAIModelConfig {
                    params: Some(object(json!({ "prompt_cache_retention": "24h" }))),
                    ..cached(ttl("5m"))
                },
                hi(),
            )
        });
        assert_eq!(body["prompt_cache_retention"], "24h");
        assert!(
            !logged
                .iter()
                .any(|message| message.contains("not an openai retention value"))
        );
    }

    #[test]
    fn warns_once_that_placement_fields_have_no_effect() {
        let (_, logged) = warnings(|| {
            captured(
                cached(CacheConfig {
                    strategy: Some(CacheStrategy::Anthropic),
                    system_prompt_ttl: Some(CacheSection::Enabled(false)),
                    tools_ttl: Some(CacheSection::Ttl(CacheTtl::OneHour)),
                    cache_key: Some(CacheKey::Key("k".into())),
                    ..Default::default()
                }),
                hi(),
            )
        });
        assert!(logged.iter().any(|message| message.contains("have no effect")));
    }
}

mod system_prompt_handling {
    use super::*;

    fn messages_for(blocks: Vec<SystemContentBlock>) -> (JsonValue, Vec<String>) {
        warnings(|| {
            captured(
                gpt(),
                ModelRequest {
                    system_prompt: Some(SystemPrompt::Blocks(blocks)),
                    ..super::super::request(vec![user("Hello")])
                },
            )["messages"]
                .clone()
        })
    }

    fn hello() -> JsonValue {
        json!({ "role": "user", "content": [{ "type": "text", "text": "Hello" }] })
    }

    fn guard(text: &str, qualifier: GuardQualifier) -> SystemContentBlock {
        crate::types::messages::GuardContentBlock::from(GuardContentText::new(text, vec![qualifier])).into()
    }

    const GUARD_WARNING: &str =
        "guard content is not supported in openai system prompts, removing guard content block";

    #[test]
    fn formats_array_system_prompt_with_text_blocks_only() {
        let (messages, _) = messages_for(vec![
            SystemContentBlock::text("You are a helpful assistant"),
            SystemContentBlock::text("Additional context here"),
        ]);
        assert_eq!(
            messages,
            json!([{ "role": "system", "content": "You are a helpful assistantAdditional context here" }, hello()])
        );
    }

    #[test]
    fn formats_array_system_prompt_with_cache_points() {
        let (messages, logged) = messages_for(vec![
            SystemContentBlock::text("You are a helpful assistant"),
            SystemContentBlock::text("Large context document"),
            CachePointBlock::new().into(),
        ]);
        assert!(logged.contains(
            &"cache points are not supported in openai system prompts, ignoring cache points".to_owned()
        ));
        assert_eq!(
            messages,
            json!([{ "role": "system", "content": "You are a helpful assistantLarge context document" }, hello()])
        );
    }

    #[test]
    fn handles_empty_array_system_prompt() {
        assert_eq!(messages_for(Vec::new()).0, json!([hello()]));
    }

    #[test]
    fn formats_array_system_prompt_with_single_text_block() {
        let (messages, _) = messages_for(vec![SystemContentBlock::text("You are a helpful assistant")]);
        assert_eq!(
            messages,
            json!([{ "role": "system", "content": "You are a helpful assistant" }, hello()])
        );
    }

    #[test]
    fn warns_and_filters_guard_content_from_system_prompt() {
        let (messages, logged) = messages_for(vec![
            SystemContentBlock::text("You are a helpful assistant"),
            guard("Guard content", GuardQualifier::GroundingSource),
        ]);
        assert!(logged.contains(&GUARD_WARNING.to_owned()));
        assert_eq!(
            messages,
            json!([{ "role": "system", "content": "You are a helpful assistant" }, hello()])
        );
    }

    #[test]
    fn preserves_text_blocks_when_filtering_guard_content() {
        let (messages, logged) = messages_for(vec![
            SystemContentBlock::text("First text"),
            guard("Guard content", GuardQualifier::Query),
            SystemContentBlock::text("Second text"),
        ]);
        assert!(logged.contains(&GUARD_WARNING.to_owned()));
        assert_eq!(
            messages,
            json!([{ "role": "system", "content": "First textSecond text" }, hello()])
        );
    }

    #[test]
    fn handles_system_prompt_with_only_guard_content() {
        let (messages, logged) =
            messages_for(vec![guard("Only guard content", GuardQualifier::GuardContent)]);
        assert!(logged.contains(&GUARD_WARNING.to_owned()));
        assert_eq!(messages, json!([hello()]));
    }
}

mod guard_content_in_messages {
    use super::*;

    const WARNING: &str =
        "block_type=<guardContentBlock> | unsupported content type in openai user message | skipping";

    fn guard_text(text: &str, qualifier: GuardQualifier) -> ContentBlock {
        crate::types::messages::GuardContentBlock::from(GuardContentText::new(text, vec![qualifier])).into()
    }

    fn sent(blocks: Vec<ContentBlock>) -> (JsonValue, Vec<String>) {
        warnings(|| {
            captured(
                gpt(),
                super::super::request(vec![Message::new(Role::User, blocks)]),
            )["messages"]
                .clone()
        })
    }

    #[test]
    fn warns_and_filters_guard_content_from_user_messages() {
        let (messages, logged) = sent(vec![
            TextBlock::new("Verify this:").into(),
            guard_text("Guard content", GuardQualifier::GroundingSource),
            TextBlock::new("Is it correct?").into(),
        ]);
        assert!(logged.contains(&WARNING.to_owned()));
        assert_eq!(
            messages,
            json!([{
                "role": "user",
                "content": [{ "type": "text", "text": "Verify this:" }, { "type": "text", "text": "Is it correct?" }],
            }])
        );
    }

    #[test]
    fn warns_and_filters_guard_content_with_image_from_user_messages() {
        let image = GuardContentImage::new(GuardImageFormat::Jpeg, GuardImageSource::new(vec![1, 2, 3, 4]));
        let (messages, logged) = sent(vec![
            TextBlock::new("Check this image:").into(),
            crate::types::messages::GuardContentBlock::from(image).into(),
        ]);
        assert!(logged.contains(&WARNING.to_owned()));
        assert_eq!(
            messages,
            json!([{ "role": "user", "content": [{ "type": "text", "text": "Check this image:" }] }])
        );
    }

    #[test]
    fn handles_message_with_only_guard_content() {
        let (messages, logged) = sent(vec![guard_text(
            "Only guard content",
            GuardQualifier::GuardContent,
        )]);
        assert!(logged.contains(&WARNING.to_owned()));
        assert_eq!(messages, json!([]));
    }
}

mod unknown_blocks_in_messages {
    use super::*;

    #[test]
    fn skips_unknown_blocks_in_user_and_assistant_messages_without_a_warning() {
        let unknown = UnknownBlock::new(object(json!({ "futureBlock": { "value": 1 } })));
        let messages = vec![
            Message::new(
                Role::User,
                vec![TextBlock::new("Hello").into(), unknown.clone().into()],
            ),
            Message::new(Role::Assistant, vec![TextBlock::new("Hi").into(), unknown.into()]),
        ];
        let (body, logged) = warnings(|| captured(gpt(), super::super::request(messages)));
        assert_eq!(
            body["messages"],
            json!([
                { "role": "user", "content": [{ "type": "text", "text": "Hello" }] },
                { "role": "assistant", "content": "Hi" },
            ])
        );
        assert_eq!(logged, Vec::<String>::new());
    }

    #[test]
    fn converts_reasoning_to_text_and_sends_tool_calls_in_assistant_messages() {
        let messages = vec![
            user("Hi"),
            Message::new(
                Role::Assistant,
                vec![
                    ReasoningBlock::new().with_text("thinking ").into(),
                    TextBlock::new("answer ").into(),
                    ToolUseBlock::new("calc", "call_1", json!({ "x": 1 })).into(),
                ],
            ),
        ];
        let (body, logged) = warnings(|| captured(gpt(), super::super::request(messages)));
        assert_eq!(
            body["messages"][1],
            json!({
                "role": "assistant",
                "content": "thinking answer",
                "tool_calls": [{ "id": "call_1", "type": "function", "function": { "name": "calc", "arguments": "{\"x\":1}" } }],
            })
        );
        assert!(logged.contains(&"block_type=<reasoningBlock> | reasoning blocks not supported by openai | converting to text".to_owned()));
    }
}

mod media_blocks {
    use super::*;

    fn sent(blocks: Vec<ContentBlock>) -> (JsonValue, Vec<String>) {
        warnings(|| {
            captured(
                gpt(),
                super::super::request(vec![Message::new(Role::User, blocks)]),
            )["messages"]
                .clone()
        })
    }

    fn png(bytes: &[u8]) -> ImageBlock {
        ImageBlock::new(ImageFormat::Png, bytes.to_vec())
    }

    #[test]
    fn formats_image_block_in_user_message_as_image_url_with_base64() {
        let (messages, _) = sent(vec![
            TextBlock::new("What is in this image?").into(),
            png(b"Hello").into(),
        ]);
        assert_eq!(
            messages[0],
            json!({
                "role": "user",
                "content": [
                    { "type": "text", "text": "What is in this image?" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,SGVsbG8=" } },
                ],
            })
        );
    }

    #[test]
    fn formats_image_block_in_user_message_with_url_source() {
        let image = ImageBlock::new(
            ImageFormat::Jpeg,
            ImageSource::Url("https://example.com/img.jpg".into()),
        );
        let (messages, _) = sent(vec![image.into()]);
        assert_eq!(
            messages[0]["content"][0],
            json!({ "type": "image_url", "image_url": { "url": "https://example.com/img.jpg" } })
        );
    }

    #[test]
    fn formats_document_block_with_bytes_source_as_file_in_user_message() {
        let (messages, _) = sent(vec![
            DocumentBlock::new("report.pdf", "pdf", vec![1u8, 2, 3]).into(),
        ]);
        assert_eq!(
            messages[0]["content"][0],
            json!({ "type": "file", "file": { "file_data": "data:application/pdf;base64,AQID", "filename": "report.pdf" } })
        );
    }

    #[test]
    fn converts_text_and_content_document_sources_to_text() {
        use crate::types::media::DocumentSource;
        let (messages, logged) = sent(vec![
            DocumentBlock::new("a.txt", "txt", DocumentSource::Text("plain".into())).into(),
            DocumentBlock::new(
                "b.txt",
                "txt",
                DocumentSource::Content(vec![TextBlock::new("part")]),
            )
            .into(),
        ]);
        assert_eq!(
            messages[0]["content"],
            json!([{ "type": "text", "text": "plain" }, { "type": "text", "text": "part" }])
        );
        assert!(
            logged
                .iter()
                .any(|message| message.starts_with("source_type=<documentSourceText>"))
        );
    }

    #[test]
    fn splits_image_from_tool_result_into_separate_user_message() {
        let (messages, logged) = sent(vec![tool_result(
            "tool-1",
            ToolResultStatus::Success,
            vec!["Screenshot captured".into(), png(b"Hello").into()],
        )]);
        assert_eq!(
            messages[0],
            json!({ "role": "tool", "tool_call_id": "tool-1", "content": "Screenshot captured" })
        );
        assert_eq!(
            messages[1],
            json!({ "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:image/png;base64,SGVsbG8=" } }] })
        );
        assert!(logged.contains(
            &"tool_call_id=<tool-1> | moving images from tool result to separate user message for openai compatibility".to_owned()
        ));
    }

    #[test]
    fn injects_placeholder_text_when_tool_result_contains_only_images() {
        let (messages, _) = sent(vec![tool_result(
            "tool-1",
            ToolResultStatus::Success,
            vec![png(&[1]).into()],
        )]);
        assert_eq!(
            messages[0]["content"],
            "Tool successfully returned an image. The image is being provided in the following user message."
        );
    }

    #[test]
    fn skips_document_block_in_tool_result_with_warning() {
        let (messages, logged) = sent(vec![tool_result(
            "tool-1",
            ToolResultStatus::Success,
            vec![
                "result".into(),
                DocumentBlock::new("doc.pdf", "pdf", vec![1u8]).into(),
            ],
        )]);
        assert_eq!(messages[0]["content"], "result");
        assert!(
            logged.contains(
                &"block_type=<documentBlock> | documents not supported in openai tool results, skipping"
                    .to_owned()
            )
        );
    }

    #[test]
    fn skips_video_block_in_tool_result_with_warning() {
        let (messages, logged) = sent(vec![tool_result(
            "tool-1",
            ToolResultStatus::Success,
            vec!["result".into(), VideoBlock::new("mp4", vec![1u8]).into()],
        )]);
        assert_eq!(messages[0]["content"], "result");
        assert!(logged.contains(
            &"block_type=<videoBlock> | videos not supported in openai tool results, skipping".to_owned()
        ));
    }
}

mod tool_result_and_text_ordering {
    use super::*;

    #[test]
    fn keeps_tool_results_adjacent_to_the_assistant_tool_calls_for_name() {
        let cases: [(&[&str], &[&str]); 3] = [
            (&["tu1", "text"], &["user", "assistant", "tool", "user"]),
            (&["text", "tu1"], &["user", "assistant", "tool", "user"]),
            (
                &["tu1", "text", "tu2"],
                &["user", "assistant", "tool", "tool", "user"],
            ),
        ];
        for (order, roles) in cases {
            let tool_uses = order
                .iter()
                .filter(|entry| **entry != "text")
                .map(|id| ToolUseBlock::new("calc", *id, json!({})).into())
                .collect();
            let results = order
                .iter()
                .map(|entry| {
                    if *entry == "text" {
                        TextBlock::new("\n\nNOTE").into()
                    } else {
                        tool_result(entry, ToolResultStatus::Success, vec!["42".into()])
                    }
                })
                .collect();
            let messages = vec![
                user("task"),
                Message::new(Role::Assistant, tool_uses),
                Message::new(Role::User, results),
            ];
            let body = captured(gpt(), super::super::request(messages));
            let sent = body["messages"].as_array().unwrap();
            let sent_roles: Vec<&str> = sent
                .iter()
                .map(|message| message["role"].as_str().unwrap())
                .collect();
            assert_eq!(sent_roles, roles, "{order:?}");
            assert_eq!(
                sent[2],
                json!({ "role": "tool", "tool_call_id": "tu1", "content": "42" })
            );
            assert_eq!(
                sent.last().unwrap(),
                &json!({ "role": "user", "content": [{ "type": "text", "text": "\n\nNOTE" }] })
            );
        }
    }

    #[test]
    fn keeps_media_hoisted_out_of_a_tool_result_ahead_of_the_injected_text() {
        let messages = vec![Message::new(
            Role::User,
            vec![
                tool_result(
                    "tu1",
                    ToolResultStatus::Success,
                    vec!["42".into(), ImageBlock::new(ImageFormat::Png, vec![1u8]).into()],
                ),
                TextBlock::new("\n\nNOTE").into(),
            ],
        )];
        let body = captured(gpt(), super::super::request(messages));
        assert_eq!(
            body["messages"],
            json!([
                { "role": "tool", "tool_call_id": "tu1", "content": "42" },
                { "role": "user", "content": [{ "type": "image_url", "image_url": { "url": "data:image/png;base64,AQ==" } }] },
                { "role": "user", "content": [{ "type": "text", "text": "\n\nNOTE" }] },
            ])
        );
    }
}

mod error_handling {
    use super::*;
    use crate::error::Error;
    use crate::models::openai::client::{ApiError, ApiErrorKind};

    fn failure(items: Vec<Result<JsonValue, ApiError>>) -> Error {
        let model = chat_model(&MockClient::scripted(move |_| items.clone()), gpt());
        collect_err(&model, hi())
    }

    fn api(message: &str) -> ApiError {
        ApiError::new(ApiErrorKind::Api, message)
    }

    #[test]
    fn throws_context_window_overflow_error_for_structured_error_with_code() {
        let error = failure(vec![Err(api_code_error(
            "context_length_exceeded",
            "Context length exceeded",
        ))]);
        assert!(matches!(error, Error::ContextWindowOverflow(_)), "{error:?}");
    }

    #[test]
    fn throws_context_window_overflow_error_for_error_with_message_pattern() {
        let error = failure(vec![Err(api("maximum context length exceeded"))]);
        assert!(matches!(error, Error::ContextWindowOverflow(_)), "{error:?}");
    }

    #[test]
    fn throws_context_window_overflow_error_for_api_error_instance() {
        let error = failure(vec![Err(ApiError::from_status(
            400,
            r#"{"error":{"message":"Context length exceeded","code":"context_length_exceeded"}}"#,
        ))]);
        assert!(matches!(error, Error::ContextWindowOverflow(_)), "{error:?}");
        assert_eq!(error.to_string(), "400 Context length exceeded");
    }

    #[test]
    fn passes_through_other_errors_unchanged() {
        let error = failure(vec![Err(api_status_error(401, "Invalid API key"))]);
        assert!(matches!(error, Error::Model(_)), "{error:?}");
        assert_eq!(error.to_string(), "Invalid API key");
    }

    #[test]
    fn handles_stream_interruption_errors() {
        let error = failure(vec![
            Ok(role_chunk()),
            Ok(text_chunk("Hello")),
            Err(ApiError::new(ApiErrorKind::Connection, "Network connection lost")),
        ]);
        assert!(matches!(error, Error::ModelNetwork(_)), "{error:?}");
        assert_eq!(error.to_string(), "Network connection lost");
    }

    #[test]
    fn throws_model_throttled_error_for_http_429_status() {
        let error = failure(vec![Err(api_status_error(429, "Too many requests"))]);
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
    }

    #[test]
    fn throws_model_throttled_error_for_rate_limit_exceeded_error_code() {
        let error = failure(vec![Err(api_code_error(
            "rate_limit_exceeded",
            "Rate limit reached",
        ))]);
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
    }

    #[test]
    fn throws_model_throttled_error_for_error_message_containing_rate_limit_pattern() {
        let error = failure(vec![Err(api("You have exceeded your rate limit"))]);
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
    }

    #[test]
    fn throws_model_throttled_error_for_too_many_requests_message() {
        let error = failure(vec![Err(api("Too many requests, please slow down"))]);
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
    }

    #[test]
    fn preserves_original_error_as_cause_in_model_throttled_error() {
        let error = failure(vec![Err(api_status_error(
            429,
            "Request too large for gpt-5.4 on tokens per min",
        ))]);
        assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
        assert_eq!(
            error.to_string(),
            "Request too large for gpt-5.4 on tokens per min"
        );
        let mut source = std::error::Error::source(&error);
        let mut cause = None;
        while let Some(current) = source {
            if let Some(api) = current.downcast_ref::<ApiError>() {
                cause = Some(api.clone());
            }
            source = current.source();
        }
        let cause = cause.expect("the API error is the cause");
        assert_eq!(cause.status, Some(429));
        assert_eq!(cause.message, "Request too large for gpt-5.4 on tokens per min");
    }
}
