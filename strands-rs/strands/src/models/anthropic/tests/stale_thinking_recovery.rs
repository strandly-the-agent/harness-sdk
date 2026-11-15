//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('stale thinking recovery')`).

use std::sync::Arc;

use http::{HeaderMap, HeaderValue};
use serde_json::json;

use super::*;
use crate::models::anthropic::AnthropicClientConfig;
use crate::types::json::JsonObject;

const BINDING_MESSAGE: &str = "400 messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation.";

fn binding_error() -> ApiError {
    status_error(400, BINDING_MESSAGE)
}

fn drop_block_thinking() -> JsonValue {
    json!({ "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "drop_block" } })
}

/// Rejects the first call with `error`, the second with `retry_error` when given, then succeeds.
fn rejecting_once(error: ApiError, retry_error: Option<ApiError>) -> Arc<MockClient> {
    Arc::new(MockClient::scripted(move |call| match (call, &retry_error) {
        (0, _) => vec![Err(error.clone())],
        (_, Some(retry_error)) => vec![Err(retry_error.clone())],
        _ => vec![Ok(
            json!({ "type": "message_start", "message": { "role": "assistant", "usage": {} } }),
        )],
    }))
}

fn adaptive_params() -> Option<JsonObject> {
    json!({ "thinking": { "type": "adaptive" } }).as_object().cloned()
}

fn m1_adaptive() -> AnthropicModelConfig {
    AnthropicModelConfig {
        params: adaptive_params(),
        ..model_id("m1")
    }
}

fn hi() -> ModelRequest {
    request(vec![user("Hi")])
}

#[test]
fn retries_once_with_drop_block_and_keeps_it_on_for_later_requests() {
    let client = rejecting_once(binding_error(), None);
    let model = injected(
        &client,
        AnthropicModelConfig {
            betas: Some(vec!["other-beta".to_owned()]),
            ..m1_adaptive()
        },
    );
    let (events, warnings) = warnings(|| collect_ok(&model, hi()));
    assert_eq!(
        events[0],
        json!({ "type": "modelMessageStartEvent", "role": "assistant" })
    );
    assert!(warned(&warnings, "retrying with drop_block"));
    let expected_beta = Some("other-beta,thinking-binding-controls-2026-08-01".to_owned());
    let sent: Vec<(JsonValue, Option<String>)> = client
        .requests()
        .into_iter()
        .map(|request| (request.body["thinking"].clone(), request.beta))
        .collect();
    assert_eq!(
        sent,
        [
            (json!({ "type": "adaptive" }), Some("other-beta".to_owned())),
            (drop_block_thinking(), expected_beta.clone()),
        ]
    );

    collect_ok(&model, hi());
    assert_eq!(client.requests().len(), 3);
    assert_eq!(client.last_body()["thinking"], drop_block_thinking());
    assert_eq!(client.last_beta(), expected_beta);
}

#[test]
fn keeps_betas_from_client_default_headers_given_as_on_the_retry() {
    let single = HeaderMap::from_iter([(
        http::header::HeaderName::from_static("anthropic-beta"),
        HeaderValue::from_static("client-beta"),
    )]);
    let mut list = HeaderMap::new();
    list.append("anthropic-beta", HeaderValue::from_static("a"));
    list.append("anthropic-beta", HeaderValue::from_static("b"));
    for (default_headers, betas) in [(single, "client-beta,"), (list, "a,b,")] {
        let client = rejecting_once(binding_error(), None);
        let model = built(
            &client,
            AnthropicModelOptions {
                client_config: AnthropicClientConfig {
                    default_headers,
                    ..Default::default()
                },
                ..keyed(m1_adaptive())
            },
        );
        warnings(|| collect_ok(&model, hi()));
        assert_eq!(client.last_body()["thinking"], drop_block_thinking());
        assert_eq!(
            client.last_beta(),
            Some(format!("{betas}thinking-binding-controls-2026-08-01"))
        );
    }
}

#[test]
fn maps_on_the_retry_to_its_sdk_error() {
    let cases: [(ApiError, ErrorCheck); 2] = [
        (status_error(429, "Rate limit exceeded"), |error| {
            matches!(error, Error::ModelThrottled(_))
        }),
        (status_error(400, "prompt is too long"), |error| {
            matches!(error, Error::ContextWindowOverflow(_))
        }),
    ];
    for (retry_error, expected) in cases {
        let client = rejecting_once(binding_error(), Some(retry_error));
        let model = injected(&client, m1_adaptive());
        let (error, _) = warnings(|| collect_err(&model, hi()));
        assert!(expected(&error), "{error:?}");
        assert_eq!(client.requests().len(), 2);
    }
}

#[test]
fn re_throws_a_second_binding_error_without_retrying_again() {
    let client = rejecting_once(binding_error(), Some(binding_error()));
    let model = injected(&client, m1_adaptive());
    let (error, _) = warnings(|| collect_err(&model, hi()));
    assert!(error.to_string().contains("bound to a different conversation"));
    assert_eq!(client.requests().len(), 2);
}

#[test]
fn re_throws_the_binding_error_with() {
    let caller_set = json!({ "thinking": { "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "error" } } });
    for params in [caller_set.as_object().cloned(), None] {
        let client = rejecting_once(binding_error(), None);
        let model = injected(
            &client,
            AnthropicModelConfig {
                params,
                ..model_id("m1")
            },
        );
        let error = collect_err(&model, hi());
        assert!(error.to_string().contains("bound to a different conversation"));
        assert_eq!(client.requests().len(), 1);
    }
}

#[test]
fn re_throws_the_binding_message_on_a_non_400_error() {
    let client = rejecting_once(status_error(500, BINDING_MESSAGE), None);
    let model = injected(&client, m1_adaptive());
    let error = collect_err(&model, hi());
    assert!(error.to_string().contains("bound to a different conversation"));
    assert_eq!(client.requests().len(), 1);
}

#[test]
fn stops_dropping_stale_thinking_after_the_model_changes() {
    let client = rejecting_once(binding_error(), None);
    let model = injected(&client, m1_adaptive());
    warnings(|| collect_ok(&model, hi()));
    model.update_config(model_id("m2")).unwrap();
    collect_ok(&model, hi());
    assert_eq!(client.last_body()["thinking"], json!({ "type": "adaptive" }));
}
