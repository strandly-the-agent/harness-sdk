//! Ported from strands-ts/src/models/openai/__tests__/mantle.test.ts. The TS mocked `OpenAI`
//! constructor becomes the recording client factory and the mocked `getTokenProvider` a
//! recording token provider factory; the token shape is checked against tokens the vendor's
//! signer (`@smithy/signature-v4`, as `@aws/bedrock-token-generator` calls it) produced for the
//! same credentials and clock.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use aws_credential_types::Credentials;
use aws_credential_types::provider::SharedCredentialsProvider;
use futures::executor::block_on;
use serde_json::json;

use super::*;
use crate::models::openai::client::{HttpClientParts, KeySource};
use crate::models::openai::tests::{MockClient, Recorder, seams_with_env};
use crate::models::openai::{
    OpenAIApi, OpenAIClientConfig, OpenAIModel, OpenAIModelConfig, OpenAIModelOptions, Seams, Transport,
};

const TEST_MODEL_ID: &str = "openai.gpt-oss-120b";
const TEST_TOKEN: &str = "bedrock-api-key-deadbeef&Version=1";

/// Records the options of every token provider built and counts the tokens minted.
#[derive(Default)]
struct TokenRecorder {
    options: Mutex<Vec<TokenOptions>>,
    minted: Mutex<usize>,
    fail_with: Mutex<Option<String>>,
}

impl TokenRecorder {
    fn factory(self: &Arc<Self>) -> TokenProviderFactory {
        let recorder = Arc::clone(self);
        Arc::new(move |options| {
            recorder.options.lock().unwrap().push(options);
            let recorder = Arc::clone(&recorder);
            let provider: TokenProvider = Arc::new(move || {
                *recorder.minted.lock().unwrap() += 1;
                let failure = recorder.fail_with.lock().unwrap().clone();
                Box::pin(async move {
                    match failure {
                        Some(message) => Err(message.into()),
                        None => Ok(TEST_TOKEN.to_owned()),
                    }
                })
            });
            Ok(provider)
        })
    }

    fn regions(&self) -> Vec<String> {
        self.options
            .lock()
            .unwrap()
            .iter()
            .map(|options| options.region.clone())
            .collect()
    }
}

struct Harness {
    clients: Arc<Recorder>,
    tokens: Arc<TokenRecorder>,
}

impl Harness {
    fn new() -> Self {
        Self {
            clients: Arc::new(Recorder::default()),
            tokens: Arc::new(TokenRecorder::default()),
        }
    }

    fn build_with_env(
        &self,
        options: OpenAIModelOptions,
        env: &[(&str, &str)],
    ) -> crate::Result<OpenAIModel> {
        let transport = Transport::Built(self.clients.factory(MockClient::new(json!([]))));
        let seams = Seams {
            token_provider: self.tokens.factory(),
            ..seams_with_env(transport, env)
        };
        OpenAIModel::with_seams(options, seams)
    }

    fn build(&self, options: OpenAIModelOptions) -> crate::Result<OpenAIModel> {
        self.build_with_env(options, &[])
    }

    fn last(&self) -> HttpClientParts {
        self.clients.last()
    }

    fn count(&self) -> usize {
        self.clients.built().len()
    }
}

fn mantle(region: Option<&str>) -> BedrockMantleConfig {
    BedrockMantleConfig {
        region: region.map(str::to_owned),
        ..Default::default()
    }
}

fn options(model_id: &str, config: BedrockMantleConfig) -> OpenAIModelOptions {
    OpenAIModelOptions {
        config: OpenAIModelConfig {
            model_id: Some(model_id.into()),
            ..Default::default()
        },
        bedrock_mantle_config: Some(config),
        ..Default::default()
    }
}

fn setter_of(parts: &HttpClientParts) -> crate::models::openai::ApiKeySetter {
    match &parts.key {
        KeySource::Setter(setter) => Arc::clone(setter),
        KeySource::Static(_) => panic!("the Mantle client takes a key setter"),
    }
}

fn mint(parts: &HttpClientParts) -> Result<String, BoxError> {
    block_on(setter_of(parts)())
}

mod constructor_wiring {
    use super::*;

    #[test]
    fn sets_base_url_and_installs_async_api_key_setter_that_mints_a_bearer_token() {
        let harness = Harness::new();
        harness
            .build(options(TEST_MODEL_ID, mantle(Some("us-east-1"))))
            .unwrap();
        let parts = harness.last();
        assert_eq!(parts.base_url, "https://bedrock-mantle.us-east-1.api.aws/v1");
        assert_eq!(mint(&parts).unwrap(), TEST_TOKEN);
        assert_eq!(harness.tokens.regions(), ["us-east-1"]);
        let options = harness.tokens.options.lock().unwrap()[0].clone();
        assert!(options.credentials.is_none());
        assert_eq!(options.expires_in_seconds, None);
    }

    #[test]
    fn forwards_optional_credentials_and_expires_in_seconds_to_get_token_provider() {
        let harness = Harness::new();
        let credentials = SharedCredentialsProvider::new(Credentials::for_tests());
        harness
            .build(options(
                TEST_MODEL_ID,
                BedrockMantleConfig {
                    region: Some("us-west-2".into()),
                    credentials: Some(credentials),
                    expires_in_seconds: Some(900),
                },
            ))
            .unwrap();
        mint(&harness.last()).unwrap();
        let options = harness.tokens.options.lock().unwrap()[0].clone();
        assert_eq!(options.region, "us-west-2");
        assert!(options.credentials.is_some());
        assert_eq!(options.expires_in_seconds, Some(900));
    }

    #[test]
    fn mints_a_fresh_token_on_every_api_key_setter_call() {
        let harness = Harness::new();
        harness
            .build(options(TEST_MODEL_ID, mantle(Some("us-east-1"))))
            .unwrap();
        let parts = harness.last();
        for _ in 0..3 {
            mint(&parts).unwrap();
        }
        assert_eq!(harness.tokens.options.lock().unwrap().len(), 1);
        assert_eq!(*harness.tokens.minted.lock().unwrap(), 3);
    }

    #[test]
    fn merges_with_other_client_config_fields_while_overriding_base_url_and_api_key() {
        let harness = Harness::new();
        let mut default_headers = http::HeaderMap::new();
        default_headers.insert("x-trace-id", http::HeaderValue::from_static("abc"));
        harness
            .build(OpenAIModelOptions {
                client_config: OpenAIClientConfig {
                    timeout: Some(Duration::from_millis(42)),
                    default_headers,
                    ..Default::default()
                },
                ..options(TEST_MODEL_ID, mantle(Some("us-east-1")))
            })
            .unwrap();
        let parts = harness.last();
        assert_eq!(parts.base_url, "https://bedrock-mantle.us-east-1.api.aws/v1");
        assert!(matches!(parts.key, KeySource::Setter(_)));
        assert_eq!(parts.timeout, Duration::from_millis(42));
        assert_eq!(parts.headers.get("x-trace-id").unwrap(), "abc");
    }

    #[test]
    fn does_not_check_openai_api_key_when_bedrock_mantle_config_is_set() {
        let harness = Harness::new();
        let model = harness.build_with_env(
            options(TEST_MODEL_ID, mantle(Some("us-east-1"))),
            &[("OPENAI_API_KEY", "")],
        );
        assert!(model.is_ok());
    }

    #[test]
    fn mints_a_bearer_token_regardless_of_api_mode() {
        let harness = Harness::new();
        harness
            .build(OpenAIModelOptions {
                api: OpenAIApi::Chat,
                ..options(TEST_MODEL_ID, mantle(Some("us-east-1")))
            })
            .unwrap();
        assert_eq!(mint(&harness.last()).unwrap(), TEST_TOKEN);
    }
}

mod base_path_resolution_by_model_family {
    use super::*;

    fn base_url_for(api: OpenAIApi, model_id: &str) -> String {
        let harness = Harness::new();
        harness
            .build(OpenAIModelOptions {
                api,
                ..options(model_id, mantle(Some("us-west-2")))
            })
            .unwrap();
        harness.last().base_url
    }

    const OPENAI_V1: &str = "https://bedrock-mantle.us-west-2.api.aws/openai/v1";
    const V1: &str = "https://bedrock-mantle.us-west-2.api.aws/v1";

    #[test]
    fn uses_openai_v1_for_gpt_5_on_the_responses_api() {
        assert_eq!(
            base_url_for(OpenAIApi::Responses, "openai.gpt-5.6-luna"),
            OPENAI_V1
        );
    }

    #[test]
    fn uses_openai_v1_for_gpt_5_on_the_chat_api() {
        assert_eq!(base_url_for(OpenAIApi::Chat, "openai.gpt-5.6-luna"), OPENAI_V1);
    }

    #[test]
    fn uses_v1_for_gpt_oss_on_the_responses_api() {
        assert_eq!(base_url_for(OpenAIApi::Responses, "openai.gpt-oss-120b"), V1);
    }

    #[test]
    fn uses_v1_for_gpt_oss_on_the_chat_api() {
        assert_eq!(base_url_for(OpenAIApi::Chat, "openai.gpt-oss-120b"), V1);
    }

    #[test]
    fn matches_only_bedrock_style_ids_openai_gpt_5_openai_v1_bare_gpt_5_v1() {
        assert_eq!(base_url_for(OpenAIApi::Responses, "gpt-5.4"), V1);
    }

    #[test]
    fn routes_s_to_s_on_both_api_surfaces() {
        let cases = [
            ("xai.grok-4.3", "/openai/v1"),
            ("google.gemma-4-31b", "/openai/v1"),
            ("google.gemma-4-26b-a4b", "/openai/v1"),
            ("google.gemma-4-e2b", "/openai/v1"),
            ("openai.gpt-5.6-terra", "/openai/v1"),
            ("openai.gpt-5.6-cyber", "/openai/v1"),
            ("openai.gpt-daybreak-blue-5.6-sol", "/openai/v1"),
            ("openai.gpt-6-astra", "/openai/v1"),
            ("openai.gpt-6.1-sol", "/openai/v1"),
            ("google.gemma-3-27b-it", "/v1"),
            ("google.gemma-3-4b-it", "/v1"),
            ("openai.gpt-oss-120b", "/v1"),
            ("openai.gpt-oss-safeguard-20b", "/v1"),
            ("qwen.qwen3-32b", "/v1"),
            ("deepseek.v3.2", "/v1"),
            ("mistral.ministral-3-8b-instruct", "/v1"),
            ("zai.glm-5", "/v1"),
            ("moonshotai.kimi-k2.5", "/v1"),
            ("minimax.minimax-m2", "/v1"),
            ("nvidia.nemotron-nano-9b-v2", "/v1"),
            ("writer.palmyra-vision-7b", "/v1"),
        ];
        for (model_id, path) in cases {
            let url = format!("https://bedrock-mantle.us-west-2.api.aws{path}");
            assert_eq!(base_url_for(OpenAIApi::Responses, model_id), url, "{model_id}");
            assert_eq!(base_url_for(OpenAIApi::Chat, model_id), url, "{model_id}");
        }
    }

    #[test]
    fn resolves_unverified_s_to_s() {
        let cases = [
            ("xai.grok-4.9", "/openai/v1"),
            ("openai.gpt-5.9-unreleased", "/openai/v1"),
            ("openai.gpt-6-nova", "/openai/v1"),
            ("openai.gpt-6.1-sol", "/openai/v1"),
            ("xai.grok-5", "/v1"),
            ("xai.grok-5-preview", "/v1"),
            ("openai.gpt-6oss-20b", "/v1"),
        ];
        for (model_id, path) in cases {
            assert_eq!(
                base_url_for(OpenAIApi::Responses, model_id),
                format!("https://bedrock-mantle.us-west-2.api.aws{path}"),
                "{model_id}"
            );
        }
    }

    fn model_id(id: &str) -> OpenAIModelConfig {
        OpenAIModelConfig {
            model_id: Some(id.into()),
            ..Default::default()
        }
    }

    #[test]
    fn reroutes_the_mantle_base_url_when_update_config_moves_model_id_across_base_paths() {
        let harness = Harness::new();
        let model = harness
            .build(options("openai.gpt-oss-120b", mantle(Some("us-west-2"))))
            .unwrap();
        assert_eq!(harness.last().base_url, V1);
        let before = harness.count();
        model.update_config(model_id("xai.grok-4.3"));
        assert_eq!(model.config().model_id.as_deref(), Some("xai.grok-4.3"));
        assert_eq!(harness.count(), before + 1);
        assert_eq!(harness.last().base_url, OPENAI_V1);
    }

    #[test]
    fn reroutes_through_the_model_trait_update_config_too() {
        let harness = Harness::new();
        let model = harness
            .build(options("openai.gpt-oss-120b", mantle(Some("us-west-2"))))
            .unwrap();
        crate::models::model::Model::update_config(
            &model,
            crate::models::model::BaseModelConfig {
                model_id: Some("openai.gpt-6.1-sol".into()),
                ..Default::default()
            },
        );
        assert_eq!(harness.last().base_url, OPENAI_V1);
    }

    #[test]
    fn reuses_the_construction_time_api_key_setter_when_rerouting() {
        let harness = Harness::new();
        let model = harness
            .build(options("openai.gpt-oss-120b", mantle(Some("us-west-2"))))
            .unwrap();
        let original = setter_of(&harness.last());
        model.update_config(model_id("xai.grok-4.3"));
        assert!(Arc::ptr_eq(&original, &setter_of(&harness.last())));
    }

    #[test]
    fn keeps_the_existing_client_when_the_new_model_id_stays_on_the_same_base_path() {
        let harness = Harness::new();
        let model = harness
            .build(options("openai.gpt-oss-120b", mantle(Some("us-west-2"))))
            .unwrap();
        let before = harness.count();
        model.update_config(model_id("qwen.qwen3-32b"));
        assert_eq!(model.config().model_id.as_deref(), Some("qwen.qwen3-32b"));
        assert_eq!(harness.count(), before);
        assert_eq!(harness.last().base_url, V1);
    }

    #[test]
    fn keeps_the_existing_client_when_update_config_leaves_model_id_alone() {
        let harness = Harness::new();
        let model = harness
            .build(options("openai.gpt-oss-120b", mantle(Some("us-west-2"))))
            .unwrap();
        let before = harness.count();
        model.update_config(OpenAIModelConfig {
            temperature: Some(0.5),
            ..Default::default()
        });
        assert_eq!(harness.count(), before);
    }

    #[test]
    fn does_not_touch_the_client_when_bedrock_mantle_config_is_not_used() {
        let harness = Harness::new();
        let model = harness
            .build_with_env(
                OpenAIModelOptions {
                    config: model_id("gpt-5.4"),
                    ..Default::default()
                },
                &[("OPENAI_API_KEY", "sk")],
            )
            .unwrap();
        let before = harness.count();
        model.update_config(model_id("openai.gpt-oss-120b"));
        assert_eq!(harness.count(), before);
    }
}

mod validation {
    use super::*;

    fn failure(options: OpenAIModelOptions) -> String {
        Harness::new().build(options).unwrap_err().to_string()
    }

    #[test]
    fn throws_when_bedrock_mantle_config_is_combined_with_a_pre_built_client() {
        let seams = seams_with_env(Transport::Injected(MockClient::new(json!([]))), &[]);
        let error =
            OpenAIModel::with_seams(options(TEST_MODEL_ID, mantle(Some("us-east-1"))), seams).unwrap_err();
        assert_eq!(
            error.to_string(),
            "'bedrock_mantle_config' cannot be combined with a pre-built 'client'."
        );
    }

    #[test]
    fn throws_when_client_config_base_url_is_set_alongside_bedrock_mantle_config() {
        let message = failure(OpenAIModelOptions {
            client_config: OpenAIClientConfig {
                base_url: Some("https://example.invalid".into()),
                ..Default::default()
            },
            ..options(TEST_MODEL_ID, mantle(Some("us-east-1")))
        });
        assert_eq!(
            message,
            "client_config must not contain base_url when bedrock_mantle_config is set; these are derived from the Mantle config automatically."
        );
    }

    #[test]
    fn throws_when_client_config_api_key_is_set_alongside_bedrock_mantle_config() {
        let message = failure(OpenAIModelOptions {
            client_config: OpenAIClientConfig {
                api_key: Some("sk-nope".into()),
                ..Default::default()
            },
            ..options(TEST_MODEL_ID, mantle(Some("us-east-1")))
        });
        assert!(
            message.starts_with("client_config must not contain api_key "),
            "{message}"
        );
    }

    #[test]
    fn throws_when_top_level_api_key_is_set_alongside_bedrock_mantle_config() {
        let message = failure(OpenAIModelOptions {
            api_key: Some("sk-nope".into()),
            ..options(TEST_MODEL_ID, mantle(Some("us-east-1")))
        });
        assert_eq!(
            message,
            "'api_key' cannot be combined with 'bedrock_mantle_config'; the API key is derived from the Mantle config automatically."
        );
    }
}

mod region_resolution {
    use super::*;

    #[test]
    fn throws_when_no_region_is_available_from_config_or_env() {
        let error = Harness::new()
            .build(options(TEST_MODEL_ID, mantle(None)))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "could not resolve an AWS region for Bedrock Mantle. Pass 'region' in bedrock_mantle_config or set AWS_REGION in the environment. See https://docs.aws.amazon.com/bedrock/latest/userguide/inference-openai.html for supported regions."
        );
    }

    #[test]
    fn falls_back_to_aws_region_env_var() {
        let harness = Harness::new();
        harness
            .build_with_env(
                options(TEST_MODEL_ID, mantle(None)),
                &[("AWS_REGION", "eu-west-1")],
            )
            .unwrap();
        mint(&harness.last()).unwrap();
        assert_eq!(
            harness.last().base_url,
            "https://bedrock-mantle.eu-west-1.api.aws/v1"
        );
        assert_eq!(harness.tokens.regions(), ["eu-west-1"]);
    }

    #[test]
    fn falls_back_to_aws_default_region_when_aws_region_is_unset() {
        let harness = Harness::new();
        harness
            .build_with_env(
                options(TEST_MODEL_ID, mantle(None)),
                &[("AWS_REGION", ""), ("AWS_DEFAULT_REGION", "ap-southeast-2")],
            )
            .unwrap();
        mint(&harness.last()).unwrap();
        assert_eq!(harness.tokens.regions(), ["ap-southeast-2"]);
    }

    #[test]
    fn prefers_explicit_region_over_env_vars() {
        let harness = Harness::new();
        harness
            .build_with_env(
                options(TEST_MODEL_ID, mantle(Some("us-east-1"))),
                &[("AWS_REGION", "eu-west-1")],
            )
            .unwrap();
        mint(&harness.last()).unwrap();
        assert_eq!(harness.tokens.regions(), ["us-east-1"]);
    }

    #[test]
    fn rejects_a_malformed_region_j_before_building_the_endpoint_url() {
        for region in [
            "x@attacker.com/",
            "us-east-1\n",
            "us-east-1/",
            "US-EAST-1",
            "useast1",
            "us-east-١",
            "us-éast-1",
        ] {
            let harness = Harness::new();
            let error = harness
                .build(options(TEST_MODEL_ID, mantle(Some(region))))
                .unwrap_err();
            assert_eq!(error.to_string(), format!("invalid AWS region: '{region}'"));
            assert_eq!(harness.count(), 0);
            assert!(harness.tokens.options.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn accepts_the_well_formed_region_j() {
        for region in ["us-east-1", "ap-southeast-1", "us-gov-east-1"] {
            assert!(
                Harness::new()
                    .build(options(TEST_MODEL_ID, mantle(Some(region))))
                    .is_ok(),
                "{region}"
            );
        }
    }

    #[test]
    fn rejects_a_malformed_region_supplied_via_aws_region() {
        let error = Harness::new()
            .build_with_env(
                options(TEST_MODEL_ID, mantle(None)),
                &[("AWS_REGION", "x@attacker.com/")],
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "invalid AWS region: 'x@attacker.com/'");
    }
}

mod token_minting_errors {
    use super::*;

    #[test]
    fn wraps_token_provider_failures_with_actionable_context() {
        let harness = Harness::new();
        *harness.tokens.fail_with.lock().unwrap() = Some("no credentials in chain".into());
        harness
            .build(options(TEST_MODEL_ID, mantle(Some("us-east-1"))))
            .unwrap();
        let error = mint(&harness.last()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "failed to mint Bedrock Mantle bearer token for region 'us-east-1' | verify your AWS credentials and network connectivity"
        );
        assert_eq!(
            std::error::Error::source(error.as_ref()).unwrap().to_string(),
            "no credentials in chain"
        );
    }

    #[test]
    fn rejects_a_lifetime_outside_the_token_generator_range() {
        for seconds in [0, 43_201] {
            let options = TokenOptions {
                region: "us-west-2".into(),
                credentials: None,
                expires_in_seconds: Some(seconds),
            };
            let error = sigv4_token_provider(options, Arc::new(SystemTime::now))
                .err()
                .unwrap();
            assert_eq!(
                error.to_string(),
                "ExpiresInSeconds must be in the range (0, 43200] seconds."
            );
        }
    }
}

mod bearer_token {
    use super::*;

    fn fixed_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_744_000)
    }

    fn credentials(session_token: Option<&str>) -> Credentials {
        Credentials::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token.map(str::to_owned),
            None,
            "test",
        )
    }

    fn decoded(token: &str) -> String {
        let encoded = token.strip_prefix("bedrock-api-key-").expect("the token prefix");
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(encoded).unwrap()).unwrap()
    }

    #[test]
    fn matches_the_token_generator_for_long_term_credentials() {
        let token = create_token(
            &credentials(None),
            "us-west-2",
            Duration::from_secs(43_200),
            fixed_time(),
        )
        .unwrap();
        assert_eq!(
            decoded(&token),
            "bedrock.amazonaws.com/?Action=CallWithBearerToken&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20261115%2Fus-west-2%2Fbedrock%2Faws4_request&X-Amz-Date=20261115T120000Z&X-Amz-Expires=43200&X-Amz-Signature=8a77d5b737b868a0757893468c2e1454b55cfca2d1b5b722c1fbf99a537d603e&X-Amz-SignedHeaders=host&Version=1"
        );
    }

    #[test]
    fn matches_the_token_generator_for_session_credentials() {
        let token = create_token(
            &credentials(Some("session/token+value=")),
            "us-west-2",
            Duration::from_secs(900),
            fixed_time(),
        )
        .unwrap();
        assert_eq!(
            token,
            "bedrock-api-key-YmVkcm9jay5hbWF6b25hd3MuY29tLz9BY3Rpb249Q2FsbFdpdGhCZWFyZXJUb2tlbiZYLUFtei1BbGdvcml0aG09QVdTNC1ITUFDLVNIQTI1NiZYLUFtei1DcmVkZW50aWFsPUFLSURFWEFNUExFJTJGMjAyNjExMTUlMkZ1cy13ZXN0LTIlMkZiZWRyb2NrJTJGYXdzNF9yZXF1ZXN0JlgtQW16LURhdGU9MjAyNjExMTVUMTIwMDAwWiZYLUFtei1FeHBpcmVzPTkwMCZYLUFtei1TZWN1cml0eS1Ub2tlbj1zZXNzaW9uJTJGdG9rZW4lMkJ2YWx1ZSUzRCZYLUFtei1TaWduYXR1cmU9NzU1MGQ2YTBjMDA3ZmEyODEyZDYxNWU2MjFlMDUwODVmMzkwNWRkNmMzYjc3M2RkNjQ3ZjI5ZjMyMTY1NmM1OSZYLUFtei1TaWduZWRIZWFkZXJzPWhvc3QmVmVyc2lvbj0x"
        );
    }

    #[tokio::test]
    async fn signs_with_configured_credentials_for_twelve_hours_by_default() {
        let options = TokenOptions {
            region: "us-west-2".into(),
            credentials: Some(SharedCredentialsProvider::new(credentials(None))),
            expires_in_seconds: None,
        };
        let provider = sigv4_token_provider(options, Arc::new(fixed_time)).unwrap();
        let token = provider().await.unwrap();
        assert!(decoded(&token).contains("&X-Amz-Expires=43200&"));
        assert!(decoded(&token).ends_with("&Version=1"));
    }
}
