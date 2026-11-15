//! Stdio transport tests against a Node MCP server (`tests/fixtures/mcp/echo-mcp-server.mjs`,
//! modelled on harness-ts/test/tools/echo-mcp-server.mjs).
//!
//! They need Node and a `node_modules` with `@modelcontextprotocol/sdk` and `zod`: set
//! `STRANDS_TEST_NODE` to the node binary and `STRANDS_TEST_NODE_MODULES` to that directory.
//! Without them the tests skip with a message.
#![cfg(feature = "mcp")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test code"
)]

use std::path::PathBuf;

use serde_json::json;
use strands::Tool;
use strands::mcp::{McpClient, McpClientConfig, McpTool, McpTransport, StdioServerParameters};

struct NodeServer {
    node: String,
    dir: tempfile::TempDir,
}

fn node_server() -> Option<NodeServer> {
    let (Ok(node), Ok(modules)) = (
        std::env::var("STRANDS_TEST_NODE"),
        std::env::var("STRANDS_TEST_NODE_MODULES"),
    ) else {
        eprintln!("skipping: STRANDS_TEST_NODE and STRANDS_TEST_NODE_MODULES are not set");
        return None;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/echo-mcp-server.mjs");
    std::fs::copy(fixture, dir.path().join("server.mjs")).expect("copy fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(modules, dir.path().join("node_modules")).expect("link node_modules");
    #[cfg(not(unix))]
    {
        let _ = modules;
        eprintln!("skipping: the stdio fixture links node_modules, which these tests do on Unix only");
        return None;
    }
    Some(NodeServer { node, dir })
}

impl NodeServer {
    fn parameters(&self) -> StdioServerParameters {
        StdioServerParameters {
            command: self.node.clone(),
            args: vec![self.dir.path().join("server.mjs").display().to_string()],
            ..Default::default()
        }
    }

    fn client(&self, parameters: StdioServerParameters) -> McpClient {
        McpClient::new(McpClientConfig {
            transport: Some(McpTransport::stdio(parameters)),
            ..Default::default()
        })
        .expect("client")
    }
}

async fn call(client: &McpClient, name: &str, args: serde_json::Value) -> String {
    let tools: Vec<McpTool> = client.list_tools().await.expect("tools");
    let tool = tools.iter().find(|tool| tool.name() == name).expect("tool");
    let result = client.call_tool(tool, args).await.expect("result");
    result
        .pointer("/content/0/text")
        .and_then(serde_json::Value::as_str)
        .expect("text")
        .to_owned()
}

#[tokio::test]
async fn calls_tools_on_a_stdio_server() {
    let Some(server) = node_server() else { return };
    let client = server.client(server.parameters());
    assert_eq!(
        call(&client, "echo", json!({"text": "stdio"})).await,
        "echo:stdio"
    );
    assert_eq!(call(&client, "add", json!({"a": 15, "b": 27})).await, "42");
    assert_eq!(
        client.negotiated_protocol_version().as_deref(),
        Some("2025-11-25")
    );
    client.disconnect().await.expect("disconnect");
}

#[tokio::test]
async fn merges_env_over_the_default_environment_and_drops_the_rest() {
    let Some(server) = node_server() else { return };
    let mut parameters = server.parameters();
    parameters.env = Some(
        [("STRANDS_MCP_TEST".to_owned(), "set".to_owned())]
            .into_iter()
            .collect(),
    );
    let client = server.client(parameters);
    assert_eq!(
        call(&client, "env", json!({"name": "STRANDS_MCP_TEST"})).await,
        "set"
    );
    let path = std::env::var("PATH").unwrap_or_else(|_| "<unset>".to_owned());
    assert_eq!(call(&client, "env", json!({"name": "PATH"})).await, path);
    assert_eq!(
        call(&client, "env", json!({"name": "STRANDS_TEST_NODE"})).await,
        "<unset>"
    );
    client.disconnect().await.expect("disconnect");
}

#[tokio::test]
async fn runs_the_child_in_cwd() {
    let Some(server) = node_server() else { return };
    let mut parameters = server.parameters();
    parameters.cwd = Some(server.dir.path().to_path_buf());
    let client = server.client(parameters);
    let cwd = call(&client, "cwd", json!({})).await;
    assert_eq!(
        std::fs::canonicalize(cwd).expect("cwd"),
        std::fs::canonicalize(server.dir.path()).expect("dir")
    );
    client.disconnect().await.expect("disconnect");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn disconnect_ends_the_child_process() {
    let Some(server) = node_server() else { return };
    let client = server.client(server.parameters());
    let pid = call(&client, "pid", json!({})).await;
    client.disconnect().await.expect("disconnect");
    let mut alive = true;
    for _ in 0..200 {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        alive = status
            .lines()
            .any(|line| line.starts_with("State:") && !line.contains("Z"));
        if !alive {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!alive, "child {pid} survived disconnect");
}

#[tokio::test]
async fn reconnects_after_disconnect() {
    let Some(server) = node_server() else { return };
    let client = server.client(server.parameters());
    assert_eq!(call(&client, "echo", json!({"text": "one"})).await, "echo:one");
    client.disconnect().await.expect("disconnect");
    assert_eq!(call(&client, "echo", json!({"text": "two"})).await, "echo:two");
    client.disconnect().await.expect("disconnect");
}
