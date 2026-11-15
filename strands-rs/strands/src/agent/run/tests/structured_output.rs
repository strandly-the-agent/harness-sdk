//! Structured output, ported from strands-ts/src/agent/__tests__/agent.test.ts
//! (`structured output`) and the structured output case of agent.invocation-context.test.ts.

use std::num::NonZeroU32;

use schemars::JsonSchema;
use serde::Deserialize;

use super::*;
use crate::agent::InvocationContext;
use crate::error::StructuredOutputError;
use crate::tools::{STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema};
use crate::types::agent::InvokeLimits;
use crate::types::tools::ToolChoice;

const NO_OUTPUT: &str = "The model did not produce structured output: it neither called the structured output tool nor returned a JSON object.";
const TEXT_REPLY: &str = "Plain text instead of structured output";

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct Plan {
    steps: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct Value {
    value: f64,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct Person {
    name: String,
    age: f64,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct Name {
    name: String,
}

fn valid() -> JsonValue {
    json!({"steps": ["review"]})
}

fn output_call(input: JsonValue, id: &str) -> ContentBlock {
    tool_use(STRUCTURED_OUTPUT_TOOL_NAME, id, input)
}

fn schema_agent<T: serde::de::DeserializeOwned + JsonSchema + 'static>(
    model: &Arc<MockMessageModel>,
) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .structured_output_schema(StructuredOutputSchema::of::<T>())
        .build()
        .unwrap()
}

fn attempts(n: u32) -> InvokeOptions {
    InvokeOptions {
        limits: Some(InvokeLimits {
            structured_output_attempts: NonZeroU32::new(n),
            ..InvokeLimits::default()
        }),
        ..InvokeOptions::default()
    }
}

fn unexpected() -> Error {
    Error::other("Unexpected model call")
}

fn structured_error(result: crate::error::Result<AgentResult>) -> String {
    match result {
        Err(Error::StructuredOutput(error)) => error.to_string(),
        other => panic!("expected a StructuredOutputError, got {other:?}"),
    }
}

mod attempt_limit {
    use super::*;

    async fn retries_until_the_limit(responses: [&str; 4], stop_reason: StopReason) {
        let block = |name: &str| match name {
            "text" => text(TEXT_REPLY),
            "invalid" => output_call(json!({}), "invalid"),
            "valid" => output_call(valid(), "valid"),
            _ => tool_use("unknown_tool", "other", json!({})),
        };
        let model = Arc::new(MockMessageModel::new());
        for response in responses {
            model.add_turn(vec![block(response)]);
        }
        model.add_turn(vec![block("valid")]);
        let result = schema_agent::<Plan>(&model)
            .invoke_with("Plan a review", attempts(3))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), stop_reason);
        let expected_output = (stop_reason == StopReason::ToolUse).then(valid);
        assert_eq!(result.structured_output_value(), expected_output.as_ref());
        assert_eq!(result.last_message().content(), [block(responses[3])]);
        assert_eq!(model.call_count(), 4);
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_the_limit_with_no_output_tool_calls() {
        retries_until_the_limit(["text"; 4], StopReason::LimitStructuredOutputAttempts).await;
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_the_limit_with_invalid_input_before_forcing() {
        retries_until_the_limit(
            ["invalid", "text", "text", "text"],
            StopReason::LimitStructuredOutputAttempts,
        )
        .await;
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_the_limit_when_a_different_tool_is_called_when_forced() {
        retries_until_the_limit(
            ["text", "other", "invalid", "text"],
            StopReason::LimitStructuredOutputAttempts,
        )
        .await;
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_valid_output_on_the_final_attempt() {
        retries_until_the_limit(["text", "text", "invalid", "valid"], StopReason::ToolUse).await;
    }

    #[tokio::test]
    async fn returns_the_model_reply_when_the_limit_is_reached_on_the_forced_retry() {
        let model = mock(vec![vec![text(TEXT_REPLY)], vec![text(TEXT_REPLY)]]);
        let mut agent = schema_agent::<Plan>(&model);
        let result = agent.invoke_with("Plan a review", attempts(1)).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitStructuredOutputAttempts);
        assert_eq!(result.last_message().content(), [text(TEXT_REPLY)]);
        let roles: Vec<Role> = agent.messages().iter().map(Message::role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant]);
        assert_eq!(model.call_count(), 2);
        assert_eq!(&agent.messages()[1], result.last_message());
    }

    #[tokio::test]
    async fn returns_the_model_reply_when_a_turn_limit_is_reached_before_forcing() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![text(TEXT_REPLY)]).add_error(unexpected);
        let mut agent = schema_agent::<Plan>(&model);
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                turns: NonZeroU32::new(1),
                ..InvokeLimits::default()
            }),
            ..InvokeOptions::default()
        };
        let result = agent.invoke_with("Plan a review", options).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(result.last_message().content(), [text(TEXT_REPLY)]);
        assert_eq!(model.call_count(), 1);
        assert_eq!(&agent.messages()[1], result.last_message());
    }

    #[tokio::test]
    async fn fails_without_an_attempt_limit_when_the_forced_retry_fails() {
        let model = mock(vec![vec![text(TEXT_REPLY)]]);
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                turns: NonZeroU32::new(5),
                ..InvokeLimits::default()
            }),
            ..InvokeOptions::default()
        };
        let result = schema_agent::<Plan>(&model)
            .invoke_with("Plan a review", options)
            .await;
        assert_eq!(structured_error(result), NO_OUTPUT);
    }

    #[tokio::test]
    async fn counts_filtered_responses_toward_the_attempt_limit() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![text(TEXT_REPLY)]);
        for _ in 0..3 {
            model.add_turn_with(
                vec![text("Filtered")],
                MockTurnOptions {
                    stop_reason: Some(StopReason::ContentFiltered),
                    ..MockTurnOptions::default()
                },
            );
        }
        model.add_turn(vec![output_call(valid(), "output")]);
        let result = schema_agent::<Plan>(&model)
            .invoke_with("Plan a review", attempts(3))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitStructuredOutputAttempts);
        assert_eq!(result.structured_output_value(), None);
        assert_eq!(model.call_count(), 4);
    }

    #[tokio::test]
    async fn starts_each_root_call_with_a_fresh_attempt_budget() {
        let model = Arc::new(MockMessageModel::new());
        for attempt in 0..6 {
            model.add_turn(vec![output_call(json!({}), &format!("output-{attempt}"))]);
        }
        model.add_turn(vec![output_call(valid(), "output")]);
        let mut agent = test_agent(model.clone());
        let first = agent
            .invoke_with("Plan a review", attempts(3).structured_output_schema::<Plan>())
            .await
            .unwrap();
        let first_calls = model.call_count();
        let second = agent
            .invoke_with("Plan a review", attempts(3).structured_output_schema::<Plan>())
            .await
            .unwrap();
        assert_eq!(
            [first.stop_reason(), second.stop_reason()],
            [
                StopReason::LimitStructuredOutputAttempts,
                StopReason::LimitStructuredOutputAttempts
            ]
        );
        assert_eq!([first_calls, model.call_count()], [3, 6]);
        assert_eq!(agent.tool_registry().list().len(), 0);
        let last = agent.messages().last().unwrap();
        assert!(
            matches!(&last.content()[0], ContentBlock::ToolResult(result) if result.status() == &ToolResultStatus::Error)
        );
    }

    #[tokio::test]
    async fn continues_the_attempt_budget_when_passed_the_previous_call_context() {
        let model = Arc::new(MockMessageModel::new());
        for attempt in 0..3 {
            model.add_turn(vec![output_call(json!({}), &format!("output-{attempt}"))]);
        }
        let mut agent = schema_agent::<Plan>(&model);
        let first = agent.invoke_with("Plan a review", attempts(2)).await.unwrap();
        let options = InvokeOptions {
            invocation_context: first.invocation_context().cloned(),
            ..InvokeOptions::default()
        };
        let second = agent.invoke_with("Try again", options).await.unwrap();
        assert_eq!(
            [first.stop_reason(), second.stop_reason()],
            [
                StopReason::LimitStructuredOutputAttempts,
                StopReason::LimitStructuredOutputAttempts
            ]
        );
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn returns_the_valid_output_of_a_response_that_also_has_an_invalid_call() {
        for blocks in [
            vec![output_call(json!({}), "invalid"), output_call(valid(), "valid")],
            vec![output_call(valid(), "valid"), output_call(json!({}), "invalid")],
        ] {
            let model = Arc::new(MockMessageModel::new());
            model.add_turn(blocks).add_error(unexpected);
            let result = schema_agent::<Plan>(&model)
                .invoke_with("Plan a review", attempts(1))
                .await
                .unwrap();
            assert_eq!(result.structured_output_value(), Some(&valid()));
            assert_eq!(model.call_count(), 1);
        }
    }

    #[tokio::test]
    async fn counts_one_attempt_per_model_response() {
        let model = mock(vec![
            vec![
                output_call(json!({}), "invalid-1"),
                output_call(json!({}), "invalid-2"),
            ],
            vec![output_call(valid(), "output")],
        ]);
        let result = schema_agent::<Plan>(&model)
            .invoke_with("Plan a review", attempts(2))
            .await
            .unwrap();
        assert_eq!(result.structured_output_value(), Some(&valid()));
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn reports_other_limits_before_the_attempt_limit() {
        let cases = [
            (
                InvokeLimits {
                    turns: NonZeroU32::new(1),
                    ..InvokeLimits::default()
                },
                StopReason::LimitTurns,
            ),
            (
                InvokeLimits {
                    output_tokens: std::num::NonZeroU64::new(1),
                    ..InvokeLimits::default()
                },
                StopReason::LimitOutputTokens,
            ),
            (
                InvokeLimits {
                    total_tokens: std::num::NonZeroU64::new(2),
                    ..InvokeLimits::default()
                },
                StopReason::LimitTotalTokens,
            ),
        ];
        for (limits, stop_reason) in cases {
            let model = Arc::new(MockMessageModel::new());
            model
                .add_turn_with(
                    vec![output_call(json!({}), "output")],
                    MockTurnOptions {
                        usage: Some(Usage::new(1, 1, 2)),
                        ..MockTurnOptions::default()
                    },
                )
                .add_error(unexpected);
            let options = InvokeOptions {
                limits: Some(InvokeLimits {
                    structured_output_attempts: NonZeroU32::new(1),
                    ..limits
                }),
                ..InvokeOptions::default()
            };
            let result = schema_agent::<Plan>(&model)
                .invoke_with("Plan a review", options)
                .await
                .unwrap();
            assert_eq!(result.stop_reason(), stop_reason);
            assert_eq!(result.structured_output_value(), None);
            assert_eq!(model.call_count(), 1);
        }
    }

    #[tokio::test]
    async fn does_not_count_ordinary_tool_failures() {
        let unavailable = FunctionTool::new("unavailable", "fails", |_input: JsonValue, _ctx| async move {
            Err::<JsonValue, _>(ToolError::msg("service unavailable"))
        });
        let model = mock(vec![
            vec![tool_use("unavailable", "service", json!({}))],
            vec![output_call(valid(), "output")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([unavailable])
            .structured_output_schema(StructuredOutputSchema::of::<Plan>())
            .build()
            .unwrap();
        let result = agent.invoke_with("Plan a review", attempts(1)).await.unwrap();
        assert_eq!(result.structured_output_value(), Some(&valid()));
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn counts_a_nested_invoke_failures_toward_the_request() {
        let model = mock(vec![
            vec![output_call(json!({}), "tu-1")],
            vec![output_call(json!({}), "tu-2")],
        ]);
        let request = InvocationContext::new(
            Some(InvokeLimits {
                structured_output_attempts: NonZeroU32::new(1),
                ..InvokeLimits::default()
            }),
            None,
            false,
        );
        let options = InvokeOptions {
            invocation_context: Some(request.clone()),
            ..attempts(5)
        };
        let result = schema_agent::<Value>(&model)
            .invoke_with("go", options)
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitStructuredOutputAttempts);
        assert_eq!(model.call_count(), 1);
        assert_eq!(request.structured_output_failures(), 1);
    }
}

#[tokio::test]
async fn preserves_validated_structured_output_without_context_management() {
    let model = mock(vec![
        vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "tool-1",
            json!({"name": "answer"}),
        )],
        vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "tool-2",
            json!({"name": "fallback"}),
        )],
    ]);
    let mut agent = test_agent(model.clone());
    let result = agent
        .invoke_with(
            "Test",
            InvokeOptions::default().structured_output_schema::<Name>(),
        )
        .await
        .unwrap();
    assert_eq!(
        result.structured_output::<Name>().unwrap(),
        Name {
            name: "answer".into()
        }
    );
    assert_eq!(model.call_count(), 1);
    assert_eq!(
        agent.messages()[2].content(),
        [ToolResultBlock::new(
            "tool-1",
            ToolResultStatus::Success,
            vec![crate::types::messages::JsonBlock::new(json!({"name": "answer"})).into()],
        )
        .into()]
    );
}

#[tokio::test]
async fn returns_structured_output_when_the_tool_is_used() {
    let model = mock(vec![
        vec![output_call(json!({"name": "John", "age": 30}), "tool-1")],
        vec![text("Done")],
    ]);
    let result = schema_agent::<Person>(&model).invoke("Test").await.unwrap();
    assert_eq!(
        result.structured_output::<Person>().unwrap(),
        Person {
            name: "John".into(),
            age: 30.0
        }
    );
    assert_eq!(result.stop_reason(), StopReason::ToolUse);
}

#[tokio::test]
async fn forces_the_tool_when_the_model_does_not_use_it_without_sending_an_assistant_ended_conversation() {
    let model = mock(vec![
        vec![text("Plain text, no tool call")],
        vec![output_call(json!({"value": 42}), "tool-1")],
        vec![text("Done")],
    ]);
    let result = schema_agent::<Value>(&model).invoke("Test").await.unwrap();
    assert_eq!(result.structured_output_value(), Some(&json!({"value": 42})));
    let requests = model.requests();
    assert_eq!(requests[1].messages.last().unwrap().role(), Role::User);
    assert_eq!(
        requests[1].tool_choice,
        Some(ToolChoice::tool(STRUCTURED_OUTPUT_TOOL_NAME))
    );
}

#[tokio::test]
async fn fails_with_structured_output_error_when_the_model_refuses_the_forced_tool() {
    let model = mock(vec![vec![text("Response")]]);
    let result = schema_agent::<Value>(&model).invoke("Test").await;
    assert_eq!(structured_error(result), NO_OUTPUT);
}

#[tokio::test]
async fn passes_the_schema_on_the_retry_and_accepts_a_json_text_response() {
    let model = mock(vec![
        vec![text("Plain text, no tool call")],
        vec![text("{\"value\": 42}")],
    ]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([add_tool()])
        .structured_output_schema(StructuredOutputSchema::of::<Value>())
        .build()
        .unwrap();
    let result = agent.invoke("Test").await.unwrap();
    assert_eq!(result.structured_output_value(), Some(&json!({"value": 42})));
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let roles: Vec<Role> = agent.messages().iter().map(Message::role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant]);
    assert_eq!(&agent.messages()[1], result.last_message());
    let requests = model.requests();
    let schemas: Vec<bool> = requests
        .iter()
        .map(|r| r.structured_output_schema.is_some())
        .collect();
    assert_eq!(schemas, [false, true]);
    assert_eq!(
        requests[1].structured_output_schema.as_ref(),
        Some(StructuredOutputSchema::of::<Value>().json_schema())
    );
    let choices: Vec<Option<ToolChoice>> = requests.iter().map(|r| r.tool_choice.clone()).collect();
    assert_eq!(
        choices,
        [None, Some(ToolChoice::tool(STRUCTURED_OUTPUT_TOOL_NAME))]
    );
    let names: Vec<&str> = requests[1]
        .tool_specs
        .iter()
        .map(crate::types::tools::ToolSpec::name)
        .collect();
    assert_eq!(names, ["add", STRUCTURED_OUTPUT_TOOL_NAME]);
}

#[tokio::test]
async fn fails_when_the_json_text_response_fails_validation() {
    let model = mock(vec![
        vec![text("Plain text, no tool call")],
        vec![text("{\"value\": \"not a number\"}")],
    ]);
    let error = structured_error(schema_agent::<Value>(&model).invoke("Test").await);
    assert!(
        error.starts_with("Structured output failed validation: "),
        "{error}"
    );
}

#[tokio::test]
async fn fails_with_max_tokens_before_structured_output_and_cleans_up_the_tool() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn_with(
        vec![text("Partial...")],
        MockTurnOptions {
            stop_reason: Some(StopReason::MaxTokens),
            ..MockTurnOptions::default()
        },
    );
    let mut agent = schema_agent::<Value>(&model);
    let result = agent.invoke("Test").await;
    assert!(matches!(result, Err(Error::MaxTokens(_))), "{result:?}");
    assert!(agent.tool_registry().get(STRUCTURED_OUTPUT_TOOL_NAME).is_none());
}

#[tokio::test]
async fn retries_with_validation_feedback_when_the_tool_returns_an_error() {
    let model = mock(vec![
        vec![output_call(json!({"name": "John", "age": "invalid"}), "tool-1")],
        vec![output_call(json!({"name": "John", "age": 30}), "tool-2")],
        vec![text("Done")],
    ]);
    let mut agent = schema_agent::<Person>(&model);
    let result = agent.invoke("Test").await.unwrap();
    assert_eq!(
        result.structured_output_value(),
        Some(&json!({"name": "John", "age": 30}))
    );
    assert_eq!(model.call_count(), 2);
    let ContentBlock::ToolResult(feedback) = &agent.messages()[2].content()[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(
        (feedback.tool_use_id(), feedback.status()),
        ("tool-1", &ToolResultStatus::Error)
    );
}

#[tokio::test]
async fn works_without_a_schema() {
    let result = test_agent(mock(vec![vec![text("Hello")]]))
        .invoke("Test")
        .await
        .unwrap();
    assert_eq!(result.structured_output_value(), None);
    assert!(matches!(
        result.structured_output::<Value>(),
        Err(Error::StructuredOutput(StructuredOutputError { .. }))
    ));
}

#[tokio::test]
async fn cleans_up_the_tool_after_the_invocation() {
    let model = mock(vec![
        vec![output_call(json!({"value": 42}), "tool-1")],
        vec![text("Done")],
    ]);
    let mut agent = schema_agent::<Value>(&model);
    agent.invoke("Test").await.unwrap();
    assert!(agent.tool_registry().get(STRUCTURED_OUTPUT_TOOL_NAME).is_none());
}

#[tokio::test]
async fn validates_nested_objects_and_arrays() {
    #[derive(Deserialize, JsonSchema)]
    #[expect(dead_code, reason = "schema-only test type")]
    struct Nested {
        user: Person,
        items: Vec<String>,
    }
    let input = json!({"user": {"name": "Alice", "age": 25}, "items": ["a", "b", "c"]});
    let model = mock(vec![
        vec![output_call(input.clone(), "tool-1")],
        vec![text("Done")],
    ]);
    let result = schema_agent::<Nested>(&model).invoke("Test").await.unwrap();
    assert_eq!(result.structured_output_value(), Some(&input));
}

#[tokio::test]
async fn uses_a_per_invocation_override_and_restores_the_agent_schema_next_call() {
    let model = mock(vec![
        vec![output_call(json!({"value": 99}), "tool-1")],
        vec![text("Done")],
        vec![output_call(json!({"name": "Bob"}), "tool-2")],
        vec![text("Done")],
    ]);
    let mut agent = schema_agent::<Name>(&model);
    let first = agent
        .invoke_with(
            "First",
            InvokeOptions::default().structured_output_schema::<Value>(),
        )
        .await
        .unwrap();
    assert_eq!(first.structured_output_value(), Some(&json!({"value": 99})));
    let second = agent.invoke("Second").await.unwrap();
    assert_eq!(
        second.structured_output::<Name>().unwrap(),
        Name { name: "Bob".into() }
    );
}

#[tokio::test]
async fn a_user_tool_named_like_the_structured_output_tool_fails_the_invocation() {
    let clash = FunctionTool::new(
        STRUCTURED_OUTPUT_TOOL_NAME,
        "user tool",
        |_input: JsonValue, _ctx| async move { Ok::<_, ToolError>("x") },
    );
    let model = mock(vec![vec![text("Done")]]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([clash])
        .structured_output_schema(StructuredOutputSchema::of::<Value>())
        .build()
        .unwrap();
    assert!(agent.invoke("Test").await.is_err());
    assert!(agent.tool_registry().get(STRUCTURED_OUTPUT_TOOL_NAME).is_some());
}

#[tokio::test]
async fn a_dropped_stream_leaves_no_structured_output_tool_for_the_next_run() {
    let model = mock(vec![
        vec![text("Partial")],
        vec![output_call(json!({"value": 1}), "so-1")],
    ]);
    let mut agent = test_agent(model);
    {
        let mut stream = agent.stream_with(
            "First",
            InvokeOptions::default().structured_output_schema::<Value>(),
        );
        while let Some(event) = stream.next().await {
            if matches!(event.unwrap(), AgentStreamEvent::ModelStreamUpdate(_)) {
                break;
            }
        }
    }
    let result = agent
        .invoke_with(
            "Second",
            InvokeOptions::default().structured_output_schema::<Value>(),
        )
        .await
        .unwrap();
    assert_eq!(result.structured_output_value(), Some(&json!({"value": 1})));
    assert!(agent.tool_registry().get(STRUCTURED_OUTPUT_TOOL_NAME).is_none());
}
