use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock as Content, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool};
use rmcp::service::{RequestContext, RunningService};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;
use strands::Agent;
use strands::cancel::CancelOptions;
use strands::mcp::{McpClient, McpClientConfig, McpTransport};
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};

#[derive(Clone, Default)]
struct Server {
    started: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        let mut tool = Tool::default();
        tool.name = "slow".into();
        tool.input_schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        Ok(ListToolsResult::with_all_items(vec![tool]))
    }
    async fn call_tool(&self, _: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        self.started.store(true, Ordering::SeqCst);
        context.ct.cancelled().await;
        self.cancelled.store(true, Ordering::SeqCst);
        Ok(CallToolResult::success(vec![Content::text("cancelled")]).into())
    }
}

fn setup() -> (Server, McpClient, Agent) {
    let server = Server::default();
    let handler = server.clone();
    let transport = McpTransport::from_factory(move || {
        let (client, pipe) = tokio::io::duplex(64 * 1024);
        let handler = handler.clone();
        tokio::spawn(async move {
            if let Ok(running) = handler.serve(pipe).await {
                let _ = RunningService::waiting(running).await;
            }
        });
        client
    });
    let client = McpClient::new(McpClientConfig { transport: Some(transport), ..Default::default() }).unwrap();
    let model = MockMessageModel::new();
    model.add_turn(vec![ToolUseBlock::new("slow", "t1", json!({})).into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let agent = Agent::builder().model(model).tools([client.clone()]).printer(false).build().unwrap();
    (server, client, agent)
}

async fn wait(flag: &AtomicBool, limit: Duration) -> bool {
    let end = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < end {
        if flag.load(Ordering::SeqCst) { return true; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    flag.load(Ordering::SeqCst)
}

// Control: a signalled cancel reaches the server as `notifications/cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signalled_cancel_reaches_the_server() {
    let (server, _client, mut agent) = setup();
    let handle = agent.cancel_handle();
    let started = server.started.clone();
    tokio::spawn(async move {
        wait(&started, Duration::from_secs(5)).await;
        handle.cancel(CancelOptions::default());
    });
    let _ = agent.invoke("go").await;
    assert!(wait(&server.cancelled, Duration::from_secs(3)).await, "server never saw the cancel");
}

// A dropped invocation (P-91: "every drop is a cancel") must cancel the MCP request too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_invocation_cancels_the_mcp_request() {
    let (server, _client, mut agent) = setup();
    let dropped = tokio::time::timeout(Duration::from_secs(1), agent.invoke("go")).await;
    assert!(dropped.is_err(), "the invocation should still be waiting on the tool");
    assert!(server.started.load(Ordering::SeqCst), "tool call reached the server");
    agent.shutdown().await.unwrap();
    assert!(wait(&server.cancelled, Duration::from_secs(3)).await, "server never saw a cancel for the dropped call: the tool keeps running");
}

// A dropped direct call must release its progress callback (the route in `Inner.progress.routes`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_call_releases_its_progress_route() {
    use strands::mcp::{McpCallToolOptions, McpProgressCallback};
    let (server, client, _agent) = setup();
    let tool = client.list_tools().await.unwrap().remove(0);
    let callback: McpProgressCallback = Arc::new(|_| {});
    for _ in 0..5 {
        let options = McpCallToolOptions { progress_callback: Some(Arc::clone(&callback)), ..Default::default() };
        let call = client.call_tool_with(&tool, json!({}), options);
        assert!(tokio::time::timeout(Duration::from_millis(200), call).await.is_err());
    }
    assert!(server.started.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(Arc::strong_count(&callback), 1, "the client still holds the callbacks of the five dropped calls");
}
