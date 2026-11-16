//! The MCP router tool (TS `vended-tools/mcp-router/`): connects to allowlisted MCP servers at
//! runtime to list and call their tools.
//!
//! ```no_run
//! use strands::mcp::McpServerConfig;
//! use strands::vended_tools::{MakeMcpRouterOptions, make_mcp_router};
//!
//! # fn run() -> strands::Result<()> {
//! let server = McpServerConfig {
//!     url: Some("https://mcp.example.com/mcp".into()),
//!     ..Default::default()
//! };
//! let router = make_mcp_router(MakeMcpRouterOptions {
//!     servers: [("my-api".to_owned(), server)].into_iter().collect(),
//!     ..Default::default()
//! })?;
//! let agent = strands::Agent::builder().tools([router]).build()?;
//! # Ok(())
//! # }
//! ```

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use futures::StreamExt;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde::Deserialize;

use crate::JsonValue;
use crate::error::{Error, Result};
use crate::mcp::{
    McpClient, McpClientOptions, McpConnectionState, McpListToolsOptions, McpLoadServersOptions,
    McpServerConfig, McpTool,
};
use crate::tools::function_tool::InputValidator;
use crate::tools::tool::ToolStreamItem;
use crate::tools::{FunctionTool, Tool, ToolContext, ToolError, ToolOutcome};
use crate::types::json::{JsonObject, to_js_string};
use crate::types::messages::ToolResultContent;
use crate::types::tools::{ToolResultStatus, ToolUse};

use super::{no_precheck, schema_tool};

/// The description of the router tool before the permitted server names (TS
/// `MCP_ROUTER_DESCRIPTION`).
pub const MCP_ROUTER_DESCRIPTION: &str = "Connects to Model Context Protocol (MCP) servers at runtime to discover and invoke their tools. 'connect' opens a connection to a permitted server. 'list_connections' returns all currently open connection IDs. 'list_tools' returns the tools the connected server exposes, including their names and input schemas. 'call_tool' invokes a named tool on a connected server and returns its result. 'disconnect' closes a connection. Multiple servers can be connected simultaneously. Use connection_id to identify which connection to use for list_tools, call_tool, and disconnect.";

/// The default cap on open connections per agent (TS `DEFAULT_MAX_CONNECTIONS`).
const DEFAULT_MAX_CONNECTIONS: usize = 10;

/// A router operation failed (TS `McpRouterToolError`).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct McpRouterToolError {
    message: String,
}

impl McpRouterToolError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "McpRouterToolError";

    /// An error with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Options for [`make_mcp_router`] (TS `MakeMcpRouterOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct MakeMcpRouterOptions {
    /// The tool name (default `mcp_router`).
    pub name: Option<String>,
    /// The description (default [`MCP_ROUTER_DESCRIPTION`] plus the permitted server names).
    pub description: Option<String>,
    /// Allowlisted servers keyed by a developer-chosen name. A server's `tool_filters` decide
    /// which tools `list_tools` returns and `call_tool` accepts. Must not be empty.
    pub servers: IndexMap<String, McpServerConfig>,
    /// Maximum simultaneous open connections per agent (default 10).
    pub max_connections: Option<usize>,
}

/// Builds the clients for one allowlisted server (TS `McpClient.loadServers({ [name]: config })`).
pub(crate) type ServerLoader =
    Arc<dyn Fn(String, McpServerConfig) -> BoxFuture<'static, Result<Vec<McpClient>>> + Send + Sync>;

/// A tool that manages MCP connections to the allowlisted `servers` (TS `makeMcpRouter`).
///
/// Connections are scoped per agent and persist across invocations. They close when the model
/// calls `disconnect`; the connections of a dropped agent close on the router's next call, or
/// when the router is dropped. `call_tool` rejects a tool the connection's `list_tools` doesn't
/// return, so a server's `tool_filters` bound what the model can call.
///
/// # Errors
///
/// `servers` is empty or `max_connections` is zero.
pub fn make_mcp_router(options: MakeMcpRouterOptions) -> Result<FunctionTool> {
    let loader: ServerLoader = Arc::new(|name, config| {
        Box::pin(async move {
            McpClient::load_servers(
                IndexMap::from([(name, config)]),
                McpClientOptions::default(),
                McpLoadServersOptions::default(),
            )
            .await
        })
    });
    make_mcp_router_with(options, loader)
}

/// [`make_mcp_router`] building clients with `loader`.
pub(crate) fn make_mcp_router_with(
    options: MakeMcpRouterOptions,
    loader: ServerLoader,
) -> Result<FunctionTool> {
    if options.servers.is_empty() {
        return Err(Error::other(
            "`servers` must not be empty; the mcp_router tool requires at least one server",
        ));
    }
    let max_connections = options.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS);
    if max_connections < 1 {
        return Err(Error::other("`maxConnections` must be at least 1"));
    }
    let mut names: Vec<&str> = options.servers.keys().map(String::as_str).collect();
    names.sort_unstable();
    let description = options.description.unwrap_or_else(|| {
        let quoted: Vec<String> = names.iter().map(|name| format!("'{name}'")).collect();
        format!(
            "{MCP_ROUTER_DESCRIPTION} Permitted server names: {}.",
            quoted.join(", ")
        )
    });
    let router = Arc::new(Router {
        permitted: names.join(", "),
        servers: options.servers,
        max_connections,
        loader,
        agents: Mutex::new(Vec::new()),
    });
    let schema = mcp_router_input_schema();
    Ok(schema_tool(
        options.name.unwrap_or_else(|| "mcp_router".to_owned()),
        description,
        schema.clone(),
        InputValidator::new(&schema),
        no_precheck,
        move |input: McpRouterInput, context: ToolContext| {
            let router = Arc::clone(&router);
            async move { router.call(input, &context).await }
        },
    ))
}

fn mcp_router_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "enum": ["connect", "list_connections", "list_tools", "call_tool", "disconnect"],
                "description": "The operation to perform: 'connect', 'list_connections', 'list_tools', 'call_tool', 'disconnect'.",
            },
            "connection_id": {
                "description": "A descriptive name for this connection. Required for all commands except list_connections. Must be unique per agent. Reusing an active id is rejected.",
                "type": "string",
            },
            "server_name": {
                "description": "Server name to connect to. Required for 'connect'.",
                "type": "string",
            },
            "tool_name": {
                "description": "Tool name to invoke. Required for 'call_tool'.",
                "type": "string",
            },
            "arguments": {
                "description": "Arguments to pass to the invoked tool. Used with 'call_tool'.",
                "type": "object",
                "propertyNames": { "type": "string" },
                "additionalProperties": {},
            },
        },
        "required": ["command"],
        "additionalProperties": false,
    })
}

#[derive(Deserialize)]
struct McpRouterInput {
    command: Command,
    connection_id: Option<String>,
    server_name: Option<String>,
    tool_name: Option<String>,
    arguments: Option<JsonObject>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Command {
    Connect,
    ListConnections,
    ListTools,
    CallTool,
    Disconnect,
}

/// One open connection and the tool names its last filtered listing returned. Filters are fixed
/// per client, so a listed name stays callable.
#[derive(Clone)]
struct Connection {
    client: McpClient,
    listed: Arc<Mutex<HashSet<String>>>,
}

type Connections = BTreeMap<String, Connection>;

struct Router {
    servers: IndexMap<String, McpServerConfig>,
    permitted: String,
    max_connections: usize,
    loader: ServerLoader,
    /// Connections keyed by agent (TS keeps them in a `WeakMap` keyed by the agent).
    agents: Mutex<Vec<(Weak<()>, Connections)>>,
}

impl Router {
    async fn call(&self, input: McpRouterInput, context: &ToolContext) -> Result<JsonValue, ToolError> {
        self.close_dropped_agents().await;
        let owner = context.agent().agent_sandbox().owner();
        match input.command {
            Command::ListConnections => {
                let ids: Vec<String> =
                    self.with_connections(owner, |connections| connections.keys().cloned().collect());
                Ok(ids.join(", ").into())
            }
            Command::Connect => {
                let connection_id = required_connection_id(input.connection_id)?;
                let Some(server_name) = input.server_name.filter(|name| !name.is_empty()) else {
                    return Err(router_error("`server_name` is required for command='connect'"));
                };
                Ok(self.connect(owner, &server_name, connection_id).await?.into())
            }
            Command::ListTools => {
                let (_, connection) = self.active(owner, input.connection_id)?;
                Ok(JsonValue::Array(list_and_cache(&connection).await?))
            }
            Command::CallTool => {
                let (connection_id, connection) = self.active(owner, input.connection_id)?;
                let Some(tool_name) = input.tool_name.filter(|name| !name.is_empty()) else {
                    return Err(router_error("`tool_name` is required for command='call_tool'"));
                };
                ensure_tool_listed(&connection, &connection_id, &tool_name).await?;
                let arguments = JsonValue::Object(input.arguments.unwrap_or_default());
                call_tool(&connection.client, tool_name, arguments, context).await
            }
            Command::Disconnect => {
                let (connection_id, connection) = self.active(owner, input.connection_id)?;
                self.with_connections(owner, |connections| connections.remove(&connection_id));
                stop_client(&connection.client).await;
                Ok("Successfully disconnected".into())
            }
        }
    }

    /// The open connection named by `connection_id`.
    fn active(
        &self,
        owner: &Arc<()>,
        connection_id: Option<String>,
    ) -> Result<(String, Connection), ToolError> {
        let connection_id = required_connection_id(connection_id)?;
        match self.with_connections(owner, |connections| connections.get(&connection_id).cloned()) {
            Some(connection) => Ok((connection_id, connection)),
            None => Err(router_error(format!(
                "No active connection for id {}",
                quoted(&connection_id)
            ))),
        }
    }

    async fn connect(
        &self,
        owner: &Arc<()>,
        server_name: &str,
        connection_id: String,
    ) -> Result<String, ToolError> {
        let Some(config) = self.servers.get(server_name) else {
            return Err(router_error(format!(
                "Server {} is not on the MCP server allowlist: {}",
                quoted(server_name),
                self.permitted
            )));
        };
        let loaded = (self.loader)(server_name.to_owned(), config.clone()).await?;
        let Some(client) = loaded.into_iter().next() else {
            return Err(router_error(format!(
                "Server {} failed to initialise; check the server config",
                quoted(server_name)
            )));
        };
        if let Err(error) = client.connect(false).await {
            stop_client(&client).await;
            return Err(error.into());
        }
        // `connect` swallows failures when the server config sets `continueOnError`.
        if client.connection_state() == McpConnectionState::Failed {
            stop_client(&client).await;
            return Err(router_error(format!(
                "Server {} failed to connect; check the server config",
                quoted(server_name)
            )));
        }
        // Cap and duplicate checks after all awaits, so concurrent connects can't overshoot.
        let rejection = self.with_connections(owner, |connections| {
            if connections.len() >= self.max_connections {
                let active: Vec<&str> = connections.keys().map(String::as_str).collect();
                return Some(format!(
                    "Connection limit of {} reached. Disconnect one of the active connections before opening a new one. Active connection_ids: {}",
                    self.max_connections,
                    active.join(", ")
                ));
            }
            if connections.contains_key(&connection_id) {
                return Some(format!(
                    "Connection {} already exists. Disconnect it first or use a different connection_id.",
                    quoted(&connection_id)
                ));
            }
            let connection = Connection {
                client: client.clone(),
                listed: Arc::default(),
            };
            connections.insert(connection_id.clone(), connection);
            None
        });
        if let Some(message) = rejection {
            stop_client(&client).await;
            return Err(router_error(message));
        }
        tracing::debug!(
            "connection_id=<{connection_id}>, server_name=<{server_name}> | opened MCP connection"
        );
        Ok(format!(
            "Successfully connected to {server_name} as {}",
            quoted(&connection_id)
        ))
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(Weak<()>, Connections)>> {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` on `owner`'s connections; an agent's entry is kept once it has a connection.
    fn with_connections<R>(&self, owner: &Arc<()>, f: impl FnOnce(&mut Connections) -> R) -> R {
        let mut agents = self.lock();
        if let Some((_, connections)) = agents
            .iter_mut()
            .find(|(agent, _)| agent.as_ptr() == Arc::as_ptr(owner))
        {
            return f(connections);
        }
        let mut connections = Connections::new();
        let result = f(&mut connections);
        if !connections.is_empty() {
            agents.push((Arc::downgrade(owner), connections));
        }
        result
    }

    /// Closes the connections of agents that were dropped without disconnecting.
    async fn close_dropped_agents(&self) {
        let dropped: Vec<Connections> = {
            let mut agents = self.lock();
            let (dropped, live): (Vec<_>, Vec<_>) =
                agents.drain(..).partition(|(agent, _)| agent.strong_count() == 0);
            *agents = live;
            dropped.into_iter().map(|(_, connections)| connections).collect()
        };
        for (connection_id, connection) in dropped.into_iter().flatten() {
            tracing::debug!("connection_id=<{connection_id}> | closing MCP connection of a dropped agent");
            stop_client(&connection.client).await;
        }
    }
}

fn required_connection_id(connection_id: Option<String>) -> Result<String, ToolError> {
    connection_id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| router_error("`connection_id` is required for all commands except 'list_connections'"))
}

fn router_error(message: impl Into<String>) -> ToolError {
    McpRouterToolError::new(message).into()
}

/// `JSON.stringify` of a string.
fn quoted(value: &str) -> String {
    to_js_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

async fn stop_client(client: &McpClient) {
    if let Err(error) = client.disconnect().await {
        tracing::debug!("error=<{error}> | failed to stop MCP client");
    }
}

/// The filtered listing under server-side names, so `call_tool` takes them verbatim.
async fn list_and_cache(connection: &Connection) -> Result<Vec<JsonValue>, ToolError> {
    let options = McpListToolsOptions {
        prefix: Some(String::new()),
        tool_filters: None,
    };
    let tools = connection.client.list_tools_with(options).await?;
    let mut specs = Vec::with_capacity(tools.len());
    let mut names = HashSet::with_capacity(tools.len());
    for tool in tools {
        let mut spec = serde_json::to_value(tool.tool_spec())?;
        if let Some(spec) = spec.as_object_mut() {
            spec.insert("name".to_owned(), tool.server_name().into());
        }
        names.insert(tool.server_name().to_owned());
        specs.push(spec);
    }
    *connection.listed.lock().unwrap_or_else(PoisonError::into_inner) = names;
    Ok(specs)
}

/// `McpClient` applies tool filters only when listing, so a call is checked against the
/// filtered listing, refreshed when the name isn't in it yet.
async fn ensure_tool_listed(
    connection: &Connection,
    connection_id: &str,
    tool_name: &str,
) -> Result<(), ToolError> {
    let listed = |connection: &Connection| {
        connection
            .listed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(tool_name)
    };
    if listed(connection) {
        return Ok(());
    }
    list_and_cache(connection).await?;
    if listed(connection) {
        return Ok(());
    }
    Err(router_error(format!(
        "Tool {} is not available on connection {}. Call list_tools to see the tools you can call.",
        quoted(tool_name),
        quoted(connection_id)
    )))
}

/// Calls the tool through an ad-hoc [`McpTool`], which maps the content to SDK blocks and errors.
async fn call_tool(
    client: &McpClient,
    tool_name: String,
    arguments: JsonValue,
    context: &ToolContext,
) -> Result<JsonValue, ToolError> {
    let schema = Arc::new(JsonObject::from_iter([("type".to_owned(), "object".into())]));
    let server_tool = rmcp::model::Tool::new(tool_name.clone(), "", schema);
    let tool = McpTool::from_server_tool(tool_name.clone(), &server_tool, client.clone());
    let tool_use = ToolUse::new(tool_name, context.tool_use().tool_use_id(), arguments);
    let call_context = ToolContext::new(
        tool_use,
        context.agent().clone(),
        context.invocation_state().clone(),
    );
    let outcome = tool.stream(call_context).collect::<Vec<_>>().await.pop();
    let Some(ToolStreamItem::Outcome(ToolOutcome::Result(result))) = outcome else {
        return Err(router_error("MCP tool returned an error"));
    };
    if *result.status() == ToolResultStatus::Error {
        let text: Vec<String> = result.content().iter().map(content_text).collect();
        let text = text.join("\n");
        return Err(router_error(if text.is_empty() {
            "MCP tool returned an error".to_owned()
        } else {
            text
        }));
    }
    Ok(serde_json::to_value(result.content())?)
}

fn content_text(content: &ToolResultContent) -> String {
    match content {
        ToolResultContent::Text(block) => block.text().to_owned(),
        other => to_js_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests;
