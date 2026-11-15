//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('structured output enforcement')`).

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::models::native_output::to_native_output_schema;
use crate::types::tools::{ToolChoice, ToolSpec};

/// A 400 built the way the vendor SDK builds one: the message is the JSON-encoded body.
fn bad_request(message: &str) -> ApiError {
    let body = json!({ "type": "error", "error": { "type": "invalid_request_error", "message": message } });
    ApiError {
        body: Some(body.clone()),
        ..status_error(400, &format!("400 {body}"))
    }
}

fn forced_rejection() -> ApiError {
    bad_request("tool_choice: type \"tool\" and \"any\" are not supported for this model.")
}

fn native_rejection() -> ApiError {
    bad_request("output_config.format: Extra inputs are not permitted")
}

fn schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": { "name": { "type": "string" }, "age": { "type": "number" } },
        "required": ["name", "age"],
        "additionalProperties": false,
    })
}

fn structured_output_request(schema: JsonValue) -> ModelRequest {
    ModelRequest {
        messages: vec![user("Generate a person")],
        tool_specs: vec![
            ToolSpec::new("strands_structured_output", "Output").with_input_schema(schema.clone()),
        ],
        tool_choice: Some(ToolChoice::tool("strands_structured_output")),
        structured_output_schema: Some(schema),
        ..Default::default()
    }
}

/// A client whose stream rejects with each error in turn, then succeeds.
fn rejecting(errors: Vec<ApiError>) -> Arc<MockClient> {
    let errors = Mutex::new(errors.into_iter());
    Arc::new(MockClient::scripted(move |_| {
        match errors.lock().unwrap().next() {
            Some(error) => vec![Err(error)],
            None => vec![Ok(
                json!({ "type": "message_start", "message": { "role": "assistant", "usage": {} } }),
            )],
        }
    }))
}

/// The structured output mechanism a sent request uses.
fn mechanism_of(body: &JsonValue) -> &'static str {
    if body["tool_choice"]["type"] == "tool" {
        return "forced";
    }
    if body["output_config"].get("format").is_some() {
        "native"
    } else {
        "auto"
    }
}

fn sent_mechanisms(client: &MockClient) -> Vec<&'static str> {
    client
        .requests()
        .iter()
        .map(|request| mechanism_of(&request.body))
        .collect()
}

#[test]
fn falls_back_to_the_next_mechanism_after_and_remembers_it() {
    let cases = [
        (vec![], vec!["forced"]),
        (vec![forced_rejection()], vec!["forced", "native"]),
        (
            vec![forced_rejection(), native_rejection()],
            vec!["forced", "native", "auto"],
        ),
    ];
    for (rejections, expected) in cases {
        let client = rejecting(rejections);
        let model = injected(&client, model_id("m1"));
        warnings(|| collect_ok(&model, structured_output_request(schema())));
        assert_eq!(sent_mechanisms(&client), expected);

        let before = client.requests().len();
        warnings(|| collect_ok(&model, structured_output_request(schema())));
        assert_eq!(sent_mechanisms(&client)[before..], [*expected.last().unwrap()]);
    }
}

#[test]
fn sends_native_output_with_tool_choice_none_keeping_the_tools_and_other_output_config_settings() {
    let client = rejecting(Vec::new());
    let model = injected(
        &client,
        AnthropicModelConfig {
            params: json!({ "output_config": { "effort": "high" } })
                .as_object()
                .cloned(),
            ..model_id("m1")
        },
    );
    let request = ModelRequest {
        tool_choice: Some(ToolChoice::Auto),
        ..structured_output_request(schema())
    };
    collect_ok(&model, request);
    let body = &client.requests()[0].body;
    assert_eq!(body["tool_choice"], json!({ "type": "none" }));
    assert_eq!(
        body["output_config"],
        json!({
            "effort": "high",
            "format": { "type": "json_schema", "schema": to_native_output_schema(&schema()).unwrap() },
        })
    );
    let tool_names: Vec<&JsonValue> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| &tool["name"])
        .collect();
    assert_eq!(tool_names, [&json!("strands_structured_output")]);
}

#[test]
fn uses_auto_tool_choice_for_a_schema_native_output_cannot_represent() {
    let client = rejecting(vec![forced_rejection()]);
    let model = injected(&client, model_id("m1"));
    // TS uses a record field; a tuple field leaves the process-wide warn-once for records to the
    // native_output test that asserts it.
    let tuple_schema = json!({
        "type": "object",
        "properties": {
            "pair": { "type": "array", "prefixItems": [{ "type": "string" }, { "type": "number" }] },
        },
        "required": ["pair"],
        "additionalProperties": false,
    });
    let request = ModelRequest {
        structured_output_schema: Some(tuple_schema),
        ..structured_output_request(schema())
    };
    warnings(|| collect_ok(&model, request));
    assert_eq!(sent_mechanisms(&client), ["forced", "auto"]);
    assert_eq!(
        client.requests()[1].body["tool_choice"],
        json!({ "type": "auto" })
    );
}

#[test]
fn retries_with_drop_block_before_trying_the_next_mechanism() {
    let binding = status_error(
        400,
        "400 messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation.",
    );
    let client = rejecting(vec![binding, forced_rejection()]);
    let model = injected(
        &client,
        AnthropicModelConfig {
            params: json!({ "thinking": { "type": "adaptive" } }).as_object().cloned(),
            ..model_id("m1")
        },
    );
    warnings(|| collect_ok(&model, structured_output_request(schema())));
    let sent: Vec<(&str, bool)> = client
        .requests()
        .iter()
        .map(|request| {
            (
                mechanism_of(&request.body),
                request.body["thinking"].get("block_binding").is_some(),
            )
        })
        .collect();
    assert_eq!(sent, [("forced", false), ("forced", true), ("native", true)]);
}

#[test]
fn re_throws_without_retrying() {
    let forced_only = ModelRequest {
        messages: vec![user("Generate a person")],
        tool_choice: Some(ToolChoice::tool("strands_structured_output")),
        ..Default::default()
    };
    let non_400 = ApiError {
        status: Some(500),
        ..bad_request("tool_choice: type \"tool\" and \"any\" are not supported")
    };
    let cases = [
        (forced_rejection(), forced_only),
        (bad_request("something else"), structured_output_request(schema())),
        (non_400, structured_output_request(schema())),
    ];
    for (error, request) in cases {
        let message = error.message.clone();
        let client = rejecting(vec![error]);
        let model = injected(&client, model_id("m1"));
        let failure = collect_err(&model, request);
        assert!(failure.to_string().contains(&message), "{failure}");
        assert_eq!(client.requests().len(), 1);
    }
}
