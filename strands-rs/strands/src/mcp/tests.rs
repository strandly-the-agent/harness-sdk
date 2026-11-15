//! Ported from strands-ts/src/mcp/__tests__/client.test.ts
//!
//! The TS tests mock the MCP SDK `Client`; these run against an in-process `rmcp` server
//! (`tests/server.rs`) over an in-memory pipe.

mod server;

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
        let second = client.connect_with(false, Some(&signal));
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
            async move { client.connect_with(false, Some(&signal)).await }
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
            async move { client.connect_with(false, Some(&signal)).await }
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
            .connect_with(false, Some(&signal))
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

    #[test]
    fn rejects_an_invalid_url() {
        assert_eq!(error(config(false, Some("not a url"))), "Invalid URL");
    }
}
