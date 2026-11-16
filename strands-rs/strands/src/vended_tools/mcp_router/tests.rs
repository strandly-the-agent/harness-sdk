//! Ported from strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.ts and
//! strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.node.ts
//!
//! The TS tests spy on `McpClient.loadServers` and mock the clients; these inject the server
//! loader and hand out clients of the in-process `rmcp` test server (`mcp/tests/server.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::mcp::tests::server::{Gate, TestServer, eventually, tool};
use crate::types::agent::LocalAgent;
use crate::vended_tools::test_support::{agent_with, context_for};

type Loads = Arc<Mutex<Vec<(String, McpServerConfig)>>>;

fn url_config(url: &str) -> McpServerConfig {
    McpServerConfig {
        url: Some(url.to_owned()),
        ..Default::default()
    }
}

fn options(names: &[&str]) -> MakeMcpRouterOptions {
    MakeMcpRouterOptions {
        servers: names
            .iter()
            .map(|name| ((*name).to_owned(), url_config("https://mcp.example.com/mcp")))
            .collect(),
        ..Default::default()
    }
}

/// A router whose loader hands out `clients` in order (none once they run out) and records
/// what it was asked to load.
fn router_with(options: MakeMcpRouterOptions, clients: Vec<McpClient>) -> (FunctionTool, Loads) {
    let queue = Arc::new(Mutex::new(VecDeque::from(clients)));
    let loads = Loads::default();
    let seen = Arc::clone(&loads);
    let loader: ServerLoader = Arc::new(move |name, config| {
        seen.lock().unwrap().push((name, config));
        let client = queue.lock().unwrap().pop_front();
        Box::pin(async move { Ok(client.into_iter().collect()) })
    });
    (make_mcp_router_with(options, loader).unwrap(), loads)
}

fn router(names: &[&str], clients: Vec<McpClient>) -> FunctionTool {
    router_with(options(names), clients).0
}

fn agent() -> LocalAgent {
    agent_with(None)
}

async fn run(tool: &FunctionTool, agent: &LocalAgent, input: JsonValue) -> Result<JsonValue, ToolError> {
    tool.invoke(input, context_for(agent, "mcp_router", JsonValue::Null))
        .await
}

async fn error_of(tool: &FunctionTool, agent: &LocalAgent, input: JsonValue) -> String {
    run(tool, agent, input).await.unwrap_err().to_string()
}

async fn connect(
    tool: &FunctionTool,
    agent: &LocalAgent,
    server: &str,
    id: &str,
) -> Result<JsonValue, ToolError> {
    run(
        tool,
        agent,
        json!({"command": "connect", "server_name": server, "connection_id": id}),
    )
    .await
}

fn failing_client(gate: &Gate, continue_on_error: bool) -> McpClient {
    gate.fail();
    McpClient::new(crate::mcp::McpClientConfig {
        transport: Some(gate.transport(TestServer::default())),
        options: McpClientOptions {
            continue_on_error,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap()
}

mod server_validation {
    use super::*;

    #[test]
    fn rejects_empty_allowlist() {
        let error = make_mcp_router(MakeMcpRouterOptions::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "`servers` must not be empty; the mcp_router tool requires at least one server"
        );
    }

    #[test]
    fn rejects_zero_max_connections() {
        let error = make_mcp_router(MakeMcpRouterOptions {
            max_connections: Some(0),
            ..options(&["mcp"])
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "`maxConnections` must be at least 1");
    }

    #[test]
    fn description_includes_server_names() {
        let tool = make_mcp_router(options(&["my-server"])).unwrap();
        assert!(tool.tool_spec().description().contains("my-server"));
    }
}

mod connect {
    use super::*;

    #[tokio::test]
    async fn rejects_server_not_on_allowlist() {
        let tool = router(&["mcp", "other"], vec![]);
        let error = error_of(
            &tool,
            &agent(),
            json!({"command": "connect", "server_name": "evil", "connection_id": "c1"}),
        )
        .await;
        assert_eq!(
            error,
            "Server \"evil\" is not on the MCP server allowlist: mcp, other"
        );
    }

    #[tokio::test]
    async fn rejects_missing_connection_id() {
        let tool = router(&["mcp"], vec![]);
        let error = error_of(
            &tool,
            &agent(),
            json!({"command": "connect", "server_name": "mcp"}),
        )
        .await;
        assert_eq!(
            error,
            "`connection_id` is required for all commands except 'list_connections'"
        );
    }

    #[tokio::test]
    async fn rejects_missing_server_name() {
        let tool = router(&["mcp"], vec![]);
        let error = error_of(
            &tool,
            &agent(),
            json!({"command": "connect", "connection_id": "c1"}),
        )
        .await;
        assert_eq!(error, "`server_name` is required for command='connect'");
    }

    #[tokio::test]
    async fn supports_multiple_simultaneous_connections() {
        let (server_a, server_b) = (TestServer::default(), TestServer::default());
        let tool = router(
            &["server-a", "server-b"],
            vec![server_a.client(), server_b.client()],
        );
        let agent = agent();
        connect(&tool, &agent, "server-a", "conn-a").await.unwrap();
        connect(&tool, &agent, "server-b", "conn-b").await.unwrap();

        for id in ["conn-a", "conn-b"] {
            run(
                &tool,
                &agent,
                json!({"command": "list_tools", "connection_id": id}),
            )
            .await
            .unwrap();
        }

        assert_eq!(server_a.list_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(server_b.list_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rejects_reconnect_with_same_id() {
        let server = TestServer::default();
        let (client_a, client_b) = (server.client(), server.client());
        let tool = router(&["mcp"], vec![client_a.clone(), client_b.clone()]);
        let agent = agent();
        connect(&tool, &agent, "mcp", "c1").await.unwrap();

        let error = connect(&tool, &agent, "mcp", "c1").await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Connection \"c1\" already exists. Disconnect it first or use a different connection_id."
        );
        assert_eq!(client_a.connection_state(), McpConnectionState::Connected);
        assert_eq!(client_b.connection_state(), McpConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn enforces_connection_cap() {
        let server = TestServer::default();
        let third = server.client();
        let (tool, _) = router_with(
            MakeMcpRouterOptions {
                max_connections: Some(2),
                ..options(&["mcp"])
            },
            vec![server.client(), server.client(), third.clone()],
        );
        let agent = agent();
        connect(&tool, &agent, "mcp", "c2").await.unwrap();
        connect(&tool, &agent, "mcp", "c1").await.unwrap();

        let error = connect(&tool, &agent, "mcp", "c3").await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Connection limit of 2 reached. Disconnect one of the active connections before opening a new one. Active connection_ids: c1, c2"
        );
        assert_eq!(third.connection_state(), McpConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn isolates_connections_between_agents() {
        let tool = router(&["mcp"], vec![TestServer::default().client()]);
        let (agent_a, agent_b) = (agent(), agent());
        connect(&tool, &agent_a, "mcp", "c1").await.unwrap();

        let error = error_of(
            &tool,
            &agent_b,
            json!({"command": "list_tools", "connection_id": "c1"}),
        )
        .await;

        assert_eq!(error, "No active connection for id \"c1\"");
    }

    #[tokio::test]
    async fn cleans_up_on_start_failure() {
        let gate = Gate::default();
        let broken = failing_client(&gate, false);
        let tool = router(&["mcp"], vec![broken.clone()]);
        let agent = agent();

        assert!(connect(&tool, &agent, "mcp", "c1").await.is_err());

        assert_eq!(broken.connection_state(), McpConnectionState::Disconnected);
        let error = error_of(
            &tool,
            &agent,
            json!({"command": "list_tools", "connection_id": "c1"}),
        )
        .await;
        assert_eq!(error, "No active connection for id \"c1\"");
    }

    #[tokio::test]
    async fn rejects_when_client_silently_enters_failed_state_continue_on_error() {
        let gate = Gate::default();
        let failed = failing_client(&gate, true);
        let tool = router(&["mcp"], vec![failed.clone()]);

        let error = connect(&tool, &agent(), "mcp", "c1").await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Server \"mcp\" failed to connect; check the server config"
        );
        assert_eq!(failed.connection_state(), McpConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn throws_when_allowlisted_server_has_disabled_true() {
        let config = McpServerConfig {
            disabled: Some(true),
            ..url_config("https://mcp.example.com/mcp")
        };
        let tool = make_mcp_router(MakeMcpRouterOptions {
            servers: IndexMap::from([("mcp".to_owned(), config)]),
            ..Default::default()
        })
        .unwrap();

        let error = connect(&tool, &agent(), "mcp", "c1").await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Server \"mcp\" failed to initialise; check the server config"
        );
    }
}

mod session_lifecycle {
    use super::*;

    fn call(id: &str, name: &str, arguments: &JsonValue) -> JsonValue {
        json!({"command": "call_tool", "connection_id": id, "tool_name": name, "arguments": arguments})
    }

    fn called_names(server: &TestServer) -> Vec<String> {
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.name.to_string())
            .collect()
    }

    #[tokio::test]
    async fn runs_full_connect_list_tools_call_tool_disconnect_flow() {
        let server = TestServer::with_tools(vec![tool("echo", Some("Echoes input"))]);
        let client = server.client();
        let tool = router(&["mcp"], vec![client.clone()]);
        let agent = agent();

        let connected = connect(&tool, &agent, "mcp", "c1").await.unwrap();
        assert_eq!(connected, json!("Successfully connected to mcp as \"c1\""));

        let tools = run(
            &tool,
            &agent,
            json!({"command": "list_tools", "connection_id": "c1"}),
        )
        .await
        .unwrap();
        assert_eq!(
            tools,
            json!([{
                "name": "echo",
                "description": "Echoes input",
                "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}},
            }])
        );

        let result = run(
            &tool,
            &agent,
            call("c1", "echo", &json!({"message": "hello world"})),
        )
        .await
        .unwrap();
        assert_eq!(result, json!([{"text": "hello world"}]));
        let calls = server.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
        assert_eq!(
            calls[0].arguments.clone().map(JsonValue::Object),
            Some(json!({"message": "hello world"}))
        );

        let disconnected = run(
            &tool,
            &agent,
            json!({"command": "disconnect", "connection_id": "c1"}),
        )
        .await
        .unwrap();
        assert_eq!(disconnected, json!("Successfully disconnected"));
        assert_eq!(client.connection_state(), McpConnectionState::Disconnected);

        let error = error_of(
            &tool,
            &agent,
            json!({"command": "list_tools", "connection_id": "c1"}),
        )
        .await;
        assert_eq!(error, "No active connection for id \"c1\"");
    }

    #[tokio::test]
    async fn rejects_call_tool_without_tool_name() {
        let tool = router(&["mcp"], vec![TestServer::default().client()]);
        let agent = agent();
        connect(&tool, &agent, "mcp", "c1").await.unwrap();

        let error = error_of(
            &tool,
            &agent,
            json!({"command": "call_tool", "connection_id": "c1"}),
        )
        .await;

        assert_eq!(error, "`tool_name` is required for command='call_tool'");
    }

    #[tokio::test]
    async fn rejects_call_tool_for_a_tool_that_list_tools_does_not_return() {
        let server = TestServer::with_tools(vec![tool("echo", None)]);
        let tool = router(&["mcp"], vec![server.client()]);
        let agent = agent();
        connect(&tool, &agent, "mcp", "c1").await.unwrap();

        let error = error_of(&tool, &agent, call("c1", "missing", &json!({}))).await;

        assert_eq!(
            error,
            "Tool \"missing\" is not available on connection \"c1\". Call list_tools to see the tools you can call."
        );
        assert!(called_names(&server).is_empty());
    }

    #[tokio::test]
    async fn lists_tools_for_call_tool_only_when_the_name_is_not_yet_listed() {
        let server = TestServer::with_tools(vec![tool("echo", None)]);
        let tool = router(&["mcp"], vec![server.client()]);
        let agent = agent();
        let list_calls = || server.list_calls.load(std::sync::atomic::Ordering::SeqCst);
        connect(&tool, &agent, "mcp", "c1").await.unwrap();

        run(&tool, &agent, call("c1", "echo", &json!({}))).await.unwrap();
        run(&tool, &agent, call("c1", "echo", &json!({}))).await.unwrap();
        assert_eq!(list_calls(), 1);

        server.tools.lock().unwrap().push(super::tool("add", None));
        run(&tool, &agent, call("c1", "add", &json!({"a": 1, "b": 2})))
            .await
            .unwrap();

        assert_eq!(list_calls(), 2);
        assert_eq!(called_names(&server), ["echo", "echo", "add"]);
    }

    #[tokio::test]
    async fn throws_mcp_router_tool_error_when_mcp_tool_returns_is_error() {
        let server = TestServer::with_tools(vec![tool("error_tool", None)]);
        let tool = router(&["mcp"], vec![server.client()]);
        let agent = agent();
        connect(&tool, &agent, "mcp", "c1").await.unwrap();

        let error = error_of(&tool, &agent, call("c1", "error_tool", &json!({}))).await;

        assert_eq!(error, "This tool always fails");
    }

    #[tokio::test]
    async fn disconnect_succeeds_even_when_client_disconnect_throws() {
        let client = TestServer::default().client();
        let tool = router(&["mcp"], vec![client.clone()]);
        let agent = agent();
        connect(&tool, &agent, "mcp", "c1").await.unwrap();
        client.disconnect().await.unwrap();

        let result = run(
            &tool,
            &agent,
            json!({"command": "disconnect", "connection_id": "c1"}),
        )
        .await
        .unwrap();

        assert_eq!(result, json!("Successfully disconnected"));
        let error = error_of(
            &tool,
            &agent,
            json!({"command": "list_tools", "connection_id": "c1"}),
        )
        .await;
        assert_eq!(error, "No active connection for id \"c1\"");
    }

    #[tokio::test(start_paused = true)]
    async fn forwards_cancel_signal_to_call_tool() {
        let server = TestServer::with_tools(vec![tool("slow", None)]);
        let tool = router(&["mcp"], vec![server.client()]);
        let (signal, handle) = crate::CancelSignal::new();
        let mut agent = agent();
        agent.begin_cancel(Some(&signal));
        connect(&tool, &agent, "mcp", "c1").await.unwrap();

        let pending = tokio::spawn({
            let tool = tool.clone();
            let context = context_for(&agent, "mcp_router", JsonValue::Null);
            async move { tool.invoke(call("c1", "slow", &json!({})), context).await }
        });
        eventually(|| !called_names(&server).is_empty()).await;
        handle.cancel(crate::CancelOptions {
            message: Some("stop now".to_owned()),
            after_current_tools: false,
        });

        let error = pending.await.unwrap().unwrap_err();
        // The ad-hoc McpTool turns the abort into an `Error: ..` result, which the router rethrows.
        assert_eq!(error.to_string(), "Error: stop now");
    }
}

mod config_forwarding {
    use super::*;

    #[tokio::test]
    async fn forwards_server_config_and_name_to_load_servers() {
        let config = McpServerConfig {
            headers: Some(IndexMap::from([("X-Api-Key".to_owned(), "secret".to_owned())])),
            ..url_config("https://mcp.example.com/mcp")
        };
        let (tool, loads) = router_with(
            MakeMcpRouterOptions {
                servers: IndexMap::from([("my-api".to_owned(), config.clone())]),
                ..Default::default()
            },
            vec![TestServer::default().client()],
        );

        connect(&tool, &agent(), "my-api", "c1").await.unwrap();

        assert_eq!(*loads.lock().unwrap(), [("my-api".to_owned(), config)]);
    }
}

mod list_connections {
    use super::*;

    #[tokio::test]
    async fn returns_sorted_open_connection_ids() {
        let server = TestServer::default();
        let tool = router(&["server-a", "server-b"], vec![server.client(), server.client()]);
        let agent = agent();
        let list = json!({"command": "list_connections"});

        assert_eq!(run(&tool, &agent, list.clone()).await.unwrap(), json!(""));

        connect(&tool, &agent, "server-b", "conn-b").await.unwrap();
        connect(&tool, &agent, "server-a", "conn-a").await.unwrap();

        assert_eq!(run(&tool, &agent, list).await.unwrap(), json!("conn-a, conn-b"));
    }

    #[tokio::test]
    async fn isolates_connections_between_agents() {
        let tool = router(&["mcp"], vec![TestServer::default().client()]);
        let (agent_a, agent_b) = (agent(), agent());
        connect(&tool, &agent_a, "mcp", "c1").await.unwrap();

        let result = run(&tool, &agent_b, json!({"command": "list_connections"}))
            .await
            .unwrap();

        assert_eq!(result, json!(""));
    }
}

mod tool_metadata {
    use super::*;

    #[test]
    fn uses_custom_name() {
        let tool = make_mcp_router(MakeMcpRouterOptions {
            name: Some("my_mcp".to_owned()),
            ..options(&["mcp"])
        })
        .unwrap();
        assert_eq!(tool.name(), "my_mcp");
    }

    #[test]
    fn exposes_expected_schema_fields() {
        let tool = make_mcp_router(options(&["mcp"])).unwrap();
        // `z.toJSONSchema` output of the TS input schema, without `$schema`.
        assert_eq!(
            tool.tool_spec().input_schema(),
            Some(&json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": ["connect", "list_connections", "list_tools", "call_tool", "disconnect"],
                        "description": "The operation to perform: 'connect', 'list_connections', 'list_tools', 'call_tool', 'disconnect'."
                    },
                    "connection_id": {
                        "description": "A descriptive name for this connection. Required for all commands except list_connections. Must be unique per agent. Reusing an active id is rejected.",
                        "type": "string"
                    },
                    "server_name": {
                        "description": "Server name to connect to. Required for 'connect'.",
                        "type": "string"
                    },
                    "tool_name": {
                        "description": "Tool name to invoke. Required for 'call_tool'.",
                        "type": "string"
                    },
                    "arguments": {
                        "description": "Arguments to pass to the invoked tool. Used with 'call_tool'.",
                        "type": "object",
                        "propertyNames": {"type": "string"},
                        "additionalProperties": {}
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }))
        );
        // The whole model-facing spec against the TS-generated fixture.
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../../../../conformance/fixtures/tools/mcp_router_spec.json")).unwrap();
        assert_eq!(serde_json::to_value(tool.tool_spec()).unwrap(), expected);
    }

    #[test]
    fn uses_default_description_with_mcp_router_description() {
        let tool = make_mcp_router(options(&["mcp", "beta"])).unwrap();
        assert_eq!(
            tool.tool_spec().description(),
            format!("{MCP_ROUTER_DESCRIPTION} Permitted server names: 'beta', 'mcp'.")
        );
    }

    #[test]
    fn accepts_custom_description() {
        let tool = make_mcp_router(MakeMcpRouterOptions {
            description: Some("Custom desc".to_owned()),
            ..options(&["mcp"])
        })
        .unwrap();
        assert_eq!(tool.tool_spec().description(), "Custom desc");
    }
}

mod garbage_collection_cleanup {
    use super::*;

    /// TS fires a `FinalizationRegistry` callback when the agent is collected; Rust closes a
    /// dropped agent's connections on the router's next call.
    #[tokio::test]
    async fn disconnects_all_connections_when_the_callback_fires_and_swallows_errors() {
        let server = TestServer::default();
        let (client_a, client_b) = (server.client(), server.client());
        let tool = router(&["mcp"], vec![client_a.clone(), client_b.clone()]);
        let dropped = agent();
        connect(&tool, &dropped, "mcp", "conn-a").await.unwrap();
        connect(&tool, &dropped, "mcp", "conn-b").await.unwrap();
        client_b.disconnect().await.unwrap();
        drop(dropped);
        assert_eq!(client_a.connection_state(), McpConnectionState::Connected);

        let result = run(&tool, &agent(), json!({"command": "list_connections"}))
            .await
            .unwrap();

        assert_eq!(result, json!(""));
        assert_eq!(client_a.connection_state(), McpConnectionState::Disconnected);
        assert_eq!(client_b.connection_state(), McpConnectionState::Disconnected);
    }
}

/// Ported from strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.node.ts
mod config_loaded_servers {
    use super::*;
    use crate::mcp::SerializableMcpToolFilters;
    use crate::mcp::config::{McpServers, fixed_env, load_servers_object, resolve_server_configs};

    /// A loader that resolves the config as `McpClient::load_servers` does and serves the
    /// client from `server`.
    fn config_loader(server: TestServer) -> ServerLoader {
        Arc::new(move |name, config| {
            let server = server.clone();
            Box::pin(async move {
                let lookup = fixed_env(&[]);
                let servers =
                    load_servers_object(McpServers::Map(IndexMap::from([(name, config)])), &lookup).await?;
                let resolved = resolve_server_configs(
                    servers,
                    &McpClientOptions::default(),
                    McpLoadServersOptions::default(),
                    &lookup,
                )?;
                Ok(resolved
                    .into_iter()
                    .map(|resolved| server.client_with(resolved.options))
                    .collect())
            })
        })
    }

    #[tokio::test]
    async fn enforces_label_tool_filters_from_the_server_config_on_call_tool() {
        let filters = [
            SerializableMcpToolFilters {
                rejected: Some(vec!["add".to_owned()]),
                ..Default::default()
            },
            SerializableMcpToolFilters {
                allowed: Some(vec!["echo".to_owned()]),
                ..Default::default()
            },
        ];
        for tool_filters in filters {
            let server = TestServer::with_tools(vec![tool("echo", None), tool("add", None)]);
            let config = McpServerConfig {
                tool_filters: Some(tool_filters),
                ..url_config("https://mcp.example.com/mcp")
            };
            let router = make_mcp_router_with(
                MakeMcpRouterOptions {
                    servers: IndexMap::from([("mcp".to_owned(), config)]),
                    ..Default::default()
                },
                config_loader(server.clone()),
            )
            .unwrap();
            let agent = agent();
            connect(&router, &agent, "mcp", "c1").await.unwrap();

            let tools = run(
                &router,
                &agent,
                json!({"command": "list_tools", "connection_id": "c1"}),
            )
            .await
            .unwrap();
            let rejected = error_of(
                &router,
                &agent,
                json!({"command": "call_tool", "connection_id": "c1", "tool_name": "add"}),
            )
            .await;
            let result = run(
                &router,
                &agent,
                json!({"command": "call_tool", "connection_id": "c1", "tool_name": "echo", "arguments": {"message": "ok"}}),
            )
            .await
            .unwrap();

            let names: Vec<&str> = tools
                .as_array()
                .unwrap()
                .iter()
                .map(|spec| spec["name"].as_str().unwrap())
                .collect();
            assert_eq!(names, ["echo"]);
            assert_eq!(
                rejected,
                "Tool \"add\" is not available on connection \"c1\". Call list_tools to see the tools you can call."
            );
            assert_eq!(result, json!([{"text": "ok"}]));
            let called: Vec<String> = server
                .calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| call.name.to_string())
                .collect();
            assert_eq!(called, ["echo"]);
        }
    }
}
