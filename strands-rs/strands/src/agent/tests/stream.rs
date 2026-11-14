//! Ported from strands-ts/src/agent/__tests__/agent.test.ts (the `stream`, `invoke`, `metrics`
//! and `API consistency` cases not in `agent.rs`).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;

use crate::agent::{Agent, AgentConfig, AgentResult, AgentStreamEvent};
use crate::error::Error;
use crate::hooks::{AfterToolsEvent, BeforeModelCallEvent, BeforeToolsEvent, EndTurn};
use crate::models::{Model, Usage};
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::ToolSource;
use crate::types::{ContentBlock, ImageBlock, ImageSource, Message, Role, StopReason, ToolUseBlock};

fn tool_use(name: &str, id: &str) -> ContentBlock {
    ContentBlock::ToolUse(ToolUseBlock::new(name, id, json!({})))
}

fn agent(model: &Arc<MockModel>, tools: Vec<ToolSource>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        tools,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn tool_agent(model: &Arc<MockModel>, text: &'static str) -> Agent {
    agent(
        model,
        crate::tool_list![mock_tool("testTool", move |ctx| text_result(ctx, text))],
    )
}

async fn collect(agent: &mut Agent, prompt: &str) -> (Vec<AgentStreamEvent>, AgentResult) {
    let mut events = Vec::new();
    let mut stream = agent.stream(prompt);
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    let Some(AgentStreamEvent::AgentResult(result)) = events.last().cloned() else {
        panic!("the last event is the result");
    };
    (events, result)
}

#[tokio::test]
async fn handles_tool_execution_flow() {
    let model = Arc::new(MockModel::new());
    model
        .add_turn(vec![tool_use("testTool", "tool-1")])
        .add_turn(vec!["Tool result processed".into()]);
    let mut agent = tool_agent(&model, "Tool executed");

    let (events, result) = collect(&mut agent, "Use the tool").await;

    assert!(events.iter().any(|event| matches!(
        event,
        AgentStreamEvent::BeforeTools(_) | AgentStreamEvent::AfterTools(_)
    )));
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
}

#[tokio::test]
async fn yields_tool_related_events() {
    let model = Arc::new(MockModel::new());
    model
        .add_turn(vec![tool_use("testTool", "tool-1")])
        .add_turn(vec!["Done".into()]);
    let mut agent = tool_agent(&model, "Success");

    let (events, _) = collect(&mut agent, "Test").await;

    let before = events
        .iter()
        .find_map(|event| match event {
            AgentStreamEvent::BeforeTools(data) => Some(data),
            _ => None,
        })
        .unwrap();
    assert_eq!(before.message.role, Role::Assistant);
    assert_eq!(before.message.content, [tool_use("testTool", "tool-1")]);
    let after = events
        .iter()
        .find(|event| matches!(event, AgentStreamEvent::AfterTools(_)))
        .unwrap();
    let mut json = serde_json::to_value(after).unwrap();
    json["message"].as_object_mut().unwrap().remove("trackingId");
    assert_eq!(
        json,
        json!({
            "type": "afterToolsEvent",
            "message": {
                "role": "user",
                "content": [{ "toolResult": {
                    "toolUseId": "tool-1",
                    "status": "success",
                    "content": [{ "text": "Success" }]
                } }]
            }
        })
    );
}

async fn rejects_a_blank_prompt_without_breaking_later_calls(prompt: crate::InvokeArgs) {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec!["Hello".into()]);
    let mut agent = agent(&model, Vec::new());

    let error = agent.invoke(prompt).await.unwrap_err();

    assert!(error.to_string().starts_with("Input prompt is blank"), "{error}");
    assert!(agent.messages().is_empty());
    assert_eq!(agent.invoke("Hi").await.unwrap().to_string(), "Hello");
}

#[tokio::test]
async fn rejects_a_blank_prompt_empty_string_without_breaking_later_calls() {
    rejects_a_blank_prompt_without_breaking_later_calls("".into()).await;
}

#[tokio::test]
async fn rejects_a_blank_prompt_whitespace_string_without_breaking_later_calls() {
    rejects_a_blank_prompt_without_breaking_later_calls("  \n".into()).await;
}

#[tokio::test]
async fn rejects_a_blank_prompt_blank_text_blocks_without_breaking_later_calls() {
    let blocks = vec![ContentBlock::from(" "), ContentBlock::from("\n")];
    rejects_a_blank_prompt_without_breaking_later_calls(blocks.into()).await;
}

#[tokio::test]
async fn rejects_a_blank_prompt_blank_content_block_data_without_breaking_later_calls() {
    rejects_a_blank_prompt_without_breaking_later_calls(vec![ContentBlock::from("")].into()).await;
}

#[tokio::test]
async fn rejects_a_blank_prompt_user_message_with_blank_text_without_breaking_later_calls() {
    let message = Message::new(Role::User, vec!["".into()]);
    rejects_a_blank_prompt_without_breaking_later_calls(vec![message].into()).await;
}

#[tokio::test]
async fn rejects_a_blank_prompt_user_message_with_no_content_without_breaking_later_calls() {
    let message = Message::new(Role::User, Vec::new());
    rejects_a_blank_prompt_without_breaking_later_calls(vec![message].into()).await;
}

async fn accepts_a_prompt_with_some_non_blank_content(blocks: Vec<ContentBlock>) {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec!["Hello".into()]);
    let mut agent = agent(&model, Vec::new());

    assert_eq!(agent.invoke(blocks).await.unwrap().to_string(), "Hello");
}

#[tokio::test]
async fn accepts_a_prompt_with_some_non_blank_content_text_alongside_blank_text() {
    accepts_a_prompt_with_some_non_blank_content(vec!["hello".into(), "".into()]).await;
}

#[tokio::test]
async fn accepts_a_prompt_with_some_non_blank_content_image_alongside_blank_text() {
    let image = ImageBlock::new("png", ImageSource::Bytes(vec![1]));
    accepts_a_prompt_with_some_non_blank_content(vec!["".into(), ContentBlock::Image(image)]).await;
}

#[tokio::test(start_paused = true)]
async fn includes_the_completed_cycle_duration_in_the_returned_result_metrics() {
    let model = Arc::new(MockModel::new());
    model.add_turn_with(
        vec![tool_use("slowTool", "tool-1")],
        None,
        Some(Usage::new(10, 5, 15)),
    );
    let mut agent = agent(
        &model,
        crate::tool_list![mock_tool("slowTool", |ctx| text_result(ctx, "Done"))],
    );
    agent.add_async_hook::<BeforeToolsEvent<'_>>(|_| {
        Box::pin(async { tokio::time::advance(Duration::from_millis(60)).await })
    });
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(EndTurn::Default));

    let result = agent.invoke("Test").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let metrics = result.metrics();
    assert_eq!(metrics.cycle_count, 1);
    assert_eq!(metrics.total_duration, Duration::from_millis(60));
    assert_eq!(metrics.average_cycle_time(), Duration::from_millis(60));
    let cycles = &metrics.latest_agent_invocation().unwrap().cycles;
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].duration, Duration::from_millis(60));
}

#[tokio::test(start_paused = true)]
async fn includes_the_cycle_duration_when_the_model_ends_the_turn_without_tools() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec!["Hello".into()]);
    let mut agent = agent(&model, Vec::new());
    agent.add_async_hook::<BeforeModelCallEvent<'_>>(|_| {
        Box::pin(async { tokio::time::advance(Duration::from_millis(60)).await })
    });

    let result = agent.invoke("Test").await.unwrap();

    let metrics = result.metrics();
    assert_eq!(metrics.cycle_count, 1);
    assert_eq!(metrics.total_duration, Duration::from_millis(60));
    assert_eq!(metrics.average_cycle_time(), Duration::from_millis(60));
    assert_eq!(
        metrics.latest_agent_invocation().unwrap().cycles[0].duration,
        Duration::from_millis(60)
    );
}

#[tokio::test]
async fn collects_local_traces_for_completed_cycles_when_error_occurs_mid_run() {
    let model = Arc::new(MockModel::new());
    model
        .add_turn_with(
            vec![tool_use("testTool", "tool-1")],
            None,
            Some(Usage::new(100, 50, 150)),
        )
        .add_turn_with(
            vec!["Partial".into()],
            Some(StopReason::MaxTokens),
            Some(Usage::new(80, 20, 100)),
        );
    let mut agent = tool_agent(&model, "Done");

    let error = agent.invoke("Test").await.unwrap_err();

    assert!(matches!(error, Error::MaxTokens(_)), "{error:?}");
    let traces = agent.tracer.traces();
    let shape: Vec<(String, Vec<String>)> = traces
        .iter()
        .map(|trace| {
            let children = trace.children.iter().map(|child| child.name.clone()).collect();
            (trace.name.clone(), children)
        })
        .collect();
    assert_eq!(
        shape,
        [
            (
                "Cycle 1".to_owned(),
                vec!["stream_messages".to_owned(), "Tool: testTool".to_owned()]
            ),
            ("Cycle 2".to_owned(), vec!["stream_messages".to_owned()]),
        ]
    );
}

#[tokio::test]
async fn both_methods_produce_same_result_with_tool_use() {
    let scripted = || {
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec![tool_use("testTool", "id")])
            .add_turn(vec!["Final".into()]);
        model
    };
    let first = scripted();
    let second = scripted();
    let mut invoked_agent = tool_agent(&first, "Tool ran");
    let mut streamed_agent = tool_agent(&second, "Tool ran");

    let invoked = invoked_agent.invoke("Test").await.unwrap();
    let (_, streamed) = collect(&mut streamed_agent, "Test").await;

    assert_eq!(invoked.stop_reason(), streamed.stop_reason());
    assert_eq!(invoked.last_message().content, streamed.last_message().content);
    assert_eq!(invoked_agent.messages().len(), streamed_agent.messages().len());
}
