//! Ported from strands-ts/src/mcp/__tests__/client.test.ts
//!
//! The TS tests mock the MCP SDK `Client`; these run against an in-process `rmcp` server
//! (`tests/server.rs`) over an in-memory pipe.

mod config;
mod robustness;
pub(crate) mod server;

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::json;

use self::server::{Gate, TestServer, tool};
use super::mcp_tool::{error_block, result_block};
use super::*;
use crate::CancelOptions;
use crate::cancel::CancelSignal;
use crate::error::Error;
use crate::tools::{Tool, ToolSource};
use crate::types::messages::ToolResultContent;
use crate::types::tools::ToolResultStatus;

fn cancel(_message: &str) -> (CancelSignal, crate::CancelHandle) {
    CancelSignal::new()
}

fn cancel_with(handle: &crate::CancelHandle, message: &str) {
    handle.cancel(CancelOptions {
        message: Some(message.to_owned()),
        ..Default::default()
    });
}

fn gated_client(gate: &Gate, continue_on_error: bool) -> McpClient {
    McpClient::new(McpClientConfig {
        transport: Some(gate.transport(TestServer::default())),
        options: McpClientOptions {
            application_name: Some("TestApp".to_owned()),
            continue_on_error,
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("valid config")
}

async fn tool_named(client: &McpClient, name: &str) -> McpTool {
    client
        .list_tools()
        .await
        .expect("tools")
        .into_iter()
        .find(|tool| tool.server_name() == name)
        .expect("tool exists")
}

fn text_of(content: &[ToolResultContent]) -> Vec<String> {
    content
        .iter()
        .map(|block| match block {
            ToolResultContent::Text(text) => text.text().to_owned(),
            other => format!("{other:?}"),
        })
        .collect()
}

mod mcp_client {
    use super::*;

    #[tokio::test]
    async fn initializes_sdk_client_with_correct_configuration() {
        let client = TestServer::default().client_with(McpClientOptions {
            application_name: Some("TestApp".to_owned()),
            application_version: Some("0.0.1".to_owned()),
            ..Default::default()
        });
        assert_eq!(client.client_name(), "TestApp");
        assert_eq!(client.client_version(), "0.0.1");
        let default = TestServer::default().client();
        assert_eq!(default.client_name(), "strands-agents-rs-sdk");
        assert_eq!(default.client_version(), env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn manages_connection_state_lazily() {
        let gate = Gate::default();
        gate.open();
        let client = gated_client(&gate, false);
        assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
        client.connect(false).await.expect("connect");
        assert_eq!(gate.attempts(), 1);
        client.connect(false).await.expect("connect");
        assert_eq!(gate.attempts(), 1);
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
    }

    #[tokio::test]
    async fn supports_forced_reconnection() {
        let gate = Gate::default();
        gate.open();
        gate.open();
        let client = gated_client(&gate, false);
        client.connect(false).await.expect("connect");
        client.connect(true).await.expect("reconnect");
        assert_eq!(gate.attempts(), 2);
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
    }

    #[tokio::test]
    async fn stops_a_caller_waiting_on_a_shared_connection_when_its_signal_aborts() {
        let gate = Gate::default();
        let client = gated_client(&gate, false);
        let (signal, handle) = cancel("caller left");
        let first = tokio::spawn({
            let client = client.clone();
            async move { client.connect(false).await }
        });
        server::eventually(|| gate.attempts() == 1).await;
        let second = client.connect_with(
            false,
            McpConnectOptions {
                cancel_signal: Some(signal.clone()),
            },
        );
        cancel_with(&handle, "caller left");
        let error = second.await.expect_err("aborted");
        assert_eq!(error.to_string(), "caller left");
        gate.open();
        first.await.expect("join").expect("first connects");
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
        assert_eq!(gate.attempts(), 1);
    }

    async fn keeps_the_connection_shared_after_its_creator_aborts(continue_on_error: bool) {
        let gate = Gate::default();
        let client = gated_client(&gate, continue_on_error);
        let (signal, handle) = cancel("creator left");
        let creator = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .connect_with(
                        false,
                        McpConnectOptions {
                            cancel_signal: Some(signal.clone()),
                        },
                    )
                    .await
            }
        });
        server::eventually(|| gate.attempts() == 1).await;
        cancel_with(&handle, "creator left");
        assert_eq!(
            creator.await.expect("join").expect_err("aborted").to_string(),
            "creator left"
        );
        let follower = tokio::spawn({
            let client = client.clone();
            async move { client.connect(false).await }
        });
        assert_eq!(gate.attempts(), 1);
        gate.open();
        follower.await.expect("join").expect("follower connects");
        client.connect(false).await.expect("connect");
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
        assert_eq!(gate.attempts(), 1);
    }

    #[tokio::test]
    async fn keeps_the_connection_shared_after_its_creator_aborts_continue_on_error_false() {
        keeps_the_connection_shared_after_its_creator_aborts(false).await;
    }

    #[tokio::test]
    async fn keeps_the_connection_shared_after_its_creator_aborts_continue_on_error_true() {
        keeps_the_connection_shared_after_its_creator_aborts(true).await;
    }

    #[tokio::test]
    async fn releases_a_rejected_connection_after_its_creator_aborts_so_an_explicit_retry_can_run() {
        let gate = Gate::default();
        let client = gated_client(&gate, false);
        let (signal, handle) = cancel("creator left");
        let creator = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .connect_with(
                        false,
                        McpConnectOptions {
                            cancel_signal: Some(signal.clone()),
                        },
                    )
                    .await
            }
        });
        server::eventually(|| gate.attempts() == 1).await;
        cancel_with(&handle, "creator left");
        assert_eq!(
            creator.await.expect("join").expect_err("aborted").to_string(),
            "creator left"
        );
        let follower = tokio::spawn({
            let client = client.clone();
            async move { client.connect(false).await }
        });
        gate.fail();
        assert!(follower.await.expect("join").is_err());
        assert_eq!(gate.attempts(), 1);
        gate.open();
        client.connect(true).await.expect("retry");
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
        assert_eq!(gate.attempts(), 2);
    }

    #[tokio::test]
    async fn rejects_immediately_when_connect_is_called_with_an_aborted_signal() {
        let gate = Gate::default();
        let client = gated_client(&gate, false);
        let (signal, handle) = cancel("already cancelled");
        cancel_with(&handle, "already cancelled");
        let error = client
            .connect_with(
                false,
                McpConnectOptions {
                    cancel_signal: Some(signal.clone()),
                },
            )
            .await
            .expect_err("aborted");
        assert_eq!(error.to_string(), "already cancelled");
        assert_eq!(gate.attempts(), 0);
    }

    #[tokio::test]
    async fn stops_waiting_for_the_connection_when_call_tool_is_aborted_without_tasks_config() {
        let gate = Gate::default();
        let server = TestServer::default();
        let client = gated_client(&gate, false);
        let listed = TestServer::default().client();
        let echo = tool_named(&listed, "echo").await;
        let (signal, handle) = cancel("caller aborted during connect");
        let pending = tokio::spawn({
            let client = client.clone();
            async move {
                let options = McpCallToolOptions {
                    cancel_signal: Some(signal),
                    ..Default::default()
                };
                client.call_tool_with(&echo, json!({}), options).await
            }
        });
        server::eventually(|| gate.attempts() == 1).await;
        cancel_with(&handle, "caller aborted during connect");
        let error = pending.await.expect("join").expect_err("aborted");
        assert_eq!(error.to_string(), "caller aborted during connect");
        assert!(server.calls.lock().expect("calls").is_empty());
    }

    #[tokio::test]
    async fn converts_sdk_tool_specs_to_mcp_tool_instances() {
        let long_name = "a".repeat(65);
        let output_schema = json!({"type": "object", "properties": {"forecast": {"type": "string"}}, "required": ["forecast"]});
        let mut weather = tool("weather", Some("Get weather"));
        weather.input_schema = Arc::new(rmcp::model::JsonObject::new());
        weather.output_schema = Some(Arc::new(server::object(output_schema.clone())));
        let mut long = tool(&long_name, None);
        long.input_schema = Arc::new(rmcp::model::JsonObject::new());
        let server = TestServer::with_tools(vec![weather, long]);
        let client = server.client();

        let tools = client.list_tools().await.expect("tools");

        let names: Vec<&str> = tools.iter().map(Tool::name).collect();
        assert_eq!(names, ["weather", long_name.as_str()]);
        assert_eq!(
            serde_json::to_value(tools[0].tool_spec()).expect("json"),
            json!({"name": "weather", "description": "Get weather", "inputSchema": {}, "outputSchema": output_schema})
        );
        client
            .call_tool(&tools[1], json!({}))
            .await
            .expect_err("unknown tool on server");
        let calls = server.calls.lock().expect("calls").clone();
        assert_eq!(calls[0].name, long_name);
        assert_eq!(calls[0].arguments, Some(rmcp::model::JsonObject::new()));
    }

    #[tokio::test]
    async fn generates_description_fallback_when_description_is_missing() {
        let client = TestServer::with_tools(vec![tool("my_tool", None)]).client();
        let tools = client.list_tools().await.expect("tools");
        assert_eq!(tools[0].description(), "Tool which performs my_tool");
    }

    #[tokio::test]
    async fn generates_description_fallback_when_description_is_empty_string() {
        let client = TestServer::with_tools(vec![tool("my_tool", Some(""))]).client();
        let tools = client.list_tools().await.expect("tools");
        assert_eq!(tools[0].description(), "Tool which performs my_tool");
    }

    #[tokio::test]
    async fn calls_the_tool_with_no_request_options_by_default() {
        let server = TestServer::default();
        let client = server.client();
        let echo = tool_named(&client, "echo_args").await;
        let result = client
            .call_tool(&echo, json!({"op": "add"}))
            .await
            .expect("result");
        assert_eq!(result["content"][0]["text"], json!("{\"op\":\"add\"}"));
        assert_eq!(server.calls.lock().expect("calls")[0].name, "echo_args");
    }

    #[tokio::test]
    async fn forwards_abort_signal_to_sdk_call_tool() {
        let server = TestServer::default();
        let client = server.client();
        let slow = tool_named(&client, "slow").await;
        let (signal, handle) = CancelSignal::new();
        let pending = tokio::spawn({
            let client = client.clone();
            async move {
                let options = McpCallToolOptions {
                    cancel_signal: Some(signal),
                    ..Default::default()
                };
                client.call_tool_with(&slow, json!({}), options).await
            }
        });
        server::eventually(|| !server.calls.lock().expect("calls").is_empty()).await;
        handle.cancel(CancelOptions::default());
        let error = pending.await.expect("join").expect_err("aborted");
        assert_eq!(error.to_string(), "The operation was aborted");
        // The connection still serves other calls.
        let echo = tool_named(&client, "echo").await;
        client
            .call_tool(&echo, json!({"message": "after"}))
            .await
            .expect("result");
    }

    #[tokio::test]
    async fn validates_tool_arguments() {
        let client = TestServer::default().client();
        let echo = tool_named(&client, "echo").await;
        let error = client
            .call_tool(&echo, json!(["invalid-array"]))
            .await
            .expect_err("rejected");
        assert_eq!(
            error.to_string(),
            "MCP Protocol Error: Tool arguments must be a JSON Object (named parameters). Received: Array"
        );
    }

    #[tokio::test]
    async fn cleans_up_resources() {
        let client = TestServer::default().client();
        client.connect(false).await.expect("connect");
        let peer = client.peer().expect("peer");
        client.disconnect().await.expect("disconnect");
        assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
        assert!(peer.is_transport_closed());
    }

    #[tokio::test]
    async fn rejects_a_zero_call_timeout() {
        let client = TestServer::default().client();
        let echo = tool_named(&client, "echo").await;
        let options = McpCallToolOptions {
            timeout: Some(std::time::Duration::ZERO),
            ..Default::default()
        };
        let error = client
            .call_tool_with(&echo, json!({}), options)
            .await
            .expect_err("rejected");
        assert_eq!(
            error.to_string(),
            "MCP call timeout must be a positive safe integer no greater than 2147483647"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn serves_concurrent_calls_on_one_connection() {
        let server = TestServer::default();
        let client = server.client();
        let slow = tool_named(&client, "slow").await;
        let echo = tool_named(&client, "echo").await;
        let (signal, handle) = CancelSignal::new();
        let blocked = tokio::spawn({
            let client = client.clone();
            async move {
                let options = McpCallToolOptions {
                    cancel_signal: Some(signal),
                    ..Default::default()
                };
                client.call_tool_with(&slow, json!({}), options).await
            }
        });
        let calls = (0..8).map(|n| {
            let client = client.clone();
            let echo = echo.clone();
            async move { client.call_tool(&echo, json!({"message": format!("m{n}")})).await }
        });
        let results = futures::future::try_join_all(calls)
            .await
            .expect("concurrent calls");
        assert_eq!(results[7]["content"][0]["text"], json!("m7"));
        handle.cancel(CancelOptions::default());
        assert!(blocked.await.expect("join").is_err());
    }

    #[tokio::test]
    async fn dropping_the_last_handle_closes_the_connection() {
        let client = TestServer::default().client();
        client.connect(false).await.expect("connect");
        let peer = client.peer().expect("peer");
        drop(client);
        server::eventually(|| peer.is_transport_closed()).await;
    }
}

mod mcp_tool {
    use super::*;
    use crate::types::media::ImageSource;

    #[tokio::test]
    async fn exposes_the_original_client_without_allowing_reassignment() {
        let client = TestServer::default().client();
        let echo = tool_named(&client, "echo").await;
        assert!(Arc::ptr_eq(&echo.mcp_client().inner, &client.inner));
    }

    fn mapped(raw: &serde_json::Value) -> crate::types::messages::ToolResultBlock {
        result_block(raw, "tool-1")
    }

    #[test]
    fn returns_text_results_on_success() {
        let block = mapped(&json!({"content": [{"type": "text", "text": "Hello"}]}));
        assert_eq!(block.status(), &ToolResultStatus::Success);
        assert_eq!(text_of(block.content()), ["Hello"]);
    }

    #[test]
    fn returns_structured_data_results_on_success() {
        let block = mapped(&json!({"content": [{"type": "data", "value": 1}]}));
        assert_eq!(
            block.content(),
            [ToolResultContent::Json(crate::types::messages::JsonBlock::new(
                json!({"type": "data", "value": 1})
            ))]
        );
    }

    #[test]
    fn provides_default_message_for_empty_output() {
        let block = mapped(&json!({"content": []}));
        assert_eq!(
            text_of(block.content()),
            ["Tool execution completed successfully with no output."]
        );
    }

    #[test]
    fn handles_protocol_level_errors() {
        let block = mapped(&json!({"isError": true, "content": [{"type": "text", "text": "Failure"}]}));
        assert_eq!(block.status(), &ToolResultStatus::Error);
        assert_eq!(text_of(block.content()), ["Failure"]);
    }

    #[test]
    fn catches_and_wraps_client_exceptions() {
        let block = error_block(Error::other("Network Error"), "tool-1");
        assert_eq!(block.status(), &ToolResultStatus::Error);
        assert_eq!(text_of(block.content()), ["Error: Network Error"]);
    }

    #[test]
    fn validates_sdk_response_format() {
        let block = mapped(&json!({"wrong": "format"}));
        assert_eq!(block.status(), &ToolResultStatus::Error);
        assert_eq!(
            text_of(block.content()),
            ["Error: Invalid tool result from MCP Client: missing content array"]
        );
    }

    #[test]
    fn maps_mcp_image_content_to_image_block() {
        let block =
            mapped(&json!({"content": [{"type": "image", "data": "aGVsbG8=", "mimeType": "image/png"}]}));
        let ToolResultContent::Image(image) = &block.content()[0] else {
            panic!("image expected");
        };
        assert_eq!(image.format().as_str(), "png");
        assert_eq!(image.source(), &ImageSource::Bytes(b"hello".to_vec()));
    }

    #[test]
    fn falls_back_to_json_block_for_unsupported_image_mime_type() {
        let item = json!({"type": "image", "data": "aGVsbG8=", "mimeType": "image/bmp"});
        let block = mapped(&json!({"content": [&item]}));
        assert_eq!(
            block.content(),
            [ToolResultContent::Json(crate::types::messages::JsonBlock::new(
                item
            ))]
        );
    }

    #[test]
    fn falls_back_to_json_block_for_image_content_missing_data() {
        let item = json!({"type": "image", "mimeType": "image/png"});
        let block = mapped(&json!({"content": [&item]}));
        assert_eq!(
            block.content(),
            [ToolResultContent::Json(crate::types::messages::JsonBlock::new(
                item
            ))]
        );
    }

    #[test]
    fn maps_mcp_text_resource_to_text_block() {
        let block = mapped(
            &json!({"content": [{"type": "resource", "resource": {"uri": "file:///a.txt", "text": "file contents"}}]}),
        );
        assert_eq!(text_of(block.content()), ["file contents"]);
    }

    #[test]
    fn maps_mcp_blob_resource_with_image_mime_type_to_image_block() {
        let block = mapped(
            &json!({"content": [{"type": "resource", "resource": {"uri": "file:///a.png", "blob": "aGVsbG8=", "mimeType": "image/png"}}]}),
        );
        assert!(matches!(&block.content()[0], ToolResultContent::Image(_)));
    }

    #[test]
    fn falls_back_to_json_block_for_blob_resource_with_non_image_mime_type() {
        let item = json!({"type": "resource", "resource": {"uri": "file:///a.pdf", "blob": "aGVsbG8=", "mimeType": "application/pdf"}});
        let block = mapped(&json!({"content": [&item]}));
        assert_eq!(
            block.content(),
            [ToolResultContent::Json(crate::types::messages::JsonBlock::new(
                item
            ))]
        );
    }

    #[test]
    fn falls_back_to_json_block_for_resource_with_neither_text_nor_blob() {
        let item = json!({"type": "resource", "resource": {"uri": "file:///a"}});
        let block = mapped(&json!({"content": [&item]}));
        assert_eq!(
            block.content(),
            [ToolResultContent::Json(crate::types::messages::JsonBlock::new(
                item
            ))]
        );
    }

    #[test]
    fn handles_mixed_content_types_in_a_single_result() {
        let block = mapped(&json!({"content": [
            {"type": "text", "text": "Hello"},
            {"type": "image", "data": "aGVsbG8=", "mimeType": "image/jpeg"},
            {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "doc"}}
        ]}));
        assert_eq!(block.content().len(), 3);
        assert!(matches!(&block.content()[1], ToolResultContent::Image(_)));
        assert_eq!(text_of(&block.content()[2..]), ["doc"]);
    }

    fn elicitation_error(data: Option<serde_json::Value>, code: i32) -> Error {
        Error::Mcp(McpError::protocol(code, "URL elicitation required", data))
    }

    #[test]
    fn surfaces_elicitation_data_for_protocol_error_with_code_32042() {
        let elicitations = json!([{"mode": "url", "url": "https://example.com/auth", "elicitationId": "e1", "message": "Authorize"}]);
        let block = error_block(
            elicitation_error(Some(json!({"elicitations": elicitations})), -32042),
            "tool-1",
        );
        assert_eq!(block.status(), &ToolResultStatus::Error);
        assert_eq!(
            text_of(block.content()),
            [format!(
                "MCP Elicitation required: [ProtocolError: URL elicitation required] with data {elicitations}"
            )]
        );
    }

    #[test]
    fn surfaces_multiple_elicitations_for_protocol_error_with_code_32042() {
        let elicitations = json!([{"url": "https://a"}, {"url": "https://b"}]);
        let block = error_block(
            elicitation_error(Some(json!({"elicitations": elicitations})), -32042),
            "tool-1",
        );
        assert_eq!(
            text_of(block.content()),
            [format!(
                "MCP Elicitation required: [ProtocolError: URL elicitation required] with data {elicitations}"
            )]
        );
    }

    #[test]
    fn falls_through_to_generic_error_for_protocol_error_32042_with_malformed_data() {
        let block = error_block(elicitation_error(Some(json!("bad")), -32042), "tool-1");
        assert_eq!(text_of(block.content()), ["Error: URL elicitation required"]);
    }

    #[tokio::test]
    async fn surfaces_elicitation_data_for_url_elicitation_required_error() {
        let mut tools = TestServer::default().tools.lock().expect("tools").clone();
        tools.push(tool("url_elicitation", None));
        let client = TestServer::with_tools(tools).client();
        let tool = tool_named(&client, "url_elicitation").await;
        let error = client
            .call_tool(&tool, json!({}))
            .await
            .expect_err("elicitation required");
        let block = error_block(error, "tool-1");
        assert_eq!(
            text_of(block.content()),
            [
                "MCP Elicitation required: [ProtocolError: URL elicitation required] with data [{\"mode\":\"url\",\"url\":\"https://example.com/auth\",\"elicitationId\":\"e1\",\"message\":\"Authorize\"}]"
            ]
        );
    }

    #[test]
    fn falls_through_to_generic_error_for_protocol_error_32042_with_undefined_data() {
        let block = error_block(elicitation_error(None, -32042), "tool-1");
        assert_eq!(text_of(block.content()), ["Error: URL elicitation required"]);
    }

    #[test]
    fn falls_through_to_generic_error_for_protocol_error_32042_with_non_array_elicitations() {
        let block = error_block(
            elicitation_error(Some(json!({"elicitations": "nope"})), -32042),
            "tool-1",
        );
        assert_eq!(text_of(block.content()), ["Error: URL elicitation required"]);
    }

    #[test]
    fn falls_through_to_generic_error_for_protocol_error_32042_with_empty_elicitations() {
        let block = error_block(
            elicitation_error(Some(json!({"elicitations": []})), -32042),
            "tool-1",
        );
        assert_eq!(text_of(block.content()), ["Error: URL elicitation required"]);
    }

    #[test]
    fn falls_through_to_generic_error_for_protocol_error_with_a_different_code() {
        let elicitations = json!({"elicitations": [{"url": "https://a"}]});
        let block = error_block(elicitation_error(Some(elicitations), -32600), "tool-1");
        assert_eq!(text_of(block.content()), ["Error: URL elicitation required"]);
    }
}

mod server_metadata_getters {
    use super::*;

    #[tokio::test]
    async fn returns_undefined_for_all_getters_before_connect() {
        let client = TestServer::default().client();
        assert!(client.server_capabilities().is_none());
        assert!(client.server_version().is_none());
        assert!(client.server_instructions().is_none());
    }

    #[tokio::test]
    async fn returns_server_capabilities_after_connect() {
        let client = TestServer::default().client();
        client.connect(false).await.expect("connect");
        assert!(
            client
                .server_capabilities()
                .expect("capabilities")
                .tools
                .is_some()
        );
    }

    #[tokio::test]
    async fn returns_server_version_after_connect() {
        let client = TestServer::default().client();
        client.connect(false).await.expect("connect");
        let version = client.server_version().expect("version");
        assert_eq!(
            (version.name.as_str(), version.version.as_str()),
            ("test-server", "1.2.3")
        );
    }

    #[tokio::test]
    async fn returns_server_instructions_after_connect() {
        let client = TestServer::default().client();
        client.connect(false).await.expect("connect");
        assert_eq!(
            client.server_instructions().as_deref(),
            Some("Use the echo tool.")
        );
    }

    #[test]
    fn connection_state_is_disconnected_before_connect() {
        let client = McpClient::new(McpClientConfig {
            transport: Some(McpTransport::from_factory(|| tokio::io::duplex(8).0)),
            ..Default::default()
        })
        .expect("client");
        assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn connection_state_is_connected_after_successful_connect() {
        let client = TestServer::default().client();
        client.connect(false).await.expect("connect");
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
    }
}

mod continue_on_error {
    use super::*;

    #[tokio::test]
    async fn throws_on_connection_failure_by_default() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, false);
        assert!(client.connect(false).await.is_err());
        assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn swallows_connection_failure_when_continue_on_error_is_true() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, true);
        client.connect(false).await.expect("swallowed");
        assert_eq!(client.connection_state(), McpConnectionState::Failed);
    }

    #[test]
    fn logs_a_warning_when_continue_on_error_swallows_a_connection_failure() {
        let (_, warnings) = crate::test_support::WarnRecorder::capture(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let gate = Gate::default();
                gate.fail();
                gated_client(&gate, true).connect(false).await.expect("swallowed");
            });
        });
        assert!(
            warnings
                .iter()
                .any(|warning| warning.starts_with("client=<TestApp>, error=<")
                    && warning.ends_with("> | MCP server failed to connect, continuing (continueOnError)")),
            "{warnings:?}"
        );
    }

    #[tokio::test]
    async fn list_tools_returns_empty_array_when_continue_on_error_and_connection_failed() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, true);
        assert!(client.list_tools().await.expect("empty").is_empty());
    }

    #[tokio::test]
    async fn call_tool_throws_when_continue_on_error_and_connection_failed() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, true);
        let echo = tool_named(&TestServer::default().client(), "echo").await;
        let error = client.call_tool(&echo, json!({})).await.expect_err("failed");
        assert_eq!(
            error.to_string(),
            "MCP server failed to connect. Call connect(true) to retry."
        );
    }

    #[tokio::test]
    async fn does_not_retry_connection_on_subsequent_calls_after_continue_on_error_failure() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, true);
        client.connect(false).await.expect("swallowed");
        client.list_tools().await.expect("empty");
        client.connect(false).await.expect("no retry");
        assert_eq!(gate.attempts(), 1);
    }

    #[tokio::test]
    async fn recovers_after_explicit_connect_true_when_server_comes_back() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, true);
        client.connect(false).await.expect("swallowed");
        gate.open();
        client.connect(true).await.expect("recovered");
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
        assert!(!client.list_tools().await.expect("tools").is_empty());
    }
}

mod connect_retry_transport_rebuild {
    use super::*;

    #[tokio::test]
    async fn retries_a_caller_supplied_transport_on_the_same_client_and_transport() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, false);
        assert!(client.connect(false).await.is_err());
        gate.open();
        client.connect(false).await.expect("retry");
        assert_eq!(gate.attempts(), 2);
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
    }

    #[tokio::test]
    async fn connects_a_client_again_after_disconnect() {
        let gate = Gate::default();
        gate.open();
        gate.open();
        let client = gated_client(&gate, false);
        client.connect(false).await.expect("connect");
        client.disconnect().await.expect("disconnect");
        assert_eq!(tool_named(&client, "echo").await.server_name(), "echo");
        assert_eq!(gate.attempts(), 2);
    }
}

mod streamable_http {
    use std::sync::Mutex;

    use super::*;

    #[tokio::test]
    async fn lists_and_calls_tools_over_streamable_http_with_headers() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = server::serve_http(TestServer::default(), Arc::clone(&seen)).await;
        let client = McpClient::new(McpClientConfig {
            url: Some(url),
            headers: Some(
                [("X-Api-Token".to_owned(), "abc".to_owned())]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        })
        .expect("client");
        let echo = tool_named(&client, "echo").await;
        let result = client
            .call_tool(&echo, json!({"message": "over http"}))
            .await
            .expect("result");
        assert_eq!(result["content"][0]["text"], json!("over http"));
        assert_eq!(
            client.negotiated_protocol_version().as_deref(),
            Some("2026-07-28")
        );
        let headers = seen.lock().expect("headers").clone();
        assert!(!headers.is_empty());
        assert!(
            headers
                .iter()
                .all(|headers| headers.get("x-api-token").is_some_and(|value| value == "abc"))
        );
        client.disconnect().await.expect("disconnect");
    }

    #[tokio::test]
    async fn retries_a_url_configured_client_on_a_fresh_transport_and_client() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = probe.local_addr().expect("address");
        drop(probe);
        let client = McpClient::new(McpClientConfig {
            url: Some(format!("http://{address}/mcp")),
            options: McpClientOptions {
                continue_on_error: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("client");
        client.connect(false).await.expect("swallowed");
        assert_eq!(client.connection_state(), McpConnectionState::Failed);
        let listener = tokio::net::TcpListener::bind(address).await.expect("rebind");
        server::serve_http_on(listener, TestServer::default(), Arc::default(), Arc::default());
        client.connect(true).await.expect("reconnect");
        assert_eq!(client.connection_state(), McpConnectionState::Connected);
    }

    #[tokio::test]
    async fn forces_reconnection_of_a_url_configured_client_on_a_fresh_transport() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = server::serve_http(TestServer::default(), Arc::clone(&seen)).await;
        let client = McpClient::new(McpClientConfig {
            url: Some(url),
            ..Default::default()
        })
        .expect("client");
        client.connect(false).await.expect("connect");
        let first = client.peer().expect("peer");
        client.connect(true).await.expect("reconnect");
        assert!(first.is_transport_closed());
        assert_eq!(tool_named(&client, "echo").await.server_name(), "echo");
    }

    #[tokio::test]
    async fn connects_a_url_configured_client_on_a_fresh_transport_after_disconnect() {
        let url = server::serve_http(TestServer::default(), Arc::default()).await;
        let client = McpClient::new(McpClientConfig {
            url: Some(url),
            ..Default::default()
        })
        .expect("client");
        client.connect(false).await.expect("connect");
        client.disconnect().await.expect("disconnect");
        assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
        assert_eq!(tool_named(&client, "add").await.server_name(), "add");
    }

    #[tokio::test]
    async fn falls_back_to_the_legacy_protocol_revision_over_streamable_http() {
        let server = TestServer {
            legacy_only: true,
            ..TestServer::default()
        };
        let url = server::serve_http(server, Arc::default()).await;
        let client = McpClient::new(McpClientConfig {
            url: Some(url),
            ..Default::default()
        })
        .expect("client");
        let add = tool_named(&client, "add").await;
        assert_eq!(
            client.negotiated_protocol_version().as_deref(),
            Some("2025-11-25")
        );
        let result = client
            .call_tool(&add, json!({"a": 15, "b": 27}))
            .await
            .expect("result");
        assert_eq!(result["content"][0]["text"], json!("42"));
    }

    #[tokio::test]
    async fn falls_back_to_the_legacy_protocol_revision_over_a_pipe() {
        let server = TestServer {
            legacy_only: true,
            ..TestServer::default()
        };
        let client = server.client();
        let echo = tool_named(&client, "echo").await;
        assert_eq!(
            client.negotiated_protocol_version().as_deref(),
            Some("2025-11-25")
        );
        let result = client
            .call_tool(&echo, json!({"message": "legacy"}))
            .await
            .expect("result");
        assert_eq!(result["content"][0]["text"], json!("legacy"));
    }
}

mod legacy_sse {
    use std::sync::Mutex;

    use super::*;

    fn sse_client(url: &str, headers: IndexMap<String, String>) -> McpClient {
        McpClient::new(McpClientConfig {
            transport: Some(McpTransport::sse(url, headers).expect("transport")),
            ..Default::default()
        })
        .expect("client")
    }

    #[tokio::test]
    async fn lists_and_calls_tools_over_the_legacy_sse_transport_with_headers() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let server = TestServer {
            legacy_only: true,
            ..TestServer::default()
        };
        let url = server::serve_sse(server, None, Arc::clone(&seen)).await;
        let client = sse_client(
            &url,
            [("X-Api-Token".to_owned(), "abc".to_owned())]
                .into_iter()
                .collect(),
        );
        let echo = tool_named(&client, "echo").await;
        let result = client
            .call_tool(&echo, json!({"message": "over sse"}))
            .await
            .expect("result");
        assert_eq!(result["content"][0]["text"], json!("over sse"));
        assert_eq!(
            client.negotiated_protocol_version().as_deref(),
            Some("2025-11-25")
        );
        let headers = seen.lock().expect("headers").clone();
        assert!(headers.len() >= 3, "GET and at least two POSTs");
        assert!(
            headers
                .iter()
                .all(|headers| headers.get("x-api-token").is_some_and(|value| value == "abc"))
        );
        client.disconnect().await.expect("disconnect");
    }

    #[tokio::test]
    async fn reconnects_over_the_legacy_sse_transport_after_disconnect() {
        let url = server::serve_sse(TestServer::default(), None, Arc::default()).await;
        let client = sse_client(&url, IndexMap::new());
        assert_eq!(tool_named(&client, "add").await.server_name(), "add");
        client.disconnect().await.expect("disconnect");
        assert_eq!(tool_named(&client, "add").await.server_name(), "add");
    }

    #[tokio::test]
    async fn rejects_an_endpoint_on_another_origin() {
        let url = server::serve_sse(
            TestServer::default(),
            Some("http://other.example/messages".to_owned()),
            Arc::default(),
        )
        .await;
        let client = sse_client(&url, IndexMap::new());
        let error = client.connect(false).await.expect_err("origin mismatch");
        assert!(
            error
                .to_string()
                .contains("Endpoint origin does not match connection origin: http://other.example"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn fails_to_connect_when_the_event_stream_is_missing() {
        let url = server::serve_sse(TestServer::default(), None, Arc::default()).await;
        let client = sse_client(&url.replace("/sse", "/missing"), IndexMap::new());
        let error = client.connect(false).await.expect_err("404");
        assert!(
            error.to_string().contains("SSE error: Non-200 status code (404)"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_invalid_sse_url() {
        assert_eq!(
            McpTransport::sse("nope", IndexMap::new())
                .expect_err("invalid")
                .to_string(),
            "Invalid URL"
        );
    }
}

mod filters_and_prefixes {
    use regex::Regex;

    use super::*;

    fn names(tools: &[McpTool]) -> Vec<String> {
        tools.iter().map(|tool| Tool::name(tool).to_owned()).collect()
    }

    fn server_of(names: &[&str]) -> TestServer {
        TestServer::with_tools(names.iter().map(|name| tool(name, None)).collect())
    }

    #[tokio::test]
    async fn matches_strings_and_regexes_against_the_server_side_name_and_passes_the_renamed_tool_to_callbacks()
     {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let callback = {
            let seen = Arc::clone(&seen);
            McpToolMatcher::callback(move |tool| {
                seen.lock().expect("seen").push(Tool::name(tool).to_owned());
                Tool::name(tool) == "server_callback_tool"
            })
        };
        let server = server_of(&[
            "exact",
            "exact_extra",
            "regex_allowed",
            "xregex_unanchored",
            "regex_rejected",
            "callback_tool",
        ]);
        let client = server.client_with(McpClientOptions {
            prefix: Some("server".to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(vec![
                    "exact".into(),
                    Regex::new("regex_").expect("regex").into(),
                    callback,
                ]),
                rejected: Some(vec!["regex_rejected".into()]),
            }),
            ..Default::default()
        });
        let tools = client.list_tools().await.expect("tools");
        assert_eq!(
            names(&tools),
            ["server_exact", "server_regex_allowed", "server_callback_tool"]
        );
        assert_eq!(
            *seen.lock().expect("seen"),
            [
                "server_exact_extra",
                "server_xregex_unanchored",
                "server_callback_tool"
            ]
        );
    }

    #[tokio::test]
    async fn applies_allowed_before_rejected_with_rejected_winning_and_distinguishes_missing_from_empty_allowed()
     {
        let client = server_of(&["one", "two"]).client();
        let list = |allowed: Option<Vec<McpToolMatcher>>, rejected: Option<Vec<McpToolMatcher>>| {
            let client = client.clone();
            async move {
                let options = McpListToolsOptions {
                    tool_filters: Some(McpToolFilters { allowed, rejected }),
                    ..Default::default()
                };
                names(&client.list_tools_with(options).await.expect("tools"))
            }
        };
        assert_eq!(list(None, Some(vec!["two".into()])).await, ["one"]);
        assert!(list(Some(vec![]), None).await.is_empty());
        assert!(
            list(Some(vec!["one".into()]), Some(vec!["one".into()]))
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn inherits_constructor_defaults_while_explicit_empty_prefix_and_filters_disable_them() {
        let mut one = tool("one", None);
        one.input_schema = Arc::new(rmcp::model::JsonObject::new());
        let client = TestServer::with_tools(vec![one, tool("two", None)]).client_with(McpClientOptions {
            prefix: Some("default".to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(vec!["one".into()]),
                rejected: None,
            }),
            ..Default::default()
        });
        let defaulted = client.list_tools().await.expect("tools");
        assert_eq!(names(&defaulted), ["default_one"]);
        assert_eq!(
            serde_json::to_value(defaulted[0].tool_spec()).expect("json"),
            json!({"name": "default_one", "description": "Tool which performs one", "inputSchema": {}})
        );
        let disabled = McpListToolsOptions {
            prefix: Some(String::new()),
            tool_filters: Some(McpToolFilters::default()),
        };
        assert_eq!(
            names(&client.list_tools_with(disabled).await.expect("tools")),
            ["one", "two"]
        );
        let overridden = McpListToolsOptions {
            prefix: Some("override".to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(vec!["two".into()]),
                rejected: None,
            }),
        };
        assert_eq!(
            names(&client.list_tools_with(overridden).await.expect("tools")),
            ["override_two"]
        );
    }

    #[tokio::test]
    async fn filters_and_prefixes_sdk_results_and_invokes_tools_by_the_server_side_name() {
        let mut tools = vec![tool("keep_one", None), tool("drop_one", None)];
        let mut echo = tool("echo_args", None);
        echo.name = "keep_two".into();
        tools.push(echo);
        let server = TestServer::with_tools(tools);
        let client = server.client_with(McpClientOptions {
            prefix: Some("server".to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(vec![Regex::new("keep_").expect("regex").into()]),
                rejected: None,
            }),
            ..Default::default()
        });
        let tools = client.list_tools().await.expect("tools");
        assert_eq!(names(&tools), ["server_keep_one", "server_keep_two"]);
        let _ = client.call_tool(&tools[1], json!({"value": 1})).await;
        let calls = server.calls.lock().expect("calls").clone();
        assert_eq!(calls[0].name, "keep_two");
        assert_eq!(calls[0].arguments, Some(server::object(json!({"value": 1}))));
    }

    #[test]
    fn skips_overlong_names_in_listed_tools_and_preserves_the_64_character_boundary() {
        let boundary = "a".repeat(57);
        let overlong = "a".repeat(58);
        let ((tools, state), warnings) = crate::test_support::WarnRecorder::capture(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let client = server_of(&[&overlong, &boundary, "short"]).client_with(McpClientOptions {
                    prefix: Some("server".to_owned()),
                    continue_on_error: true,
                    ..Default::default()
                });
                (
                    names(&client.list_tools().await.expect("tools")),
                    client.connection_state(),
                )
            })
        });
        assert_eq!(tools, [format!("server_{boundary}"), "server_short".to_owned()]);
        assert_eq!(state, McpConnectionState::Connected);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        for detail in [
            "test-server",
            overlong.as_str(),
            &format!("server_{overlong}"),
            "65",
            "64",
            "prefix",
            "skipping",
        ] {
            assert!(
                warnings[0].contains(detail),
                "{detail} missing from {}",
                warnings[0]
            );
        }
    }

    async fn throws_an_actionable_error(continue_on_error: bool) {
        let server = server_of(&["get_cloudformation_pre_deploy_validation_instructions"]);
        let client = server.client_with(McpClientOptions {
            prefix: Some("awslabs_aws-iac-mcp-server".to_owned()),
            continue_on_error,
            ..Default::default()
        });
        let error = client.list_tools().await.expect_err("overlong");
        assert_eq!(error.name(), "ToolValidationError");
        let message = error.to_string();
        let regex = Regex::new("test-server.*get_cloudformation.*64.*prefix").expect("regex");
        assert!(regex.is_match(&message), "{message}");
    }

    #[tokio::test]
    async fn throws_an_actionable_error_with_continue_on_error_undefined() {
        throws_an_actionable_error(false).await;
    }

    #[tokio::test]
    async fn surfaces_tool_annotations_in_the_tool_spec() {
        let mut annotations = rmcp::model::ToolAnnotations::default();
        annotations.read_only_hint = Some(true);
        annotations.destructive_hint = Some(false);
        let mut annotated = tool("annotated", Some("A"));
        annotated.annotations = Some(annotations);
        let client = TestServer::with_tools(vec![annotated]).client();
        let tools = client.list_tools().await.expect("tools");
        assert_eq!(
            serde_json::to_value(tools[0].tool_spec()).expect("json")["annotations"],
            json!({"readOnlyHint": true, "destructiveHint": false})
        );
    }

    #[tokio::test]
    async fn omits_annotations_from_the_tool_spec_when_the_server_provides_none() {
        let client = server_of(&["plain"]).client();
        let tools = client.list_tools().await.expect("tools");
        assert!(
            serde_json::to_value(tools[0].tool_spec())
                .expect("json")
                .get("annotations")
                .is_none()
        );
    }

    #[tokio::test]
    async fn omits_annotations_from_the_tool_spec_when_the_server_provides_an_empty_annotations_object() {
        let mut empty = tool("empty", None);
        empty.annotations = Some(rmcp::model::ToolAnnotations::default());
        let client = TestServer::with_tools(vec![empty]).client();
        let tools = client.list_tools().await.expect("tools");
        assert!(
            serde_json::to_value(tools[0].tool_spec())
                .expect("json")
                .get("annotations")
                .is_none()
        );
    }
}

mod pagination {
    use super::*;

    #[tokio::test]
    async fn aggregates_every_tools_page() {
        let server = TestServer {
            page_size: Some(2),
            ..TestServer::default()
        };
        let client = server.client();
        assert_eq!(client.list_tools().await.expect("tools").len(), 9);
        assert_eq!(server.list_calls.load(std::sync::atomic::Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn passes_list_max_pages_to_the_sdk_client_when_set() {
        let server = TestServer {
            page_size: Some(2),
            ..TestServer::default()
        };
        let client = server.client_with(McpClientOptions {
            list_max_pages: Some(3),
            ..Default::default()
        });
        let error = client.list_tools().await.expect_err("cap");
        assert_eq!(
            error.to_string(),
            "tools/list: exceeded listMaxPages (3); server pagination did not terminate"
        );
        assert_eq!(error.name(), "SdkError");
    }

    #[tokio::test]
    async fn omits_list_max_pages_when_unset_so_the_sdk_default_applies() {
        let mut tools = Vec::new();
        for n in 0..70 {
            tools.push(tool(&format!("t{n}"), None));
        }
        let server = TestServer {
            page_size: Some(1),
            ..TestServer::with_tools(tools)
        };
        let error = server.client().list_tools().await.expect_err("cap");
        assert_eq!(
            error.to_string(),
            "tools/list: exceeded listMaxPages (64); server pagination did not terminate"
        );
        let uncapped = server.client_with(McpClientOptions {
            list_max_pages: Some(0),
            ..Default::default()
        });
        assert_eq!(uncapped.list_tools().await.expect("tools").len(), 70);
    }

    #[tokio::test]
    async fn ends_the_walk_on_a_repeated_page() {
        let server = TestServer {
            endless_pages: true,
            ..TestServer::default()
        };
        assert_eq!(server.client().list_tools().await.expect("tools").len(), 9);
        assert_eq!(server.list_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}

mod trace_context {
    use super::*;
    use crate::mcp::trace::{current_trace_context, inject_trace_context};

    fn carrier() -> crate::types::json::JsonObject {
        server::object(json!({"traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"}))
    }

    fn echoed_arguments(result: &serde_json::Value) -> serde_json::Value {
        serde_json::from_str(result["content"][0]["text"].as_str().expect("text")).expect("json")
    }

    async fn call_with_trace(disable: bool, args: serde_json::Value) -> serde_json::Value {
        let config = McpClientConfig {
            transport: Some(TestServer::default().transport()),
            options: McpClientOptions {
                disable_mcp_instrumentation: disable,
                ..Default::default()
            },
            ..Default::default()
        };
        let client = McpClient::with_trace_source(config, Arc::new(|| Some(carrier()))).expect("client");
        let echo = tool_named(&client, "echo_args").await;
        echoed_arguments(&client.call_tool(&echo, args).await.expect("result"))
    }

    #[tokio::test]
    async fn sends_trace_context_in_the_arguments_meta() {
        assert_eq!(
            call_with_trace(false, json!({"a": 1})).await,
            json!({"a": 1, "_meta": carrier()})
        );
    }

    #[tokio::test]
    async fn skips_trace_context_injection_when_disable_mcp_instrumentation_is_true() {
        assert_eq!(call_with_trace(true, json!({"a": 1})).await, json!({"a": 1}));
    }

    #[test]
    fn injects_trace_context_into_tool_arguments_when_active_span_exists() {
        let args = inject_trace_context(server::object(json!({"a": 1})), Some(carrier()));
        assert_eq!(
            serde_json::Value::Object(args),
            json!({"a": 1, "_meta": {"traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"}})
        );
    }

    #[test]
    fn merges_trace_context_with_existing_meta_field() {
        let args = inject_trace_context(
            server::object(json!({"a": 1, "_meta": {"progressToken": "p"}})),
            Some(carrier()),
        );
        assert_eq!(
            serde_json::Value::Object(args),
            json!({"a": 1, "_meta": {"progressToken": "p", "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"}})
        );
    }

    #[test]
    fn passes_args_unchanged_when_no_active_span_exists() {
        assert!(current_trace_context().is_none());
        let args = inject_trace_context(server::object(json!({"a": 1})), current_trace_context());
        assert_eq!(serde_json::Value::Object(args), json!({"a": 1}));
    }

    #[cfg(feature = "otel")]
    fn span_context(trace_id: [u8; 16]) -> opentelemetry::Context {
        use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};

        let span = SpanContext::new(
            TraceId::from_bytes(trace_id),
            SpanId::from_bytes([0xb7, 0xad, 0x6b, 0x71, 0x69, 0x20, 0x33, 0x31]),
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        opentelemetry::Context::new().with_remote_span_context(span)
    }

    #[cfg(feature = "otel")]
    #[test]
    fn passes_args_unchanged_when_span_has_empty_trace_id() {
        let carrier = crate::mcp::trace::trace_context_of(&span_context([0; 16]));
        let args = inject_trace_context(server::object(json!({"a": 1})), carrier);
        assert_eq!(serde_json::Value::Object(args), json!({"a": 1}));
    }

    #[cfg(feature = "otel")]
    #[test]
    fn formats_the_w3c_traceparent_of_a_valid_span_context() {
        let trace_id = [
            0x0a, 0xf7, 0x65, 0x19, 0x16, 0xcd, 0x43, 0xdd, 0x84, 0x48, 0xeb, 0x21, 0x1c, 0x80, 0x31, 0x9c,
        ];
        assert_eq!(
            crate::mcp::trace::trace_context_of(&span_context(trace_id)).map(serde_json::Value::Object),
            Some(
                json!({"traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01", "tracestate": ""})
            )
        );
    }
}

mod progress {
    use std::sync::Mutex;

    use super::*;

    fn recorder() -> (McpProgressCallback, Arc<Mutex<Vec<McpProgress>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        (
            Arc::new(move |progress| sink.lock().expect("seen").push(progress)),
            seen,
        )
    }

    fn steps() -> Vec<McpProgress> {
        (1..=3)
            .map(|step| McpProgress::new(f64::from(step), Some(3.0), Some(format!("step {step}"))))
            .collect()
    }

    #[tokio::test]
    async fn forwards_the_client_progress_callback_to_sdk_call_tool_and_delivers_progress_updates() {
        let (callback, seen) = recorder();
        let client = TestServer::default().client_with(McpClientOptions {
            progress_callback: Some(callback),
            ..Default::default()
        });
        let tool = tool_named(&client, "progress").await;
        client.call_tool(&tool, json!({})).await.expect("result");
        server::eventually(|| seen.lock().expect("seen").len() == 3).await;
        assert_eq!(*seen.lock().expect("seen"), steps());
    }

    #[tokio::test]
    async fn per_call_progress_callback_takes_precedence_over_the_client_callback() {
        let (client_callback, client_seen) = recorder();
        let (call_callback, call_seen) = recorder();
        let client = TestServer::default().client_with(McpClientOptions {
            progress_callback: Some(client_callback),
            ..Default::default()
        });
        let tool = tool_named(&client, "progress").await;
        let options = McpCallToolOptions {
            progress_callback: Some(call_callback),
            ..Default::default()
        };
        client
            .call_tool_with(&tool, json!({}), options)
            .await
            .expect("result");
        server::eventually(|| call_seen.lock().expect("seen").len() == 3).await;
        assert!(client_seen.lock().expect("seen").is_empty());
    }

    #[tokio::test]
    async fn delivers_progress_to_the_user_callback_when_a_call_timeout_is_also_set() {
        let (callback, seen) = recorder();
        let client = TestServer::default().client_with(McpClientOptions {
            progress_callback: Some(callback),
            ..Default::default()
        });
        let tool = tool_named(&client, "progress").await;
        let options = McpCallToolOptions {
            timeout: Some(std::time::Duration::from_secs(60)),
            ..Default::default()
        };
        client
            .call_tool_with(&tool, json!({}), options)
            .await
            .expect("result");
        server::eventually(|| seen.lock().expect("seen").len() == 3).await;
        assert_eq!(*seen.lock().expect("seen"), steps());
    }

    #[test]
    fn logs_a_warning_when_the_progress_callback_throws() {
        let (_, warnings) = crate::test_support::WarnRecorder::capture(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let client = TestServer::default().client_with(McpClientOptions {
                    application_name: Some("TestApp".to_owned()),
                    progress_callback: Some(Arc::new(|_| panic!("boom"))),
                    ..Default::default()
                });
                let tool = tool_named(&client, "progress").await;
                client.call_tool(&tool, json!({})).await.expect("result");
                tokio::task::yield_now().await;
            });
        });
        assert!(
            warnings
                .iter()
                .any(|warning| warning == "client=<TestApp>, error=<boom> | progress callback failed"),
            "{warnings:?}"
        );
    }
}

mod elicitation {
    use rmcp::model::{ElicitResult, ElicitationAction};

    use super::*;

    fn answering(
        result: ElicitResult,
    ) -> (ElicitationCallback, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let callback: ElicitationCallback = Arc::new(move |context, params| {
            assert!(!context.cancel_signal().is_cancelled());
            sink.lock()
                .expect("seen")
                .push(serde_json::to_value(&params).expect("json"));
            let result = result.clone();
            Box::pin(async move { Ok(result) })
        });
        (callback, seen)
    }

    async fn elicit(callback: ElicitationCallback) -> serde_json::Value {
        let client = TestServer {
            legacy_only: true,
            ..TestServer::default()
        }
        .client_with(McpClientOptions {
            elicitation_callback: Some(callback),
            ..Default::default()
        });
        let tool = tool_named(&client, "elicit").await;
        client.call_tool(&tool, json!({})).await.expect("result")
    }

    fn answer(action: ElicitationAction, content: Option<serde_json::Value>) -> ElicitResult {
        let mut result: ElicitResult = serde_json::from_value(json!({"action": "decline"})).expect("result");
        result.action = action;
        result.content = content;
        result
    }

    #[tokio::test]
    async fn passes_elicitation_capabilities_to_client_when_callback_is_provided() {
        let (callback, _) = answering(answer(ElicitationAction::Accept, None));
        let client = TestServer::default().client_with(McpClientOptions {
            elicitation_callback: Some(callback),
            ..Default::default()
        });
        assert_eq!(
            serde_json::to_value(client.inner_capabilities()).expect("json"),
            json!({"elicitation": {"form": {}, "url": {}}})
        );
        assert_eq!(
            serde_json::to_value(TestServer::default().client().inner_capabilities()).expect("json"),
            json!({})
        );
    }

    #[tokio::test]
    async fn elicitation_handler_returns_accepted_result_with_content_and_exposes_the_abort_signal_at_mcp_req_signal()
     {
        let (callback, seen) = answering(answer(
            ElicitationAction::Accept,
            Some(json!({"confirmed": true})),
        ));
        let result = elicit(callback).await;
        assert_eq!(
            result["content"][0]["text"],
            json!("{\"action\":\"accept\",\"content\":{\"confirmed\":true}}")
        );
        assert_eq!(seen.lock().expect("seen")[0]["message"], json!("Confirm?"));
    }

    #[tokio::test]
    async fn elicitation_handler_returns_decline_result() {
        let (callback, _) = answering(answer(ElicitationAction::Decline, None));
        assert_eq!(
            elicit(callback).await["content"][0]["text"],
            json!("{\"action\":\"decline\"}")
        );
    }

    #[tokio::test]
    async fn elicitation_handler_returns_cancel_result() {
        let (callback, _) = answering(answer(ElicitationAction::Cancel, None));
        assert_eq!(
            elicit(callback).await["content"][0]["text"],
            json!("{\"action\":\"cancel\"}")
        );
    }

    #[tokio::test]
    async fn elicitation_handler_works_for_url_mode_params() {
        let (callback, seen) = answering(answer(ElicitationAction::Accept, None));
        let mut tools = TestServer::default().tools.lock().expect("tools").clone();
        tools.push(tool("elicit_url", None));
        let client = TestServer {
            legacy_only: true,
            ..TestServer::with_tools(tools)
        }
        .client_with(McpClientOptions {
            elicitation_callback: Some(callback),
            ..Default::default()
        });
        let tool = tool_named(&client, "elicit_url").await;
        let result = client.call_tool(&tool, json!({})).await.expect("result");
        assert_eq!(result["content"][0]["text"], json!("{\"action\":\"accept\"}"));
        assert_eq!(
            seen.lock().expect("seen")[0],
            json!({"mode": "url", "message": "Please authenticate", "url": "https://example.com/auth", "elicitationId": "elicit-123"})
        );
    }

    #[tokio::test]
    async fn answers_input_required_elicitations_on_a_2026_07_28_connection() {
        let (callback, seen) = answering(answer(ElicitationAction::Accept, Some(json!({"ok": true}))));
        let mut tools = TestServer::default().tools.lock().expect("tools").clone();
        tools.push(tool("elicit_mrtr", None));
        let server = TestServer::with_tools(tools);
        let client = server.client_with(McpClientOptions {
            elicitation_callback: Some(callback),
            ..Default::default()
        });
        let tool = tool_named(&client, "elicit_mrtr").await;
        let result = client.call_tool(&tool, json!({})).await.expect("result");
        assert_eq!(
            client.negotiated_protocol_version().as_deref(),
            Some("2026-07-28")
        );
        assert_eq!(
            result["content"][0]["text"],
            json!("{\"action\":\"accept\",\"content\":{\"ok\":true}}")
        );
        assert_eq!(seen.lock().expect("seen").len(), 1);
        let calls = server.calls.lock().expect("calls").clone();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].request_state.as_deref(), Some("state-1"));
    }

    #[tokio::test]
    async fn does_not_register_elicitation_handler_when_no_callback_is_provided() {
        let client = TestServer {
            legacy_only: true,
            ..TestServer::default()
        }
        .client();
        let tool = tool_named(&client, "elicit").await;
        let error = client.call_tool(&tool, json!({})).await.expect_err("no handler");
        assert!(
            error.to_string().contains("Method not found") || error.to_string().contains("-32601"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn elicitation_callback_errors_propagate() {
        let callback: ElicitationCallback = Arc::new(|_, _| Box::pin(async { Err("User cancelled".into()) }));
        let client = TestServer {
            legacy_only: true,
            ..TestServer::default()
        }
        .client_with(McpClientOptions {
            elicitation_callback: Some(callback),
            ..Default::default()
        });
        let tool = tool_named(&client, "elicit").await;
        let error = client.call_tool(&tool, json!({})).await.expect_err("propagated");
        assert!(error.to_string().contains("User cancelled"), "{error}");
    }
}

mod tools_list_changed {
    use std::sync::Mutex;

    use super::*;

    type Calls = Arc<Mutex<Vec<(Vec<String>, Vec<String>)>>>;

    fn record(client: &McpClient) -> Calls {
        let calls = Calls::default();
        let sink = Arc::clone(&calls);
        client.set_on_tools_changed(Some(Arc::new(move |old, new: Vec<McpTool>| {
            let new = new.iter().map(|tool| Tool::name(tool).to_owned()).collect();
            sink.lock().expect("calls").push((old, new));
        })));
        calls
    }

    fn set_tools(server: &TestServer, names: &[&str]) {
        *server.tools.lock().expect("tools") = names.iter().map(|name| tool(name, None)).collect();
    }

    #[tokio::test(start_paused = true)]
    async fn calls_on_tools_changed_with_old_names_and_new_tools_when_list_changes() {
        let server = TestServer::with_tools(vec![tool("tool_a", None)]);
        let client = server.client_with(McpClientOptions {
            prefix: Some("server".to_owned()),
            continue_on_error: true,
            ..Default::default()
        });
        client.list_tools().await.expect("tools");
        let calls = record(&client);
        set_tools(&server, &["tool_a", "tool_b", &"a".repeat(58)]);
        client.refresh_tools().await;
        set_tools(&server, &[]);
        client.refresh_tools().await;
        set_tools(&server, &["tool_c"]);
        client.refresh_tools().await;
        let strings = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            *calls.lock().expect("calls"),
            [
                (
                    strings(&["server_tool_a"]),
                    strings(&["server_tool_a", "server_tool_b"])
                ),
                (strings(&["server_tool_a", "server_tool_b"]), vec![]),
                (vec![], strings(&["server_tool_c"])),
            ]
        );
    }

    #[tokio::test]
    async fn updates_registered_tool_names_after_each_list_tools_call() {
        let server = TestServer::with_tools(vec![tool("x", None), tool("y", None)]);
        let client = server.client();
        client.list_tools().await.expect("tools");
        let calls = record(&client);
        set_tools(&server, &["z"]);
        client.refresh_tools().await;
        let calls = calls.lock().expect("calls").clone();
        assert_eq!(
            calls,
            [(vec!["x".to_owned(), "y".to_owned()], vec!["z".to_owned()])]
        );
    }

    #[tokio::test]
    async fn keeps_default_registered_names_after_an_overridden_listing() {
        let client = TestServer::with_tools(vec![tool("tool_a", None)]).client_with(McpClientOptions {
            prefix: Some("default".to_owned()),
            ..Default::default()
        });
        client.list_tools().await.expect("tools");
        let temporary = McpListToolsOptions {
            prefix: Some("temporary".to_owned()),
            ..Default::default()
        };
        client.list_tools_with(temporary).await.expect("tools");
        let calls = record(&client);
        client.refresh_tools().await;
        assert_eq!(calls.lock().expect("calls")[0].0, ["default_tool_a"]);
    }

    #[tokio::test]
    async fn reapplies_constructor_filters_and_prefix_when_refreshing() {
        let server = TestServer::with_tools(vec![tool("tool_a", None), tool("tool_b", None)]);
        let client = server.client_with(McpClientOptions {
            prefix: Some("server".to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(vec!["tool_a".into()]),
                rejected: None,
            }),
            ..Default::default()
        });
        client.list_tools().await.expect("tools");
        let calls = record(&client);
        client.refresh_tools().await;
        assert_eq!(
            *calls.lock().expect("calls"),
            [(vec!["server_tool_a".to_owned()], vec!["server_tool_a".to_owned()])]
        );
    }

    #[tokio::test]
    async fn does_not_throw_when_on_tools_changed_is_not_set() {
        let server = TestServer::with_tools(vec![tool("tool_a", None)]);
        let client = server.client();
        client.list_tools().await.expect("tools");
        set_tools(&server, &["tool_b"]);
        client.refresh_tools().await;
    }

    fn logs_warning_and_preserves_registry_when_list_tools_fails_during_refresh(failing: &str) {
        let (calls, warnings) = crate::test_support::WarnRecorder::capture(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let server = TestServer::with_tools(vec![tool("tool_a", None)]);
                let client = server.client_with(McpClientOptions {
                    prefix: Some("server".to_owned()),
                    ..Default::default()
                });
                client.list_tools().await.expect("tools");
                let calls = record(&client);
                set_tools(&server, &[failing]);
                client.refresh_tools().await;
                let after_failure = calls.lock().expect("calls").len();
                set_tools(&server, &["tool_b"]);
                client.refresh_tools().await;
                (after_failure, calls.lock().expect("calls").clone())
            })
        });
        assert_eq!(calls.0, 0);
        assert_eq!(calls.1[0].0, ["server_tool_a"]);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("failed to refresh tools")),
            "{warnings:?}"
        );
    }

    #[test]
    fn logs_warning_and_preserves_registry_when_list_tools_fails_during_refresh_request() {
        logs_warning_and_preserves_registry_when_list_tools_fails_during_refresh("fail_list");
    }

    #[test]
    fn logs_warning_and_preserves_registry_when_list_tools_fails_during_refresh_name_length() {
        logs_warning_and_preserves_registry_when_list_tools_fails_during_refresh(&"a".repeat(58));
    }

    #[tokio::test]
    async fn coalesces_notifications_received_during_an_in_flight_refresh_into_one_extra_refresh() {
        let server = TestServer::default();
        let client = server.client();
        client.list_tools().await.expect("tools");
        let calls = record(&client);
        let before = server.list_calls.load(std::sync::atomic::Ordering::SeqCst);
        let refreshes = (0..4).map(|_| client.refresh_tools());
        futures::future::join_all(refreshes).await;
        assert_eq!(calls.lock().expect("calls").len(), 2);
        assert_eq!(
            server.list_calls.load(std::sync::atomic::Ordering::SeqCst),
            before + 2
        );
    }

    #[tokio::test(start_paused = true)]
    async fn debounces_server_notifications_into_one_refresh() {
        let server = TestServer::default();
        let client = server.client();
        let tool = tool_named(&client, "change_tools").await;
        let calls = record(&client);
        for _ in 0..3 {
            client.call_tool(&tool, json!({})).await.expect("result");
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        server::eventually(|| !calls.lock().expect("calls").is_empty()).await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert_eq!(calls.lock().expect("calls").len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_agent_replaces_its_mcp_tools_after_a_list_change() {
        use crate::testing::MockMessageModel;
        use crate::types::messages::{ContentBlock, TextBlock, ToolUseBlock};

        let server = TestServer::default();
        let client = server.client();
        let model = MockMessageModel::new();
        model.add_turn(vec![ContentBlock::from(ToolUseBlock::new(
            "change_tools",
            "t1",
            json!({}),
        ))]);
        model.add_turn(vec![TextBlock::new("done").into()]);
        let mut agent = crate::Agent::builder()
            .model(model)
            .tools(vec![ToolSource::from(client.clone())])
            .printer(false)
            .build()
            .expect("agent");
        agent.initialize().await.expect("initialize");
        set_tools(&server, &["fresh"]);
        agent.invoke("go").await.expect("first turn");
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        server::eventually(|| server.list_calls.load(std::sync::atomic::Ordering::SeqCst) >= 2).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let model = MockMessageModel::new();
        model.add_turn(vec![TextBlock::new("again").into()]);
        agent.set_model(Arc::new(model));
        agent.invoke("again").await.expect("second turn");
        assert!(agent.tool_registry().get("fresh").is_some());
        assert!(agent.tool_registry().get("echo").is_none());
    }
}

mod prompts_and_resources {
    use super::*;

    fn options(token: Option<&str>) -> McpListOptions {
        McpListOptions {
            pagination_token: token.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn connects_lazily_and_lists_prompts_with_the_raw_result_returned() {
        let client = TestServer::default().client();
        let result = client.list_prompts_with(options(None)).await.expect("prompts");
        let names: Vec<&str> = result.prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        assert_eq!(names, ["greeting", "farewell"]);
        assert!(result.next_cursor.is_none());
    }

    #[tokio::test]
    async fn fetches_a_single_prompts_page_when_pagination_token_is_set() {
        let client = TestServer::default().client();
        let result = client
            .list_prompts_with(options(Some("p2")))
            .await
            .expect("prompts");
        assert_eq!(result.prompts.len(), 1);
        assert_eq!(result.prompts[0].name, "farewell");
    }

    #[tokio::test]
    async fn gets_a_prompt_by_name_with_arguments() {
        let client = TestServer::default().client();
        let arguments = [("name".to_owned(), "Ada".to_owned())].into_iter().collect();
        let result = client
            .get_prompt("greeting", Some(arguments))
            .await
            .expect("prompt");
        assert_eq!(
            serde_json::to_value(&result.messages[0]).expect("json")["content"]["text"],
            json!("greeting: hello Ada")
        );
    }

    #[tokio::test]
    async fn gets_a_prompt_by_name_without_arguments() {
        let client = TestServer::default().client();
        let result = client.get_prompt("greeting", None).await.expect("prompt");
        assert_eq!(
            serde_json::to_value(&result.messages[0]).expect("json")["content"]["text"],
            json!("greeting: hello world")
        );
    }

    #[tokio::test]
    async fn lists_resources_with_the_raw_result_returned() {
        let client = TestServer::default().client();
        let result = client
            .list_resources_with(options(None))
            .await
            .expect("resources");
        let uris: Vec<&str> = result
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect();
        assert_eq!(uris, ["test://static", "test://second"]);
    }

    #[tokio::test]
    async fn fetches_a_single_resources_page_when_pagination_token_is_set() {
        let client = TestServer::default().client();
        let result = client
            .list_resources_with(options(Some("r2")))
            .await
            .expect("resources");
        assert_eq!(result.resources.len(), 1);
    }

    #[tokio::test]
    async fn reads_a_resource_by_string_uri() {
        let client = TestServer::default().client();
        let result = client.read_resource("test://static").await.expect("resource");
        assert_eq!(
            serde_json::to_value(&result.contents[0]).expect("json")["text"],
            json!("contents of test://static")
        );
    }

    #[tokio::test]
    async fn lists_resource_templates_with_pagination() {
        let client = TestServer::default().client();
        let result = client
            .list_resource_templates_with(options(None))
            .await
            .expect("templates");
        assert_eq!(result.resource_templates[0].uri_template, "test://users/{id}");
    }

    #[tokio::test]
    async fn throws_when_the_connection_previously_failed_under_continue_on_error() {
        let gate = Gate::default();
        gate.fail();
        let client = gated_client(&gate, true);
        let error = client.list_prompts_with(options(None)).await.expect_err("failed");
        assert_eq!(
            error.to_string(),
            "MCP server failed to connect. Call connect(true) to retry."
        );
        assert!(client.read_resource("test://static").await.is_err());
    }
}

mod log_routing {
    #![allow(deprecated, reason = "MCP logging is deprecated in rmcp")]

    use rmcp::model::{LoggingLevel, LoggingMessageNotificationParam};

    use super::*;
    use crate::mcp::handler::{log_level, log_message};

    fn routed(level: LoggingLevel) -> tracing::Level {
        log_level(level)
    }

    #[test]
    fn routes_debug_level_to_logger_debug() {
        assert_eq!(routed(LoggingLevel::Debug), tracing::Level::DEBUG);
    }

    #[test]
    fn routes_info_level_to_logger_info() {
        assert_eq!(routed(LoggingLevel::Info), tracing::Level::INFO);
    }

    #[test]
    fn routes_notice_level_to_logger_info() {
        assert_eq!(routed(LoggingLevel::Notice), tracing::Level::INFO);
    }

    #[test]
    fn routes_warning_level_to_logger_warn() {
        assert_eq!(routed(LoggingLevel::Warning), tracing::Level::WARN);
    }

    #[test]
    fn routes_error_level_to_logger_error() {
        assert_eq!(routed(LoggingLevel::Error), tracing::Level::ERROR);
    }

    #[test]
    fn routes_critical_level_to_logger_error() {
        assert_eq!(routed(LoggingLevel::Critical), tracing::Level::ERROR);
    }

    #[test]
    fn routes_alert_level_to_logger_error() {
        assert_eq!(routed(LoggingLevel::Alert), tracing::Level::ERROR);
    }

    #[test]
    fn routes_emergency_level_to_logger_error() {
        assert_eq!(routed(LoggingLevel::Emergency), tracing::Level::ERROR);
    }

    #[test]
    fn includes_logger_name_and_data_in_the_message() {
        let params = LoggingMessageNotificationParam::new(LoggingLevel::Info, json!({"key": "value"}))
            .with_logger("my-logger");
        assert_eq!(
            log_message(&params),
            "logger=<my-logger>, data=<{\"key\":\"value\"}> | MCP server log"
        );
        let unnamed = LoggingMessageNotificationParam::new(LoggingLevel::Info, json!("text"));
        assert_eq!(
            log_message(&unnamed),
            "logger=<mcp>, data=<\"text\"> | MCP server log"
        );
    }

    #[tokio::test]
    async fn calls_custom_log_handler_when_provided() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let client = TestServer::default().client_with(McpClientOptions {
            log_handler: Some(Arc::new(move |params| sink.lock().expect("seen").push(params))),
            ..Default::default()
        });
        let tool = tool_named(&client, "log").await;
        client.call_tool(&tool, json!({})).await.expect("result");
        server::eventually(|| !seen.lock().expect("seen").is_empty()).await;
        let params = seen.lock().expect("seen")[0].clone();
        assert_eq!(params.logger.as_deref(), Some("srv"));
        assert_eq!(params.data, json!({"note": "hi"}));
    }

    #[test]
    fn routes_server_logs_through_tracing_by_default() {
        let (_, warnings) = crate::test_support::WarnRecorder::capture(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let client = TestServer::default().client();
                let tool = tool_named(&client, "log").await;
                client.call_tool(&tool, json!({})).await.expect("result");
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
            });
        });
        assert!(
            warnings
                .iter()
                .any(|warning| warning == "logger=<srv>, data=<{\"note\":\"hi\"}> | MCP server log"),
            "{warnings:?}"
        );
    }
}

mod agent_integration {
    use super::*;
    use crate::Agent;
    use crate::testing::MockMessageModel;
    use crate::types::messages::{ContentBlock, TextBlock, ToolUseBlock};

    fn scripted() -> MockMessageModel {
        let model = MockMessageModel::new();
        model.add_turn(vec![ContentBlock::from(ToolUseBlock::new(
            "echo",
            "t1",
            json!({"message": "hi"}),
        ))]);
        model.add_turn(vec![TextBlock::new("done").into()]);
        model
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_agent_lists_and_calls_mcp_tools_from_a_client_source() {
        let server = TestServer::default();
        let client = server.client();
        let mut agent = Agent::builder()
            .model(scripted())
            .tools(vec![ToolSource::from(client.clone())])
            .printer(false)
            .build()
            .expect("agent");
        let result = agent.invoke("go").await.expect("result");
        assert_eq!(result.to_string(), "done");
        assert!(agent.tool_registry().get("echo").is_some());
        let calls = server.calls.lock().expect("calls").clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, Some(server::object(json!({"message": "hi"}))));
        client.disconnect().await.expect("disconnect");
    }

    #[tokio::test]
    async fn forwards_the_tool_execution_cancel_signal_to_call_tool() {
        let server = TestServer::default();
        let model = MockMessageModel::new();
        model.add_turn(vec![ContentBlock::from(ToolUseBlock::new(
            "slow",
            "t1",
            json!({}),
        ))]);
        model.add_turn(vec![TextBlock::new("done").into()]);
        let mut agent = Agent::builder()
            .model(model)
            .tools(vec![ToolSource::from(server.client())])
            .printer(false)
            .build()
            .expect("agent");
        let handle = agent.cancel_handle();
        let calls = Arc::clone(&server.calls);
        let canceller = tokio::spawn(async move {
            server::eventually(|| !calls.lock().expect("calls").is_empty()).await;
            handle.cancel(CancelOptions::default());
        });
        let result = agent.invoke("go").await.expect("result");
        canceller.await.expect("join");
        assert_eq!(result.stop_reason(), crate::StopReason::Cancelled);
    }

    #[tokio::test]
    async fn reuses_one_client_across_two_agents() {
        let client = TestServer::default().client();
        for _ in 0..2 {
            let mut agent = Agent::builder()
                .model(scripted())
                .tools(vec![ToolSource::List(vec![client.clone().into()])])
                .printer(false)
                .build()
                .expect("agent");
            agent.invoke("go").await.expect("result");
        }
    }
}

mod transport_resolution {
    use super::*;

    fn config(transport: bool, url: Option<&str>) -> McpClientConfig {
        McpClientConfig {
            transport: transport.then(|| TestServer::default().transport()),
            url: url.map(str::to_owned),
            ..Default::default()
        }
    }

    fn error(config: McpClientConfig) -> String {
        McpClient::new(config).expect_err("rejected").to_string()
    }

    #[test]
    fn constructs_streamable_http_client_transport_when_url_is_provided() {
        let client = McpClient::new(config(false, Some("http://localhost:3000/mcp"))).expect("client");
        assert_eq!(client.connection_state(), McpConnectionState::Disconnected);
    }

    #[test]
    fn throws_when_both_transport_and_url_are_provided() {
        assert_eq!(
            error(config(true, Some("http://localhost:3000/mcp"))),
            "McpClientConfig: provide either \"transport\" or \"url\", not both"
        );
    }

    #[test]
    fn throws_when_neither_transport_nor_url_is_provided() {
        assert_eq!(
            error(config(false, None)),
            "McpClientConfig: either \"transport\" or \"url\" must be provided"
        );
    }

    #[test]
    fn throws_when_headers_is_provided_with_transport() {
        let mut config = config(true, None);
        config.headers = Some(
            [("Authorization".to_owned(), "Bearer t".to_owned())]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            error(config),
            "McpClientConfig: \"auth\", \"authProvider\", and \"headers\" require \"url\" (not compatible with \"transport\")"
        );
    }

    fn credentials() -> McpClientCredentials {
        McpClientCredentials {
            client_id: "id".to_owned(),
            client_secret: "secret".to_owned(),
            scopes: Some(vec!["read".to_owned(), "write".to_owned()]),
        }
    }

    #[test]
    fn constructs_client_credentials_provider_when_auth_is_provided() {
        let mut config = config(false, Some("http://localhost:3000/mcp"));
        config.auth = Some(credentials());
        assert!(McpClient::new(config).is_ok());
    }

    fn bearer_tokens(seen: &std::sync::Mutex<Vec<http::HeaderMap>>) -> Vec<String> {
        seen.lock()
            .expect("headers")
            .iter()
            .filter_map(|headers| headers.get(http::header::AUTHORIZATION))
            .map(|value| value.to_str().expect("ascii").to_owned())
            .collect()
    }

    #[tokio::test]
    async fn passes_scopes_as_space_separated_string() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let tokens = Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = server::serve_http_on(
            listener,
            TestServer::default(),
            Arc::clone(&seen),
            Arc::clone(&tokens),
        );
        let mut config = config(false, Some(&url));
        config.auth = Some(credentials());
        let client = McpClient::new(config).expect("client");
        assert_eq!(tool_named(&client, "echo").await.server_name(), "echo");
        let body = tokens.lock().expect("tokens").join("&");
        for pair in [
            "grant_type=client_credentials",
            "client_id=id",
            "scope=read+write",
        ] {
            assert!(
                body.split('&').any(|field| field == pair),
                "{pair} missing from {body}"
            );
        }
        assert!(body.contains("resource=http"), "{body}");
        assert!(bearer_tokens(&seen).contains(&"Bearer tok-123".to_owned()));
        client.disconnect().await.expect("disconnect");
    }

    #[tokio::test]
    async fn passes_custom_auth_provider_to_transport() {
        use rmcp::transport::auth::{CredentialStore, InMemoryCredentialStore, StoredCredentials};

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let url = server::serve_http(TestServer::default(), Arc::clone(&seen)).await;
        let store = InMemoryCredentialStore::new();
        let stored: StoredCredentials = serde_json::from_value(json!({
            "client_id": "id",
            "token_response": {"access_token": "custom-token", "token_type": "bearer"}
        }))
        .expect("credentials");
        store.save(stored).await.expect("save");
        let mut manager = rmcp::transport::AuthorizationManager::new(url.as_str())
            .await
            .expect("manager");
        manager.set_credential_store(store);
        let mut config = config(false, Some(&url));
        config.auth_provider = Some(rmcp::transport::AuthClient::new(reqwest::Client::new(), manager));
        let client = McpClient::new(config).expect("client");
        assert_eq!(tool_named(&client, "echo").await.server_name(), "echo");
        let tokens = bearer_tokens(&seen);
        assert!(!tokens.is_empty());
        assert!(
            tokens.iter().all(|token| token == "Bearer custom-token"),
            "{tokens:?}"
        );
        client.disconnect().await.expect("disconnect");
    }

    #[test]
    fn throws_when_auth_is_provided_with_transport() {
        let mut config = config(true, None);
        config.auth = Some(credentials());
        assert_eq!(
            error(config),
            "McpClientConfig: \"auth\", \"authProvider\", and \"headers\" require \"url\" (not compatible with \"transport\")"
        );
    }

    #[tokio::test]
    async fn throws_when_both_auth_and_auth_provider_are_provided() {
        let mut config = config(false, Some("http://localhost:3000/mcp"));
        config.auth = Some(credentials());
        config.auth_provider = Some(rmcp::transport::AuthClient::new(
            reqwest::Client::new(),
            rmcp::transport::AuthorizationManager::new("http://localhost:3000/mcp")
                .await
                .expect("manager"),
        ));
        assert_eq!(
            error(config),
            "McpClientConfig: provide either \"auth\" or \"authProvider\", not both"
        );
    }

    #[test]
    fn redacts_the_client_secret_in_debug_output() {
        let debug = format!("{:?}", credentials());
        assert!(!debug.contains("secret\""), "{debug}");
        assert!(debug.contains("<redacted>"));
    }

    #[tokio::test]
    async fn passes_both_auth_and_headers_to_transport() {
        let mut config = config(false, Some("http://127.0.0.1:9/mcp"));
        config.auth = Some(credentials());
        config.headers = Some([("X-Test".to_owned(), "1".to_owned())].into_iter().collect());
        let client = McpClient::new(config).expect("client");
        assert!(
            client.connect(false).await.is_err(),
            "no server answers on the discard port"
        );
    }

    #[test]
    fn rejects_an_invalid_url() {
        assert_eq!(error(config(false, Some("not a url"))), "Invalid URL");
    }
}

mod client_annotations {
    //! Ported from strands-ts/src/mcp/__tests__/client-annotations.test.ts, over the in-memory pipe.
    use super::*;

    /// The server end, with `from` replaced by `to` in every line the server writes.
    fn rewriting(server: TestServer, from: &'static str, to: &'static str) -> McpTransport {
        McpTransport::from_factory(move || {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

            let (client, proxy) = tokio::io::duplex(64 * 1024);
            let (mut from_client, mut to_client) = tokio::io::split(proxy);
            let (from_server, mut to_server) = tokio::io::split(server.pipe());
            tokio::spawn(async move {
                let _copied = tokio::io::copy(&mut from_client, &mut to_server).await;
            });
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(from_server).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = format!("{}\n", line.replace(from, to));
                    if to_client.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
            client
        })
    }

    async fn annotations(transport: McpTransport) -> Option<serde_json::Value> {
        let client = McpClient::new(McpClientConfig {
            transport: Some(transport),
            ..Default::default()
        })
        .expect("client");
        let tools = client.list_tools().await.expect("tools");
        serde_json::to_value(tools[0].tool_spec())
            .expect("json")
            .get("annotations")
            .cloned()
    }

    fn server(annotations: Option<serde_json::Value>) -> TestServer {
        let mut echo = tool("echo", Some("Echoes input"));
        echo.annotations = annotations.map(|value| serde_json::from_value(value).expect("annotations"));
        TestServer::with_tools(vec![echo])
    }

    #[tokio::test]
    async fn surfaces_known_annotation_keys_from_the_server() {
        let known = json!({"title": "Echo", "readOnlyHint": true, "openWorldHint": false});
        assert_eq!(
            annotations(server(Some(known.clone())).transport()).await,
            Some(known)
        );
    }

    #[tokio::test]
    async fn preserves_explicitly_false_hints() {
        let hints = json!({"readOnlyHint": false, "destructiveHint": false});
        assert_eq!(
            annotations(server(Some(hints.clone())).transport()).await,
            Some(hints)
        );
    }

    #[tokio::test]
    async fn strips_unknown_annotation_keys_at_the_mcp_sdk_boundary() {
        let transport = rewriting(
            server(Some(json!({"readOnlyHint": true}))),
            r#""readOnlyHint":true"#,
            r#""readOnlyHint":true,"futureSepKey":"x""#,
        );
        assert_eq!(annotations(transport).await, Some(json!({"readOnlyHint": true})));
    }

    #[tokio::test]
    async fn omits_annotations_when_the_server_declares_none() {
        assert_eq!(annotations(server(None).transport()).await, None);
    }
}
