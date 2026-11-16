//! Live MCP tests: a Node MCP test server over stdio (`tests/fixtures/mcp/live-mcp-server.mjs`,
//! a port of strands-ts/test/integ/__fixtures__/test-mcp-server.ts) plus a Bedrock `Agent` (TS
//! checklist: strands-ts/test/integ/mcp/mcp.test.node.ts).
//!
//! They need Node and a `node_modules` with `@modelcontextprotocol/sdk` and `zod` (set
//! `STRANDS_TEST_NODE` to the node binary and `STRANDS_TEST_NODE_MODULES` to that directory, as
//! for `tests/mcp_stdio.rs`) and AWS credentials from the default chain; without either they
//! skip with a message. Run with `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(all(feature = "mcp", feature = "bedrock"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr,
    reason = "test code"
)]

mod live_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use live_support::{
    bedrock, credentials_available, results_of, tool_attempted, tool_result_contains, tool_succeeded,
    tool_uses, trace,
};
use serde_json::{Value, json};
use strands::mcp::rmcp::model::ElicitResult;
use strands::mcp::{
    ElicitationCallback, McpClient, McpClientConfig, McpClientOptions, McpToolFilters, McpTransport,
    StdioServerParameters,
};
use strands::{Agent, StopReason};

/// The Node test server, copied next to a `node_modules` link so its imports resolve.
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
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/live-mcp-server.mjs");
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
    fn client(&self, options: McpClientOptions) -> McpClient {
        McpClient::new(McpClientConfig {
            transport: Some(McpTransport::stdio(StdioServerParameters {
                command: self.node.clone(),
                args: vec![self.dir.path().join("server.mjs").display().to_string()],
                ..Default::default()
            })),
            options,
            ..Default::default()
        })
        .expect("client")
    }

    fn filtered(&self, prefix: &str, allowed: &[&str]) -> McpClient {
        self.client(McpClientOptions {
            application_name: Some(format!("test-mcp-{prefix}")),
            prefix: Some(prefix.to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(allowed.iter().map(|name| (*name).into()).collect()),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

fn names(agent: &Agent) -> Vec<String> {
    let mut names: Vec<String> = agent.tools().map(|tool| tool.name().to_owned()).collect();
    names.sort();
    names
}

async fn direct(agent: &mut Agent, tool: &str, input: Value) -> Value {
    let result = agent
        .tool(tool)
        .invoke(input)
        .await
        .expect("the direct call succeeds");
    serde_json::to_value(result).unwrap()
}

/// TS cases: `agent can use multiple MCP tools in a conversation` and `agent handles MCP tool
/// errors gracefully`, over the stdio transport.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_mcp_agent_can_use_multiple_mcp_tools_and_handles_tool_errors() {
    let Some(server) = node_server() else { return };
    if !credentials_available().await {
        return;
    }
    let client = server.client(McpClientOptions {
        application_name: Some("test-mcp-stdio".into()),
        ..Default::default()
    });
    let mut agent = Agent::builder()
        .model(bedrock(400))
        .system_prompt(
            "You are a helpful assistant. Use the echo tool to echo messages and the calculator tool for math. \
             If asked to test errors, use the error_tool.",
        )
        .tools([client.clone()])
        .printer(false)
        .build()
        .unwrap();

    agent
        .invoke("Use the echo tool to say \"Multi-turn test\"")
        .await
        .unwrap();
    let messages = trace(&agent);
    assert!(tool_succeeded(&messages, "echo"));
    assert!(tool_result_contains(&messages, "echo", "Multi-turn test"));

    let result = agent
        .invoke("Now use the calculator tool to add 15 and 27")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    let call = tool_uses(&messages)
        .into_iter()
        .find(|tool_use| tool_use["name"] == "calculator")
        .expect("a calculator call");
    assert_eq!(call["input"]["operation"], "add");
    assert!(tool_result_contains(&messages, "calculator", "Result: 42"));

    let result = agent
        .invoke("Use the error_tool to test error handling.")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "error_tool"));
    assert!(
        results_of(&messages, "error_tool")
            .iter()
            .any(|result| result["status"] == "error")
    );
    agent.shutdown().await.unwrap();
    client.disconnect().await.unwrap();
}

/// TS case: `agent can use MCP tool that requests elicitation`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_mcp_agent_can_use_mcp_tool_that_requests_elicitation() {
    let Some(server) = node_server() else { return };
    if !credentials_available().await {
        return;
    }
    let asked = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&asked);
    let callback: ElicitationCallback = Arc::new(move |_context, _request| {
        seen.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            let accept: ElicitResult =
                serde_json::from_value(json!({"action": "accept", "content": {"confirmed": true}}))?;
            Ok(accept)
        })
    });
    let client = server.client(McpClientOptions {
        application_name: Some("test-mcp-elicitation".into()),
        elicitation_callback: Some(callback),
        ..Default::default()
    });
    let mut agent = Agent::builder()
        .model(bedrock(400))
        .system_prompt(
            "You are a helpful assistant. Use the confirm_action tool when asked to confirm something.",
        )
        .tools([client.clone()])
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke("Use the confirm_action tool to confirm \"deploy to production\"")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(asked.load(Ordering::SeqCst) >= 1);
    let messages = trace(&agent);
    assert!(tool_succeeded(&messages, "confirm_action"));
    assert!(tool_result_contains(
        &messages,
        "confirm_action",
        "confirmed by user"
    ));
    agent.shutdown().await.unwrap();
    client.disconnect().await.unwrap();
}

/// TS cases (no model call): `registers a prefixed filtered tool and executes it through the
/// direct Agent tool API`, `reuses one filtered and prefixed client across two Agents`,
/// `registers two distinct prefixes without collisions and invokes each raw server tool`, `lists
/// and renders prompts, reads static and templated resources`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_mcp_filtering_prefixing_prompts_and_resources() {
    let Some(server) = node_server() else { return };

    let shared = server.filtered("shared", &["echo"]);
    let mut first = Agent::builder()
        .model(bedrock(100))
        .tools([shared.clone()])
        .printer(false)
        .build()
        .unwrap();
    let mut second = Agent::builder()
        .model(bedrock(100))
        .tools([shared.clone()])
        .printer(false)
        .build()
        .unwrap();
    first.initialize().await.unwrap();
    second.initialize().await.unwrap();
    assert_eq!(names(&first), ["shared_echo"]);
    assert_eq!(names(&second), ["shared_echo"]);
    let result = direct(&mut first, "shared_echo", json!({"message": "Agent 1"})).await;
    assert_eq!(result["status"], "success");
    assert_eq!(result["content"][0]["text"], "Agent 1");
    let result = direct(&mut second, "shared_echo", json!({"message": "Agent 2"})).await;
    assert_eq!(result["content"][0]["text"], "Agent 2");
    shared.disconnect().await.unwrap();

    let echo = server.filtered("server1", &["echo"]);
    let calculator = server.filtered("server2", &["calculator"]);
    let mut agent = Agent::builder()
        .model(bedrock(100))
        .tools([echo.clone(), calculator.clone()])
        .printer(false)
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    assert_eq!(names(&agent), ["server1_echo", "server2_calculator"]);
    let result = direct(&mut agent, "server1_echo", json!({"message": "From Server 1"})).await;
    assert_eq!(result["content"][0]["text"], "From Server 1");
    let result = direct(
        &mut agent,
        "server2_calculator",
        json!({"operation": "add", "a": 2, "b": 3}),
    )
    .await;
    assert_eq!(result["content"][0]["text"], "Result: 5");
    echo.disconnect().await.unwrap();
    calculator.disconnect().await.unwrap();

    let client = server.client(McpClientOptions::default());
    client.connect(false).await.unwrap();
    let prompts = serde_json::to_value(client.list_prompts().await.unwrap()).unwrap();
    assert_eq!(prompts["prompts"].as_array().unwrap().len(), 1);
    assert_eq!(prompts["prompts"][0]["name"], "summarize");
    let arguments = [("topic".to_owned(), "AI".to_owned())].into_iter().collect();
    let prompt =
        serde_json::to_value(client.get_prompt("summarize", Some(arguments)).await.unwrap()).unwrap();
    assert_eq!(
        prompt["messages"][0],
        json!({"role": "user", "content": {"type": "text", "text": "Summarize the topic: AI"}})
    );
    let resources = serde_json::to_value(client.list_resources().await.unwrap()).unwrap();
    assert_eq!(resources["resources"].as_array().unwrap().len(), 1);
    assert_eq!(resources["resources"][0]["uri"], "test://greeting.txt");
    let read = serde_json::to_value(client.read_resource("test://greeting.txt").await.unwrap()).unwrap();
    assert_eq!(read["contents"][0]["text"], "Hello from the test resource");
    let templates = serde_json::to_value(client.list_resource_templates().await.unwrap()).unwrap();
    assert_eq!(templates["resourceTemplates"].as_array().unwrap().len(), 1);
    assert_eq!(
        templates["resourceTemplates"][0]["uriTemplate"],
        "test://users/{userId}"
    );
    let read = serde_json::to_value(client.read_resource("test://users/42").await.unwrap()).unwrap();
    assert_eq!(read["contents"][0]["text"], "Data for user 42");
    client.disconnect().await.unwrap();
}
