//! The `mcp_router` tool (`strands-ts/src/vended-tools/mcp-router/`).
//!
//! Connects to MCP servers from a developer-set allowlist at runtime, lists their tools and
//! invokes them, keeping the open connections across tool calls.
//!
//! Divergences from TS: connections live on the tool instance (one `McpRouterTool` per agent)
//! instead of in a `WeakMap` keyed by agent. Rust has no finalization registry and `Drop` cannot
//! await, so nothing closes orphaned connections implicitly — the owner must call
//! [`McpRouterTool::disconnect_all`] before dropping the tool. Error messages are the TS ones
//! verbatim, as [`Error::ToolValidation`] for bad model input and [`Error::other`] for runtime
//! failures rather than a dedicated `McpRouterToolError`, and `call_tool` goes through
//! [`McpClient::call_tool`] plus the same content mapping [`crate::mcp::McpTool`] uses.

use async_trait::async_trait;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::mcp::{ListOptions, LoadOptions, McpClient, McpConnectionState, McpServerConfig};
use crate::tools::tool::{tool_error_or_propagate, Tool, ToolContext};
use crate::tools::wrap_in_tool_result;
use crate::types::{ToolResultBlock, ToolResultContent, ToolResultStatus, ToolSpec};

/// Name of the MCP router tool.
pub const MCP_ROUTER_TOOL_NAME: &str = "mcp_router";

/// Description of the MCP router tool, before the permitted server names (verbatim from TS).
pub const MCP_ROUTER_DESCRIPTION: &str = "Connects to Model Context Protocol (MCP) servers at runtime to discover and invoke their tools. 'connect' opens a connection to a permitted server. 'list_connections' returns all currently open connection IDs. 'list_tools' returns the tools the connected server exposes, including their names and input schemas. 'call_tool' invokes a named tool on a connected server and returns its result. 'disconnect' closes a connection. Multiple servers can be connected simultaneously. Use connection_id to identify which connection to use for list_tools, call_tool, and disconnect.";

/// Maximum simultaneous open connections when the config omits one (TS default: 10).
pub const DEFAULT_MAX_CONNECTIONS: usize = 10;

/// Configuration of an [`McpRouterTool`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct McpRouterConfig {
    /// Tool name reported to the model.
    pub name: String,
    /// Tool description reported to the model; defaults to [`MCP_ROUTER_DESCRIPTION`] with the
    /// permitted server names appended.
    pub description: String,
    /// Allowlisted MCP servers keyed by a developer-chosen name. Must not be empty.
    pub servers: IndexMap<String, McpServerConfig>,
    /// Maximum simultaneous open connections.
    pub max_connections: usize,
}

impl McpRouterConfig {
    /// Configuration for the given server allowlist, with the TS defaults.
    pub fn new(servers: impl IntoIterator<Item = (String, McpServerConfig)>) -> Self {
        let servers: IndexMap<String, McpServerConfig> = servers.into_iter().collect();
        Self {
            name: MCP_ROUTER_TOOL_NAME.to_owned(),
            description: default_description(&servers),
            servers,
            max_connections: DEFAULT_MAX_CONNECTIONS,
        }
    }
}

/// Operation the `mcp_router` tool should perform.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum McpRouterCommand {
    /// Open a connection to a permitted server.
    Connect,
    /// Return all currently open connection ids.
    ListConnections,
    /// List the tools a connected server exposes.
    ListTools,
    /// Invoke a named tool on a connected server.
    CallTool,
    /// Close a connection.
    Disconnect,
}

/// Input of the MCP router tool (`McpRouterInput` in TS).
#[derive(Clone, Debug, Deserialize)]
pub struct McpRouterInput {
    /// The operation to perform.
    pub command: McpRouterCommand,
    /// Connection name; required for every command except `list_connections`.
    #[serde(default)]
    pub connection_id: Option<String>,
    /// Server to connect to; required for `connect`.
    #[serde(default)]
    pub server_name: Option<String>,
    /// Tool to invoke; required for `call_tool`.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// Arguments passed to the invoked tool; used with `call_tool`.
    #[serde(default)]
    pub arguments: Option<JsonMap<String, JsonValue>>,
}

/// The `mcp_router` tool: runtime MCP connections scoped to this tool instance.
#[derive(Debug)]
pub struct McpRouterTool {
    config: McpRouterConfig,
    spec: ToolSpec,
    connections: Mutex<IndexMap<String, McpClient>>,
}

impl McpRouterTool {
    /// Router over the given server allowlist, with the TS defaults.
    ///
    /// Errors when the allowlist is empty.
    pub fn with_servers(servers: impl IntoIterator<Item = (String, McpServerConfig)>) -> Result<Self> {
        Self::with_config(McpRouterConfig::new(servers))
    }

    /// Router with an explicit configuration.
    ///
    /// Errors when the allowlist is empty or `max_connections` is zero.
    pub fn with_config(config: McpRouterConfig) -> Result<Self> {
        if config.servers.is_empty() {
            return Err(Error::ToolValidation(
                "`servers` must not be empty; the mcp_router tool requires at least one server".to_owned(),
            ));
        }
        if config.max_connections < 1 {
            return Err(Error::ToolValidation(
                "`max_connections` must be at least 1".to_owned(),
            ));
        }
        let spec = mcp_router_tool_spec(&config);
        Ok(Self {
            config,
            spec,
            connections: Mutex::new(IndexMap::new()),
        })
    }

    /// Run one router command, returning the JSON the model sees.
    pub async fn execute(&self, input: &McpRouterInput, cancel: CancellationToken) -> Result<JsonValue> {
        if input.command == McpRouterCommand::ListConnections {
            let connections = self.connections.lock().await;
            let mut ids: Vec<&str> = connections.keys().map(String::as_str).collect();
            ids.sort_unstable();
            return Ok(JsonValue::String(ids.join(", ")));
        }

        let connection_id = input.connection_id.as_deref().ok_or_else(|| {
            Error::ToolValidation(
                "`connection_id` is required for all commands except 'list_connections'".to_owned(),
            )
        })?;

        if input.command == McpRouterCommand::Connect {
            let server_name = input.server_name.as_deref().ok_or_else(|| {
                Error::ToolValidation("`server_name` is required for command='connect'".to_owned())
            })?;
            return self.connect(server_name, connection_id).await;
        }

        let client = self.connections.lock().await.get(connection_id).cloned();
        let client = client.ok_or_else(|| {
            Error::ToolValidation(format!("No active connection for id \"{connection_id}\""))
        })?;

        match input.command {
            McpRouterCommand::ListTools => list_tools(&client).await,
            McpRouterCommand::CallTool => {
                let tool_name = input.tool_name.as_deref().ok_or_else(|| {
                    Error::ToolValidation("`tool_name` is required for command='call_tool'".to_owned())
                })?;
                let arguments = input
                    .arguments
                    .clone()
                    .map_or_else(|| json!({}), JsonValue::Object);
                call_tool(&client, tool_name, arguments, cancel).await
            }
            McpRouterCommand::Disconnect => {
                let client = self.connections.lock().await.shift_remove(connection_id);
                if let Some(client) = client {
                    stop_client(&client).await;
                }
                Ok(JsonValue::String("Successfully disconnected".to_owned()))
            }
            McpRouterCommand::Connect | McpRouterCommand::ListConnections => unreachable!(),
        }
    }

    /// Close every open connection. Call this before dropping the tool: `Drop` cannot await, so
    /// stdio servers otherwise outlive the router.
    pub async fn disconnect_all(&self) {
        let clients: Vec<McpClient> = self.connections.lock().await.drain(..).map(|(_, c)| c).collect();
        for client in &clients {
            stop_client(client).await;
        }
    }

    async fn connect(&self, server_name: &str, connection_id: &str) -> Result<JsonValue> {
        let Some(server_config) = self.config.servers.get(server_name) else {
            return Err(Error::ToolValidation(format!(
                "Server \"{server_name}\" is not on the MCP server allowlist: {}",
                sorted_join(self.config.servers.keys().map(String::as_str))
            )));
        };

        let servers: IndexMap<String, McpServerConfig> = [(server_name.to_owned(), server_config.clone())]
            .into_iter()
            .collect();
        let mut loaded = McpClient::load_servers(servers, LoadOptions::default(), unprefixed()).await?;
        let Some(client) = loaded.pop() else {
            return Err(Error::other(format!(
                "Server \"{server_name}\" failed to initialise; check the server config"
            )));
        };

        // `load_servers` swallows the failure when the server config sets `continue_on_error`.
        if client.connection_state() == McpConnectionState::Failed {
            stop_client(&client).await;
            return Err(Error::other(format!(
                "Server \"{server_name}\" failed to connect; check the server config"
            )));
        }

        // Cap and duplicate checks after all awaits to avoid races.
        let mut connections = self.connections.lock().await;
        if connections.len() >= self.config.max_connections {
            let message = format!(
                "Connection limit of {} reached. Disconnect one of the active connections before \
                 opening a new one. Active connection_ids: {}",
                self.config.max_connections,
                sorted_join(connections.keys().map(String::as_str))
            );
            drop(connections);
            stop_client(&client).await;
            return Err(Error::ToolValidation(message));
        }
        if connections.contains_key(connection_id) {
            let message = format!(
                "Connection \"{connection_id}\" already exists. Disconnect it first or use a \
                 different connection_id."
            );
            drop(connections);
            stop_client(&client).await;
            return Err(Error::ToolValidation(message));
        }
        connections.insert(connection_id.to_owned(), client);

        tracing::debug!(connection_id, server_name, "opened MCP connection");
        Ok(JsonValue::String(format!(
            "Successfully connected to {server_name} as \"{connection_id}\""
        )))
    }
}

#[async_trait]
impl Tool for McpRouterTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        let input: McpRouterInput = ctx.input()?;
        match self.execute(&input, ctx.cancel.clone()).await {
            Ok(value) => Ok(wrap_in_tool_result(value, &ctx.tool_use.tool_use_id)),
            Err(err) => tool_error_or_propagate(err, &ctx.tool_use.tool_use_id),
        }
    }
}

/// Tools the server exposes, under their server-side names so `call_tool` works verbatim.
async fn list_tools(client: &McpClient) -> Result<JsonValue> {
    let tools = client.list_tools_with(unprefixed()).await?;
    let specs = tools
        .iter()
        .map(|tool| serde_json::to_value(tool.spec()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| Error::JsonValidation(err.to_string()))?;
    Ok(JsonValue::Array(specs))
}

/// Invoke `tool_name`, mapping the MCP content exactly as [`crate::mcp::McpTool`] does.
async fn call_tool(
    client: &McpClient,
    tool_name: &str,
    arguments: JsonValue,
    cancel: CancellationToken,
) -> Result<JsonValue> {
    let result = client.call_tool(tool_name, arguments, cancel).await?;
    let block = crate::mcp::map_call_tool_result(result, tool_name);
    if block.status == ToolResultStatus::Error {
        let text = block
            .content
            .iter()
            .map(|content| match content {
                ToolResultContent::Text(text) => text.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let message = if text.is_empty() {
            "MCP tool returned an error".to_owned()
        } else {
            text
        };
        return Err(Error::other(message));
    }
    serde_json::to_value(block.content).map_err(|err| Error::JsonValidation(err.to_string()))
}

/// Disconnect, logging rather than propagating a shutdown failure (TS `stopClient`).
async fn stop_client(client: &McpClient) {
    if let Err(error) = client.disconnect().await {
        tracing::debug!(%error, "failed to stop MCP client");
    }
}

/// List options that keep the server-side tool names.
fn unprefixed() -> ListOptions {
    ListOptions {
        prefix: Some(String::new()),
        ..Default::default()
    }
}

/// [`MCP_ROUTER_DESCRIPTION`] with the allowlisted server names appended (verbatim from TS).
fn default_description(servers: &IndexMap<String, McpServerConfig>) -> String {
    let quoted: Vec<String> = servers.keys().map(|name| format!("'{name}'")).collect();
    let permitted = sorted_join(quoted.iter().map(String::as_str));
    format!("{MCP_ROUTER_DESCRIPTION} Permitted server names: {permitted}.")
}

fn sorted_join<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut values: Vec<&str> = values.collect();
    values.sort_unstable();
    values.join(", ")
}

fn mcp_router_tool_spec(config: &McpRouterConfig) -> ToolSpec {
    ToolSpec {
        name: config.name.clone(),
        description: config.description.clone(),
        input_schema: Some(json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["connect", "list_connections", "list_tools", "call_tool", "disconnect"],
                    "description": "The operation to perform: 'connect', 'list_connections', 'list_tools', 'call_tool', 'disconnect'.",
                },
                "connection_id": {
                    "type": "string",
                    "description": "A descriptive name for this connection. Required for all commands except list_connections. Must be unique per agent. Reusing an active id is rejected.",
                },
                "server_name": {
                    "type": "string",
                    "description": "Server name to connect to. Required for 'connect'.",
                },
                "tool_name": {
                    "type": "string",
                    "description": "Tool name to invoke. Required for 'call_tool'.",
                },
                "arguments": {
                    "type": "object",
                    "additionalProperties": true,
                    "description": "Arguments to pass to the invoked tool. Used with 'call_tool'.",
                },
            },
            "required": ["command"],
        })),
        output_schema: None,
        annotations: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool::test_context;

    fn servers(names: &[&str]) -> IndexMap<String, McpServerConfig> {
        names
            .iter()
            .map(|name| {
                (
                    (*name).to_owned(),
                    McpServerConfig::streamable_http(format!("https://{name}.example.com/mcp")),
                )
            })
            .collect()
    }

    fn router(names: &[&str]) -> McpRouterTool {
        McpRouterTool::with_servers(servers(names)).unwrap()
    }

    async fn run(tool: &McpRouterTool, input: JsonValue) -> Result<JsonValue> {
        let parsed: McpRouterInput = serde_json::from_value(input).unwrap();
        tool.execute(&parsed, CancellationToken::new()).await
    }

    #[test]
    fn rejects_empty_allowlist() {
        let error = McpRouterTool::with_servers(IndexMap::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("must not be empty"), "{error}");
    }

    #[test]
    fn rejects_zero_max_connections() {
        let mut config = McpRouterConfig::new(servers(&["mcp"]));
        config.max_connections = 0;
        let error = McpRouterTool::with_config(config).unwrap_err().to_string();
        assert!(error.contains("max_connections"), "{error}");
    }

    #[test]
    fn description_includes_server_names() {
        let tool = router(&["my-server"]);
        assert!(tool.description().contains("my-server"), "{}", tool.description());
    }

    #[test]
    fn uses_default_description_with_mcp_router_description() {
        let tool = router(&["mcp"]);
        assert!(tool.description().starts_with(MCP_ROUTER_DESCRIPTION));
        assert!(tool.description().ends_with("Permitted server names: 'mcp'."));
    }

    #[test]
    fn accepts_custom_name_and_description() {
        let mut config = McpRouterConfig::new(servers(&["mcp"]));
        config.name = "router".to_owned();
        config.description = "custom".to_owned();
        let tool = McpRouterTool::with_config(config).unwrap();
        assert_eq!(tool.name(), "router");
        assert_eq!(tool.description(), "custom");
    }

    #[test]
    fn exposes_expected_schema_fields() {
        let tool = router(&["mcp"]);
        let schema = tool.spec().input_schema.clone().unwrap();
        let properties = schema["properties"].as_object().unwrap();
        for field in [
            "command",
            "connection_id",
            "server_name",
            "tool_name",
            "arguments",
        ] {
            assert!(properties.contains_key(field), "missing {field}");
        }
        assert_eq!(
            schema["properties"]["command"]["enum"],
            json!([
                "connect",
                "list_connections",
                "list_tools",
                "call_tool",
                "disconnect"
            ])
        );
        assert_eq!(schema["required"], json!(["command"]));
        assert_eq!(tool.spec().name, MCP_ROUTER_TOOL_NAME);
    }

    #[tokio::test]
    async fn rejects_server_not_on_allowlist() {
        let tool = router(&["mcp"]);
        let error = run(
            &tool,
            json!({"command": "connect", "server_name": "evil", "connection_id": "c1"}),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("not on the MCP server allowlist"), "{error}");
    }

    #[tokio::test]
    async fn rejects_missing_connection_id() {
        let tool = router(&["mcp"]);
        let error = run(&tool, json!({"command": "connect", "server_name": "mcp"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("connection_id"), "{error}");
    }

    #[tokio::test]
    async fn rejects_missing_server_name() {
        let tool = router(&["mcp"]);
        let error = run(&tool, json!({"command": "connect", "connection_id": "c1"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("server_name"), "{error}");
    }

    #[tokio::test]
    async fn returns_empty_string_when_no_connections_are_open() {
        let tool = router(&["mcp"]);
        let result = run(&tool, json!({"command": "list_connections"})).await.unwrap();
        assert_eq!(result, json!(""));
    }

    #[tokio::test]
    async fn rejects_commands_for_an_unknown_connection_id() {
        let tool = router(&["mcp"]);
        for command in ["list_tools", "call_tool", "disconnect"] {
            let error = run(
                &tool,
                json!({"command": command, "connection_id": "nope", "tool_name": "t"}),
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains("No active connection for id \"nope\""), "{error}");
        }
    }

    #[tokio::test]
    async fn invoke_returns_an_error_result_for_a_failed_command() {
        let tool = router(&["mcp"]);
        let ctx = test_context(
            MCP_ROUTER_TOOL_NAME,
            "id-1",
            json!({"command": "connect", "server_name": "evil", "connection_id": "c1"}),
        );
        let result = tool.invoke(ctx).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(result.tool_use_id, "id-1");
    }

    #[tokio::test]
    async fn invoke_lists_connections_as_a_success_result() {
        let tool = router(&["mcp"]);
        let ctx = test_context(
            MCP_ROUTER_TOOL_NAME,
            "id-2",
            json!({"command": "list_connections"}),
        );
        let result = tool.invoke(ctx).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
    }
}
