//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('countTokens')`).

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::models::model::CountTokensOptions;
use crate::types::messages::SystemPrompt;
use crate::types::tools::ToolSpec;

fn hello() -> Vec<Message> {
    vec![user("hello")]
}

fn native(client: &Arc<MockClient>, native: Option<bool>) -> AnthropicModel {
    injected(
        client,
        AnthropicModelConfig {
            use_native_token_count: native,
            ..model_id("claude-sonnet-4-6")
        },
    )
}

fn counting(tokens: u64) -> Arc<MockClient> {
    Arc::new(MockClient::empty().with_count_tokens(move |_| Ok(json!({ "input_tokens": tokens }))))
}

fn count(model: &AnthropicModel, options: CountTokensOptions) -> u64 {
    block_on(model.count_tokens(&hello(), options)).unwrap()
}

#[test]
fn should_use_heuristic_by_default_when_use_native_token_count_is_not_set() {
    let client = counting(1);
    // heuristic: ceil("hello".len() / 4)
    assert_eq!(count(&native(&client, None), CountTokensOptions::default()), 2);
    assert!(client.count_requests().is_empty());
}

#[test]
fn should_return_native_token_count_on_success() {
    let client = counting(42);
    assert_eq!(
        count(&native(&client, Some(true)), CountTokensOptions::default()),
        42
    );
    assert_eq!(client.count_requests().len(), 1);
}

#[test]
fn should_fall_back_to_the_heuristic_when_the_client_has_no_count_tokens() {
    let client = Arc::new(MockClient::empty());
    assert_eq!(
        count(&native(&client, Some(true)), CountTokensOptions::default()),
        2
    );
}

#[test]
fn should_include_system_prompt_in_request() {
    let client = counting(55);
    let options = CountTokensOptions {
        system_prompt: Some(SystemPrompt::from("Be helpful.")),
        ..Default::default()
    };
    assert_eq!(count(&native(&client, Some(true)), options), 55);
    assert_eq!(
        client.count_requests()[0].body,
        json!({
            "model": "claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello" }] }],
            "system": "Be helpful.",
        })
    );
}

#[test]
fn should_include_tool_specs_in_request() {
    let client = counting(100);
    let options = CountTokensOptions {
        tool_specs: vec![
            ToolSpec::new("test_tool", "A test tool")
                .with_input_schema(json!({ "type": "object", "properties": {} })),
        ],
        ..Default::default()
    };
    assert_eq!(count(&native(&client, Some(true)), options), 100);
    assert_eq!(
        client.count_requests()[0].body,
        json!({
            "model": "claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello" }] }],
            "tools": [{ "name": "test_tool", "description": "A test tool", "input_schema": { "type": "object", "properties": {} } }],
        })
    );
}

#[test]
fn should_strip_max_tokens_from_request() {
    let client = counting(10);
    count(&native(&client, Some(true)), CountTokensOptions::default());
    assert_eq!(
        client.count_requests()[0].body,
        json!({
            "model": "claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hello" }] }],
        })
    );
}

#[test]
fn should_fall_back_to_estimation_on_api_error() {
    let client = Arc::new(MockClient::empty().with_count_tokens(|_| Err(status_error(400, "Unsupported"))));
    assert_eq!(
        count(&native(&client, Some(true)), CountTokensOptions::default()),
        2
    );
}

#[test]
fn should_fall_back_to_estimation_on_generic_exception() {
    let client = Arc::new(MockClient::empty().with_count_tokens(|_| Err(plain_error("Connection failed"))));
    assert_eq!(
        count(&native(&client, Some(true)), CountTokensOptions::default()),
        2
    );
}

#[test]
fn should_skip_native_api_and_use_heuristic_when_use_native_token_count_is_false() {
    let client = counting(1);
    assert_eq!(
        count(&native(&client, Some(false)), CountTokensOptions::default()),
        2
    );
    assert!(client.count_requests().is_empty());
}
