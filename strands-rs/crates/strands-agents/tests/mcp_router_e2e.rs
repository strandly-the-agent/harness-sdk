//! End-to-end `mcp_router` tests: the router drives the real `mcp_echo_server` stdio binary, so
//! `connect`, `list_tools`, `call_tool`, the cap/duplicate paths and `disconnect` all execute.

#![cfg(feature = "mcp")]

use serde_json::{json, Value as JsonValue};
use strands_agents::mcp::McpServerConfig;
use strands_agents::tools::vended::{McpRouterConfig, McpRouterInput, McpRouterTool};
use tokio_util::sync::CancellationToken;

const ECHO_BIN: &str = env!("CARGO_BIN_EXE_mcp_echo_server");

fn echo_server() -> McpServerConfig {
    McpServerConfig::stdio(ECHO_BIN, Vec::<String>::new())
}

fn router_with(servers: Vec<(String, McpServerConfig)>, max_connections: usize) -> McpRouterTool {
    let mut config = McpRouterConfig::new(servers);
    config.max_connections = max_connections;
    McpRouterTool::with_config(config).unwrap()
}

fn echo_router() -> McpRouterTool {
    McpRouterTool::with_servers([("echo".to_owned(), echo_server())]).unwrap()
}

async fn run(tool: &McpRouterTool, input: JsonValue) -> strands_agents::Result<JsonValue> {
    let parsed: McpRouterInput = serde_json::from_value(input).unwrap();
    tool.execute(&parsed, CancellationToken::new()).await
}

#[tokio::test]
async fn runs_the_full_connect_list_tools_call_tool_disconnect_flow() {
    let tool = echo_router();

    let connected = run(
        &tool,
        json!({"command": "connect", "server_name": "echo", "connection_id": "c1"}),
    )
    .await
    .unwrap();
    assert_eq!(connected, json!("Successfully connected to echo as \"c1\""));

    let ids = run(&tool, json!({"command": "list_connections"})).await.unwrap();
    assert_eq!(ids, json!("c1"));

    let tools = run(&tool, json!({"command": "list_tools", "connection_id": "c1"}))
        .await
        .unwrap();
    let names: Vec<&str> = tools
        .as_array()
        .expect("tool specs array")
        .iter()
        .map(|spec| spec["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"echo"), "{names:?}");

    let result = run(
        &tool,
        json!({
            "command": "call_tool",
            "connection_id": "c1",
            "tool_name": "echo",
            "arguments": {"text": "hello router"},
        }),
    )
    .await
    .unwrap();
    assert!(result.to_string().contains("hello router"), "{result:?}");

    let disconnected = run(&tool, json!({"command": "disconnect", "connection_id": "c1"}))
        .await
        .unwrap();
    assert_eq!(disconnected, json!("Successfully disconnected"));
    assert_eq!(
        run(&tool, json!({"command": "list_connections"})).await.unwrap(),
        json!("")
    );
}

#[tokio::test]
async fn returns_an_error_when_the_mcp_tool_reports_is_error() {
    let tool = echo_router();
    run(
        &tool,
        json!({"command": "connect", "server_name": "echo", "connection_id": "c1"}),
    )
    .await
    .unwrap();

    let error = run(
        &tool,
        json!({"command": "call_tool", "connection_id": "c1", "tool_name": "fail"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("boom"), "{error}");

    tool.disconnect_all().await;
}

#[tokio::test]
async fn disconnect_succeeds_when_the_server_is_already_gone() {
    // The server exits 300 ms in, so both calls below run against a dead child process.
    let short_lived = McpServerConfig::stdio(ECHO_BIN, vec!["--exit-after-ms".to_owned(), "300".to_owned()]);
    let tool = router_with(vec![("echo".to_owned(), short_lived)], 10);

    run(
        &tool,
        json!({"command": "connect", "server_name": "echo", "connection_id": "c1"}),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    run(
        &tool,
        json!({
            "command": "call_tool",
            "connection_id": "c1",
            "tool_name": "echo",
            "arguments": {"text": "hello"},
        }),
    )
    .await
    .expect_err("call_tool against a dead server must fail");

    let disconnected = run(&tool, json!({"command": "disconnect", "connection_id": "c1"}))
        .await
        .unwrap();
    assert_eq!(disconnected, json!("Successfully disconnected"));
    assert_eq!(
        run(&tool, json!({"command": "list_connections"})).await.unwrap(),
        json!("")
    );
}

#[tokio::test]
async fn enforces_the_connection_cap() {
    let tool = router_with(vec![("echo".to_owned(), echo_server())], 1);
    run(
        &tool,
        json!({"command": "connect", "server_name": "echo", "connection_id": "c1"}),
    )
    .await
    .unwrap();

    let error = run(
        &tool,
        json!({"command": "connect", "server_name": "echo", "connection_id": "c2"}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("Connection limit of 1 reached"), "{error}");
    assert!(error.contains("Active connection_ids: c1"), "{error}");
    assert_eq!(
        run(&tool, json!({"command": "list_connections"})).await.unwrap(),
        json!("c1")
    );

    tool.disconnect_all().await;
}

#[tokio::test]
async fn rejects_a_reconnect_with_the_same_id() {
    let tool = echo_router();
    run(
        &tool,
        json!({"command": "connect", "server_name": "echo", "connection_id": "c1"}),
    )
    .await
    .unwrap();

    let error = run(
        &tool,
        json!({"command": "connect", "server_name": "echo", "connection_id": "c1"}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(
        run(&tool, json!({"command": "list_connections"})).await.unwrap(),
        json!("c1")
    );

    tool.disconnect_all().await;
}

#[tokio::test]
async fn supports_multiple_simultaneous_connections() {
    let tool = echo_router();
    for id in ["c1", "c2"] {
        run(
            &tool,
            json!({"command": "connect", "server_name": "echo", "connection_id": id}),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        run(&tool, json!({"command": "list_connections"})).await.unwrap(),
        json!("c1, c2")
    );

    for id in ["c1", "c2"] {
        let result = run(
            &tool,
            json!({
                "command": "call_tool",
                "connection_id": id,
                "tool_name": "echo",
                "arguments": {"text": id},
            }),
        )
        .await
        .unwrap();
        assert!(result.to_string().contains(id), "{result:?}");
    }

    tool.disconnect_all().await;
    assert_eq!(
        run(&tool, json!({"command": "list_connections"})).await.unwrap(),
        json!("")
    );
}

#[tokio::test]
async fn cleans_up_when_the_server_fails_to_start() {
    let bad = McpServerConfig::stdio("strands-rs-no-such-binary", Vec::<String>::new());
    let tool = router_with(vec![("broken".to_owned(), bad)], 10);

    let error = run(
        &tool,
        json!({"command": "connect", "server_name": "broken", "connection_id": "c1"}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("strands-rs-no-such-binary") || error.contains("broken"),
        "the error must name the failing server or command: {error}"
    );
    assert_eq!(
        run(&tool, json!({"command": "list_connections"})).await.unwrap(),
        json!(""),
        "a failed connect must not leave a connection behind: {error}"
    );
}
