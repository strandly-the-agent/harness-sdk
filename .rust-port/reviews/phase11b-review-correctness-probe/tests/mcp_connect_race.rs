use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rmcp::model::{ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool};
use rmcp::service::{RequestContext, RunningService};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;
use strands::mcp::{McpClient, McpClientConfig, McpTransport};

#[derive(Clone, Default)]
struct Server;

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        let mut tool = Tool::default();
        tool.name = "echo".into();
        tool.input_schema = Arc::new(json!({"type": "object"}).as_object().unwrap().clone());
        Ok(ListToolsResult::with_all_items(vec![tool]))
    }
}

// Tasks on different worker threads use a fresh client at the same moment: one connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_calls_share_one_connection() {
    let (mut doubled, mut failed, rounds) = (0, 0, 3000);
    let mut first_error = None;
    for _ in 0..rounds {
        let opened = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&opened);
        let transport = McpTransport::from_factory(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let (client, pipe) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                if let Ok(running) = Server.serve(pipe).await {
                    let _ = RunningService::waiting(running).await;
                }
            });
            client
        });
        let client = McpClient::new(McpClientConfig { transport: Some(transport), ..Default::default() }).unwrap();
        let barrier = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let (client, barrier, ready) = (client.clone(), Arc::clone(&barrier), Arc::clone(&ready));
            tasks.push(tokio::spawn(async move {
                ready.fetch_add(1, Ordering::SeqCst);
                while !barrier.load(Ordering::Acquire) { tokio::task::yield_now().await; }
                client.list_tools().await.map(|tools| tools.len()).map_err(|error| error.to_string())
            }));
        }
        while ready.load(Ordering::SeqCst) < 4 { tokio::task::yield_now().await; }
        barrier.store(true, Ordering::Release);
        let mut round_failed = false;
        for task in tasks {
            if let Err(error) = task.await.unwrap() {
                round_failed = true;
                first_error.get_or_insert(error);
            }
        }
        failed += usize::from(round_failed);
        doubled += usize::from(opened.load(Ordering::SeqCst) > 1);
        client.disconnect().await.unwrap();
    }
    eprintln!("rounds={rounds} rounds_with_more_than_one_connection={doubled} rounds_with_a_failed_call={failed} first_error={first_error:?}");
    assert_eq!((doubled, failed), (0, 0));
}

// Callers that ask for a reconnect at the same moment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reconnects() {
    let (mut failed, mut left_disconnected, rounds) = (0, 0, 1500);
    let mut first_error = None;
    let mut opened_total = 0;
    for _ in 0..rounds {
        let opened = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&opened);
        let transport = McpTransport::from_factory(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let (client, pipe) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                if let Ok(running) = Server.serve(pipe).await {
                    let _ = RunningService::waiting(running).await;
                }
            });
            client
        });
        let client = McpClient::new(McpClientConfig { transport: Some(transport), ..Default::default() }).unwrap();
        client.connect(false).await.unwrap();
        let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut tasks = Vec::new();
        for index in 0..4 {
            let (client, go) = (client.clone(), Arc::clone(&go));
            tasks.push(tokio::spawn(async move {
                while !go.load(Ordering::Acquire) { tokio::task::yield_now().await; }
                if index % 2 == 0 {
                    client.connect(true).await.map_err(|error| format!("connect(true): {error}"))
                } else {
                    client.list_tools().await.map(|_| ()).map_err(|error| format!("list_tools: {error}"))
                }
            }));
        }
        go.store(true, Ordering::Release);
        let mut round_failed = false;
        for task in tasks {
            if let Err(error) = task.await.unwrap() {
                round_failed = true;
                first_error.get_or_insert(error);
            }
        }
        failed += usize::from(round_failed);
        if client.list_tools().await.is_err() { left_disconnected += 1; }
        opened_total += opened.load(Ordering::SeqCst);
        client.disconnect().await.unwrap();
    }
    eprintln!("rounds={rounds} rounds_with_a_failed_call={failed} left_unusable={left_disconnected} connections_opened={opened_total} first_error={first_error:?}");
}
