//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('API version compatibility')`).
//!
//! The TS `on @anthropic-ai/sdk 0.x` cases run only against that SDK's previous major version,
//! whose default API version Rust doesn't send; the 1.x cases are ported.

use std::sync::Arc;

use http::{HeaderMap, HeaderValue};
use serde_json::json;

use super::*;
use crate::models::anthropic::AnthropicClientConfig;
use crate::types::media::DocumentBlock;
use crate::types::messages::ContentBlock;

fn hello() -> ModelRequest {
    request(vec![user("Hello")])
}

fn hello_request(temperature: f64) -> JsonValue {
    json!({
        "model": "m1",
        "max_tokens": 64000,
        "stream": true,
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hello" }] }],
        "temperature": temperature,
    })
}

fn version_headers(name: &'static str, version: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(name, HeaderValue::from_static(version));
    headers
}

/// A built model whose `ANTHROPIC_CUSTOM_HEADERS` is `custom_headers`.
fn built_with_env(
    client: &Arc<MockClient>,
    options: AnthropicModelOptions,
    custom_headers: Option<&'static str>,
) -> AnthropicModel {
    let seams = Seams {
        env: Arc::new(move |name| {
            (name == "ANTHROPIC_CUSTOM_HEADERS")
                .then(|| custom_headers.map(str::to_owned))
                .flatten()
        }),
        ..seams(Transport::Built(client.clone()))
    };
    AnthropicModel::with_seams(options, seams).unwrap()
}

fn m1_temperature() -> AnthropicModelConfig {
    AnthropicModelConfig {
        temperature: Some(0.2),
        ..model_id("m1")
    }
}

fn pinned(headers: HeaderMap, config: AnthropicModelConfig) -> AnthropicModelOptions {
    AnthropicModelOptions {
        client_config: AnthropicClientConfig {
            default_headers: headers,
            ..Default::default()
        },
        ..keyed(config)
    }
}

#[test]
fn rejects_temperature_when_anthropic_custom_headers_pins_the_current_api_version() {
    let client = Arc::new(MockClient::empty());
    let model = built_with_env(
        &client,
        keyed(m1_temperature()),
        Some("anthropic-version: 2026-10-21"),
    );
    let error = collect_err(&model, hello());
    assert!(error.to_string().contains(
        "temperature is not supported by Anthropic API version 2026-10-21, pinned by the ANTHROPIC_CUSTOM_HEADERS environment variable"
    ));
    assert!(client.requests().is_empty());
}

#[test]
fn leaves_the_check_to_the_api_for_an_injected_client() {
    let client = Arc::new(MockClient::empty());
    let model = injected(
        &client,
        AnthropicModelConfig {
            temperature: Some(0.2),
            ..Default::default()
        },
    );
    collect_ok(&model, hello());
    assert_eq!(client.last_body()["temperature"], json!(0.2));
}

#[test]
fn rejects_temperature_when_the_current_api_version_is_pinned() {
    let client = Arc::new(MockClient::empty());
    let options = pinned(
        version_headers("anthropic-version", "2026-10-21"),
        m1_temperature(),
    );
    let error = collect_err(&built(&client, options), hello());
    assert!(error.to_string().contains(
        "temperature is not supported by Anthropic API version 2026-10-21, pinned by client_config.default_headers['anthropic-version']"
    ));
}

#[test]
fn rejects_name_before_sending_the_request() {
    let cases = [
        (m1_temperature(), "temperature is"),
        (
            AnthropicModelConfig {
                top_p: Some(0.9),
                ..model_id("m1")
            },
            "top_p is",
        ),
        (
            AnthropicModelConfig {
                params: json!({ "top_k": 3 }).as_object().cloned(),
                ..model_id("m1")
            },
            "top_k is",
        ),
        (
            AnthropicModelConfig {
                params: json!({ "top_p": 0.9 }).as_object().cloned(),
                ..m1_temperature()
            },
            "temperature, top_p are",
        ),
    ];
    for (config, params) in cases {
        let client = Arc::new(MockClient::empty());
        let model = built(&client, keyed(config));
        let error = collect_err(&model, hello()).to_string();
        assert!(
            error.contains(&format!(
                "{params} not supported by Anthropic API version 2026-10-21, the AnthropicModel default"
            )),
            "{error}"
        );
        assert!(error.contains("'anthropic-version': '2023-06-01'"));
        assert!(client.requests().is_empty());
    }
}

#[test]
fn sends_temperature_when_the_previous_api_version_is_pinned_as_name() {
    // HeaderMap names are case-insensitive, so the TS object, tuple and Headers forms are one case here.
    let client = Arc::new(MockClient::empty());
    let options = pinned(
        version_headers("anthropic-version", "2023-06-01"),
        m1_temperature(),
    );
    collect_ok(&built(&client, options), hello());
    assert_eq!(client.last_body(), hello_request(0.2));
}

#[test]
fn lets_a_client_config_pin_win_over_a_differently_cased_anthropic_custom_headers_pin() {
    let client = Arc::new(MockClient::empty());
    let options = pinned(
        version_headers("anthropic-version", "2023-06-01"),
        m1_temperature(),
    );
    let model = built_with_env(&client, options, Some("Anthropic-Version: 2026-10-21"));
    collect_ok(&model, hello());
    assert_eq!(client.last_body(), hello_request(0.2));
}

#[test]
fn sends_temperature_when_anthropic_custom_headers_pins_the_previous_api_version() {
    let client = Arc::new(MockClient::empty());
    let model = built_with_env(
        &client,
        keyed(m1_temperature()),
        Some("anthropic-version: 2023-06-01"),
    );
    collect_ok(&model, hello());
    assert_eq!(client.last_body(), hello_request(0.2));
}

#[test]
fn skips_pdf_documents_in_assistant_messages() {
    let pdf = || ContentBlock::from(DocumentBlock::new("doc.pdf", "pdf", vec![1, 2, 3]));
    let client = Arc::new(MockClient::empty());
    let model = built(&client, keyed(model_id("m1")));
    let conversation = vec![
        Message::new(Role::User, vec![TextBlock::new("Summarize").into(), pdf()]),
        Message::new(Role::Assistant, vec![TextBlock::new("Here it is").into(), pdf()]),
    ];
    let (_, warnings) = warnings(|| collect(&model, request(conversation)));
    let types = |index: usize| -> Vec<JsonValue> {
        client.last_body()["messages"][index]["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["type"].clone())
            .collect()
    };
    assert_eq!(types(0), [json!("text"), json!("document")]);
    assert_eq!(types(1), [json!("text")]);
    assert!(warned(
        &warnings,
        "document blocks are not accepted in assistant messages"
    ));
}
