//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('AnthropicModel server-side summary')`).
//!
//! TS spies on `Model.prototype.summarize`; here the base summary runs for real against the mock
//! client's stream, which answers with a `client-side` text reply.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::models::model::CountTokensOptions;
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::models::streaming::Usage;
use crate::models::summarize::{SummarizeOptions, SummarizeResult};
use crate::types::messages::{ContentBlock, SummaryBlock, SystemPrompt};
use crate::types::tools::ToolSpec;

fn stored_compaction(text: &str, signature: &str) -> ContentBlock {
    SummaryBlock::new(text)
        .with_provider("anthropic")
        .with_signature(signature)
        .into()
}

/// A client whose beta stream ends with `response` and whose plain stream is the base summary reply.
fn compaction_client(response: JsonValue) -> Arc<MockClient> {
    Arc::new(MockClient::new(text_response("client-side")).with_beta(move |_| Ok(response.clone())))
}

fn failing_compaction(error: ApiError) -> Arc<MockClient> {
    Arc::new(MockClient::new(text_response("client-side")).with_beta(move |_| Err(error.clone())))
}

fn compaction_config() -> AnthropicModelConfig {
    AnthropicModelConfig {
        max_tokens: Some(1),
        stop_sequences: Some(vec!["STOP".to_owned()]),
        ..model_id("claude-sonnet-5-5")
    }
}

fn compaction_model(client: &Arc<MockClient>) -> AnthropicModel {
    injected(client, compaction_config())
}

fn hi() -> Vec<Message> {
    vec![user("hi")]
}

fn summarize(
    model: &AnthropicModel,
    messages: Vec<Message>,
    options: SummarizeOptions,
) -> crate::error::Result<SummarizeResult> {
    block_on(model.summarize(messages, options))
}

fn compaction_usage() -> JsonValue {
    json!({ "input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 5 })
}

fn formatted_compaction_usage() -> Usage {
    Usage::new(100, 20, 155)
        .with_cache_read_input_tokens(30)
        .with_cache_write_input_tokens(5)
}

/// The base summary of the mock reply (TS `baseResult`).
fn base_content() -> Vec<ContentBlock> {
    vec![SummaryBlock::new("client-side").into()]
}

#[test]
fn sends_a_compaction_request_and_returns_the_summary_block() {
    let client = compaction_client(json!({
        "stop_reason": "compaction",
        "content": [{ "type": "compaction", "content": "the summary", "signature": "s1" }],
    }));
    let messages = vec![user("hi"), assistant("yo")];
    let model = compaction_model(&client);
    model
        .update_config(AnthropicModelConfig {
            params: json!({ "context_management": { "edits": [] } })
                .as_object()
                .cloned(),
            ..Default::default()
        })
        .unwrap();
    let options = SummarizeOptions {
        system_prompt: Some(SystemPrompt::from("sys")),
        tool_specs: vec![ToolSpec::new("read", "Read a file").with_input_schema(json!({ "type": "object" }))],
        instructions: Some("keep ids".to_owned()),
        ..Default::default()
    };
    let result = summarize(&model, messages, options).unwrap();
    assert_eq!(
        client.beta_requests(),
        [ApiRequest {
            body: json!({
                "model": "claude-sonnet-5-5",
                "max_tokens": 1,
                "stream": true,
                "system": "sys",
                "tools": [{ "name": "read", "description": "Read a file", "input_schema": { "type": "object" } }],
                "messages": [
                    { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                    { "role": "assistant", "content": [{ "type": "text", "text": "yo" }] },
                ],
                "compaction": { "type": "summarize", "instructions": "keep ids" },
            }),
            beta: Some("compact-2026-09-04".to_owned()),
        }]
    );
    assert_eq!(
        result,
        SummarizeResult::new(vec![stored_compaction("the summary", "s1")])
    );
}

#[test]
fn maps_from_the_compaction_request_like_stream() {
    let cases: [(ApiError, ErrorCheck); 2] = [
        (status_error(429, "rate limited"), |error| {
            matches!(error, Error::ModelThrottled(_))
        }),
        (plain_error("prompt is too long: 300000 tokens"), |error| {
            matches!(error, Error::ContextWindowOverflow(_))
        }),
    ];
    for (error, expected) in cases {
        let client = failing_compaction(error);
        let failure = summarize(&compaction_model(&client), hi(), SummarizeOptions::default()).unwrap_err();
        assert!(expected(&failure), "{failure:?}");
    }
}

#[test]
fn returns_the_compaction_usage() {
    let client = compaction_client(json!({
        "stop_reason": "compaction",
        "content": [{ "type": "compaction", "content": "the summary" }],
        "usage": compaction_usage(),
    }));
    let result = summarize(&compaction_model(&client), hi(), SummarizeOptions::default()).unwrap();
    let summary = SummaryBlock::new("the summary").with_provider("anthropic");
    assert_eq!(
        result,
        SummarizeResult::new(vec![summary.into()]).with_usage(formatted_compaction_usage())
    );
}

#[test]
fn counts_the_compaction_iterations_usage() {
    let mut iteration = compaction_usage();
    iteration["type"] = json!("compaction");
    let client = compaction_client(json!({
        "stop_reason": "compaction",
        "content": [{ "type": "compaction", "content": "the summary" }],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_read_input_tokens": 0,
            "cache_creation_input_tokens": 0,
            "iterations": [iteration],
        },
    }));
    let result = summarize(&compaction_model(&client), hi(), SummarizeOptions::default()).unwrap();
    let summary = SummaryBlock::new("the summary").with_provider("anthropic");
    assert_eq!(
        result,
        SummarizeResult::new(vec![summary.into()]).with_usage(formatted_compaction_usage())
    );
}

#[test]
fn uses_the_base_implementation_without_a_compaction_block() {
    let responses = [
        json!({ "stop_reason": "end_turn", "content": [] }),
        json!({ "stop_reason": "end_turn", "content": [{ "type": "text", "text": "x" }] }),
    ];
    for mut response in responses {
        response["usage"] = compaction_usage();
        let client = compaction_client(response);
        let options = SummarizeOptions {
            instructions: Some("keep ids".to_owned()),
            ..Default::default()
        };
        let (result, _) = warnings(|| summarize(&compaction_model(&client), hi(), options));
        // The base summary sends the conversation with the instructions appended.
        let requests = client.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].body["messages"].to_string().contains("keep ids"));
        assert_eq!(
            result.unwrap(),
            SummarizeResult::new(base_content()).with_usage(
                Usage::new(110, 25, 170)
                    .with_cache_read_input_tokens(30)
                    .with_cache_write_input_tokens(5)
            )
        );
    }
}

#[test]
fn uses_the_base_implementation_when_compaction_fails() {
    let errors = [
        api_status_error(400, "400 does not support the compaction strategy"),
        api_status_error(529, "529 overloaded"),
        ApiError::new(ApiErrorKind::Timeout, "Request timed out."),
        ApiError::new(ApiErrorKind::Connection, "connection"),
    ];
    for error in errors {
        let client = failing_compaction(error);
        let (result, warnings) =
            warnings(|| summarize(&compaction_model(&client), hi(), SummarizeOptions::default()));
        assert_eq!(
            result.unwrap(),
            SummarizeResult::new(base_content()).with_usage(Usage::new(10, 5, 15))
        );
        assert!(warned(
            &warnings,
            "compaction request failed | summarizing client-side"
        ));
    }
}

#[test]
fn sends_no_client_side_request_after_a_cancel_during_compaction() {
    let (signal, handle) = CancelSignal::new();
    let client = Arc::new(MockClient::new(text_response("client-side")).with_beta(move |_| {
        handle.cancel(CancelOptions::default());
        Err(ApiError::new(ApiErrorKind::Connection, "aborted"))
    }));
    let options = SummarizeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    let (result, _) = warnings(|| summarize(&compaction_model(&client), hi(), options));
    assert_eq!(
        result.unwrap(),
        SummarizeResult::new(Vec::new()).with_usage(Usage::default())
    );
    assert!(client.requests().is_empty());
}

#[test]
fn throws_errors_other_than_an_api_error() {
    let client = failing_compaction(plain_error("overloaded"));
    let error = summarize(&compaction_model(&client), hi(), SummarizeOptions::default()).unwrap_err();
    assert!(error.to_string().contains("overloaded"));
    assert!(client.requests().is_empty());
}

#[test]
fn uses_the_base_implementation_for_a_client_without_the_beta_messages_api() {
    let client = Arc::new(MockClient::new(text_response("client-side")));
    let (result, warnings) =
        warnings(|| summarize(&compaction_model(&client), hi(), SummarizeOptions::default()));
    assert_eq!(result.unwrap().content(), base_content());
    assert!(warned(&warnings, "client has no beta messages API"));
}

#[test]
fn uses_the_base_implementation_without_the_capability() {
    let turned_off = AnthropicModelConfig {
        model_info: Some(ModelInfo {
            capabilities: Some(ModelCapabilities {
                server_side_summary: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..model_id("claude-sonnet-5-5")
    };
    for config in [model_id("claude-sonnet-4-5"), turned_off] {
        let client = compaction_client(json!({}));
        let model = injected(
            &client,
            AnthropicModelConfig {
                max_tokens: Some(1),
                ..config
            },
        );
        let options = SummarizeOptions {
            system_prompt: Some(SystemPrompt::from("sys")),
            instructions: Some("keep ids".to_owned()),
            ..Default::default()
        };
        let result = summarize(&model, hi(), options).unwrap();
        assert_eq!(result.content(), base_content());
        assert_eq!(client.requests().len(), 1);
        assert_eq!(client.requests()[0].body["system"], json!("sys"));
        assert!(client.beta_requests().is_empty());
    }
}

#[test]
fn sends_messages_from_the_last_compaction_block() {
    let client = compaction_client(json!({}));
    let messages = vec![
        user("first"),
        Message::new(Role::Assistant, vec![stored_compaction("old", "sig")]),
        user("more"),
        Message::new(
            Role::Assistant,
            vec![stored_compaction("new", "s2"), TextBlock::new("after").into()],
        ),
        user("next"),
    ];
    collect(&compaction_model(&client), request(messages));
    assert_eq!(
        client.last_body()["messages"],
        json!([
            {
                "role": "assistant",
                "content": [
                    { "type": "compaction", "content": "new", "signature": "s2" },
                    { "type": "text", "text": "after" },
                ],
            },
            { "role": "user", "content": [{ "type": "text", "text": "next" }] },
        ])
    );
    assert_eq!(client.last_beta().as_deref(), Some("compact-2026-09-04"));
}

#[test]
fn counts_tokens_from_the_last_compaction_block() {
    let client = Arc::new(MockClient::empty().with_count_tokens(|_| Ok(json!({ "input_tokens": 7 }))));
    let model = injected(
        &client,
        AnthropicModelConfig {
            max_tokens: Some(1),
            use_native_token_count: Some(true),
            ..model_id("claude-sonnet-5-5")
        },
    );
    let messages = vec![
        user(&"first ".repeat(500)),
        Message::new(Role::Assistant, vec![stored_compaction("new", "s2")]),
        user("next"),
    ];
    assert_eq!(
        block_on(model.count_tokens(&messages, CountTokensOptions::default())).unwrap(),
        7
    );
    assert_eq!(
        client.count_requests()[0],
        ApiRequest {
            body: json!({
                "model": "claude-sonnet-5-5",
                "messages": [
                    { "role": "assistant", "content": [{ "type": "compaction", "content": "new", "signature": "s2" }] },
                    { "role": "user", "content": [{ "type": "text", "text": "next" }] },
                ],
            }),
            beta: Some("compact-2026-09-04".to_owned()),
        }
    );

    let estimating = injected(
        &client,
        AnthropicModelConfig {
            max_tokens: Some(1),
            ..model_id("claude-sonnet-5-5")
        },
    );
    let count = |messages: &[Message]| {
        block_on(estimating.count_tokens(messages, CountTokensOptions::default())).unwrap()
    };
    assert_eq!(count(&messages), count(&messages[1..]));
}

#[test]
fn turns_server_side_summary_on_by_default_for_listed_model() {
    for id in [
        "claude-haiku-5-5",
        "claude-sonnet-5-5",
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-sonnet-5-5-20260401",
    ] {
        let model = keyed_model(AnthropicModelConfig {
            max_tokens: Some(1),
            ..model_id(id)
        });
        let capability = model
            .model_info()
            .capabilities
            .and_then(|c| c.server_side_summary);
        assert_eq!(capability, Some(true), "{id}");
    }
}

#[test]
fn leaves_server_side_summary_unknown_for_other_model() {
    for id in ["claude-sonnet-4-5", "m1"] {
        let model = keyed_model(AnthropicModelConfig {
            max_tokens: Some(1),
            ..model_id(id)
        });
        let capability = model
            .model_info()
            .capabilities
            .and_then(|c| c.server_side_summary);
        assert_eq!(capability, None, "{id}");
    }
}

#[test]
fn sends_the_compaction_text_without_the_capability() {
    let client = compaction_client(json!({}));
    let model = compaction_model(&client);
    model
        .update_config(AnthropicModelConfig {
            model_info: Some(ModelInfo {
                capabilities: Some(ModelCapabilities {
                    server_side_summary: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
    let messages = vec![
        user("first"),
        Message::new(Role::Assistant, vec![stored_compaction("old", "sig")]),
    ];
    collect(&model, request(messages));
    assert_eq!(
        model
            .model_info()
            .capabilities
            .and_then(|c| c.server_side_summary),
        Some(false)
    );
    assert_eq!(
        client.last_body()["messages"][1]["content"],
        json!([{ "type": "text", "text": "old" }])
    );
    assert_eq!(client.last_beta(), None);
}
