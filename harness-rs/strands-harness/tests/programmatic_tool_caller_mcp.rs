//! Ported from harness-ts/test/tools/programmatic-tool-caller.test.ts: the
//! `programmatic_tool_caller over MCP` cases, against the Node MCP server
//! `tests/fixtures/programmatic-tool-caller-mcp-server.mjs`.
//!
//! They need Node and a `node_modules` with `@modelcontextprotocol/sdk` and `zod`. By default
//! `node` is looked up on `PATH` and `node_modules` is the repository root's (after `npm ci`);
//! `STRANDS_TEST_NODE` and `STRANDS_TEST_NODE_MODULES` override either. Where one is missing
//! each case prints a `SKIPPED` line to stderr (uncaptured) and passes.
#![cfg(feature = "mcp")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::io::Write;
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

/// The node binary: `STRANDS_TEST_NODE`, or `node` on `PATH`.
fn node_binary() -> Option<PathBuf> {
    if let Some(node) = std::env::var_os("STRANDS_TEST_NODE") {
        return Some(PathBuf::from(node));
    }
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// A `node_modules` with the fixture's packages: `STRANDS_TEST_NODE_MODULES`, or the
/// repository root's.
fn node_modules() -> Option<PathBuf> {
    let modules = std::env::var_os("STRANDS_TEST_NODE_MODULES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../node_modules"),
        PathBuf::from,
    );
    ["@modelcontextprotocol/sdk", "zod"]
        .iter()
        .all(|package| modules.join(package).is_dir())
        .then_some(modules)
}

/// Says on the real stderr, which the test harness does not capture, that a case did not run.
fn skipped(reason: &str) {
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED programmatic_tool_caller over MCP: {reason}"
    );
}

async fn mcp_agent() -> Option<McpAgent> {
    let Some(node) = node_binary() else {
        skipped("no `node` on PATH (or set STRANDS_TEST_NODE)");
        return None;
    };
    let Some(modules) = node_modules() else {
        skipped(
            "no node_modules with @modelcontextprotocol/sdk and zod at the repository root (run `npm ci`, or set STRANDS_TEST_NODE_MODULES)",
        );
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
        let _ = (node, modules);
        skipped("the stdio fixture links node_modules, which these tests do on Unix only");
        return None;
    }
    let client = McpClient::new(McpClientConfig {
        transport: Some(McpTransport::stdio(StdioServerParameters {
            command: node.display().to_string(),
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
