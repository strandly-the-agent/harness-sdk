//! Middleware through the real agent loop (`middleware/__tests__/agent-middleware.test.ts`).

use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::Mutex;
use serde_json::json;
use strands_agents::error::Result;
use strands_agents::middleware::{
    wrap_fn, EventSink, ExecuteToolContext, ExecuteToolStage, InvokeModelContext, InvokeModelStage, Next,
};
use strands_agents::models::{ModelStreamEvent, StreamAggregatedResult};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{function_tool, ToolRef, ToolStreamEvent};
use strands_agents::types::{
    ContentBlock, Role, StopReason, ToolResultBlock, ToolResultContent, ToolUseBlock,
};
use strands_agents::{Agent, Message};

fn text_model(texts: &[&str]) -> Arc<MockMessageModel> {
    MockMessageModel::shared(
        texts
            .iter()
            .map(|t| MockResponse::message(Message::assistant(*t)))
            .collect(),
    )
}

fn echo_tool() -> ToolRef {
    Arc::new(
        function_tool("echo")
            .description("Echoes its input")
            .handler(|input, _ctx| async move { Ok(json!({ "echoed": input })) }),
    )
}

async fn agent_with(model: Arc<MockMessageModel>) -> Agent {
    Agent::builder().model_arc(model).build().await.unwrap()
}

#[tokio::test]
async fn middleware_handler_is_invoked_during_model_call() {
    let model = text_model(&["hi"]);
    let agent = agent_with(Arc::clone(&model)).await;
    let seen = Arc::new(Mutex::new(Vec::new()));

    agent.add_middleware::<InvokeModelStage>(wrap_fn({
        let seen = Arc::clone(&seen);
        move |ctx: InvokeModelContext, sink: EventSink<ModelStreamEvent>, next: Next<'_, InvokeModelStage>| {
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.lock().push(ctx.messages.len());
                next.run(ctx, &sink).await
            }) as BoxFuture<'_, Result<StreamAggregatedResult>>
        }
    }));

    let result = agent.invoke("hello").await.unwrap();
    assert_eq!(result.last_message.text(), "hi");
    assert_eq!(*seen.lock(), [1]);
    assert_eq!(model.call_count(), 1);
}

#[tokio::test]
async fn returns_synthetic_result_without_calling_the_model() {
    let model = text_model(&["never"]);
    let agent = agent_with(Arc::clone(&model)).await;

    agent.add_wrap_middleware::<InvokeModelStage>(wrap_fn(
        |_ctx: InvokeModelContext, _sink: EventSink<ModelStreamEvent>, _next: Next<'_, InvokeModelStage>| {
            Box::pin(async move {
                Ok(StreamAggregatedResult::new(
                    Message::assistant("synthetic"),
                    StopReason::EndTurn,
                ))
            }) as BoxFuture<'_, Result<StreamAggregatedResult>>
        },
    ));

    let result = agent.invoke("hello").await.unwrap();
    assert_eq!(result.last_message.text(), "synthetic");
    assert_eq!(model.call_count(), 0);
    assert_eq!(agent.messages().last().unwrap().text(), "synthetic");
}

#[tokio::test]
async fn modified_messages_are_passed_to_the_model() {
    let model = text_model(&["ok"]);
    let agent = agent_with(Arc::clone(&model)).await;

    agent.add_input_middleware::<InvokeModelStage>(|mut ctx: InvokeModelContext| {
        ctx.messages.push(Message::user("injected"));
        Ok(ctx)
    });

    agent.invoke("hello").await.unwrap();
    let calls = model.calls();
    assert_eq!(calls[0].0.len(), 2);
    assert_eq!(calls[0].0[1].text(), "injected");
    // The injected message is per-call only: the conversation itself is untouched.
    assert_eq!(agent.messages().len(), 2);
}

#[tokio::test]
async fn output_middleware_transforms_the_appended_message() {
    let model = text_model(&["raw"]);
    let agent = agent_with(model).await;

    agent.add_output_middleware::<InvokeModelStage>(|mut result: StreamAggregatedResult| {
        result.message = Message::assistant("rewritten");
        Ok(result)
    });

    let result = agent.invoke("hello").await.unwrap();
    assert_eq!(result.last_message.text(), "rewritten");
    assert_eq!(agent.messages().last().unwrap().text(), "rewritten");
}

#[tokio::test]
async fn returns_mock_tool_result_without_executing_the_real_tool() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "echo",
                "t1",
                json!({ "value": 1 }),
            ))],
        )),
        MockResponse::message(Message::assistant("done")),
    ]);
    let agent = Agent::builder()
        .model_arc(model)
        .tool_ref(echo_tool())
        .build()
        .await
        .unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    agent.add_middleware::<ExecuteToolStage>(wrap_fn({
        let seen = Arc::clone(&seen);
        move |ctx: ExecuteToolContext,
              _sink: EventSink<ToolStreamEvent>,
              _next: Next<'_, ExecuteToolStage>| {
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.lock().push(ctx.tool_use.name.clone());
                Ok(ToolResultBlock::success(
                    &ctx.tool_use.tool_use_id,
                    vec![ToolResultContent::Text("mocked".to_owned())],
                ))
            }) as BoxFuture<'_, Result<ToolResultBlock>>
        }
    }));

    agent.invoke("use the tool").await.unwrap();
    assert_eq!(*seen.lock(), ["echo"]);
    let results: Vec<String> = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|b| match b {
            ContentBlock::ToolResult(r) => Some(format!("{:?}", r.content)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("mocked"));
}
