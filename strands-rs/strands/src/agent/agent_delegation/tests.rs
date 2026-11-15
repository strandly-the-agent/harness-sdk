//! Ported from strands-ts/src/agent/__tests__/agent-delegation.test.ts.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;

use crate::agent::{Agent, AgentAsToolOptions, AgentStream};
use crate::error::ModelError;
use crate::hooks::{
    AfterToolCallEvent, BeforeToolCallEvent, BeforeToolsEvent, HookCallbackOptions, HookOrder,
    MessageAddedEvent,
};
use crate::models::model::{BaseModelConfig, Model};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::test_support::{mock_tool, text_result};
use crate::testing::MockMessageModel;
use crate::tools::{StructuredOutputSchema, Tool, ToolSource};
use crate::types::agent::{AgentResult, AgentStreamEvent};
use crate::types::json::JsonValue;
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, TextBlock, ToolResultBlock, ToolUseBlock,
};
use crate::types::tools::ToolResultStatus;

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn call(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

fn mock(turns: Vec<Vec<ContentBlock>>) -> MockMessageModel {
    let model = MockMessageModel::new();
    for turn in turns {
        model.add_turn(turn);
    }
    model
}

fn stateful(model: MockMessageModel) -> MockMessageModel {
    model.update_config(BaseModelConfig {
        model_info: Some(ModelInfo {
            capabilities: Some(ModelCapabilities {
                stateful: Some(true),
                ..ModelCapabilities::default()
            }),
            ..ModelInfo::default()
        }),
        ..BaseModelConfig::default()
    });
    model
}

fn sub_agent(name: &str, reply: &str) -> Agent {
    Agent::builder()
        .model(mock(vec![vec![text(reply)]]))
        .name(name)
        .build()
        .unwrap()
}

fn delegate(agent: Agent) -> ToolSource {
    agent
        .as_tool(AgentAsToolOptions::builder().delegate(true).build())
        .into()
}

fn orchestrator(model: MockMessageModel, tools: Vec<ToolSource>) -> Agent {
    Agent::builder()
        .model(model)
        .name("Orchestrator")
        .tools(tools)
        .build()
        .unwrap()
}

fn calculator() -> ToolSource {
    mock_tool("calculator", |context| {
        text_result(context.tool_use().tool_use_id(), "Result: 42")
    })
    .into()
}

fn first_text(result: &AgentResult) -> String {
    result
        .last_message()
        .content()
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn routes_to_the_correct_specialist_and_returns_stop_reason_end_turn() {
    let model = mock(vec![vec![call(
        "TechSupport",
        "call-1",
        json!({"input": "My wifi does not work"}),
    )]]);
    let mut agent = orchestrator(
        model,
        vec![
            delegate(sub_agent("Billing", "Your balance is $42.")),
            delegate(sub_agent("TechSupport", "Try rebooting your router.")),
        ],
    );

    let result = agent.invoke("My wifi does not work").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(
        result.last_message().content(),
        [text("Try rebooting your router.")]
    );
}

#[tokio::test]
async fn regular_tools_work_normally_when_called_alone() {
    let model = mock(vec![
        vec![call("calculator", "calc-1", json!({}))],
        vec![text("The answer is 42.")],
    ]);
    let mut agent = orchestrator(
        model,
        vec![
            calculator(),
            delegate(sub_agent("TechSupport", "Specialist response")),
        ],
    );

    let result = agent.invoke("What is 6 * 7?").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(result.last_message().content(), [text("The answer is 42.")]);
}

#[tokio::test]
async fn cancels_all_tools_when_delegation_tool_is_called_alongside_other_tools_then_retries() {
    let model = mock(vec![
        vec![
            call("calculator", "calc-1", json!({})),
            call("TechSupport", "tech-1", json!({"input": "help"})),
        ],
        vec![call("TechSupport", "tech-2", json!({"input": "help"}))],
    ]);
    let mut agent = orchestrator(
        model,
        vec![
            calculator(),
            delegate(sub_agent("TechSupport", "Specialist answer")),
        ],
    );

    let result = agent.invoke("Do both things").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "Specialist answer");
    let ContentBlock::ToolResult(cancelled) = &agent.messages()[2].content()[0] else {
        panic!("expected the cancelled batch's results");
    };
    assert_eq!(*cancelled.status(), ToolResultStatus::Error);
    assert_eq!(
        serde_json::to_value(cancelled.content()).unwrap(),
        json!([{"text": "This tool call was not executed. A delegation tool must be the only tool called in a turn. Retry with a single delegation tool call or use only non-delegation tools."}])
    );
}

#[tokio::test]
async fn cancels_all_tools_when_two_delegation_tools_are_called_simultaneously_then_retries() {
    let model = mock(vec![
        vec![
            call("Billing", "bill-1", json!({"input": "refund"})),
            call("TechSupport", "tech-1", json!({"input": "wifi"})),
        ],
        vec![call("TechSupport", "tech-2", json!({"input": "wifi"}))],
    ]);
    let mut agent = orchestrator(
        model,
        vec![
            delegate(sub_agent("Billing", "Billing response")),
            delegate(sub_agent("TechSupport", "Tech response")),
        ],
    );

    let result = agent.invoke("Handle both billing and tech").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "Tech response");
}

#[tokio::test]
async fn refuses_a_delegation_tool_that_shares_its_turn_when_a_later_hook_lifts_the_batch_cancel() {
    let model = mock(vec![
        vec![
            call("calculator", "calc-1", json!({})),
            call("TechSupport", "tech-1", json!({"input": "help"})),
        ],
        vec![text("Recovered.")],
    ]);
    let mut agent = orchestrator(
        model,
        vec![
            calculator(),
            delegate(sub_agent("TechSupport", "Specialist answer")),
        ],
    );
    agent.add_hook_with(
        HookCallbackOptions::builder().order(HookOrder::SDK_LAST).build(),
        |event: &mut BeforeToolsEvent<'_>| event.clear_cancel(),
    );

    let result = agent.invoke("Do both things").await.unwrap();

    assert_eq!(first_text(&result), "Recovered.");
    let results: Vec<JsonValue> = agent.messages()[2]
        .content()
        .iter()
        .map(|block| serde_json::to_value(block).unwrap()["toolResult"]["content"].clone())
        .collect();
    assert_eq!(
        results,
        [
            json!([{"text": "Result: 42"}]),
            json!([{"text": "Delegation failed: a delegation tool must be the only tool called in a turn. Retry with a single delegation tool call or use only non-delegation tools."}]),
        ]
    );
}

#[tokio::test]
async fn does_not_trigger_delegation_when_sub_agent_fails_loop_continues() {
    let failing = MockMessageModel::new();
    failing.add_error(|| ModelError::new("Sub-agent crashed").into());
    let failing = Agent::builder()
        .model(failing)
        .name("FailAgent")
        .no_retry_strategy()
        .build()
        .unwrap();
    let model = mock(vec![
        vec![call("FailAgent", "fail-1", json!({"input": "do something"}))],
        vec![text("Sorry, the specialist is unavailable.")],
    ]);
    let mut agent = orchestrator(model, vec![delegate(failing)]);

    let result = agent.invoke("Help me").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "Sorry, the specialist is unavailable.");
}

#[tokio::test]
async fn skips_delegation_when_sub_agent_returns_empty_text_and_loop_continues() {
    let model = mock(vec![
        vec![call("EmptyAgent", "empty-1", json!({"input": "go"}))],
        vec![text("I continued.")],
    ]);
    let mut agent = orchestrator(model, vec![delegate(sub_agent("EmptyAgent", ""))]);

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "I continued.");
}

#[tokio::test]
async fn skips_delegation_logic_for_delegation_tools_added_after_init_tool_runs_normally() {
    let model = stateful(mock(vec![
        vec![call("Sub", "del-1", json!({"input": "do something"}))],
        vec![text("Got the sub-agent response, done.")],
    ]));
    let mut agent = orchestrator(model, vec![]);
    agent.initialize().await.unwrap();
    let tool = sub_agent("Sub", "Sub response").as_tool(AgentAsToolOptions::builder().delegate(true).build());
    agent.tool_registry_mut().add(Arc::new(tool)).unwrap();

    let result = agent.invoke("Do something").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "Got the sub-agent response, done.");
}

#[tokio::test]
async fn does_not_enforce_single_call_constraint_for_stateful_models_with_runtime_added_tools() {
    let model = stateful(mock(vec![
        vec![
            call("calculator", "calc-1", json!({})),
            call("Sub", "del-1", json!({"input": "help"})),
        ],
        vec![text("Both tools ran successfully.")],
    ]));
    let mut agent = orchestrator(model, vec![calculator()]);
    agent.initialize().await.unwrap();
    let tool = sub_agent("Sub", "Sub response").as_tool(AgentAsToolOptions::builder().delegate(true).build());
    agent.tool_registry_mut().add(Arc::new(tool)).unwrap();

    let result = agent.invoke("Do both").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "Both tools ran successfully.");
    let statuses: Vec<ToolResultStatus> = agent.messages()[2]
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.status().clone()),
            _ => None,
        })
        .collect();
    assert_eq!(statuses, [ToolResultStatus::Success, ToolResultStatus::Success]);
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is read")]
struct Items {
    items: Vec<String>,
    count: u32,
}

#[tokio::test]
async fn converts_json_block_to_text_block_with_json_stringified_content() {
    let payload = json!({"items": ["a", "b"], "count": 2});
    let sub_model = mock(vec![
        vec![call("strands_structured_output", "so-1", payload.clone())],
        vec![text("")],
    ]);
    let json_agent = Agent::builder()
        .model(sub_model)
        .name("JsonAgent")
        .structured_output_schema(StructuredOutputSchema::of::<Items>())
        .build()
        .unwrap();
    let model = mock(vec![vec![call(
        "JsonAgent",
        "json-1",
        json!({"input": "get data"}),
    )]]);
    let mut agent = orchestrator(model, vec![delegate(json_agent)]);

    let result = agent.invoke("get data").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let [ContentBlock::Text(block)] = result.last_message().content() else {
        panic!("expected one text block");
    };
    assert_eq!(serde_json::from_str::<JsonValue>(block.text()).unwrap(), payload);
}

#[tokio::test]
async fn throws_when_delegation_tools_are_present_on_a_stateful_model() {
    let model = stateful(mock(vec![vec![text("Hi")]]));
    let mut agent = orchestrator(model, vec![delegate(sub_agent("Sub", "Hi"))]);

    let error = agent.initialize().await.unwrap_err();

    assert!(error.to_string().contains("not supported with stateful models"));
}

async fn collect(mut stream: AgentStream<'_>) -> Vec<AgentStreamEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    events
}

#[tokio::test]
async fn unwraps_inner_agent_stream_events_as_native_events_in_the_parent_stream() {
    let model = mock(vec![vec![call("Sub", "del-1", json!({"input": "handle this"}))]]);
    let mut agent = orchestrator(model, vec![delegate(sub_agent("Sub", "Delegated response"))]);

    let events = collect(agent.stream("handle this")).await;

    let types: Vec<String> = events
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(!types.contains(&"toolStreamUpdateEvent".to_owned()));
    let first_tool_call = types.iter().position(|t| t == "beforeToolCallEvent").unwrap();
    let inner = &types[first_tool_call..];
    assert!(inner.contains(&"modelStreamUpdateEvent".to_owned()));
    assert!(inner.contains(&"contentBlockEvent".to_owned()));
    assert!(matches!(events.last(), Some(AgentStreamEvent::AgentResult(_))));
}

#[tokio::test]
async fn does_not_stop_the_loop_when_a_later_after_tool_call_hook_changes_the_result_to_error() {
    let model = mock(vec![
        vec![call("Sub", "del-1", json!({"input": "do something"}))],
        vec![text("RECOVERED_AFTER_ERROR")],
    ]);
    let mut agent = orchestrator(model, vec![delegate(sub_agent("Sub", "SECRET_RAW"))]);
    // `AfterToolCallEvent` callbacks run in reverse, so this runs after the delegation hook
    // marked the call as delegated; `AfterToolsEvent` must then see the error and not end the turn.
    agent.add_hook(|event: &mut AfterToolCallEvent| {
        if event.tool_use().tool_use_id() == "del-1" {
            let id = event.tool_use().tool_use_id().to_owned();
            event.set_result(ToolResultBlock::new(
                id,
                ToolResultStatus::Error,
                vec!["REDACTED".into()],
            ));
        }
    });
    agent.initialize().await.unwrap();

    let result = agent.invoke("do something").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "RECOVERED_AFTER_ERROR");
}

#[tokio::test]
async fn child_delegation_does_not_leak_to_the_parent_agent() {
    let middle = Agent::builder()
        .model(mock(vec![vec![call("Leaf", "leaf-1", json!({"input": "go"}))]]))
        .name("Middle")
        .tools([delegate(sub_agent("Leaf", "Leaf response"))])
        .build()
        .unwrap();
    let model = mock(vec![
        vec![call("Middle", "mid-1", json!({"input": "go"}))],
        vec![text("PARENT_FINAL")],
    ]);
    let mut parent = Agent::builder()
        .model(model)
        .name("Parent")
        .tools([middle])
        .build()
        .unwrap();

    let result = parent.invoke("go").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(result.last_message().content(), [text("PARENT_FINAL")]);
}

#[tokio::test]
async fn does_not_promote_the_regular_tool_result_as_delegated_when_retry_swaps_to_a_non_delegation_tool() {
    let regular: Arc<dyn Tool> = Arc::new(mock_tool("regular", |context| {
        text_result(context.tool_use().tool_use_id(), "REGULAR_RESULT")
    }));
    let model = mock(vec![
        vec![call("Sub", "tool-1", json!({"input": "do something"}))],
        vec![text("PARENT_FOLLOW_UP")],
    ]);
    let mut agent = orchestrator(
        model,
        vec![
            delegate(sub_agent("Sub", "DELEGATED_ANSWER")),
            Arc::clone(&regular).into(),
        ],
    );
    let attempts = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&attempts);
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        if event.tool_use().tool_use_id() == "tool-1" {
            let mut attempts = counter.lock().unwrap();
            *attempts += 1;
            if *attempts == 1 {
                event.set_retry(true);
            }
        }
    });
    let counter = Arc::clone(&attempts);
    agent.add_hook(move |event: &mut BeforeToolCallEvent| {
        if event.tool_use().tool_use_id() == "tool-1" && *counter.lock().unwrap() == 1 {
            event.set_selected_tool(Arc::clone(&regular));
        }
    });

    let result = agent.invoke("do something").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(first_text(&result), "PARENT_FOLLOW_UP");
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is read")]
struct Answer {
    answer: String,
}

#[tokio::test]
async fn does_not_trigger_delegation_when_parent_has_a_structured_output_schema() {
    let model = mock(vec![
        vec![call("Sub", "del-1", json!({"input": "go"}))],
        vec![call(
            "strands_structured_output",
            "so-1",
            json!({"answer": "Sub answer"}),
        )],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .name("Orchestrator")
        .tools([delegate(sub_agent("Sub", "Sub answer"))])
        .structured_output_schema(StructuredOutputSchema::of::<Answer>())
        .build()
        .unwrap();

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::ToolUse);
    assert_eq!(
        result.structured_output_value(),
        Some(&json!({"answer": "Sub answer"}))
    );
}

#[tokio::test]
async fn emits_exactly_one_assistant_message_added_event_with_delegated_content_no_placeholder() {
    let model = mock(vec![vec![call("billing", "c1", json!({"input": "check"}))]]);
    let mut agent = orchestrator(model, vec![delegate(sub_agent("billing", "Balance: $42"))]);
    let assistant: Arc<Mutex<Vec<Message>>> = Arc::default();
    let sink = Arc::clone(&assistant);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        if event.message().role() == Role::Assistant {
            sink.lock().unwrap().push(event.message().clone());
        }
    });

    agent.invoke("Check balance").await.unwrap();

    let assistant = assistant.lock().unwrap();
    assert_eq!(assistant.len(), 2);
    assert_eq!(assistant[1].content(), [text("Balance: $42")]);
    assert_eq!(
        agent.messages().iter().map(Message::role).collect::<Vec<_>>(),
        [Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
}
