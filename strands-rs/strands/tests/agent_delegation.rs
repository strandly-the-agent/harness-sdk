//! Ported from strands-ts/src/agent/__tests__/agent-delegation.test.ts (the cases that need no
//! background tasks, middleware or stateful model).

#![expect(clippy::unwrap_used, reason = "test code")]

use std::sync::{Arc, Mutex};

use serde_json::json;
use strands::agent::AgentAsToolOptions;
use strands::hooks::MessageAddedEvent;
use strands::testing::{MockModel, test_agent};
use strands::tools::FunctionTool;
use strands::tools::structured_output::StructuredOutputSchema;
use strands::types::messages::{Role, ToolUseBlock};
use strands::{Agent, ContentBlock, Message, StopReason, ToolError, ToolSource};

fn specialist(name: &str, reply: &str) -> ToolSource {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from(reply)]);
    test_agent(model)
        .name(name)
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions {
            delegate: true,
            ..AgentAsToolOptions::default()
        })
        .into()
}

fn calculator() -> ToolSource {
    FunctionTool::new("calculator", "Calculates", |_, _| async move {
        Ok::<_, ToolError>(json!("Result: 42"))
    })
    .into()
}

fn call(name: &str, id: &str) -> ContentBlock {
    ToolUseBlock::new(name, id, json!({ "input": "help" })).into()
}

fn texts(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect()
}

fn orchestrator(turns: Vec<Vec<ContentBlock>>, tools: Vec<ToolSource>) -> Agent {
    let model = MockModel::new();
    for turn in turns {
        model.add_turn(turn);
    }
    test_agent(model)
        .name("Orchestrator")
        .tools(tools)
        .build()
        .unwrap()
}

#[tokio::test]
async fn routes_to_the_correct_specialist_and_returns_stop_reason_end_turn() {
    let mut agent = orchestrator(
        vec![vec![call("TechSupport", "call-1")]],
        vec![
            specialist("Billing", "Your balance is $42."),
            specialist("TechSupport", "Try rebooting your router."),
        ],
    );

    let result = agent.invoke("My wifi does not work").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["Try rebooting your router."]);
}

#[tokio::test]
async fn regular_tools_work_normally_when_called_alone() {
    let mut agent = orchestrator(
        vec![
            vec![ToolUseBlock::new("calculator", "calc-1", json!({})).into()],
            vec![ContentBlock::from("The answer is 42.")],
        ],
        vec![calculator(), specialist("TechSupport", "Specialist response")],
    );

    let result = agent.invoke("What is 6 * 7?").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["The answer is 42."]);
}

#[tokio::test]
async fn cancels_all_tools_when_delegation_tool_is_called_alongside_other_tools_then_retries() {
    let mut agent = orchestrator(
        vec![
            vec![
                ToolUseBlock::new("calculator", "calc-1", json!({})).into(),
                call("TechSupport", "tech-1"),
            ],
            vec![call("TechSupport", "tech-2")],
        ],
        vec![calculator(), specialist("TechSupport", "Specialist answer")],
    );

    let result = agent.invoke("Do both things").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["Specialist answer"]);
    let ContentBlock::ToolResult(cancelled) = &agent.messages()[2].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(
        serde_json::to_value(&cancelled.content).unwrap(),
        json!([{ "text": "This tool call was not executed. A delegation tool must be the only tool called in a turn. Retry with a single delegation tool call or use only non-delegation tools." }])
    );
}

#[tokio::test]
async fn cancels_all_tools_when_two_delegation_tools_are_called_simultaneously_then_retries() {
    let mut agent = orchestrator(
        vec![
            vec![call("Billing", "bill-1"), call("TechSupport", "tech-1")],
            vec![call("TechSupport", "tech-2")],
        ],
        vec![
            specialist("Billing", "Billing response"),
            specialist("TechSupport", "Tech response"),
        ],
    );

    let result = agent.invoke("Both").await.unwrap();

    assert_eq!(texts(result.last_message()), ["Tech response"]);
}

#[tokio::test]
async fn does_not_trigger_delegation_when_sub_agent_fails_loop_continues() {
    let failing_model = MockModel::new();
    failing_model.add_error(|| strands::error::ModelError::new("sub-agent failed").into());
    let failing: ToolSource = test_agent(failing_model)
        .name("TechSupport")
        .retry_strategy(Vec::new())
        .build()
        .unwrap()
        .as_tool(AgentAsToolOptions {
            delegate: true,
            ..AgentAsToolOptions::default()
        })
        .into();
    let mut agent = orchestrator(
        vec![
            vec![call("TechSupport", "tech-1")],
            vec![ContentBlock::from("Recovered.")],
        ],
        vec![failing],
    );

    let result = agent.invoke("Help").await.unwrap();

    assert_eq!(texts(result.last_message()), ["Recovered."]);
}

#[tokio::test]
async fn skips_delegation_when_sub_agent_returns_empty_text_and_loop_continues() {
    let mut agent = orchestrator(
        vec![
            vec![call("TechSupport", "tech-1")],
            vec![ContentBlock::from("Fallback.")],
        ],
        vec![specialist("TechSupport", "")],
    );

    let result = agent.invoke("Help").await.unwrap();

    assert_eq!(texts(result.last_message()), ["Fallback."]);
}

#[tokio::test]
async fn does_not_trigger_delegation_when_parent_has_a_structured_output_schema() {
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[expect(dead_code, reason = "only the schema is used")]
    struct Answer {
        answer: String,
    }
    let model = MockModel::new();
    model.add_turn(vec![call("TechSupport", "tech-1")]);
    model.add_turn(vec![
        ToolUseBlock::new("strands_structured_output", "so-1", json!({ "answer": "42" })).into(),
    ]);
    let mut agent = test_agent(model)
        .tools([specialist("TechSupport", "Delegated")])
        .structured_output_schema(StructuredOutputSchema::of::<Answer>())
        .build()
        .unwrap();

    let result = agent.invoke("Help").await.unwrap();

    assert_eq!(
        result.structured_output::<serde_json::Value>().unwrap(),
        json!({ "answer": "42" })
    );
}

#[tokio::test]
async fn emits_exactly_one_assistant_message_added_event_with_delegated_content() {
    let mut agent = orchestrator(
        vec![vec![call("TechSupport", "tech-1")]],
        vec![specialist("TechSupport", "Delegated answer")],
    );
    let added = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&added);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        if event.message().role == Role::Assistant
            && event
                .message()
                .content
                .iter()
                .all(|b| matches!(b, ContentBlock::Text(_)))
        {
            seen.lock().unwrap().push(texts(event.message()));
        }
    });

    agent.invoke("Help").await.unwrap();

    assert_eq!(*added.lock().unwrap(), [vec!["Delegated answer".to_owned()]]);
}
