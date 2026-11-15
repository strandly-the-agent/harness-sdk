//! Ported from strands-ts/src/vended-tools/handoff-to-user/__tests__/handoff-to-user.test.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::Agent;
use crate::interrupt::InterruptResponse;
use crate::testing::MockMessageModel;
use crate::tools::tool::ToolStreamItem;
use crate::tools::{Tool, ToolOutcome};
use crate::types::messages::{ContentBlock, StopReason, TextBlock, ToolResultContent, ToolUseBlock};
use crate::vended_tools::test_support::{agent_with, context_for};

async fn outcome(tool: &FunctionTool, input: JsonValue) -> ToolOutcome {
    let agent = agent_with(None);
    let items: Vec<ToolStreamItem> = tool
        .stream(context_for(&agent, "handoff_to_user", input))
        .collect()
        .await;
    match items.into_iter().last() {
        Some(ToolStreamItem::Outcome(outcome)) => outcome,
        other => panic!("expected an outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn raises_an_interrupt_carrying_the_message_and_stable_name_on_first_call() {
    let ToolOutcome::Interrupt(interrupt) =
        outcome(&handoff_to_user(), json!({ "message": "please confirm" })).await
    else {
        panic!("expected an interrupt");
    };
    assert_eq!(interrupt.name(), HANDOFF_INTERRUPT_NAME);
    assert_eq!(interrupt.reason(), Some(&json!("please confirm")));
}

#[tokio::test]
async fn rejects_blank_message_without_registering_an_interrupt() {
    for blank in ["", "   "] {
        let ToolOutcome::Result(result) = outcome(&handoff_to_user(), json!({ "message": blank })).await
        else {
            panic!("expected an error result");
        };
        let text = crate::vended_tools::test_support::text_of(&result.content()[0]);
        assert!(text.contains("must not be empty"), "{text}");
    }
}

#[tokio::test]
async fn rejects_a_non_string_message_without_registering_an_interrupt() {
    assert!(matches!(
        outcome(&handoff_to_user(), json!({ "message": 42 })).await,
        ToolOutcome::Result(_)
    ));
}

#[test]
fn exposes_the_default_name_and_input_schema() {
    let tool = handoff_to_user();
    assert_eq!(tool.name(), "handoff_to_user");
    let spec = tool.tool_spec();
    let schema = spec.input_schema().unwrap();
    assert!(schema["properties"].get("message").is_some());
    assert!(schema["properties"].get("tool_context").is_none());
    assert_eq!(schema["required"], json!(["message"]));
}

#[test]
fn customizes_the_name_and_description_via_the_factory() {
    let tool = make_handoff_to_user(MakeHandoffToUserOptions {
        name: Some("ask_user".into()),
        description: Some("my desc".into()),
    });
    assert_eq!(
        (tool.name(), tool.tool_spec().description()),
        ("ask_user", "my desc")
    );
}

fn agent(tool: FunctionTool, turns: Vec<Vec<ContentBlock>>) -> Agent {
    let model = Arc::new(MockMessageModel::new());
    for turn in turns {
        model.add_turn(turn);
    }
    Agent::builder()
        .model(model as Arc<dyn crate::models::model::Model>)
        .tools([tool])
        .printer(false)
        .build()
        .unwrap()
}

#[tokio::test]
async fn halts_with_stop_reason_interrupt_and_the_message_as_the_reason() {
    let mut agent = agent(
        handoff_to_user(),
        vec![
            vec![
                ToolUseBlock::new(
                    "handoff_to_user",
                    "htu-1",
                    json!({ "message": "What is your address?" }),
                )
                .into(),
            ],
            vec![TextBlock::new("Thanks, got it.").into()],
        ],
    );
    let result = agent.invoke("Process my order").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert_eq!(result.interrupts().len(), 1);
    assert_eq!(result.interrupts()[0].name(), HANDOFF_INTERRUPT_NAME);
    assert_eq!(
        result.interrupts()[0].reason(),
        Some(&json!("What is your address?"))
    );
}

#[tokio::test]
async fn returns_the_reply_as_the_tool_result_and_the_model_continues_on_resume() {
    let mut agent = agent(
        handoff_to_user(),
        vec![
            vec![ToolUseBlock::new("handoff_to_user", "htu-2", json!({ "message": "Your email?" })).into()],
            vec![TextBlock::new("All done.").into()],
        ],
    );
    let result = agent.invoke("Verify me").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    let id = result.interrupts()[0].id().to_owned();
    let resumed = agent
        .invoke(vec![InterruptResponse::new(id, json!("yes@example.com"))])
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason(), StopReason::EndTurn);
    assert!(resumed.to_string().contains("All done"));
    let tool_result = agent
        .messages()
        .iter()
        .flat_map(crate::types::messages::Message::content)
        .find_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        tool_result.content()[0],
        ToolResultContent::from("yes@example.com")
    );
}

#[tokio::test]
async fn reports_the_stable_interrupt_name_even_when_the_tool_is_renamed() {
    let ask_user = make_handoff_to_user(MakeHandoffToUserOptions {
        name: Some("ask_user".into()),
        ..Default::default()
    });
    let mut agent = agent(
        ask_user,
        vec![
            vec![ToolUseBlock::new("ask_user", "htu-3", json!({ "message": "Enter PIN" })).into()],
            vec![TextBlock::new("PIN accepted.").into()],
        ],
    );
    let result = agent.invoke("Authenticate").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert_eq!(result.interrupts()[0].name(), HANDOFF_INTERRUPT_NAME);
}
