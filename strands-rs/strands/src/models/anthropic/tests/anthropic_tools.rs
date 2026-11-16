//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('anthropicTools')`).

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::models::model::{CacheConfig, CacheSection};
use crate::types::citations::{Citation, CitationGeneratedContent, CitationsBlock};
use crate::types::messages::ContentBlock;
use crate::types::tools::{ToolChoice, ToolSpec};

fn web_search_tool() -> JsonValue {
    json!({ "type": "web_search_20260318", "name": "web_search", "max_uses": 3 })
}

fn function_tool_spec() -> ToolSpec {
    ToolSpec::new("calc", "calculate").with_input_schema(json!({ "type": "object", "properties": {} }))
}

fn function_tool() -> JsonValue {
    json!({ "name": "calc", "description": "calculate", "input_schema": { "type": "object", "properties": {} } })
}

fn with_function_tool(tool_choice: Option<ToolChoice>) -> ModelRequest {
    ModelRequest {
        tool_specs: vec![function_tool_spec()],
        tool_choice,
        ..request(vec![user("Hi")])
    }
}

fn server_tools(tools: Vec<JsonValue>) -> AnthropicModelConfig {
    AnthropicModelConfig {
        anthropic_tools: Some(tools),
        ..Default::default()
    }
}

fn params_tools(tools: &[JsonValue]) -> AnthropicModelConfig {
    AnthropicModelConfig {
        params: json!({ "tools": tools }).as_object().cloned(),
        ..Default::default()
    }
}

fn send(config: AnthropicModelConfig, request: ModelRequest) -> JsonValue {
    let client = Arc::new(MockClient::empty());
    collect(&injected(&client, config), request);
    client.last_body()
}

fn web_search_stream() -> JsonValue {
    json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        {
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {} },
        },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{" } },
        { "type": "content_block_stop", "index": 0 },
        {
            "type": "content_block_start",
            "index": 1,
            "content_block": { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [] },
        },
        { "type": "content_block_stop", "index": 1 },
        { "type": "content_block_start", "index": 2, "content_block": { "type": "text", "text": "" } },
        { "type": "content_block_delta", "index": 2, "delta": { "type": "text_delta", "text": "Agents are autonomous." } },
        {
            "type": "content_block_delta",
            "index": 2,
            "delta": {
                "type": "citations_delta",
                "citation": {
                    "type": "web_search_result_location",
                    "url": "https://docs.example.com/agents",
                    "title": "Agents guide",
                    "cited_text": "Agents are autonomous programs.",
                },
            },
        },
        { "type": "content_block_stop", "index": 2 },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } },
        { "type": "message_stop" },
    ])
}

fn paused_content() -> JsonValue {
    json!({ "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {} })
}

fn paused_stream() -> JsonValue {
    json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Searching. " } },
        { "type": "content_block_stop", "index": 0 },
        { "type": "content_block_start", "index": 1, "content_block": paused_content() },
        { "type": "content_block_stop", "index": 1 },
        { "type": "message_delta", "delta": { "stop_reason": "pause_turn" }, "usage": { "output_tokens": 5 } },
        { "type": "message_stop" },
    ])
}

fn event_type(item: &JsonValue) -> String {
    match item.get("type").and_then(JsonValue::as_str) {
        Some("modelContentBlockDeltaEvent") => format!("delta:{}", item["delta"]["type"].as_str().unwrap()),
        Some(event_type) => event_type.to_owned(),
        // Content blocks serialize as single-key wrappers.
        None => item.as_object().unwrap().keys().next().unwrap().clone(),
    }
}

#[test]
fn appends_anthropic_tools_alongside_function_tools() {
    let body = send(server_tools(vec![web_search_tool()]), with_function_tool(None));
    assert_eq!(body["tools"], json!([function_tool(), web_search_tool()]));
}

#[test]
fn appends_params_tools_alongside_function_tools() {
    let body = send(params_tools(&[web_search_tool()]), with_function_tool(None));
    assert_eq!(body["tools"], json!([function_tool(), web_search_tool()]));
}

#[test]
fn caches_on_the_last_server_tool_without_mutating_the_configured_tools() {
    let client = Arc::new(MockClient::empty());
    let model = injected(
        &client,
        AnthropicModelConfig {
            cache_config: Some(CacheConfig {
                tools_ttl: Some(CacheSection::Enabled(true)),
                ..Default::default()
            }),
            ..server_tools(vec![web_search_tool()])
        },
    );
    collect(&model, with_function_tool(None));
    let mut cached_tool = web_search_tool();
    cached_tool["cache_control"] = json!({ "type": "ephemeral" });
    assert_eq!(client.last_body()["tools"], json!([function_tool(), cached_tool]));
    assert_eq!(model.config().anthropic_tools, Some(vec![web_search_tool()]));

    model
        .update_config(AnthropicModelConfig {
            cache_config: Some(CacheConfig {
                tools_ttl: Some(CacheSection::Enabled(false)),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
    collect(&model, with_function_tool(None));
    assert_eq!(
        client.last_body()["tools"],
        json!([function_tool(), web_search_tool()])
    );
}

#[test]
fn omits_server_tools_when_a_tool_is_forced_with() {
    for (choice, key) in [(ToolChoice::Any, "any"), (ToolChoice::tool("calc"), "tool")] {
        let config = AnthropicModelConfig {
            params: json!({ "tools": [web_search_tool()] }).as_object().cloned(),
            ..server_tools(vec![web_search_tool()])
        };
        let (body, warnings) = warnings(|| send(config, with_function_tool(Some(choice))));
        assert_eq!(body["tools"], json!([function_tool()]));
        assert_eq!(body["tool_choice"]["type"], key);
        assert!(warned(
            &warnings,
            "server_tools=<web_search,web_search> | forced tool call, omitting server tools"
        ));
    }
}

#[test]
fn names_a_toolset_by_its_type_when_a_forced_turn_omits_it() {
    let config = server_tools(vec![json!({ "type": "browser_toolset_20260801" })]);
    let (_, warnings) = warnings(|| send(config, with_function_tool(Some(ToolChoice::Any))));
    assert!(warned(&warnings, "server_tools=<browser_toolset_20260801>"));
}

#[test]
fn does_not_let_params_tools_reach_a_forced_turn_that_has_no_function_tools() {
    let request = ModelRequest {
        tool_choice: Some(ToolChoice::Any),
        ..request(vec![user("Hi")])
    };
    let (body, _) = warnings(|| send(params_tools(&[web_search_tool()]), request));
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn applies_tool_choice_when_only_server_tools_are_configured() {
    let request = ModelRequest {
        tool_choice: Some(ToolChoice::Auto),
        ..request(vec![user("Hi")])
    };
    let body = send(server_tools(vec![web_search_tool()]), request);
    assert_eq!(body["tools"], json!([web_search_tool()]));
    assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
}

#[test]
fn rejects_function_tool_definitions_in_anthropic_tools() {
    let function_tool = json!({ "name": "f", "input_schema": { "type": "object" } });
    let error = AnthropicModel::new(server_tools(vec![function_tool.clone()])).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("anthropicTools should not contain function tool definitions")
    );
    let model = keyed_model(AnthropicModelConfig::default());
    let error = model
        .update_config(server_tools(vec![function_tool]))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("anthropicTools should not contain function tool definitions")
    );
}

#[test]
fn skips_server_side_tool_blocks_and_aggregates_web_search_citations_with_their_text() {
    let client = Arc::new(MockClient::new(web_search_stream()));
    let model = injected(&client, AnthropicModelConfig::default());
    let (items, result) = aggregate(&model, request(vec![user("Hi")])).unwrap();
    assert_eq!(
        items.iter().map(event_type).collect::<Vec<_>>(),
        [
            "modelMessageStartEvent",
            "modelContentBlockStartEvent",
            "delta:textDelta",
            "delta:citationsDelta",
            "modelContentBlockStopEvent",
            "citations",
            "modelMetadataEvent",
            "modelMessageStopEvent",
        ]
    );
    assert_eq!(result.stop_reason().as_str(), "endTurn");
    let citation: Citation = serde_json::from_value(json!({
        "location": { "type": "web", "url": "https://docs.example.com/agents", "domain": "docs.example.com" },
        "source": "https://docs.example.com/agents",
        "sourceContent": [{ "text": "Agents are autonomous programs." }],
        "title": "Agents guide",
    }))
    .unwrap();
    let expected = CitationsBlock::new(
        vec![citation],
        vec![CitationGeneratedContent::new("Agents are autonomous.")],
    );
    assert_eq!(result.message().content(), [ContentBlock::Citations(expected)]);
}

#[test]
fn resumes_a_paused_server_side_tool_turn_and_surfaces_it_as_one_stream() {
    let paused = events_of(paused_stream());
    let resumed = events_of(web_search_stream());
    let client = Arc::new(MockClient::scripted(move |call| {
        if call == 0 {
            paused.clone()
        } else {
            resumed.clone()
        }
    }));
    let model = injected(&client, server_tools(vec![web_search_tool()]));
    let (items, result) = aggregate(&model, with_function_tool(None)).unwrap();
    let types: Vec<String> = items
        .iter()
        .map(event_type)
        .filter(|kind| !kind.starts_with("modelContentBlock") && !kind.starts_with("delta:"))
        .collect();
    assert_eq!(
        types,
        [
            "modelMessageStartEvent",
            "text",
            "citations",
            "modelMetadataEvent",
            "modelMessageStopEvent"
        ]
    );
    assert_eq!(result.stop_reason().as_str(), "endTurn");
    assert_eq!(
        result.message().content()[0],
        ContentBlock::from(TextBlock::new("Searching. "))
    );
    assert!(matches!(
        result.message().content()[1],
        ContentBlock::Citations(_)
    ));
    let usage = result.metadata().and_then(|metadata| metadata.usage()).cloned();
    assert_eq!(
        serde_json::to_value(usage).unwrap(),
        json!({ "inputTokens": 20, "outputTokens": 10, "totalTokens": 30 })
    );

    let requests = client.requests();
    assert_eq!(requests.len(), 2);
    let (first, second) = (&requests[0].body, &requests[1].body);
    // The paused message goes back as the vendor SDK accumulated it, text included.
    let mut expected = first["messages"].as_array().unwrap().clone();
    expected.push(json!({
        "role": "assistant",
        "content": [{ "type": "text", "text": "Searching. " }, paused_content()],
    }));
    assert_eq!(second["messages"], JsonValue::Array(expected));
    assert_eq!(second["tools"], first["tools"]);
    assert_eq!(first["tools"], json!([function_tool(), web_search_tool()]));
}

#[test]
fn throws_once_a_paused_turn_exceeds_the_continuation_limit() {
    let client = Arc::new(MockClient::new(paused_stream()));
    let model = injected(&client, AnthropicModelConfig::default());
    let error = collect_err(&model, request(vec![user("Hi")]));
    assert!(matches!(error, Error::Model(_)));
    assert_eq!(
        error.to_string(),
        "server-side tool turn did not complete after 10 continuations"
    );
    assert_eq!(client.requests().len(), 11);
}

#[test]
fn treats_message_delta_usage_as_cumulative_within_a_response_and_sums_across_continuations() {
    let paused = events_of(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": paused_content() },
        { "type": "content_block_stop", "index": 0 },
        { "type": "message_delta", "delta": {}, "usage": { "output_tokens": 3 } },
        { "type": "message_delta", "delta": { "stop_reason": "pause_turn" }, "usage": { "output_tokens": 5 } },
        { "type": "message_stop" },
    ]));
    let resumed = events_of(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 20 } } },
        { "type": "message_delta", "delta": {}, "usage": { "output_tokens": 4 } },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 7 } },
        { "type": "message_stop" },
    ]));
    let client = Arc::new(MockClient::scripted(move |call| {
        if call == 0 {
            paused.clone()
        } else {
            resumed.clone()
        }
    }));
    let events = collect_ok(
        &injected(&client, AnthropicModelConfig::default()),
        request(vec![user("Hi")]),
    );
    assert!(events.contains(&json!({
        "type": "modelMetadataEvent",
        "usage": { "inputTokens": 30, "outputTokens": 12, "totalTokens": 42 },
    })));
}

#[test]
fn reports_the_cumulative_usage_from_message_delta_and_keeps_message_start_values_it_omits() {
    let client = Arc::new(MockClient::new(json!([
        {
            "type": "message_start",
            "message": {
                "role": "assistant",
                "usage": { "input_tokens": 2230, "output_tokens": 25, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 7 },
            },
        },
        {
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn" },
            "usage": { "input_tokens": 11768, "output_tokens": 115, "cache_read_input_tokens": null, "cache_creation_input_tokens": null },
        },
        { "type": "message_stop" },
    ])));
    let events = collect_ok(
        &injected(&client, AnthropicModelConfig::default()),
        request(vec![user("Hi")]),
    );
    assert!(events.contains(&json!({
        "type": "modelMetadataEvent",
        "usage": {
            "inputTokens": 11768,
            "outputTokens": 115,
            "totalTokens": 11990,
            "cacheReadInputTokens": 100,
            "cacheWriteInputTokens": 7,
        },
    })));
}

#[test]
fn does_not_synthesize_a_stop_event_when_the_stream_ends_without_message_stop() {
    let client = Arc::new(MockClient::new(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 1 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "half of the" } },
    ])));
    let model = injected(&client, AnthropicModelConfig::default());
    let events = collect_ok(&model, request(vec![user("Hi")]));
    assert_eq!(
        events
            .iter()
            .map(|event| event["type"].clone())
            .collect::<Vec<_>>(),
        [
            json!("modelMessageStartEvent"),
            json!("modelContentBlockStartEvent"),
            json!("modelContentBlockDeltaEvent"),
        ]
    );
    let error = aggregate(&model, request(vec![user("Hi")])).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Stream ended without completing a message")
    );
}

#[test]
fn maps_citations() {
    let cases = [
        (
            json!({ "type": "search_result_location", "search_result_index": 1, "start_block_index": 2, "end_block_index": 3, "source": "s" }),
            json!({ "type": "searchResult", "searchResultIndex": 1, "start": 2, "end": 3 }),
        ),
        (
            json!({ "type": "char_location", "document_index": 0, "start_char_index": 1, "end_char_index": 2 }),
            json!({ "type": "documentChar", "documentIndex": 0, "start": 1, "end": 2 }),
        ),
        (
            json!({ "type": "page_location", "document_index": 0, "start_page_number": 1, "end_page_number": 2 }),
            json!({ "type": "documentPage", "documentIndex": 0, "start": 1, "end": 2 }),
        ),
        (
            json!({ "type": "content_block_location", "document_index": 0, "start_block_index": 1, "end_block_index": 2 }),
            json!({ "type": "documentChunk", "documentIndex": 0, "start": 1, "end": 2 }),
        ),
    ];
    for (citation, location) in cases {
        let client = Arc::new(MockClient::new(json!([
            { "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } },
            { "type": "content_block_delta", "index": 0, "delta": { "type": "citations_delta", "citation": citation } },
            { "type": "content_block_stop", "index": 0 },
        ])));
        let events = collect_ok(
            &injected(&client, AnthropicModelConfig::default()),
            request(vec![user("Hi")]),
        );
        let deltas: Vec<&JsonValue> = events
            .iter()
            .filter(|event| event["type"] == "modelContentBlockDeltaEvent")
            .collect();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["delta"]["citations"][0]["location"], location);
    }
}

#[test]
fn logs_an_error_when_a_server_side_tool_fails() {
    let client = Arc::new(MockClient::new(json!([
        {
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_1",
                "content": { "type": "web_search_tool_result_error", "error_code": "max_uses_exceeded" },
            },
        },
        { "type": "content_block_stop", "index": 0 },
        { "type": "message_stop" },
    ])));
    let model = injected(&client, AnthropicModelConfig::default());
    let (_, warnings) = warnings(|| collect(&model, request(vec![user("Hi")])));
    assert!(warned(&warnings, "error_code=<max_uses_exceeded>"));
}

#[test]
fn formats_a_citations_block_back_into_the_request_as_text() {
    let citation: Citation = serde_json::from_value(json!({
        "location": { "type": "web", "url": "https://x" },
        "source": "https://x",
        "sourceContent": [],
        "title": "x",
    }))
    .unwrap();
    let cited = CitationsBlock::new(
        vec![citation],
        vec![
            CitationGeneratedContent::new("Agents are "),
            CitationGeneratedContent::new("autonomous."),
        ],
    );
    let body = send(
        AnthropicModelConfig::default(),
        request(vec![
            user("Hi"),
            Message::new(Role::Assistant, vec![cited.into()]),
        ]),
    );
    assert_eq!(
        body["messages"][1],
        json!({ "role": "assistant", "content": [{ "type": "text", "text": "Agents are autonomous." }] })
    );
}
