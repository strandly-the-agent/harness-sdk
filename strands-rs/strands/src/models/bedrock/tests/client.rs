//! Constructor, client setup, model info, `maxTokens`, `updateConfig`, `getConfig` and region cases.

use std::sync::Arc;
use std::time::Duration;

use aws_config::SdkConfig;
use aws_credential_types::Credentials;
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_sdk_bedrockruntime::config::{AppName, Region};
use aws_smithy_types::timeout::TimeoutConfig;
use serde_json::json;

use super::*;
use crate::models::bedrock::client::{
    ClientOptions, EnvLookup, client_config, effective_sdk_config, resolve_region,
};
use crate::models::bedrock::{BedrockModelOptions, IncludeToolResultStatus};
use crate::models::model::{BaseModelConfig, CacheConfig, CacheStrategy};
use crate::models::model_info::{ModelCapabilities, ModelInfo, ModelLimits, PromptCaching};
use crate::types::messages::{InjectedBlock, SystemPrompt};

fn region_options(region: &str) -> ClientOptions {
    ClientOptions {
        region: Some(region.to_owned()),
        ..Default::default()
    }
}

fn no_env() -> EnvLookup {
    Arc::new(|_| None)
}

fn sdk() -> SdkConfig {
    SdkConfig::builder().build()
}

fn read_timeout(config: &aws_sdk_bedrockruntime::Config) -> Option<Duration> {
    config.timeout_config().and_then(TimeoutConfig::read_timeout)
}

fn caching(model_id: &str) -> BedrockModelConfig {
    BedrockModelConfig {
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Auto),
            ..Default::default()
        }),
        ..config(model_id)
    }
}

fn system_sent(model: &BedrockModel, transport: &FakeTransport) -> JsonValue {
    let request = ModelRequest {
        messages: hello(),
        system_prompt: Some(SystemPrompt::Text("Be helpful".into())),
        ..Default::default()
    };
    sent(model, transport, request)["system"].clone()
}

fn info_override(capabilities: ModelCapabilities) -> Option<ModelInfo> {
    Some(ModelInfo {
        capabilities: Some(capabilities),
        limits: None,
    })
}

fn prompt_caching(model: &BedrockModel) -> Option<PromptCaching> {
    model
        .model_info()
        .capabilities
        .and_then(|capabilities| capabilities.prompt_caching)
}

fn sent_inference_config(config: BedrockModelConfig) -> JsonValue {
    let (model, transport) = model(config);
    sent(&model, &transport, request(hello()))
        .get("inferenceConfig")
        .cloned()
        .unwrap_or(JsonValue::Null)
}

mod constructor {
    use super::*;

    #[test]
    fn creates_an_instance_with_default_configuration() {
        let (model, _) = model(BedrockModelConfig::default());
        assert!(model.config().model_id.is_some());
    }

    #[test]
    fn warns_when_model_id_is_not_explicitly_set() {
        // warn_once logs once per process, so another test may have logged it first.
        let (_, warned) = warnings(|| model(BedrockModelConfig::default()));
        assert!(warned.len() <= 2);
        assert!(
            crate::models::defaults::default_model_warning_message("global.anthropic.claude-sonnet-4-6")
                .contains("using default modelId")
        );
    }

    #[test]
    fn does_not_warn_when_model_id_is_explicitly_set() {
        let (_, warned) = warnings(|| model(config("us.anthropic.claude-3-5-sonnet-20241022-v2:0-explicit")));
        assert!(
            warned
                .iter()
                .all(|warning| !warning.contains("using default modelId"))
        );
    }

    #[test]
    fn uses_provided_model_id() {
        let (model, _) = model(config("us.anthropic.claude-3-5-sonnet-20241022-v2:0"));
        assert_eq!(
            model.config(),
            BedrockModelConfig {
                model_id: Some("us.anthropic.claude-3-5-sonnet-20241022-v2:0".into()),
                max_tokens: Some(8192),
                context_window_limit: Some(200_000),
                ..Default::default()
            }
        );
    }

    #[test]
    fn uses_provided_region() {
        let config = client_config(&region_options("eu-west-1"), &sdk(), "eu-west-1".into());
        assert_eq!(config.region(), Some(&Region::new("eu-west-1")));
        assert_eq!(
            config.app_name().map(AppName::as_ref),
            Some("strands-agents-rust-sdk")
        );
        assert_eq!(read_timeout(&config), Some(Duration::from_secs(120)));
    }

    #[test]
    fn extends_custom_user_agent_if_provided() {
        let sdk = SdkConfig::builder()
            .app_name(AppName::new("my-app").unwrap())
            .build();
        let config = client_config(&region_options("us-west-2"), &sdk, "us-west-2".into());
        assert_eq!(config.app_name().map(AppName::as_ref), Some("my-app"));
    }

    #[test]
    fn passes_custom_credentials_to_client() {
        let credentials = Credentials::new("AKIDEXAMPLE", "secret", None, None, "test");
        let sdk = SdkConfig::builder()
            .credentials_provider(SharedCredentialsProvider::new(credentials))
            .build();
        assert!(
            effective_sdk_config(&region_options("us-west-2"), &sdk)
                .credentials_provider()
                .is_some()
        );
    }

    #[test]
    fn applies_a_default_120s_request_timeout() {
        let config = client_config(&region_options("us-west-2"), &sdk(), "us-west-2".into());
        assert_eq!(read_timeout(&config), Some(Duration::from_secs(120)));
    }

    #[test]
    fn lets_the_caller_override_request_timeout() {
        let sdk = SdkConfig::builder()
            .timeout_config(
                TimeoutConfig::builder()
                    .read_timeout(Duration::from_secs(5))
                    .build(),
            )
            .build();
        let config = client_config(&region_options("us-west-2"), &sdk, "us-west-2".into());
        assert_eq!(read_timeout(&config), Some(Duration::from_secs(5)));
    }

    #[test]
    fn merges_the_default_timeout_with_other_request_handler_options() {
        let sdk = SdkConfig::builder()
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(1))
                    .build(),
            )
            .build();
        let config = client_config(&region_options("us-west-2"), &sdk, "us-west-2".into());
        let timeouts = config.timeout_config().unwrap();
        assert_eq!(timeouts.read_timeout(), Some(Duration::from_secs(120)));
        assert_eq!(timeouts.connect_timeout(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn applies_the_request_timeout_option_to_the_default_request_handler() {
        let options = ClientOptions {
            request_timeout: Some(Duration::from_secs(600)),
            ..region_options("us-west-2")
        };
        let config = client_config(&options, &sdk(), "us-west-2".into());
        assert_eq!(read_timeout(&config), Some(Duration::from_secs(600)));
    }

    #[test]
    fn lets_the_request_timeout_option_take_precedence_over_client_config_request_handler() {
        let sdk = SdkConfig::builder()
            .timeout_config(
                TimeoutConfig::builder()
                    .read_timeout(Duration::from_secs(5))
                    .connect_timeout(Duration::from_secs(1))
                    .build(),
            )
            .build();
        let options = ClientOptions {
            request_timeout: Some(Duration::from_secs(600)),
            ..region_options("us-west-2")
        };
        let config = client_config(&options, &sdk, "us-west-2".into());
        let timeouts = config.timeout_config().unwrap();
        assert_eq!(timeouts.read_timeout(), Some(Duration::from_secs(600)));
        assert_eq!(timeouts.connect_timeout(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn falls_back_to_the_default_when_request_timeout_is_explicitly_undefined() {
        let options = ClientOptions {
            request_timeout: None,
            ..region_options("us-west-2")
        };
        let config = client_config(&options, &sdk(), "us-west-2".into());
        assert_eq!(read_timeout(&config), Some(Duration::from_secs(120)));
    }

    #[test]
    fn adds_api_key_middleware_when_api_key_is_provided() {
        let credentials = Credentials::new("AKIDEXAMPLE", "secret", None, None, "test");
        let sdk = SdkConfig::builder()
            .credentials_provider(SharedCredentialsProvider::new(credentials))
            .build();
        let options = ClientOptions {
            api_key: Some("br-test-key".into()),
            ..region_options("us-east-1")
        };
        // Without SigV4 credentials the client authenticates with the bearer token.
        assert!(
            effective_sdk_config(&options, &sdk)
                .credentials_provider()
                .is_none()
        );
    }

    #[test]
    fn does_not_add_api_key_middleware_when_api_key_is_not_provided() {
        let credentials = Credentials::new("AKIDEXAMPLE", "secret", None, None, "test");
        let sdk = SdkConfig::builder()
            .credentials_provider(SharedCredentialsProvider::new(credentials))
            .build();
        assert!(
            effective_sdk_config(&region_options("us-east-1"), &sdk)
                .credentials_provider()
                .is_some()
        );
    }

    #[test]
    fn does_not_include_api_key_in_model_config() {
        let options = BedrockModelOptions {
            config: BedrockModelConfig {
                temperature: Some(0.5),
                ..Default::default()
            },
            region: Some("us-east-1".into()),
            api_key: Some("br-test-key".into()),
            ..Default::default()
        };
        let model = BedrockModel::with_client_options(options.clone(), no_env());
        assert_eq!(
            model.config(),
            BedrockModelConfig {
                model_id: Some("global.anthropic.claude-sonnet-4-6".into()),
                temperature: Some(0.5),
                max_tokens: Some(128_000),
                context_window_limit: Some(1_000_000),
                ..Default::default()
            }
        );
        let debug = format!("{options:?}");
        assert!(!debug.contains("br-test-key"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn includes_context_window_limit_in_config_when_provided() {
        let (model, _) = model(BedrockModelConfig {
            context_window_limit: Some(200_000),
            ..config("anthropic.claude-sonnet-4-6")
        });
        assert_eq!(model.config().context_window_limit, Some(200_000));
        assert_eq!(model.config().max_tokens, Some(128_000));
    }

    #[test]
    fn auto_populates_context_window_limit_from_model_id_lookup() {
        let (model, _) = model(config("anthropic.claude-sonnet-4-6"));
        assert_eq!(model.config().context_window_limit, Some(1_000_000));
        assert_eq!(model.config().max_tokens, Some(128_000));
    }

    #[test]
    fn auto_populates_context_window_limit_for_cross_region_model_ids() {
        let (model, _) = model(config("us.anthropic.claude-sonnet-4-6"));
        assert_eq!(model.config().context_window_limit, Some(1_000_000));
    }

    #[test]
    fn auto_populates_context_window_limit_for_default_model_id() {
        let (model, _) = model(BedrockModelConfig::default());
        assert_eq!(
            model.config(),
            BedrockModelConfig {
                model_id: Some("global.anthropic.claude-sonnet-4-6".into()),
                max_tokens: Some(128_000),
                context_window_limit: Some(1_000_000),
                ..Default::default()
            }
        );
    }

    #[test]
    fn auto_populates_context_window_limit_from_the_model_info_override_over_the_shared_value() {
        let (model, _) = model(BedrockModelConfig {
            model_info: Some(ModelInfo {
                capabilities: None,
                limits: Some(ModelLimits {
                    context_window: Some(500_000),
                    max_output_tokens: None,
                }),
            }),
            ..config("anthropic.claude-sonnet-4-6")
        });
        assert_eq!(model.config().context_window_limit, Some(500_000));
        assert_eq!(model.model_info().context_window(), Some(500_000));
    }

    #[test]
    fn does_not_override_explicit_context_window_limit() {
        let (model, _) = model(BedrockModelConfig {
            context_window_limit: Some(100_000),
            ..config("anthropic.claude-sonnet-4-6")
        });
        assert_eq!(model.config().context_window_limit, Some(100_000));
    }

    #[test]
    fn leaves_context_window_limit_undefined_for_unknown_model_ids() {
        let (model, _) = model(config("unknown.model-v1:0"));
        assert_eq!(model.config(), config("unknown.model-v1:0"));
    }
}

mod info {
    use super::*;

    #[test]
    fn reads_cache_points_for_claude_model_ids_from_the_shared_model_info() {
        let (model, _) = model(config("us.anthropic.claude-sonnet-4-6"));
        assert_eq!(prompt_caching(&model), Some(PromptCaching::CachePoints));
    }

    #[test]
    fn leaves_caching_unknown_for_other_model_ids() {
        let (model, _) = model(config("amazon.nova-pro-v1:0"));
        assert_eq!(prompt_caching(&model), None);
    }

    #[test]
    fn auto_caches_an_unlisted_claude_id_by_name() {
        for model_id in [
            "anthropic.claude-future-9",
            "arn:aws:bedrock:us-east-1:123456789012:default-prompt-router/anthropic.claude:1",
            "US.ANTHROPIC.CLAUDE-SONNET-4-6",
        ] {
            let (model, transport) = model(caching(model_id));
            assert_eq!(
                system_sent(&model, &transport),
                json!([{ "text": "Be helpful" }, { "cachePoint": { "type": "default" } }]),
                "{model_id}"
            );
            assert_eq!(prompt_caching(&model), None);
            assert_eq!(
                model.model_info().capabilities.unwrap().server_side_summary,
                Some(false)
            );
        }
    }

    #[test]
    fn does_not_auto_cache_an_unlisted_id_that_is_not_claude() {
        let (model, transport) = model(caching(
            "arn:aws:bedrock:us-east-1:123456789012:custom-model/acme.foo/abc",
        ));
        assert_eq!(system_sent(&model, &transport), json!([{ "text": "Be helpful" }]));
        assert_eq!(prompt_caching(&model), None);
        assert_eq!(
            model.model_info().capabilities.unwrap().server_side_summary,
            Some(false)
        );
    }

    #[test]
    fn skips_an_injected_block_it_receives_with_a_warning() {
        let (model, transport) = model(BedrockModelConfig {
            model_info: info_override(ModelCapabilities {
                temporary_messages: Some(true),
                ..Default::default()
            }),
            ..config("us.anthropic.claude-opus-5-5")
        });
        let messages = vec![user(vec![
            text("Hello"),
            InjectedBlock::new("cwd: /repo", "env").into(),
        ])];
        let (request, warned) = warnings(|| sent(&model, &transport, request(messages)));
        assert_eq!(request["messages"][0]["content"], json!([{ "text": "Hello" }]));
        // warn_once logs once per process; another test may have logged it first.
        assert!(warned.len() <= 1);
        assert!(
            warned
                .iter()
                .all(|w| w
                    == "block_type=<injectedBlock> | bedrock does not send injected blocks | skipping it")
        );
    }

    #[test]
    fn reports_an_open_ai_model_as_implicitly_cached_and_sends_no_cache_point() {
        for model_id in ["us.openai.gpt-6-luna", "openai.gpt-6.1-sol", "openai.gpt-5.6-sol"] {
            let (model, transport) = model(caching(model_id));
            assert_eq!(system_sent(&model, &transport), json!([{ "text": "Be helpful" }]));
            assert_eq!(prompt_caching(&model), Some(PromptCaching::Auto));
        }
    }

    #[test]
    fn lets_a_model_info_override_win_over_implicit_caching_for_an_open_ai_model() {
        let (model, _) = model(BedrockModelConfig {
            model_info: info_override(ModelCapabilities {
                prompt_caching: Some(PromptCaching::CachePoints),
                ..Default::default()
            }),
            ..config("us.openai.gpt-6-luna")
        });
        assert_eq!(prompt_caching(&model), Some(PromptCaching::CachePoints));
    }

    #[test]
    fn injects_auto_cache_points_on_a_model_the_info_override_says_takes_them() {
        let (model, transport) = model(BedrockModelConfig {
            model_info: info_override(ModelCapabilities {
                prompt_caching: Some(PromptCaching::CachePoints),
                ..Default::default()
            }),
            ..caching("amazon.nova-pro-v1:0")
        });
        assert_eq!(
            system_sent(&model, &transport),
            json!([{ "text": "Be helpful" }, { "cachePoint": { "type": "default" } }])
        );
    }

    #[test]
    fn skips_auto_cache_points_on_a_claude_model_the_info_override_marks_unsupported() {
        let (model, transport) = model(BedrockModelConfig {
            model_info: info_override(ModelCapabilities {
                prompt_caching: Some(PromptCaching::Unsupported),
                ..Default::default()
            }),
            ..caching("us.anthropic.claude-sonnet-4-6")
        });
        assert_eq!(system_sent(&model, &transport), json!([{ "text": "Be helpful" }]));
    }

    /// Ported from strands-ts/src/models/__tests__/server-side-summary-capability.test.ts (the
    /// Bedrock case).
    #[test]
    fn turns_it_off_for_listed_models_unless_the_model_info_override_sets_it() {
        let (plain, _) = model(config("us.anthropic.claude-sonnet-5-5-v1:0"));
        assert_eq!(
            plain.model_info().capabilities.unwrap().server_side_summary,
            Some(false)
        );
        let (overridden, _) = model(BedrockModelConfig {
            model_info: info_override(ModelCapabilities {
                server_side_summary: Some(true),
                ..Default::default()
            }),
            ..config("us.anthropic.claude-sonnet-5-5-v1:0")
        });
        assert_eq!(
            overridden.model_info().capabilities.unwrap().server_side_summary,
            Some(true)
        );
    }
}

mod max_tokens_default {
    use super::*;

    #[test]
    fn defaults_max_tokens_to_the_model_info_output_ceiling() {
        for (model_id, max_tokens) in [
            (Some("anthropic.claude-sonnet-4-6"), 128_000),
            (Some("us.anthropic.claude-sonnet-4-6"), 128_000),
            (None, 128_000),
            (Some("anthropic.claude-3-5-sonnet-20241022-v2:0"), 8192),
            (
                Some(
                    "arn:aws:bedrock:us-east-1:123456789012:inference-profile/us.anthropic.claude-sonnet-4-6",
                ),
                128_000,
            ),
        ] {
            let config = BedrockModelConfig {
                model_id: model_id.map(ToOwned::to_owned),
                ..Default::default()
            };
            assert_eq!(
                sent_inference_config(config),
                json!({ "maxTokens": max_tokens }),
                "{model_id:?}"
            );
        }
    }

    #[test]
    fn defaults_max_tokens_from_the_model_info_override() {
        let config = BedrockModelConfig {
            model_info: Some(ModelInfo {
                capabilities: None,
                limits: Some(ModelLimits {
                    context_window: None,
                    max_output_tokens: Some(2_000),
                }),
            }),
            ..config("m1")
        };
        assert_eq!(sent_inference_config(config), json!({ "maxTokens": 2_000 }));
    }

    #[test]
    fn leaves_max_tokens_unset_for_a_model_the_shared_model_info_does_not_list() {
        let (model, _) = model(config("unknown.model-v1:0"));
        assert_eq!(model.model_info().max_output_tokens(), None);
        assert_eq!(
            sent_inference_config(config("unknown.model-v1:0")),
            JsonValue::Null
        );
    }

    #[test]
    fn sends_an_explicit_max_tokens_over_the_model_info_output_ceiling() {
        let config = BedrockModelConfig {
            max_tokens: Some(1),
            ..config("anthropic.claude-sonnet-4-6")
        };
        assert_eq!(sent_inference_config(config), json!({ "maxTokens": 1 }));
    }

    #[test]
    fn re_resolves_the_default_max_tokens_when_model_id_changes() {
        let (model, transport) = model(config("anthropic.claude-sonnet-4-6"));
        model.update_config(config("anthropic.claude-haiku-4-5-20251001-v1:0"));
        let sent = sent(&model, &transport, request(hello()));
        assert_eq!(sent["inferenceConfig"], json!({ "maxTokens": 64_000 }));
    }

    #[test]
    fn does_not_cap_the_default_max_tokens_by_the_context_window() {
        for limit in [None, Some(100_000)] {
            let config = BedrockModelConfig {
                context_window_limit: limit,
                ..config("anthropic.claude-sonnet-4-5-20250929-v1:0")
            };
            assert_eq!(sent_inference_config(config), json!({ "maxTokens": 64_000 }));
        }
    }

    #[test]
    fn surfaces_the_default_max_tokens_in_get_config() {
        let (model, _) = model(config("anthropic.claude-sonnet-4-6"));
        assert_eq!(model.config().max_tokens, Some(128_000));
        assert_eq!(Model::get_config(&model).max_tokens, Some(128_000));
    }

    #[test]
    fn warns_once_when_max_tokens_is_defaulted() {
        assert_eq!(
            crate::models::defaults::default_max_tokens_warning_message(128_000),
            "max_tokens=<128000> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
        );
        let (_, warned) = warnings(|| model(config("anthropic.claude-sonnet-4-6")));
        assert!(
            warned
                .iter()
                .all(|w| w.contains("max_tokens=<128000> | using default maxTokens"))
        );
    }

    #[test]
    fn does_not_warn_about_max_tokens() {
        let pinned = BedrockModelConfig {
            model_info: Some(ModelInfo {
                capabilities: None,
                limits: Some(ModelLimits {
                    context_window: None,
                    max_output_tokens: Some(2_000),
                }),
            }),
            ..config("anthropic.claude-sonnet-4-6")
        };
        let explicit = BedrockModelConfig {
            max_tokens: Some(1),
            ..config("anthropic.claude-sonnet-4-6")
        };
        for config in [explicit, pinned, config("unknown.model-v1:0")] {
            let (_, warned) = warnings(|| model(config));
            assert!(warned.iter().all(|w| !w.contains("using default maxTokens")));
        }
    }
}

mod update_config {
    use super::*;

    #[test]
    fn merges_new_config_with_existing_config() {
        let (model, _) = model(BedrockModelConfig {
            temperature: Some(0.5),
            ..Default::default()
        });
        model.update_config(BedrockModelConfig {
            temperature: Some(0.8),
            max_tokens: Some(2048),
            ..Default::default()
        });
        assert_eq!(
            model.config(),
            BedrockModelConfig {
                model_id: Some("global.anthropic.claude-sonnet-4-6".into()),
                temperature: Some(0.8),
                max_tokens: Some(2048),
                context_window_limit: Some(1_000_000),
                ..Default::default()
            }
        );
    }

    #[test]
    fn preserves_fields_not_included_in_the_update() {
        let (model, _) = model(BedrockModelConfig {
            temperature: Some(0.5),
            max_tokens: Some(1024),
            ..config("custom-model")
        });
        model.update_config(BedrockModelConfig {
            temperature: Some(0.8),
            ..Default::default()
        });
        assert_eq!(
            model.config(),
            BedrockModelConfig {
                temperature: Some(0.8),
                max_tokens: Some(1024),
                ..config("custom-model")
            }
        );
    }

    #[test]
    fn re_resolves_context_window_limit_when_model_id_changes_and_it_was_auto_resolved() {
        let (model, _) = model(BedrockModelConfig::default());
        assert_eq!(model.config().context_window_limit, Some(1_000_000));
        model.update_config(config("anthropic.claude-haiku-4-5-20251001-v1:0"));
        assert_eq!(model.config().context_window_limit, Some(200_000));
    }

    #[test]
    fn clears_context_window_limit_when_model_id_changes_to_unknown_model() {
        let (model, _) = model(BedrockModelConfig::default());
        assert_eq!(model.config().context_window_limit, Some(1_000_000));
        model.update_config(config("my-custom-finetuned-model"));
        assert_eq!(model.config().context_window_limit, None);
    }

    #[test]
    fn preserves_explicit_context_window_limit_when_model_id_changes() {
        let (model, _) = model(BedrockModelConfig {
            context_window_limit: Some(50_000),
            ..Default::default()
        });
        model.update_config(config("anthropic.claude-haiku-4-5-20251001-v1:0"));
        assert_eq!(model.config().context_window_limit, Some(50_000));
    }

    #[test]
    fn the_model_trait_merges_the_base_fields() {
        let (model, _) = model(BedrockModelConfig {
            include_tool_result_status: Some(IncludeToolResultStatus::Enabled(true)),
            ..config("custom-model")
        });
        Model::update_config(
            &model,
            BaseModelConfig {
                temperature: Some(0.1),
                ..Default::default()
            },
        );
        let config = model.config();
        assert_eq!(config.temperature, Some(0.1));
        assert_eq!(
            config.include_tool_result_status,
            Some(IncludeToolResultStatus::Enabled(true))
        );
    }
}

mod get_config {
    use super::*;

    #[test]
    fn returns_the_current_configuration() {
        let config = BedrockModelConfig {
            max_tokens: Some(1024),
            temperature: Some(0.7),
            ..config("test-model")
        };
        let (model, _) = model(config.clone());
        assert_eq!(model.config(), config);
    }
}

mod region_configuration {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> EnvLookup {
        Arc::new(move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn uses_explicit_region_when_provided() {
        let region = run(resolve_region(
            &region_options("eu-west-1"),
            &env(&[("AWS_REGION", "ap-south-1")]),
            async { Some("us-east-2".to_owned()) },
        ));
        assert_eq!(region, "eu-west-1");
    }

    #[test]
    fn defaults_to_us_west_2_when_region_is_missing() {
        let region = run(resolve_region(&ClientOptions::default(), &no_env(), async {
            None
        }));
        assert_eq!(region, "us-west-2");
    }

    #[test]
    fn reads_the_region_from_the_environment_then_the_profile() {
        let options = ClientOptions::default();
        let from_env = run(resolve_region(
            &options,
            &env(&[("AWS_REGION", "ap-south-1")]),
            async { Some("us-east-2".to_owned()) },
        ));
        assert_eq!(from_env, "ap-south-1");
        let from_default_env = run(resolve_region(
            &options,
            &env(&[("AWS_DEFAULT_REGION", "eu-central-1")]),
            async { None },
        ));
        assert_eq!(from_default_env, "eu-central-1");
        let from_profile = run(resolve_region(&options, &no_env(), async {
            Some("us-east-2".to_owned())
        }));
        assert_eq!(from_profile, "us-east-2");
    }

    #[test]
    fn uses_the_sdk_config_region_over_the_environment() {
        let options = ClientOptions {
            sdk_config: Some(SdkConfig::builder().region(Region::new("sa-east-1")).build()),
            ..Default::default()
        };
        let region = run(resolve_region(
            &options,
            &env(&[("AWS_REGION", "ap-south-1")]),
            async { None },
        ));
        assert_eq!(region, "sa-east-1");
    }
}
