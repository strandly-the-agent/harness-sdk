//! Live agent-loop tests against Bedrock (TS checklist: strands-ts/test/integ/agent.test.ts,
//! agent.cancel.test.ts, agent-as-tool.test.ts and interrupt.test.ts). They use the default AWS
//! credential chain and skip with a message when no credentials resolve. Run with
//! `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(feature = "bedrock")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod live_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;
use live_support::{
    bedrock, bedrock_with, credentials_available, last_assistant_text, resume_until_done, tool_attempted,
    tool_result_contains, tool_results, tool_succeeded, trace,
};
use serde_json::{Value, json};
use strands::agent::AgentAsToolOptions;
use strands::hooks::BeforeToolCallEvent;
use strands::interrupt::InterruptParams;
use strands::models::bedrock::{BedrockModel, BedrockModelConfig, BedrockModelOptions};
use strands::{Agent, AgentResult, CancelOptions, StopReason, ToolContext, ToolError, tool};

/// Performs basic arithmetic operations. Always use this tool for math.
#[tool]
async fn calculator(
    /// One of "add", "subtract", "multiply" or "divide".
    operation: String,
    a: f64,
    b: f64,
) -> Result<String, ToolError> {
    let value = match operation.as_str() {
        "add" => a + b,
        "subtract" => a - b,
        "multiply" => a * b,
        "divide" => a / b,
        other => return Err(ToolError::msg(format!("unknown operation {other}"))),
    };
    Ok(format!("Result: {value}"))
}

/// Returns the current time.
#[tool]
async fn time_tool() -> Result<String, ToolError> {
    Ok("12:00".into())
}

/// Returns the current weather.
#[tool]
async fn weather_tool() -> Result<String, ToolError> {
    Ok("sunny".into())
}

/// Returns the current time, which it asks the caller for.
#[tool(name = "time_tool")]
async fn interrupt_time_tool(#[tool(context)] ctx: &ToolContext) -> Result<String, ToolError> {
    let response = ctx.interrupt(InterruptParams {
        name: "test_interrupt".into(),
        reason: Some(json!("need time")),
        ..Default::default()
    })?;
    Ok(response.as_str().unwrap_or_default().to_owned())
}

/// Returns the shoulder height of an adult tiger.
#[tool]
async fn get_tiger_height() -> Result<String, ToolError> {
    Ok("An adult tiger stands about 100 cm at the shoulder.".into())
}

/// Drains a stream into the wire shape of its events and its result.
async fn collect(mut stream: strands::AgentStream<'_>) -> (Vec<Value>, Option<AgentResult>) {
    let mut events = Vec::new();
    let mut result = None;
    while let Some(event) = stream.next().await {
        let event = event.expect("the stream yields no error");
        events.push(serde_json::to_value(&event).unwrap());
        if let strands::AgentStreamEvent::AgentResult(done) = event {
            result = Some(done);
        }
    }
    (events, result)
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_handles_invocation_streaming_system_prompts_and_tool_use() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(300))
        .system_prompt(
            "Use the calculator tool to solve math problems. Respond with only the numeric result.",
        )
        .tools([Calculator])
        .printer(false)
        .build()
        .unwrap();
    let (events, result) = collect(agent.stream("What is 123 * 456?")).await;
    let result = result.expect("the stream ends with the result");
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "beforeInvocationEvent")
    );
    // The metadata event reaches the agent stream wrapped in a model stream update.
    let metadata = events
        .iter()
        .find(|event| {
            event["type"] == "modelStreamUpdateEvent" && event["event"]["type"] == "modelMetadataEvent"
        })
        .expect("a metadata event");
    assert!(metadata["event"]["usage"]["inputTokens"].as_u64().unwrap() > 0);
    assert!(metadata["event"]["usage"]["outputTokens"].as_u64().unwrap() > 0);
    assert!(metadata["event"]["metrics"]["latencyMs"].as_u64().unwrap() > 0);

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let usage = result.metrics().accumulated_usage();
    assert!(usage.input_tokens() > 0 && usage.output_tokens() > 0);
    assert!(result.metrics().cycle_count() >= 2);
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "calculator"));
    assert!(tool_succeeded(&messages, "calculator"));
    assert!(tool_result_contains(&messages, "calculator", "Result: 56088"));
    assert!(last_assistant_text(&messages).replace(',', "").contains("56088"));

    // Multi-turn works after tool use, and the history keeps its order.
    let before = agent.messages().len();
    let second = agent.invoke("What was the result?").await.unwrap();
    assert_eq!(second.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(messages.len() >= before + 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages.last().unwrap()["role"], "assistant");
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_maintains_message_history_and_conversation_context() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(200))
        .printer(false)
        .build()
        .unwrap();
    agent.invoke("My name is Alice").await.unwrap();
    assert_eq!(agent.messages().len(), 2);
    agent.invoke("What is my name?").await.unwrap();
    let messages = trace(&agent);
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
    assert!(last_assistant_text(&messages).contains("alice"));
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_emits_reasoning_content_with_thinking_model() {
    if !credentials_available().await {
        return;
    }
    let model = BedrockModel::new(BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some("global.anthropic.claude-sonnet-4-6".into()),
            max_tokens: Some(2048),
            additional_request_fields: Some(json!({"thinking": {"type": "enabled", "budget_tokens": 1024}})),
            ..Default::default()
        },
        region: Some(live_support::REGION.into()),
        ..Default::default()
    });
    let mut agent = Agent::builder().model(model).printer(false).build().unwrap();
    let (events, result) = collect(agent.stream("What is 15 * 23? Think it through.")).await;
    assert_eq!(result.unwrap().stop_reason(), StopReason::EndTurn);
    assert!(events.iter().any(|event| {
        event["type"] == "modelStreamUpdateEvent"
            && event["event"]["delta"]["type"] == "reasoningContentDelta"
    }));
    let messages = trace(&agent);
    let reasoning = messages[1]["content"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|block| block.get("reasoning"))
        .expect("the assistant message keeps its reasoning block");
    assert!(
        reasoning["signature"]
            .as_str()
            .is_some_and(|signature| !signature.is_empty())
    );
    // The signed reasoning block round-trips on the next turn.
    let second = agent.invoke("Now add 5 to it.").await.unwrap();
    assert_eq!(second.stop_reason(), StopReason::EndTurn);
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_cancels_during_model_streaming_and_allows_reuse_after_cancellation() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(1000))
        .system_prompt("Write a very long story about a dragon.")
        .printer(false)
        .build()
        .unwrap();
    let handle = agent.cancel_handle();
    let mut updates = 0;
    let mut result = None;
    {
        let mut stream = agent.stream("Begin");
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                strands::AgentStreamEvent::ModelStreamUpdate(_) => {
                    updates += 1;
                    if updates == 1 {
                        handle.cancel(CancelOptions::default());
                    }
                }
                strands::AgentStreamEvent::AgentResult(done) => result = Some(done),
                _ => {}
            }
        }
    }
    assert!(updates >= 1);
    assert_eq!(result.expect("a result").stop_reason(), StopReason::Cancelled);
    assert_eq!(trace(&agent).last().unwrap()["role"], "assistant");

    // The agent is reusable: the next invocation runs to the end.
    agent.set_system_prompt(Some("Reply with one short sentence.".into()));
    let result = agent.invoke("Say hello").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(!last_assistant_text(&trace(&agent)).is_empty());
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_cancels_before_tool_execution() {
    if !credentials_available().await {
        return;
    }
    let executed = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&executed);
    let tracked = strands::tools::FunctionTool::new(
        "calculator",
        "Performs basic arithmetic operations. Always use this tool for math.",
        move |_input: Value, _ctx| {
            seen.store(true, Ordering::SeqCst);
            async move { Ok::<_, ToolError>("Result: 110889") }
        },
    )
    .with_input_schema(json!({
        "type": "object",
        "properties": {
            "operation": {"type": "string", "enum": ["add", "subtract", "multiply", "divide"]},
            "a": {"type": "number"},
            "b": {"type": "number"}
        },
        "required": ["operation", "a", "b"]
    }));
    let mut agent = Agent::builder()
        .model(bedrock(300))
        .system_prompt("Use the calculator tool for all math. Do not attempt mental math.")
        .tools([tracked])
        .printer(false)
        .build()
        .unwrap();
    let handle = agent.cancel_handle();
    {
        let mut stream = agent.stream("What is 999 * 111?");
        while let Some(event) = stream.next().await {
            let event = serde_json::to_value(event.unwrap()).unwrap();
            if event["type"] == "modelMessageEvent" && event["stopReason"] == "toolUse" {
                handle.cancel(CancelOptions::default());
            }
        }
    }
    assert!(!executed.load(Ordering::SeqCst));
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "calculator"));
    assert!(
        tool_results(&messages)
            .iter()
            .any(|result| result["status"] == "error")
    );
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_parent_agent_invokes_a_sub_agent_tool_that_uses_a_standard_tool_and_gets_a_result() {
    if !credentials_available().await {
        return;
    }
    let inner = Agent::builder()
        .model(bedrock(500))
        .name("tiger_expert")
        .description("An agent knowledgeable about tigers")
        .tools([GetTigerHeight])
        .printer(false)
        .build()
        .unwrap();
    let tiger_expert = inner.as_tool(AgentAsToolOptions::default());
    let mut outer = Agent::builder()
        .model(bedrock(500))
        .tools([tiger_expert])
        .printer(false)
        .build()
        .unwrap();
    let result = outer
        .invoke("Ask the tiger_expert about the height of tigers.")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let metric = result
        .metrics()
        .tool_metric("tiger_expert")
        .expect("the sub-agent tool ran");
    assert!(metric.success_count() >= 1);
    assert!(tool_succeeded(&trace(&outer), "tiger_expert"));
    outer.shutdown().await.unwrap();
}

fn agent_with_approval_hook() -> Agent {
    let agent = Agent::builder()
        .model(bedrock(400))
        .tools(strands::tool_list![TimeTool, WeatherTool])
        .printer(false)
        .build()
        .unwrap();
    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name() == "weather_tool" {
            return;
        }
        let response = event.interrupt(InterruptParams {
            name: "test_interrupt".into(),
            reason: Some(json!("need approval")),
            ..Default::default()
        });
        if let Some(response) = response
            && response != json!("APPROVE")
        {
            event.set_cancel("tool rejected");
        }
    });
    agent
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_interrupts_before_tool_call_resumes_with_approval() {
    if !credentials_available().await {
        return;
    }
    let mut agent = agent_with_approval_hook();
    let result = agent.invoke("What is the time and weather?").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    let interrupt = result.interrupts().first().expect("at least one interrupt");
    assert_eq!(interrupt.name(), "test_interrupt");
    assert_eq!(interrupt.reason(), Some(&json!("need approval")));
    let done = resume_until_done(&mut agent, result, |_| json!("APPROVE")).await;
    assert_eq!(done.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_result_contains(&messages, "time_tool", "12:00"));
    assert!(tool_succeeded(&messages, "time_tool"));
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_interrupts_before_tool_call_resumes_with_rejection_cancels_tool() {
    if !credentials_available().await {
        return;
    }
    let mut agent = agent_with_approval_hook();
    let result = agent.invoke("What is the time and weather?").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    let done = resume_until_done(&mut agent, result, |_| json!("REJECT")).await;
    assert_eq!(done.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    let rejected = messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["toolResult"]["status"] == "error")
    });
    assert!(rejected);
    assert!(!tool_succeeded(&messages, "time_tool"));
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_interrupts_from_tool_callback_resumes_with_response() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock_with(live_support::MODEL_ID, 400))
        .tools(strands::tool_list![InterruptTimeTool, WeatherTool])
        .printer(false)
        .build()
        .unwrap();
    let result = agent.invoke("What is the time and weather?").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert!(!result.interrupts().is_empty());
    assert!(
        result
            .interrupts()
            .iter()
            .all(|interrupt| interrupt.response().is_none())
    );
    let done = resume_until_done(&mut agent, result, |interrupt| {
        if interrupt.reason() == Some(&json!("need time")) {
            json!("12:01")
        } else {
            json!("yes")
        }
    })
    .await;
    assert_eq!(done.stop_reason(), StopReason::EndTurn);
    assert!(tool_result_contains(&trace(&agent), "time_tool", "12:01"));
    agent.shutdown().await.unwrap();
}
