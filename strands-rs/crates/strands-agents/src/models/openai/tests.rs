use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Value};

use super::*;
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::types::{
    ContentBlock, Message, Role, StopReason, SystemPrompt, ToolChoice, ToolSpec, ToolUseBlock,
};

/// Transport that replays a canned SSE body.
#[derive(Debug)]
struct StaticTransport {
    body: String,
}

#[async_trait]
impl OpenAITransport for StaticTransport {
    async fn send(&self, _request: OpenAIRequest) -> Result<BoxStream<'static, Result<String>>> {
        let body = self.body.clone();
        Ok(futures::stream::iter(vec![Ok(body)]).boxed())
    }
}

fn model(api: OpenAIApi, body: &str) -> OpenAIModel {
    OpenAIModel::with_transport(
        OpenAIModelConfig {
            api,
            api_key: Some("test-key".into()),
            model_id: Some("gpt-5.4".into()),
            ..Default::default()
        },
        Arc::new(StaticTransport { body: body.into() }),
    )
    .expect("model builds")
}

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![ContentBlock::Text(text.into())])
}

fn weather_tool() -> ToolSpec {
    ToolSpec {
        name: "weather".into(),
        description: "get weather".into(),
        input_schema: Some(json!({"type": "object", "properties": {"city": {"type": "string"}}})),
        output_schema: None,
        annotations: None,
    }
}

fn stream_options() -> StreamOptions {
    StreamOptions::default()
}

async fn collect(model: &OpenAIModel, messages: &[Message]) -> Vec<ModelStreamEvent> {
    let options = stream_options();
    model
        .stream(messages, &options)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|e| e.expect("event"))
        .collect()
}

#[test]
fn formats_responses_request_with_all_options() {
    let config = OpenAIModelConfig {
        api: OpenAIApi::Responses,
        api_key: Some("k".into()),
        model_id: Some("gpt-5.4".into()),
        temperature: Some(0.5),
        max_tokens: Some(256),
        top_p: Some(0.9),
        reasoning_effort: Some("high".into()),
        stateful: true,
        params: serde_json::from_value(json!({"model": "ignored", "top_logprobs": 3})).unwrap(),
        ..Default::default()
    };
    let model =
        OpenAIModel::with_transport(config, Arc::new(StaticTransport { body: String::new() })).unwrap();
    let options = StreamOptions {
        system_prompt: Some(SystemPrompt::Text("be nice".into())),
        tool_specs: vec![weather_tool()],
        tool_choice: Some(ToolChoice::Tool {
            name: "weather".into(),
        }),
        ..Default::default()
    };
    let request = model.format_request(&[user("hi")], &options).unwrap();
    assert_eq!(request.url, "https://api.openai.com/v1/responses");
    assert_eq!(request.bearer_token.as_deref(), Some("k"));
    let body = &request.body;
    assert_eq!(body["model"], json!("gpt-5.4"));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["store"], json!(true));
    assert_eq!(body["instructions"], json!("be nice"));
    assert_eq!(body["temperature"], json!(0.5));
    assert_eq!(body["max_output_tokens"], json!(256));
    assert_eq!(body["top_p"], json!(0.9));
    assert_eq!(body["top_logprobs"], json!(3));
    assert_eq!(body["reasoning"], json!({"effort": "high"}));
    assert_eq!(
        body["input"],
        json!([{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}])
    );
    assert_eq!(
        body["tools"],
        json!([{
            "type": "function",
            "name": "weather",
            "description": "get weather",
            "parameters": {"type": "object", "properties": {"city": {"type": "string"}}},
            "strict": Value::Null,
        }])
    );
    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "name": "weather"})
    );
}

#[test]
fn responses_strict_tools_and_builtin_tools_survive() {
    let config = OpenAIModelConfig {
        api: OpenAIApi::Responses,
        api_key: Some("k".into()),
        model_id: Some("gpt-5.4".into()),
        strict_tools: Some(true),
        params: serde_json::from_value(json!({"tools": [{"type": "web_search"}]})).unwrap(),
        ..Default::default()
    };
    let model =
        OpenAIModel::with_transport(config, Arc::new(StaticTransport { body: String::new() })).unwrap();
    let options = StreamOptions {
        tool_specs: vec![weather_tool()],
        ..Default::default()
    };
    let body = model.format_request(&[user("hi")], &options).unwrap().body;
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools[0], json!({"type": "web_search"}));
    assert_eq!(tools[1]["strict"], json!(true));
}

#[test]
fn responses_input_maps_tool_use_and_result_items() {
    let model = model(OpenAIApi::Responses, "");
    let messages = vec![
        Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Text("calling".into()),
                ContentBlock::ToolUse(ToolUseBlock {
                    name: "weather".into(),
                    tool_use_id: "call_1".into(),
                    input: json!({"city": "Berlin"}),
                    reasoning_signature: None,
                }),
            ],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(crate::types::ToolResultBlock::success(
                "call_1",
                vec![crate::types::ToolResultContent::text("sunny")],
            ))],
        ),
    ];
    let body = model.format_request(&messages, &stream_options()).unwrap().body;
    assert_eq!(
        body["input"],
        json!([
            {"role": "assistant", "content": [{"type": "output_text", "text": "calling"}]},
            {"type": "function_call", "call_id": "call_1", "name": "weather", "arguments": "{\"city\":\"Berlin\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "sunny"},
        ])
    );
}

#[test]
fn rejects_empty_messages() {
    let model = model(OpenAIApi::Responses, "");
    let err = model.format_request(&[], &stream_options()).unwrap_err();
    assert_eq!(err.to_string(), "At least one message is required");
}

#[tokio::test]
async fn responses_stream_maps_text_reasoning_and_usage() {
    let body = concat!(
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        "event: response.reasoning_text.delta\ndata: {\"delta\":\"think\"}\n\n",
        "event: response.output_text.delta\ndata: {\"delta\":\"he\"}\n\n",
        "event: response.output_text.delta\ndata: {\"delta\":\"llo\"}\n\n",
        "event: response.completed\ndata: {\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":2,\"total_tokens\":7,\"input_tokens_details\":{\"cached_tokens\":3,\"cache_write_tokens\":0}}}}\n\n",
    );
    let events = collect(&model(OpenAIApi::Responses, body), &[user("hi")]).await;
    assert!(matches!(
        events[0],
        ModelStreamEvent::MessageStart {
            role: Role::Assistant
        }
    ));
    assert!(matches!(
        events[1],
        ModelStreamEvent::ContentBlockStart { start: None }
    ));
    assert!(matches!(
        &events[2],
        ModelStreamEvent::ContentBlockDelta {
            delta: ContentBlockDelta::ReasoningContent { text: Some(t), .. }
        } if t == "think"
    ));
    assert!(matches!(events[3], ModelStreamEvent::ContentBlockStop {}));
    assert_eq!(events[5], ModelStreamEvent::text_delta("he"));
    assert_eq!(events[6], ModelStreamEvent::text_delta("llo"));
    assert!(matches!(events[7], ModelStreamEvent::ContentBlockStop {}));
    let ModelStreamEvent::Metadata {
        usage: Some(usage), ..
    } = &events[8]
    else {
        panic!("expected metadata, got {:?}", events[8]);
    };
    assert_eq!(usage.input_tokens, 5);
    assert_eq!(usage.total_tokens, 7);
    assert_eq!(usage.cache_read_input_tokens, Some(3));
    assert_eq!(usage.cache_write_input_tokens, None);
    assert_eq!(
        events[9],
        ModelStreamEvent::MessageStop {
            stop_reason: StopReason::EndTurn,
            additional_model_response_fields: None
        }
    );
}

#[tokio::test]
async fn responses_stream_replays_function_calls_at_the_end() {
    let body = concat!(
        "event: response.created\ndata: {\"response\":{\"id\":\"r\"}}\n\n",
        "event: response.output_item.added\ndata: {\"item\":{\"type\":\"function_call\",\"id\":\"item_1\",\"call_id\":\"call_1\",\"name\":\"weather\"}}\n\n",
        "event: response.function_call_arguments.delta\ndata: {\"item_id\":\"item_1\",\"delta\":\"{\\\"city\\\"\"}\n\n",
        "event: response.function_call_arguments.done\ndata: {\"item_id\":\"item_1\",\"arguments\":\"{\\\"city\\\":\\\"Berlin\\\"}\"}\n\n",
        "event: response.completed\ndata: {\"response\":{}}\n\n",
    );
    let events = collect(&model(OpenAIApi::Responses, body), &[user("hi")]).await;
    assert_eq!(
        events[1],
        ModelStreamEvent::ContentBlockStart {
            start: Some(ContentBlockStart::ToolUse {
                name: "weather".into(),
                tool_use_id: "call_1".into(),
                reasoning_signature: None,
            })
        }
    );
    assert_eq!(
        events[2],
        ModelStreamEvent::ContentBlockDelta {
            delta: ContentBlockDelta::ToolUseInput {
                input: "{\"city\":\"Berlin\"}".into()
            }
        }
    );
    assert!(matches!(events[3], ModelStreamEvent::ContentBlockStop {}));
    assert_eq!(
        events[4],
        ModelStreamEvent::MessageStop {
            stop_reason: StopReason::ToolUse,
            additional_model_response_fields: None
        }
    );
}

#[tokio::test]
async fn responses_incomplete_sets_max_tokens_stop_reason() {
    let body = "event: response.incomplete\ndata: {\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n";
    let events = collect(&model(OpenAIApi::Responses, body), &[user("hi")]).await;
    assert_eq!(
        events.last().unwrap(),
        &ModelStreamEvent::MessageStop {
            stop_reason: StopReason::MaxTokens,
            additional_model_response_fields: None
        }
    );
}

#[tokio::test]
async fn responses_failed_event_errors() {
    let body = "event: response.failed\ndata: {\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"boom\"}}}\n\n";
    let model = model(OpenAIApi::Responses, body);
    let options = stream_options();
    let messages = [user("hi")];
    let results: Vec<_> = model.stream(&messages, &options).collect().await;
    let err = results.into_iter().find_map(|r| r.err()).expect("error event");
    assert_eq!(err.to_string(), "boom");
}

#[test]
fn formats_chat_request_with_all_options() {
    let config = OpenAIModelConfig {
        api: OpenAIApi::Chat,
        api_key: Some("k".into()),
        model_id: Some("gpt-5.4".into()),
        temperature: Some(0.2),
        max_tokens: Some(64),
        top_p: Some(0.4),
        frequency_penalty: Some(0.1),
        presence_penalty: Some(0.3),
        ..Default::default()
    };
    let model =
        OpenAIModel::with_transport(config, Arc::new(StaticTransport { body: String::new() })).unwrap();
    let options = StreamOptions {
        system_prompt: Some(SystemPrompt::Text("sys".into())),
        tool_specs: vec![weather_tool()],
        tool_choice: Some(ToolChoice::Any {}),
        ..Default::default()
    };
    let request = model.format_request(&[user("hi")], &options).unwrap();
    assert_eq!(request.url, "https://api.openai.com/v1/chat/completions");
    let body = &request.body;
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(body["max_completion_tokens"], json!(64));
    assert_eq!(body["frequency_penalty"], json!(0.1));
    assert_eq!(body["presence_penalty"], json!(0.3));
    assert_eq!(body["tool_choice"], json!("required"));
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": "sys"},
            {"role": "user", "content": "hi"},
        ])
    );
    assert_eq!(body["tools"][0]["function"]["name"], json!("weather"));
}

#[test]
fn chat_rejects_tool_spec_without_description_and_n_above_one() {
    let model = model(OpenAIApi::Chat, "");
    let options = StreamOptions {
        tool_specs: vec![ToolSpec {
            name: "t".into(),
            description: String::new(),
            input_schema: None,
            output_schema: None,
            annotations: None,
        }],
        ..Default::default()
    };
    assert_eq!(
        model
            .format_request(&[user("hi")], &options)
            .unwrap_err()
            .to_string(),
        "Tool specification must have both name and description"
    );

    let config = OpenAIModelConfig {
        api: OpenAIApi::Chat,
        api_key: Some("k".into()),
        model_id: Some("gpt-5.4".into()),
        params: serde_json::from_value(json!({"n": 2})).unwrap(),
        ..Default::default()
    };
    let model =
        OpenAIModel::with_transport(config, Arc::new(StaticTransport { body: String::new() })).unwrap();
    assert_eq!(
        model
            .format_request(&[user("hi")], &stream_options())
            .unwrap_err()
            .to_string(),
        "Streaming with n > 1 is not supported"
    );
}

#[tokio::test]
async fn chat_stream_maps_text_tool_calls_and_buffered_usage() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"city\\\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"Berlin\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":6,\"total_tokens\":10}}\n\n",
        "data: [DONE]\n\n",
    );
    let events = collect(&model(OpenAIApi::Chat, body), &[user("hi")]).await;
    assert!(matches!(
        events[0],
        ModelStreamEvent::MessageStart {
            role: Role::Assistant
        }
    ));
    assert_eq!(events[2], ModelStreamEvent::text_delta("hi"));
    assert!(matches!(events[3], ModelStreamEvent::ContentBlockStop {}));
    assert_eq!(
        events[4],
        ModelStreamEvent::ContentBlockStart {
            start: Some(ContentBlockStart::ToolUse {
                name: "weather".into(),
                tool_use_id: "call_1".into(),
                reasoning_signature: None,
            })
        }
    );
    assert_eq!(
        events[5],
        ModelStreamEvent::ContentBlockDelta {
            delta: ContentBlockDelta::ToolUseInput {
                input: "{\"city\"".into()
            }
        }
    );
    let ModelStreamEvent::Metadata {
        usage: Some(usage), ..
    } = events.get(events.len() - 2).unwrap()
    else {
        panic!("expected buffered usage metadata: {events:?}");
    };
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.total_tokens),
        (4, 6, 10)
    );
    assert_eq!(
        events.last().unwrap(),
        &ModelStreamEvent::MessageStop {
            stop_reason: StopReason::ToolUse,
            additional_model_response_fields: None
        }
    );
}

#[test]
fn maps_openai_stop_reasons() {
    assert_eq!(chat::map_finish_reason("stop"), StopReason::EndTurn);
    assert_eq!(chat::map_finish_reason("tool_calls"), StopReason::ToolUse);
    assert_eq!(chat::map_finish_reason("length"), StopReason::MaxTokens);
    assert_eq!(
        chat::map_finish_reason("content_filter"),
        StopReason::ContentFiltered
    );
}

#[test]
fn handles_unknown_stop_reasons_with_warning() {
    assert_eq!(
        chat::map_finish_reason("some_new_reason"),
        StopReason::Other("someNewReason".into())
    );
}

#[test]
fn classifies_throttling_before_context_overflow() {
    assert!(matches!(
        classify_openai_error(429, None, "slow down"),
        Error::ModelThrottled { .. }
    ));
    assert!(matches!(
        classify_openai_error(400, Some("rate_limit_exceeded"), "nope"),
        Error::ModelThrottled { .. }
    ));
    assert!(matches!(
        classify_openai_error(400, None, "Too Many Requests"),
        Error::ModelThrottled { .. }
    ));
    assert!(matches!(
        classify_openai_error(429, Some("context_length_exceeded"), "maximum context length"),
        Error::ModelThrottled { .. }
    ));
}

#[test]
fn classifies_context_overflow() {
    for message in [
        "maximum context length is 128000 tokens",
        "context_length_exceeded",
        "too many tokens",
        "Input is too long for requested model",
        "input length and `max_tokens` exceed context limit",
        "too many total text bytes",
        "exceed customer model maximum",
    ] {
        assert!(
            matches!(
                classify_openai_error(400, None, message),
                Error::ContextWindowOverflow(_)
            ),
            "{message}"
        );
    }
    assert!(matches!(
        classify_openai_error(400, Some("context_length_exceeded"), "nope"),
        Error::ContextWindowOverflow(_)
    ));
}

#[test]
fn passes_other_errors_through() {
    assert!(matches!(
        classify_openai_error(500, Some("x"), "boom"),
        Error::Model { .. }
    ));
}

#[test]
fn stateful_only_for_responses_api() {
    let stateful = |api| {
        OpenAIModel::with_transport(
            OpenAIModelConfig {
                api,
                api_key: Some("k".into()),
                model_id: Some("gpt-5.4".into()),
                stateful: true,
                ..Default::default()
            },
            Arc::new(StaticTransport { body: String::new() }),
        )
        .unwrap()
        .stateful()
    };
    assert!(stateful(OpenAIApi::Responses));
    assert!(!stateful(OpenAIApi::Chat));
}

#[test]
fn mantle_config_derives_base_url() {
    let model = OpenAIModel::with_transport(
        OpenAIModelConfig {
            api_key: Some("token".into()),
            model_id: Some("openai.gpt-5.4".into()),
            mantle: Some(BedrockMantleConfig {
                region: Some("us-west-2".into()),
                expires_in_seconds: None,
            }),
            ..Default::default()
        },
        Arc::new(StaticTransport { body: String::new() }),
    )
    .unwrap();
    let request = model.format_request(&[user("hi")], &stream_options()).unwrap();
    assert_eq!(
        request.url,
        "https://bedrock-mantle.us-west-2.api.aws/openai/v1/responses"
    );
    assert_eq!(request.bearer_token.as_deref(), Some("token"));
}
