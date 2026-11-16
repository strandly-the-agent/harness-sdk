//! Ported from harness-ts/test/tools/programmatic-tool-caller.test.ts: the
//! `programmatic_tool_caller over MCP` cases, against the Node MCP server
//! `tests/fixtures/programmatic-tool-caller-mcp-server.mjs`.
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
use std::sync::Arc;

use serde_json::json;
use strands::agent::{Agent, DirectToolCallOptions};
use strands::mcp::{McpClient, McpClientConfig, McpTransport, StdioServerParameters};
use strands::testing::MockMessageModel;
use strands::types::messages::{ToolResultBlock, ToolResultContent};
use strands::types::tools::ToolResultStatus;
use strands::{Tool, ToolSource};
use strands_harness::programmatic_tool_caller;

/// An agent with `programmatic_tool_caller` and the tools of the connected fixture server.
struct McpAgent {
    agent: Agent,
    client: McpClient,
    _dir: tempfile::TempDir,
}

async fn mcp_agent() -> Option<McpAgent> {
    let (Ok(node), Ok(modules)) = (
        std::env::var("STRANDS_TEST_NODE"),
        std::env::var("STRANDS_TEST_NODE_MODULES"),
    ) else {
        eprintln!("skipping: STRANDS_TEST_NODE and STRANDS_TEST_NODE_MODULES are not set");
        return None;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/programmatic-tool-caller-mcp-server.mjs");
    std::fs::copy(fixture, dir.path().join("server.mjs")).expect("copy fixture");
    #[cfg(unix)]
    std::os::unix::fs::symlink(modules, dir.path().join("node_modules")).expect("link node_modules");
    #[cfg(not(unix))]
    {
        let _ = modules;
        eprintln!("skipping: the stdio fixture links node_modules, which these tests do on Unix only");
        return None;
    }
    let client = McpClient::new(McpClientConfig {
        transport: Some(McpTransport::stdio(StdioServerParameters {
            command: node,
            args: vec![dir.path().join("server.mjs").display().to_string()],
            ..Default::default()
        })),
        ..Default::default()
    })
    .expect("client");
    client.connect(false).await.expect("connect");
    let mut tools: Vec<ToolSource> = vec![programmatic_tool_caller().into()];
    for tool in client.list_tools().await.expect("tools") {
        tools.push(ToolSource::Tool(Arc::new(tool) as Arc<dyn Tool>));
    }
    let agent = Agent::builder()
        .model(MockMessageModel::new())
        .tools(tools)
        .build()
        .expect("the agent builds");
    Some(McpAgent {
        agent,
        client,
        _dir: dir,
    })
}

impl McpAgent {
    async fn run(&mut self, code: &str) -> ToolResultBlock {
        let options = DirectToolCallOptions {
            record_direct_tool_call: false,
        };
        self.agent
            .tool("programmatic_tool_caller")
            .invoke_with(json!({ "code": code }), options)
            .await
            .expect("the direct call returns a result")
    }

    async fn disconnect(self) {
        self.client.disconnect().await.expect("disconnect");
    }
}

fn text(result: &ToolResultBlock) -> String {
    let parts: Vec<&str> = result
        .content()
        .iter()
        .map(|block| match block {
            ToolResultContent::Text(text) => text.text(),
            _ => "",
        })
        .collect();
    parts.join("\n")
}

#[tokio::test]
async fn calls_an_mcp_tool() {
    let Some(mut mcp) = mcp_agent().await else { return };
    let result = mcp.run("print(await ptc_echo(text=\"hello\"))").await;
    assert_eq!(text(&result), "echo:hello");
    mcp.disconnect().await;
}

#[tokio::test]
async fn loops_over_mcp_tool_calls() {
    let Some(mut mcp) = mcp_agent().await else { return };
    let result = mcp
        .run("for i in range(3):\n    print(await ptc_add(a=i, b=10))")
        .await;
    assert_eq!(text(&result), "10\n11\n12");
    mcp.disconnect().await;
}

#[tokio::test]
async fn runs_mcp_tools_concurrently() {
    let Some(mut mcp) = mcp_agent().await else { return };
    let result = mcp
        .run("import asyncio\nprint(await asyncio.gather(*[ptc_echo(text=str(i)) for i in range(3)]))")
        .await;
    assert_eq!(text(&result), "['echo:0', 'echo:1', 'echo:2']");
    mcp.disconnect().await;
}

#[tokio::test]
async fn surfaces_an_mcp_tool_error_as_a_catchable_exception() {
    let Some(mut mcp) = mcp_agent().await else { return };
    let result = mcp
        .run("try:\n    await ptc_boom()\nexcept RuntimeError as e:\n    print('caught:', e)")
        .await;
    assert_eq!(*result.status(), ToolResultStatus::Success);
    assert!(text(&result).contains("mcp tool exploded"), "{}", text(&result));
    mcp.disconnect().await;
}

#[tokio::test]
async fn calls_a_hyphenated_mcp_tool_via_its_identifier_alias() {
    let Some(mut mcp) = mcp_agent().await else { return };
    let result = mcp.run("print(await ptc_dash(value=\"x\"))").await;
    assert_eq!(text(&result), "dash:x");
    mcp.disconnect().await;
}

#[tokio::test]
async fn programmatic_tool_caller_over_mcp_returns_only_printed_output() {
    let Some(mut mcp) = mcp_agent().await else { return };
    let result = mcp
        .run("x = await ptc_echo(text=\"secret\")\nprint(\"done\")")
        .await;
    assert_eq!(text(&result), "done");
    mcp.disconnect().await;
}
