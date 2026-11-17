//! The MCP cases of `subagent delegate (built child)` in `harness-ts/test/agent.test.ts`.
//!
//! TS spawns `echo-mcp-server.mjs` over stdio and calls the tool's private `builder`. Here each
//! client talks to an in-process echo server over an in-memory pipe, the real `subagent` tool
//! runs, and a case reads the tools the delegate offered the shared mock model. A client reaches
//! the harness through `tools`: a server loaded from the `mcp_servers` config joins the same list
//! (see `loads_mcp_servers_through_the_sdk_resilient_and_prefixed_by_server_name`), and loading
//! one needs a child process.

use std::borrow::Cow;

use futures::StreamExt;
use rmcp::model::{
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool as McpToolSpec,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;
use strands::mcp::{McpClient, McpClientConfig, McpClientOptions, McpTransport};
use strands::models::model::ModelRequest;
use strands::testing::tool_context;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::types::tools::ToolUse;

use super::*;
use crate::test_support::WarnRecorder;

/// An MCP server with one tool, `echo`.
#[derive(Clone)]
struct EchoServer;

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        let schema = json!({ "type": "object", "properties": { "message": { "type": "string" } } });
        let JsonValue::Object(schema) = schema else {
            panic!("an object literal");
        };
        let mut echo = McpToolSpec::default();
        echo.name = Cow::Borrowed("echo");
        echo.description = Some(Cow::Borrowed("Echoes the message."));
        echo.input_schema = Arc::new(schema);
        Ok(ListToolsResult {
            tools: vec![echo],
            ..Default::default()
        })
    }
}

/// A client of an echo server, with its tools prefixed `<prefix>_`.
fn echo_client(application_name: Option<&str>, prefix: &str) -> ToolSource {
    let transport = McpTransport::from_factory(|| {
        let (client, server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            if let Ok(running) = EchoServer.serve(server).await {
                let _done: std::result::Result<_, _> = running.waiting().await;
            }
        });
        client
    });
    let client = McpClient::new(McpClientConfig {
        transport: Some(transport),
        options: McpClientOptions {
            application_name: application_name.map(str::to_owned),
            prefix: Some(prefix.to_owned()),
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    ToolSource::McpClient(client)
}

/// An initialized harness agent over `clients`, on a shared mock model, with background tasks
/// off so a delegate's own `subagent` call runs inline.
async fn parent_of(clients: Vec<ToolSource>) -> (Agent, Arc<MockMessageModel>) {
    let dir = tempfile::tempdir().unwrap();
    let (model, mut options) = shared_mock();
    options.tools = clients;
    options.background_tasks = Some(BackgroundTasks::Enabled(false));
    let mut parent = create_harness_with(options, seams(&dir)).await.unwrap();
    parent.initialize().await.unwrap();
    (parent, model)
}

/// [`parent_of`] on `runtime`, with the warnings the build logged. A second build repeats them:
/// TS logs them with the plain logger, not `warnOnce`.
fn parent_of_logging(
    runtime: &tokio::runtime::Runtime,
    clients: impl Fn() -> Vec<ToolSource>,
) -> (Agent, Arc<MockMessageModel>, Vec<String>) {
    let (_, first) = WarnRecorder::capture(|| runtime.block_on(parent_of(clients())));
    let ((parent, model), warnings) = WarnRecorder::capture(|| runtime.block_on(parent_of(clients())));
    assert_eq!(first, warnings);
    (parent, model, warnings)
}

fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn offered(request: &ModelRequest) -> Vec<String> {
    request
        .tool_specs
        .iter()
        .map(|spec| spec.name().to_owned())
        .collect()
}

/// Runs the parent's `subagent` tool once with `input` and returns the requests it led to.
async fn run_subagent(parent: &Agent, model: &Arc<MockMessageModel>, input: JsonValue) -> Vec<ModelRequest> {
    let before = model.requests().len();
    let subagent = Arc::clone(parent.tools().find(|tool| tool.name() == "subagent").unwrap());
    let mut stream = subagent.stream(tool_context(ToolUse::new("subagent", "t1", input)));
    while stream.next().await.is_some() {}
    model.requests().split_off(before)
}

/// The tools the delegate built for `input` offers its model.
async fn child_tools(parent: &Agent, model: &Arc<MockMessageModel>, input: JsonValue) -> Vec<String> {
    model.add_turn(vec![TextBlock::new("done").into()]);
    let requests = run_subagent(parent, model, input).await;
    assert_eq!(requests.len(), 1, "the delegate called the model once");
    offered(&requests[0])
}

/// The tools of the delegate built for `input`, and of the delegate that one builds in turn.
async fn child_and_grandchild_tools(
    parent: &Agent,
    model: &Arc<MockMessageModel>,
    input: JsonValue,
) -> (Vec<String>, Vec<String>) {
    let delegate_again = ToolUseBlock::new("subagent", "t2", json!({ "task": "y" }));
    model.add_turn(vec![delegate_again.into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let requests = run_subagent(parent, model, input).await;
    assert_eq!(requests.len(), 3, "child, grandchild, child");
    (offered(&requests[0]), offered(&requests[1]))
}

fn has(tools: &[String], name: &str) -> bool {
    tools.iter().any(|tool| tool == name)
}

/// The `subagent` tool's input schema properties.
fn subagent_properties(parent: &Agent) -> JsonValue {
    let subagent = parent.tools().find(|tool| tool.name() == "subagent").unwrap();
    subagent.tool_spec().input_schema().unwrap()["properties"].clone()
}

// A delegate shares the parent's connected clients, keyed by server name: an omitted
// `mcp_servers` grants every server, an empty one drops them, a named one grants it.
#[tokio::test]
async fn inherits_the_parent_mcp_tools_via_the_shared_clients_narrowable_per_server() {
    let (parent, model) = parent_of(vec![echo_client(Some("srv"), "srv")]).await;
    assert!(has_tool(&parent, "srv_echo"), "{:?}", tool_names(&parent));

    let inherit_all = child_tools(&parent, &model, json!({ "task": "x" })).await;
    assert!(has(&inherit_all, "srv_echo"), "{inherit_all:?}");

    let narrowed = child_tools(&parent, &model, json!({ "task": "x", "mcp_servers": [] })).await;
    assert!(!has(&narrowed, "srv_echo"), "{narrowed:?}");

    // A repeated name must not forward a client twice.
    let selected = child_tools(
        &parent,
        &model,
        json!({ "task": "x", "mcp_servers": ["srv", "srv"] }),
    )
    .await;
    assert_eq!(
        selected.iter().filter(|name| *name == "srv_echo").count(),
        1,
        "{selected:?}"
    );

    assert_eq!(
        subagent_properties(&parent)["mcp_servers"]["items"]["enum"],
        json!(["srv"])
    );
}

#[tokio::test]
async fn offers_mcp_clients_passed_in_tools_on_the_mcp_servers_axis() {
    let clients = vec![
        echo_client(Some("alpha"), "alpha"),
        echo_client(Some("beta"), "beta"),
    ];
    let (parent, model) = parent_of(clients).await;
    assert!(has_tool(&parent, "alpha_echo") && has_tool(&parent, "beta_echo"));
    let properties = subagent_properties(&parent);
    assert_eq!(
        properties["mcp_servers"]["items"]["enum"],
        json!(["alpha", "beta"])
    );
    // Servers never pose as tools.
    let tools_axis = properties["tools"]["items"]["enum"].as_array().unwrap();
    assert!(!tools_axis.contains(&json!("alpha")), "{tools_axis:?}");

    // Narrowing both axes at once: the tools split must not drop the selected servers.
    let input = json!({ "task": "x", "tools": ["read"], "mcp_servers": ["beta"] });
    let child = child_tools(&parent, &model, input).await;
    assert!(has(&child, "read") && has(&child, "beta_echo"), "{child:?}");
    assert!(!has(&child, "alpha_echo"), "{child:?}");
}

// Two clients under one name can't both sit behind one enum value: the first is offered, the
// second is dropped from delegation, and the collapse is warned about.
#[test]
fn warns_when_two_mcp_clients_share_a_client_name_and_offers_only_the_first() {
    let runtime = current_thread_runtime();
    let (parent, model, warnings) = parent_of_logging(&runtime, || {
        vec![
            echo_client(Some("shared"), "alpha"),
            echo_client(Some("shared"), "beta"),
        ]
    });
    assert_eq!(
        warnings,
        [
            "Two McpClients in `tools` share the name \"shared\"; only the first is offered to subagents on `mcp_servers`."
        ]
    );
    // The parent keeps both.
    assert!(has_tool(&parent, "alpha_echo") && has_tool(&parent, "beta_echo"));
    assert_eq!(
        subagent_properties(&parent)["mcp_servers"]["items"]["enum"],
        json!(["shared"])
    );

    let input = json!({ "task": "x", "mcp_servers": ["shared"] });
    let child = runtime.block_on(child_tools(&parent, &model, input));
    assert!(has(&child, "alpha_echo"), "{child:?}");
    assert!(!has(&child, "beta_echo"), "{child:?}");
}

// A client without a name the model could pick stays out of the axis, and out of delegates.
#[test]
fn warns_about_an_mcp_client_without_application_name_and_leaves_it_off_the_mcp_servers_axis() {
    let runtime = current_thread_runtime();
    let (parent, model, warnings) = parent_of_logging(&runtime, || vec![echo_client(None, "bare")]);
    assert_eq!(
        warnings,
        [
            "An McpClient in `tools` has no application_name; it can't be offered to subagents on `mcp_servers`, so delegates won't get its tools. Set application_name to name it."
        ]
    );
    assert!(has_tool(&parent, "bare_echo"), "{:?}", tool_names(&parent));
    // No offerable server, so no axis at all.
    assert_eq!(subagent_properties(&parent).get("mcp_servers"), None);

    let child = runtime.block_on(child_tools(&parent, &model, json!({ "task": "x" })));
    assert!(!has(&child, "bare_echo"), "{child:?}");
}

#[tokio::test]
async fn forwards_inherited_mcp_servers_to_a_grandchild() {
    let (parent, model) = parent_of(vec![echo_client(Some("srv"), "srv")]).await;
    let (child, grandchild) = child_and_grandchild_tools(&parent, &model, json!({ "task": "x" })).await;
    assert!(has(&child, "srv_echo"), "{child:?}");
    assert!(has(&grandchild, "srv_echo"), "{grandchild:?}");
}

// A child restricted to no MCP servers can't pass any to a grandchild; a granted server carries on.
#[tokio::test]
async fn propagates_mcp_narrowing_through_a_grandchild() {
    let (parent, model) = parent_of(vec![echo_client(Some("srv"), "srv")]).await;

    let starved = json!({ "task": "x", "mcp_servers": [] });
    let (child, grandchild) = child_and_grandchild_tools(&parent, &model, starved).await;
    assert!(!has(&child, "srv_echo"), "{child:?}");
    assert!(!has(&grandchild, "srv_echo"), "{grandchild:?}");

    let granted = json!({ "task": "x", "mcp_servers": ["srv"] });
    let (_, grandchild) = child_and_grandchild_tools(&parent, &model, granted).await;
    assert!(has(&grandchild, "srv_echo"), "{grandchild:?}");
}
