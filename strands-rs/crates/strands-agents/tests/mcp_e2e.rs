//! End-to-end MCP test: spawns the `mcp_echo_server` binary (a real stdio MCP server built with
//! rmcp), connects `McpClient`, lists and calls its tool directly and through an `Agent`.

#![cfg(feature = "mcp")]

use std::sync::Arc;

use serde_json::json;
use strands_agents::mcp::{McpClient, McpServerConfig};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::types::{ContentBlock, Message, Role, ToolUseBlock};
use strands_agents::{Agent, StopReason};
use tokio_util::sync::CancellationToken;

fn echo_server() -> McpServerConfig {
    McpServerConfig::stdio(env!("CARGO_BIN_EXE_mcp_echo_server"), Vec::<String>::new())
}

#[tokio::test]
async fn lists_and_calls_a_tool_on_a_real_stdio_server() {
    let client = McpClient::connect(echo_server())
        .await
        .expect("connect to echo server");
    let tools = client.list_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"echo"), "{names:?}");

    let result = client
        .call_tool("echo", json!({"text": "hello mcp"}), CancellationToken::new())
        .await
        .unwrap();
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("hello mcp"), "{text}");
    client.disconnect().await.unwrap();
}

#[tokio::test]
async fn an_agent_calls_the_mcp_tool_and_gets_the_echo_back() {
    let client = McpClient::connect(echo_server())
        .await
        .expect("connect to echo server");
    let tools = client.list_tools().await.unwrap();

    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "echo",
                "call-1",
                json!({"text": "ping"}),
            ))],
        )),
        MockResponse::message(Message::assistant("done")),
    ]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tools(tools)
        .build()
        .unwrap();

    let result = agent.invoke("echo ping").await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    let history = agent.messages();
    let tool_result = history
        .iter()
        .flat_map(|m| m.tool_results())
        .next()
        .expect("tool result in history");
    assert_eq!(
        tool_result.status,
        strands_agents::types::ToolResultStatus::Success
    );
    assert!(tool_result.text().contains("ping"), "{:?}", tool_result.content);
    client.disconnect().await.unwrap();
}
