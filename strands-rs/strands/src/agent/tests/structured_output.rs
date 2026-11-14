//! Ported from strands-ts/src/agent/__tests__/agent.test.ts (`structured output` > `attempt limit`
//! and the remaining structured output cases).
//!
//! Gaps (reasons in PARITY.md): the checkpoint and history-offloading variants need
//! checkpointing (out of scope) and the context manager (Phase 6).

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::InterruptResponse;
use crate::agent::{Agent, AgentConfig, InvokeArgs, InvokeLimits, InvokeOptions};
use crate::error::{Error, ModelError};
use crate::hooks::{AfterInvocationEvent, AfterToolsEvent, BeforeToolsEvent, InterruptEvent};
use crate::models::{Model, Usage};
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::{STRUCTURED_OUTPUT_TOOL_NAME, ToolOutcome, ToolResultStatus, ToolSource};
use crate::types::{ContentBlock, Role, StopReason, ToolUseBlock};

fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": { "steps": { "type": "array", "items": { "type": "string" } } },
        "required": ["steps"],
        "additionalProperties": false
    })
}

fn valid() -> serde_json::Value {
    json!({ "steps": ["review"] })
}

fn text_reply() -> ContentBlock {
    "Plain text instead of structured output".into()
}

fn output_call(input: serde_json::Value, id: &str) -> ContentBlock {
    ContentBlock::ToolUse(ToolUseBlock::new(STRUCTURED_OUTPUT_TOOL_NAME, id, input))
}

fn unexpected_call() -> Error {
    ModelError::new("Unexpected model call").into()
}

fn agent(model: &Arc<MockModel>, tools: Vec<ToolSource>, schema: Option<serde_json::Value>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        tools,
        structured_output_schema: schema,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn so_agent(model: &Arc<MockModel>) -> Agent {
    agent(model, Vec::new(), Some(schema()))
}

fn attempts(limit: u64) -> InvokeOptions {
    InvokeOptions {
        limits: Some(InvokeLimits {
            structured_output_attempts: Some(limit),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn continuing(result: &crate::AgentResult) -> InvokeOptions {
    InvokeOptions {
        invocation_context: result.invocation_context().cloned(),
        ..Default::default()
    }
}

mod attempt_limit {
    use super::*;

    async fn retries_failed_attempts_until_the_limit(responses: [&str; 4], stop_reason: StopReason) {
        let block = |name: &str| match name {
            "text" => text_reply(),
            "invalid" => output_call(json!({}), "invalid"),
            "valid" => output_call(valid(), "valid"),
            _ => ContentBlock::ToolUse(ToolUseBlock::new("unknown_tool", "other", json!({}))),
        };
        let model = Arc::new(MockModel::new());
        for response in responses {
            model.add_turn(vec![block(response)]);
        }
        model.add_turn(vec![block("valid")]);
        let mut agent = so_agent(&model);

        let result = agent.invoke_with("Plan a review", attempts(3)).await.unwrap();

        assert_eq!(result.stop_reason(), stop_reason);
        let expected = (stop_reason == StopReason::ToolUse).then(valid);
        assert_eq!(result.structured_output_value(), expected.as_ref());
        assert_eq!(result.last_message().content, [block(responses[3])]);
        assert_eq!(model.call_count(), 4);
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_the_limit_no_output_tool_calls() {
        retries_failed_attempts_until_the_limit(
            ["text", "text", "text", "text"],
            StopReason::LimitStructuredOutputAttempts,
        )
        .await;
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_the_limit_invalid_input_before_forcing() {
        retries_failed_attempts_until_the_limit(
            ["invalid", "text", "text", "text"],
            StopReason::LimitStructuredOutputAttempts,
        )
        .await;
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_the_limit_different_tool_called_when_forced() {
        retries_failed_attempts_until_the_limit(
            ["text", "other", "invalid", "text"],
            StopReason::LimitStructuredOutputAttempts,
        )
        .await;
    }

    #[tokio::test]
    async fn retries_failed_attempts_until_the_limit_valid_output_on_final_attempt() {
        retries_failed_attempts_until_the_limit(["text", "text", "invalid", "valid"], StopReason::ToolUse)
            .await;
    }

    #[tokio::test]
    async fn returns_the_model_reply_when_the_limit_is_reached_on_the_forced_retry() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec![text_reply()]).add_turn(vec![text_reply()]);
        let mut agent = so_agent(&model);

        let result = agent.invoke_with("Plan a review", attempts(1)).await.unwrap();

        assert_eq!(result.stop_reason(), StopReason::LimitStructuredOutputAttempts);
        assert_eq!(result.last_message().content, [text_reply()]);
        let roles: Vec<_> = agent.messages().iter().map(|message| message.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant]);
        assert_eq!(model.call_count(), 2);
        assert_eq!(&agent.messages()[1], result.last_message());
    }

    #[tokio::test]
    async fn returns_the_model_reply_when_a_turn_limit_is_reached_before_forcing() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec![text_reply()]).add_error(unexpected_call);
        let mut agent = so_agent(&model);
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                turns: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = agent.invoke_with("Plan a review", options).await.unwrap();

        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(result.last_message().content, [text_reply()]);
        assert_eq!(model.call_count(), 1);
        assert_eq!(&agent.messages()[1], result.last_message());
    }

    #[tokio::test]
    async fn throws_without_an_attempt_limit_when_the_forced_retry_fails() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec![text_reply()]);
        let mut agent = so_agent(&model);
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                turns: Some(5),
                ..Default::default()
            }),
            ..Default::default()
        };

        let error = agent.invoke_with("Plan a review", options).await.unwrap_err();

        assert!(matches!(error, Error::StructuredOutput(_)), "{error:?}");
        assert_eq!(error.to_string(), crate::tools::NO_STRUCTURED_OUTPUT_MESSAGE);
    }

    #[tokio::test]
    async fn counts_filtered_responses_toward_the_attempt_limit() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec![text_reply()]);
        for _ in 0..3 {
            model.add_turn_with(vec!["Filtered".into()], Some(StopReason::ContentFiltered), None);
        }
        model.add_turn(vec![output_call(valid(), "output")]);
        let mut agent = so_agent(&model);

        let result = agent.invoke_with("Plan a review", attempts(3)).await.unwrap();

        assert_eq!(result.stop_reason(), StopReason::LimitStructuredOutputAttempts);
        assert_eq!(result.structured_output_value(), None);
        assert_eq!(model.call_count(), 4);
    }

    #[tokio::test]
    async fn starts_each_root_call_with_a_fresh_attempt_budget() {
        let model = Arc::new(MockModel::new());
        for attempt in 0..6 {
            model.add_turn(vec![output_call(json!({}), &format!("output-{attempt}"))]);
        }
        model.add_turn(vec![output_call(valid(), "output")]);
        let mut agent = agent(&model, Vec::new(), None);
        let options = || InvokeOptions {
            structured_output_schema: Some(schema()),
            ..attempts(3)
        };

        let first = agent.invoke_with("Plan a review", options()).await.unwrap();
        let first_calls = model.call_count();
        let second = agent.invoke_with("Plan a review", options()).await.unwrap();

        assert_eq!(
            [first.stop_reason(), second.stop_reason()],
            [
                StopReason::LimitStructuredOutputAttempts,
                StopReason::LimitStructuredOutputAttempts
            ]
        );
        assert_eq!([first_calls, model.call_count()], [3, 6]);
        assert!(agent.tool_registry().is_empty());
        let last = agent.messages().last().unwrap();
        let [ContentBlock::ToolResult(result)] = last.content.as_slice() else {
            panic!("one tool result expected: {last:?}");
        };
        assert_eq!(result.status, ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn continues_the_attempt_budget_when_passed_the_previous_call_s_invocation_context() {
        let model = Arc::new(MockModel::new());
        for attempt in 0..3 {
            model.add_turn(vec![output_call(json!({}), &format!("output-{attempt}"))]);
        }
        let mut agent = so_agent(&model);

        let first = agent.invoke_with("Plan a review", attempts(2)).await.unwrap();
        let second = agent.invoke_with("Try again", continuing(&first)).await.unwrap();

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
    async fn preserves_the_attempt_budget_across_a_hook_resume() {
        let model = Arc::new(MockModel::new());
        for attempt in 0..6 {
            model.add_turn(vec![output_call(json!({}), &format!("output-{attempt}"))]);
        }
        let mut agent = so_agent(&model);
        let resume_input: Arc<Mutex<Option<InterruptResponse>>> = Arc::default();
        let counted = Arc::clone(&model);
        agent.add_hook(move |event: &mut BeforeToolsEvent<'_>| {
            if counted.call_count() == 3 {
                event.interrupt("approve");
            }
        });
        let stored = Arc::clone(&resume_input);
        agent.add_hook(move |event: &mut InterruptEvent<'_>| {
            let response = InterruptResponse::new(event.interrupt().id.clone(), json!("approved"));
            *stored.lock().unwrap() = Some(response);
        });
        let taken = Arc::clone(&resume_input);
        agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
            if let Some(response) = taken.lock().unwrap().take() {
                event.set_resume(response);
            }
        });

        let result = agent.invoke_with("Plan a review", attempts(3)).await.unwrap();

        assert_eq!(result.stop_reason(), StopReason::LimitStructuredOutputAttempts);
        assert_eq!(result.structured_output_value(), None);
        assert_eq!(model.call_count(), 3);
    }

    #[tokio::test]
    async fn counts_a_forced_response_that_calls_another_tool_before_an_interrupt() {
        let lookup = mock_tool("lookup", |context| text_result(context, "found"));
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec![text_reply()])
            .add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "lookup",
                "lookup",
                json!({}),
            ))])
            .add_error(unexpected_call);
        let mut agent = agent(&model, vec![ToolSource::from(lookup)], Some(schema()));
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| {
            event.interrupt("approve");
        });

        let paused = agent.invoke_with("Plan a review", attempts(1)).await.unwrap();
        let responses: Vec<_> = paused
            .interrupts()
            .iter()
            .map(|interrupt| InterruptResponse::new(interrupt.id.clone(), json!("approved")))
            .collect();
        let result = agent
            .invoke_with(InvokeArgs::from(responses), continuing(&paused))
            .await
            .unwrap();

        assert_eq!(
            [paused.stop_reason(), result.stop_reason()],
            [StopReason::Interrupt, StopReason::LimitStructuredOutputAttempts]
        );
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn counts_a_failed_attempt_when_a_hook_ends_the_turn() {
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec![output_call(json!({}), "invalid")])
            .add_error(unexpected_call);
        let mut agent = so_agent(&model);
        agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn("Paused for review"));

        let ended = agent.invoke_with("Plan a review", attempts(1)).await.unwrap();
        let result = agent.invoke_with("Continue", continuing(&ended)).await.unwrap();

        assert_eq!(
            [ended.stop_reason(), result.stop_reason()],
            [StopReason::EndTurn, StopReason::LimitStructuredOutputAttempts]
        );
        assert_eq!(model.call_count(), 1);
    }

    async fn returns_the_valid_output_of_a_response_that_also_has_an_invalid_call(blocks: Vec<ContentBlock>) {
        let model = Arc::new(MockModel::new());
        model.add_turn(blocks).add_error(unexpected_call);
        let mut agent = so_agent(&model);

        let result = agent.invoke_with("Plan a review", attempts(1)).await.unwrap();

        assert_eq!(result.structured_output_value(), Some(&valid()));
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn returns_the_valid_output_of_a_response_that_also_has_an_invalid_call_invalid_then_valid() {
        returns_the_valid_output_of_a_response_that_also_has_an_invalid_call(vec![
            output_call(json!({}), "invalid"),
            output_call(valid(), "valid"),
        ])
        .await;
    }

    #[tokio::test]
    async fn returns_the_valid_output_of_a_response_that_also_has_an_invalid_call_valid_then_invalid() {
        returns_the_valid_output_of_a_response_that_also_has_an_invalid_call(vec![
            output_call(valid(), "valid"),
            output_call(json!({}), "invalid"),
        ])
        .await;
    }

    #[tokio::test]
    async fn counts_one_attempt_per_model_response() {
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec![
                output_call(json!({}), "invalid-1"),
                output_call(json!({}), "invalid-2"),
            ])
            .add_turn(vec![output_call(valid(), "output")]);
        let mut agent = so_agent(&model);

        let result = agent.invoke_with("Plan a review", attempts(2)).await.unwrap();

        assert_eq!(result.structured_output_value(), Some(&valid()));
        assert_eq!(model.call_count(), 2);
    }

    async fn reports_the_stop_reason_before_the_attempt_limit(limits: InvokeLimits, stop_reason: StopReason) {
        let model = Arc::new(MockModel::new());
        model
            .add_turn_with(
                vec![output_call(json!({}), "output")],
                None,
                Some(Usage::new(1, 1, 2)),
            )
            .add_error(unexpected_call);
        let mut agent = so_agent(&model);
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                structured_output_attempts: Some(1),
                ..limits
            }),
            ..Default::default()
        };

        let result = agent.invoke_with("Plan a review", options).await.unwrap();

        assert_eq!(result.stop_reason(), stop_reason);
        assert_eq!(result.structured_output_value(), None);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn reports_limit_turns_before_the_attempt_limit() {
        let limits = InvokeLimits {
            turns: Some(1),
            ..Default::default()
        };
        reports_the_stop_reason_before_the_attempt_limit(limits, StopReason::LimitTurns).await;
    }

    #[tokio::test]
    async fn reports_limit_output_tokens_before_the_attempt_limit() {
        let limits = InvokeLimits {
            output_tokens: Some(1),
            ..Default::default()
        };
        reports_the_stop_reason_before_the_attempt_limit(limits, StopReason::LimitOutputTokens).await;
    }

    #[tokio::test]
    async fn reports_limit_total_tokens_before_the_attempt_limit() {
        let limits = InvokeLimits {
            total_tokens: Some(2),
            ..Default::default()
        };
        reports_the_stop_reason_before_the_attempt_limit(limits, StopReason::LimitTotalTokens).await;
    }

    #[tokio::test]
    async fn does_not_count_ordinary_tool_failures() {
        let unavailable = mock_tool("unavailable", |context| {
            ToolOutcome::Result(crate::tools::tool::error_result(
                &context.tool_use().tool_use_id,
                "service unavailable",
            ))
        });
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "unavailable",
                "service",
                json!({}),
            ))])
            .add_turn(vec![output_call(valid(), "output")]);
        let mut agent = agent(&model, vec![ToolSource::from(unavailable)], Some(schema()));

        let result = agent.invoke_with("Plan a review", attempts(1)).await.unwrap();

        assert_eq!(result.structured_output_value(), Some(&valid()));
        assert_eq!(model.call_count(), 2);
    }
}

#[tokio::test]
async fn preserves_validated_structured_output_with_disabled() {
    let answer = "Rust has a steep learning curve but produces fast single binaries. ".repeat(120);
    let model = Arc::new(MockModel::new());
    model
        .add_turn(vec![output_call(json!({ "message": answer }), "tool-1")])
        .add_turn(vec![output_call(
            json!({ "message": "Short fallback answer." }),
            "tool-2",
        )]);
    let mut agent = agent(&model, Vec::new(), None);
    let options = InvokeOptions {
        structured_output_schema: Some(json!({
            "type": "object",
            "properties": { "message": { "type": "string" } },
            "required": ["message"]
        })),
        ..Default::default()
    };

    let result = agent.invoke_with("Test", options).await.unwrap();

    let expected = json!({ "message": answer });
    assert_eq!(result.structured_output_value(), Some(&expected));
    assert_eq!(model.call_count(), 1);
    assert_eq!(
        serde_json::to_value(&agent.messages()[2].content).unwrap(),
        json!([{ "toolResult": {
            "toolUseId": "tool-1",
            "status": "success",
            "content": [{ "json": expected }]
        } }])
    );
}

#[tokio::test]
async fn does_not_send_assistant_ended_conversation_when_forcing_structured_output_retry() {
    let model = Arc::new(MockModel::new());
    model
        .add_turn(vec!["Plain text".into()])
        .add_turn(vec![output_call(valid(), "t1")]);
    let mut agent = so_agent(&model);

    agent.invoke("Test").await.unwrap();

    let retry = &model.requests()[1];
    assert_eq!(retry.messages.len(), 1);
    assert_eq!(retry.messages[0].role, Role::User);
    assert!(
        agent
            .messages()
            .iter()
            .all(|message| message.content != ["Plain text".into()])
    );
}

#[tokio::test]
async fn throws_max_tokens_error_when_max_tokens_reached_before_structured_output() {
    let model = Arc::new(MockModel::new());
    model.add_turn_with(vec!["partial".into()], Some(StopReason::MaxTokens), None);
    let mut agent = so_agent(&model);

    let error = agent.invoke("Test").await.unwrap_err();

    assert!(matches!(error, Error::MaxTokens(_)), "{error:?}");
}

#[tokio::test]
async fn works_without_structured_output_schema() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec!["Hello".into()]);
    let mut agent = agent(&model, Vec::new(), None);

    let result = agent.invoke("Test").await.unwrap();

    assert_eq!(result.structured_output_value(), None);
    assert_eq!(result.to_string(), "Hello");
    assert!(
        model.requests()[0]
            .tool_specs
            .iter()
            .all(|spec| spec.name != STRUCTURED_OUTPUT_TOOL_NAME)
    );
}

#[tokio::test]
async fn validates_nested_objects_in_structured_output() {
    let nested = json!({
        "type": "object",
        "properties": {
            "user": {
                "type": "object",
                "properties": { "name": { "type": "string" }, "age": { "type": "number" } },
                "required": ["name", "age"]
            }
        },
        "required": ["user"]
    });
    let output = json!({ "user": { "name": "John", "age": 30 } });
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![output_call(output.clone(), "t1")]);
    let mut agent = agent(&model, Vec::new(), Some(nested));

    let result = agent.invoke("Test").await.unwrap();

    assert_eq!(result.structured_output_value(), Some(&output));
}

#[tokio::test]
async fn validates_arrays_in_structured_output() {
    let output = json!({ "steps": ["a", "b", "c"] });
    let model = Arc::new(MockModel::new());
    model.add_turn(vec![output_call(output.clone(), "t1")]);
    let mut agent = so_agent(&model);

    let result = agent.invoke("Test").await.unwrap();

    assert_eq!(result.structured_output_value(), Some(&output));
}
