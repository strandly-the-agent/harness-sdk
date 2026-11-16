//! Connection lifetime, concurrency and auth behaviour beyond the ported TS cases.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::json;

use super::server::{self, TestServer, tool};
use super::*;
use crate::Agent;
use crate::testing::MockMessageModel;
use crate::types::messages::{ContentBlock, TextBlock, ToolUseBlock};

#[tokio::test(start_paused = true)]
async fn dropping_an_agent_after_a_tools_refresh_closes_the_connection() {
    let server = TestServer::default();
    let client = server.client();
    let model = MockMessageModel::new();
    model.add_turn(vec![ContentBlock::from(ToolUseBlock::new(
        "change_tools",
        "t1",
        json!({}),
    ))]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder()
        .model(model)
        .tools(vec![ToolSource::from(client.clone())])
        .printer(false)
        .build()
        .expect("agent");
    agent.invoke("go").await.expect("turn");
    server::eventually(|| server.list_calls.load(Ordering::SeqCst) >= 2).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let peer = client.peer().expect("peer");
    drop(agent);
    drop(client);
    server::eventually(|| peer.is_transport_closed()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_disconnect_right_after_connect_starts_leaves_the_client_disconnected() {
    let client = TestServer::default().client();
    let mut pending = Box::pin(client.connect(false));
    assert!(futures::poll!(&mut pending).is_pending());
    client.disconnect().await.expect("disconnect");
    let error = pending.await.expect_err("disconnected while connecting");
    assert_eq!(error.to_string(), "MCP client disconnected");
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
}

#[tokio::test(start_paused = true)]
async fn a_pending_tools_refresh_does_not_reconnect_after_disconnect() {
    let server = TestServer::default();
    let client = server.client();
    let tool = tool_named(&client, "change_tools").await;
    client.call_tool(&tool, json!({})).await.expect("result");
    tokio::time::sleep(Duration::from_millis(10)).await;
    client.disconnect().await.expect("disconnect");
    tokio::time::sleep(Duration::from_millis(500)).await;
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
    assert_eq!(server.list_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_before_the_result_reaches_the_callback_in_order() {
    let mut tools = TestServer::default().tools.lock().expect("tools").clone();
    tools.push(tool("progress_many", None));
    let client = TestServer::with_tools(tools).client();
    let tool = tool_named(&client, "progress_many").await;
    let expected: Vec<f64> = (1..=50).map(f64::from).collect();
    for _ in 0..30 {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let options = McpCallToolOptions {
            progress_callback: Some(Arc::new(move |progress: McpProgress| {
                sink.lock().expect("seen").push(progress.progress);
            })),
            ..Default::default()
        };
        client
            .call_tool_with(&tool, json!({}), options)
            .await
            .expect("call");
        assert_eq!(*seen.lock().expect("seen"), expected);
    }
}

/// A server that advertises tools only and rejects the prompt and resource methods.
#[derive(Clone)]
struct ToolsOnly;

impl rmcp::ServerHandler for ToolsOnly {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(rmcp::model::ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListPromptsResult, rmcp::ErrorData> {
        Err(rmcp::ErrorData::method_not_found::<
            rmcp::model::ListPromptsRequestMethod,
        >())
    }
}

#[tokio::test]
async fn lists_no_prompts_or_resources_from_a_server_without_those_capabilities() {
    use rmcp::ServiceExt;
    let transport = McpTransport::from_factory(|| {
        let (client, server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = ToolsOnly.serve(server).await {
                let _done = running.waiting().await;
            }
        });
        client
    });
    let client = McpClient::new(McpClientConfig {
        transport: Some(transport),
        ..Default::default()
    })
    .expect("client");
    let prompts = client.list_prompts().await.expect("prompts");
    let resources = client.list_resources().await.expect("resources");
    let templates = client.list_resource_templates().await.expect("templates");
    assert!(prompts.prompts.is_empty());
    assert!(resources.resources.is_empty());
    assert!(templates.resource_templates.is_empty());
}

/// The `Authorization` values of every MCP request (the ones accepting an event stream).
fn mcp_authorization(seen: &std::sync::Mutex<Vec<http::HeaderMap>>) -> Vec<Vec<String>> {
    seen.lock()
        .expect("headers")
        .iter()
        .filter(|headers| {
            headers
                .get(http::header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|accept| accept.contains("text/event-stream"))
        })
        .map(|headers| {
            headers
                .get_all(http::header::AUTHORIZATION)
                .iter()
                .map(|value| value.to_str().expect("ascii").to_owned())
                .collect()
        })
        .collect()
}

fn credentials() -> McpClientCredentials {
    McpClientCredentials {
        client_id: "id".to_owned(),
        client_secret: "secret".to_owned(),
        scopes: None,
    }
}

#[tokio::test]
async fn the_auth_token_replaces_a_user_authorization_header() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = server::serve_http_on(listener, TestServer::default(), Arc::clone(&seen), Arc::default());
    let client = McpClient::new(McpClientConfig {
        url: Some(url),
        auth: Some(credentials()),
        headers: Some(
            [("Authorization".to_owned(), "Bearer user-header".to_owned())]
                .into_iter()
                .collect(),
        ),
        ..Default::default()
    })
    .expect("client");
    client.list_tools().await.expect("tools");
    client.disconnect().await.expect("disconnect");
    let mcp_requests = mcp_authorization(&seen);
    assert!(!mcp_requests.is_empty());
    assert!(
        mcp_requests.iter().all(|values| values == &["Bearer tok-123"]),
        "{mcp_requests:?}"
    );
}

#[tokio::test]
async fn an_expired_client_credentials_token_is_granted_again() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tokens = Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let server = TestServer {
        token_lifetime_secs: 1,
        ..TestServer::default()
    };
    let url = server::serve_http_on(listener, server, Arc::clone(&seen), Arc::clone(&tokens));
    let client = McpClient::new(McpClientConfig {
        url: Some(url),
        auth: Some(credentials()),
        ..Default::default()
    })
    .expect("client");
    client.list_tools().await.expect("tools");
    client.list_tools().await.expect("tools again");
    let mcp_requests = mcp_authorization(&seen);
    assert!(tokens.lock().expect("tokens").len() >= 2);
    assert!(
        mcp_requests.iter().all(|values| values == &["Bearer tok-123"]),
        "{mcp_requests:?}"
    );
    client.disconnect().await.expect("disconnect");
}

fn mrtr_client(tools: &[&str], callback: ElicitationCallback) -> (TestServer, McpClient) {
    let mut listed = TestServer::default().tools.lock().expect("tools").clone();
    listed.extend(tools.iter().map(|name| tool(name, None)));
    let server = TestServer::with_tools(listed);
    let client = server.client_with(McpClientOptions {
        elicitation_callback: Some(callback),
        ..Default::default()
    });
    (server, client)
}

fn accepting() -> ElicitationCallback {
    Arc::new(|_context, _params| {
        Box::pin(async {
            Ok(serde_json::from_value(json!({"action": "accept", "content": {}})).expect("result"))
        })
    })
}

#[tokio::test]
async fn cancelling_a_call_cancels_its_pending_input_required_elicitation() {
    let saw_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let entered = Arc::new(tokio::sync::Notify::new());
    let (flag, notify) = (Arc::clone(&saw_cancel), Arc::clone(&entered));
    let callback: ElicitationCallback = Arc::new(move |context, _params| {
        let (flag, notify) = (Arc::clone(&flag), Arc::clone(&notify));
        let signal = context.cancel_signal().clone();
        tokio::spawn(async move {
            signal.cancelled().await;
            flag.store(true, Ordering::SeqCst);
        });
        notify.notify_one();
        Box::pin(futures::future::pending())
    });
    let (_server, client) = mrtr_client(&["elicit_mrtr"], callback);
    let tool = tool_named(&client, "elicit_mrtr").await;
    let (signal, handle) = crate::cancel::CancelSignal::new();
    let canceller = tokio::spawn(async move {
        entered.notified().await;
        handle.cancel(crate::CancelOptions::default());
    });
    let options = McpCallToolOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        client.call_tool_with(&tool, json!({}), options),
    )
    .await
    .expect("the call returns on cancel")
    .expect_err("cancelled");
    canceller.await.expect("join");
    assert_eq!(error.to_string(), "The operation was aborted");
    server::eventually(|| saw_cancel.load(Ordering::SeqCst)).await;
}

#[tokio::test(start_paused = true)]
async fn the_call_timeout_bounds_every_input_required_round() {
    let callback: ElicitationCallback = Arc::new(|_context, _params| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(600)).await;
            Ok(serde_json::from_value(json!({"action": "accept", "content": {}})).expect("result"))
        })
    });
    let (server, client) = mrtr_client(&["elicit_mrtr"], callback);
    let tool = tool_named(&client, "elicit_mrtr").await;
    let options = McpCallToolOptions {
        timeout: Some(Duration::from_millis(200)),
        ..Default::default()
    };
    let error = client
        .call_tool_with(&tool, json!({}), options)
        .await
        .expect_err("timed out");
    assert_eq!(error.to_string(), "Maximum total timeout exceeded");
    assert_eq!(server.calls.lock().expect("calls").len(), 1);
}

#[tokio::test]
async fn fulfils_ten_input_required_rounds_before_giving_up() {
    let rounds = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&rounds);
    let (server, client) = mrtr_client(&["always_input_required"], accepting());
    let tool = tool_named(&client, "always_input_required").await;
    let options = McpCallToolOptions {
        progress_callback: Some(Arc::new(move |progress: McpProgress| {
            sink.lock()
                .expect("rounds")
                .push(progress.message.unwrap_or_default());
        })),
        ..Default::default()
    };
    let error = client
        .call_tool_with(&tool, json!({}), options)
        .await
        .expect_err("rounds exceeded");
    assert_eq!(
        error.to_string(),
        "Multi-round-trip request 'tools/call' still required input after 10 rounds (inputRequired.maxRounds)"
    );
    assert_eq!(server.calls.lock().expect("calls").len(), 11);
    let rounds = rounds.lock().expect("rounds").clone();
    assert_eq!(rounds.len(), 10);
    assert_eq!(rounds[0], "Fulfilling input required by 'tools/call' (round 1)");
    assert_eq!(rounds[9], "Fulfilling input required by 'tools/call' (round 10)");
}

#[tokio::test(start_paused = true)]
async fn paces_a_request_state_only_round() {
    let (server, client) = mrtr_client(&["state_only"], accepting());
    let tool = tool_named(&client, "state_only").await;
    let started = tokio::time::Instant::now();
    let result = client.call_tool(&tool, json!({})).await.expect("result");
    assert_eq!(result["content"][0]["text"], json!("resumed"));
    assert!(started.elapsed() >= Duration::from_millis(250));
    let calls = server.calls.lock().expect("calls").clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].request_state.as_deref(), Some("wait"));
    assert_eq!(calls[1].input_responses, None);
}

#[tokio::test]
async fn names_the_missing_handler_for_an_input_request() {
    let mut listed = TestServer::default().tools.lock().expect("tools").clone();
    listed.push(tool("elicit_mrtr", None));
    let client = TestServer::with_tools(listed).client();
    let tool = tool_named(&client, "elicit_mrtr").await;
    let error = client.call_tool(&tool, json!({})).await.expect_err("no handler");
    assert_eq!(
        error.to_string(),
        "Cannot fulfil input request 'confirm': no handler is registered for 'elicitation/create' on this client. Declare the corresponding capability and register a handler, or handle input_required results manually."
    );
}

#[tokio::test]
async fn a_panicking_tools_changed_callback_leaves_later_refreshes_working() {
    let client = TestServer::default().client();
    client.list_tools().await.expect("tools");
    client.set_on_tools_changed(Some(Arc::new(|_, _| panic!("callback bug"))));
    client.refresh_tools().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sink = Arc::clone(&calls);
    client.set_on_tools_changed(Some(Arc::new(move |_, _| {
        sink.fetch_add(1, Ordering::SeqCst);
    })));
    client.refresh_tools().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn checks_structured_content_against_the_tool_output_schema() {
    let mut weather = tool("weather", None);
    weather.output_schema = Some(Arc::new(server::object(json!({
        "type": "object",
        "properties": {"forecast": {"type": "string"}},
        "required": ["forecast"]
    }))));
    let mut listed = TestServer::default().tools.lock().expect("tools").clone();
    listed.push(weather);
    let client = TestServer::with_tools(listed).client();
    let tool = tool_named(&client, "weather").await;

    let good = client
        .call_tool(&tool, json!({"message": "good"}))
        .await
        .expect("valid");
    assert_eq!(good["structuredContent"], json!({"forecast": "sunny"}));

    let missing = client.call_tool(&tool, json!({})).await.expect_err("missing");
    assert_eq!(
        missing.to_string(),
        "Tool weather has an output schema but did not return structured content"
    );
    let Error::Mcp(missing) = missing else {
        panic!("not an MCP error")
    };
    assert_eq!(missing.code(), Some(-32600));

    let bad = client
        .call_tool(&tool, json!({"message": "bad"}))
        .await
        .expect_err("bad");
    assert!(
        bad.to_string()
            .starts_with("Structured content does not match the tool's output schema: data/forecast "),
        "{bad}"
    );
    let Error::Mcp(bad) = bad else {
        panic!("not an MCP error")
    };
    assert_eq!(bad.code(), Some(-32602));
}

#[test]
fn a_config_entry_reads_auth_as_client_credentials() {
    let config: McpServerConfig = serde_json::from_value(json!({
        "url": "https://example.com/mcp",
        "auth": {"clientId": "id", "clientSecret": "secret", "scopes": ["a"]}
    }))
    .expect("config");
    let expected = McpClientCredentials::builder()
        .client_id("id")
        .client_secret("secret")
        .scopes(vec!["a".to_owned()])
        .build();
    assert_eq!(config.auth.as_ref(), Some(&expected));
    assert!(!format!("{expected:?}").contains("secret\""));
    assert_eq!(
        serde_json::to_value(&expected).expect("json"),
        json!({"clientId": "id", "clientSecret": "secret", "scopes": ["a"]})
    );
}

// No TS case (PROMPT section 10, lifecycle): `shutdown()` finishes the agent's own work and
// leaves the MCP clients the caller owns connected.
#[tokio::test(start_paused = true)]
async fn agent_shutdown_does_not_disconnect_the_mcp_client() {
    fn echo_agent(client: &McpClient) -> Agent {
        let model = MockMessageModel::new();
        model.add_turn(vec![ContentBlock::from(ToolUseBlock::new(
            "echo",
            "t1",
            json!({"message": "hello"}),
        ))]);
        model.add_turn(vec![TextBlock::new("done").into()]);
        Agent::builder()
            .model(model)
            .tools(vec![ToolSource::from(client.clone())])
            .printer(false)
            .build()
            .expect("agent")
    }

    let server = TestServer::default();
    let client = server.client();
    let mut first = echo_agent(&client);
    first.invoke("go").await.expect("turn");
    let peer = client.peer().expect("peer");

    first.shutdown().await.expect("shutdown");

    assert_eq!(client.connection_state(), McpConnectionState::Connected);
    assert!(!peer.is_transport_closed());
    // A second agent lists and calls the tools over the connection the first one used.
    let mut second = echo_agent(&client);
    second.invoke("go").await.expect("turn");
    let echoed: Vec<String> = server
        .calls
        .lock()
        .expect("calls")
        .iter()
        .map(|call| call.name.to_string())
        .collect();
    assert_eq!(echoed, ["echo", "echo"]);
    let result = second.messages()[2].content()[0].clone();
    let ContentBlock::ToolResult(result) = result else {
        panic!("expected a tool result, got {result:?}");
    };
    assert_eq!(*result.status(), crate::types::tools::ToolResultStatus::Success);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_call_cancels_the_request_and_frees_its_progress_route() {
    let server = TestServer::default();
    let client = server.client();
    let slow = tool_named(&client, "slow").await;
    let callback: McpProgressCallback = Arc::new(|_| {});
    for round in 1..=3 {
        let options = McpCallToolOptions {
            progress_callback: Some(Arc::clone(&callback)),
            ..Default::default()
        };
        let mut call = Box::pin(client.call_tool_with(&slow, json!({}), options));
        tokio::select! {
            biased;
            _ = &mut call => panic!("the slow tool answered"),
            () = server::eventually(|| server.calls.lock().expect("calls").len() == round) => {}
        }
        drop(call);
        server::eventually(|| server.cancelled.load(Ordering::SeqCst) == round).await;
    }
    assert_eq!(Arc::strong_count(&callback), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_invocation_cancels_its_mcp_tool_call() {
    let server = TestServer::default();
    let client = server.client();
    let model = MockMessageModel::new();
    model.add_turn(vec![ContentBlock::from(ToolUseBlock::new("slow", "t1", json!({})))]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder()
        .model(model)
        .tools(vec![ToolSource::from(client.clone())])
        .printer(false)
        .build()
        .expect("agent");
    {
        let mut turn = Box::pin(agent.invoke("go"));
        tokio::select! {
            biased;
            _ = &mut turn => panic!("the slow tool answered"),
            () = server::eventually(|| !server.calls.lock().expect("calls").is_empty()) => {}
        }
    }
    server::eventually(|| server.cancelled.load(Ordering::SeqCst) == 1).await;
}
