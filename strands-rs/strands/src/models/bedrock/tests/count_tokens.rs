//! `countTokens` cases.

use serde_json::json;

use super::*;
use crate::models::model::CountTokensOptions;
use crate::types::messages::SystemPrompt;
use crate::types::tools::ToolSpec;

fn native() -> BedrockModelConfig {
    BedrockModelConfig {
        use_native_token_count: Some(true),
        ..config("global.anthropic.claude-sonnet-4-6")
    }
}

fn messages() -> Vec<Message> {
    vec![user(vec![text("hello")])]
}

fn count(model: &BedrockModel, options: CountTokensOptions) -> u64 {
    run(model.count_tokens(&messages(), options)).unwrap()
}

#[test]
fn should_use_heuristic_by_default_when_use_native_token_count_is_not_set() {
    let (model, transport) = model(BedrockModelConfig::default());
    assert_eq!(count(&model, CountTokensOptions::default()), 2);
    assert!(transport.count_tokens_requests().is_empty());
}

#[test]
fn should_return_native_token_count_on_success() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Ok(Some(42)));
    assert_eq!(count(&model, CountTokensOptions::default()), 42);
    assert_eq!(transport.count_tokens_requests().len(), 1);
}

#[test]
fn should_include_system_prompt_in_request() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Ok(Some(55)));
    let options = CountTokensOptions {
        system_prompt: Some(SystemPrompt::Text("Be helpful.".into())),
        ..Default::default()
    };
    assert_eq!(count(&model, options), 55);
    assert_eq!(
        transport.count_tokens_requests(),
        [(
            "global.anthropic.claude-sonnet-4-6".to_owned(),
            json!({
                "messages": [{ "role": "user", "content": [{ "text": "hello" }] }],
                "system": [{ "text": "Be helpful." }],
            })
        )]
    );
}

#[test]
fn should_include_tool_specs_in_request() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Ok(Some(100)));
    let options = CountTokensOptions {
        tool_specs: vec![
            ToolSpec::new("test_tool", "A test tool")
                .with_input_schema(json!({ "type": "object", "properties": {} })),
        ],
        ..Default::default()
    };
    assert_eq!(count(&model, options), 100);
    assert_eq!(
        transport.count_tokens_requests()[0].1,
        json!({
            "messages": [{ "role": "user", "content": [{ "text": "hello" }] }],
            "toolConfig": { "tools": [{ "toolSpec": {
                "name": "test_tool",
                "description": "A test tool",
                "inputSchema": { "json": { "type": "object", "properties": {} } },
            } }] },
        })
    );
}

#[test]
fn should_strip_inference_config_from_request() {
    let (model, transport) = model(BedrockModelConfig {
        max_tokens: Some(100),
        ..native()
    });
    count(&model, CountTokensOptions::default());
    assert_eq!(
        transport.count_tokens_requests()[0].1,
        json!({ "messages": [{ "role": "user", "content": [{ "text": "hello" }] }] })
    );
}

#[test]
fn should_fall_back_to_estimation_on_api_error() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Err(BedrockError::service("Error", "API error")));
    assert_eq!(count(&model, CountTokensOptions::default()), 2);
}

#[test]
fn should_fall_back_to_estimation_on_generic_exception() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Err(BedrockError::service("Error", "Connection failed")));
    assert_eq!(count(&model, CountTokensOptions::default()), 2);
}

#[test]
fn should_cache_model_id_and_skip_api_call_when_model_does_not_support_counting_tokens() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Err(BedrockError::service(
        "ValidationException",
        "The provided model doesn't support counting tokens",
    )));
    count(&model, CountTokensOptions::default());
    count(&model, CountTokensOptions::default());
    assert_eq!(transport.count_tokens_requests().len(), 1);
}

#[test]
fn should_cache_model_id_and_skip_api_call_on_access_denied_exception() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Err(BedrockError::service(
        "AccessDeniedException",
        "User: arn:aws:sts::123456789012:assumed-role/role is not authorized to perform: bedrock:CountTokens",
    )));
    warnings(|| count(&model, CountTokensOptions::default()));
    count(&model, CountTokensOptions::default());
    assert_eq!(transport.count_tokens_requests().len(), 1);
}

#[test]
fn should_not_cache_model_id_for_other_errors() {
    let (model, transport) = model(native());
    transport.count_tokens_reply(Err(BedrockError::service("Error", "Transient network error")));
    transport.count_tokens_reply(Err(BedrockError::service("Error", "Transient network error")));
    count(&model, CountTokensOptions::default());
    count(&model, CountTokensOptions::default());
    assert_eq!(transport.count_tokens_requests().len(), 2);
}

#[test]
fn should_skip_native_api_and_use_heuristic_when_use_native_token_count_is_false() {
    let (model, transport) = model(BedrockModelConfig {
        use_native_token_count: Some(false),
        ..config("global.anthropic.claude-sonnet-4-6")
    });
    assert_eq!(count(&model, CountTokensOptions::default()), 2);
    assert!(transport.count_tokens_requests().is_empty());
}
