//! `McpClient::load_servers` (TS `mcp/config.ts`, `mcp/config.node.ts`): MCP clients from a
//! JSON config file or map in the common `mcpServers` format.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::client::{McpClient, McpClientConfig, McpClientCredentials, McpClientOptions};
use super::filters::{McpToolFilters, McpToolMatcher};
use super::transport::{EnvLookup, McpTransport, StdioServerParameters, process_env};
use crate::error::{Error, Result};
use crate::types::json::JsonValue;

/// Tool filters in a config file: each pattern is a regex matched from the start of the
/// server-side tool name (TS `SerializableMcpToolFilters`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializableMcpToolFilters {
    /// When present, only tools matching one of these patterns are exposed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<String>>,
    /// Tools matching one of these patterns are excluded, even when also allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected: Option<Vec<String>>,
}

/// Client credentials in a config file (TS `McpClientCredentials`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerCredentials {
    /// The client id.
    pub client_id: String,
    /// The client secret.
    pub client_secret: String,
    /// Scopes to request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
}

impl std::fmt::Debug for McpServerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerCredentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// One server entry (TS `McpServerConfig`). String fields support `${VAR}` and `${env:VAR}`.
///
/// Give `command` (stdio) or `url` (Streamable HTTP, or SSE with `transport: "sse"`); without
/// `transport` it is detected from which is present.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Command to spawn (stdio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments for the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Variables for the child, merged over the default environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<IndexMap<String, String>>,
    /// Working directory for the child.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Server url (Streamable HTTP or SSE).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers sent with every request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<IndexMap<String, String>>,
    /// `stdio`, `sse` or `streamable-http`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// Client credentials (Streamable HTTP only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<McpServerCredentials>,
    /// Prefix for agent-facing tool names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Filters on server-side tool names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_filters: Option<SerializableMcpToolFilters>,
    /// Skip this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// Skip this server's config and connection failures with warnings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_on_error: Option<bool>,
    /// MCP task settings (TS `tasksConfig`); MCP tasks are not supported in Rust yet, so a
    /// present value fails this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks_config: Option<JsonValue>,
}

/// How [`McpClient::load_servers`] translates entries (TS `McpLoadServersOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct McpLoadServersOptions {
    /// Servers without an explicit `prefix` use their config key (characters outside
    /// `[A-Za-z0-9_-]` replaced with `_`) as the tool-name prefix, over a default prefix.
    pub prefix_with_server_name: bool,
}

/// What [`McpClient::load_servers`] reads: a JSON file (flat, or wrapped in `mcpServers`; a
/// leading `~/` is the home directory) or a flat map.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum McpServers {
    /// A JSON config file.
    Path(PathBuf),
    /// Server entries by name.
    Map(IndexMap<String, McpServerConfig>),
    /// Server entries by name as JSON (a flat object).
    Json(JsonValue),
}

impl From<&str> for McpServers {
    fn from(path: &str) -> Self {
        Self::Path(PathBuf::from(path))
    }
}

impl From<String> for McpServers {
    fn from(path: String) -> Self {
        Self::Path(PathBuf::from(path))
    }
}

impl From<PathBuf> for McpServers {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<&Path> for McpServers {
    fn from(path: &Path) -> Self {
        Self::Path(path.to_path_buf())
    }
}

impl From<IndexMap<String, McpServerConfig>> for McpServers {
    fn from(servers: IndexMap<String, McpServerConfig>) -> Self {
        Self::Map(servers)
    }
}

impl From<JsonValue> for McpServers {
    fn from(servers: JsonValue) -> Self {
        Self::Json(servers)
    }
}

/// A server entry resolved into the parts of an [`McpClientConfig`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResolvedTransport {
    Stdio(StdioServerParameters),
    Http {
        url: String,
        headers: Option<IndexMap<String, String>>,
        auth: Option<McpClientCredentials>,
    },
    Sse {
        url: String,
        headers: Option<IndexMap<String, String>>,
    },
}

pub(crate) struct ResolvedServer {
    pub(crate) transport: ResolvedTransport,
    pub(crate) options: McpClientOptions,
}

const CONFIG_SHAPE: &str = "MCP config must be a JSON object mapping server names to configs, e.g. { \"my-server\": { \"command\": \"node\" } }";

impl McpClient {
    /// Clients for every enabled server in `servers` (TS `McpClient.loadServers`). Nothing
    /// connects yet.
    ///
    /// `defaults` apply to every client unless an entry overrides them; the handshake name
    /// defaults to the entry's key.
    ///
    /// # Errors
    ///
    /// The file can't be read or parsed, the config isn't an object of objects, or an entry
    /// is invalid (missing fields, an unset variable, a bad regex) without `continue_on_error`.
    pub async fn load_servers(
        servers: impl Into<McpServers>,
        defaults: McpClientOptions,
        options: McpLoadServersOptions,
    ) -> Result<Vec<Self>> {
        let lookup = process_env();
        let servers = load_servers_object(servers.into(), &lookup).await?;
        let resolved = resolve_server_configs(servers, &defaults, options, &lookup)?;
        let mut clients = Vec::new();
        for server in resolved {
            let continue_on_error = server.options.continue_on_error;
            let name = server.options.application_name.clone().unwrap_or_default();
            match client_config(server, &lookup).and_then(Self::new) {
                Ok(client) => clients.push(client),
                Err(error) if continue_on_error => tracing::warn!(
                    "server=<{name}>, error=<{error}> | MCP client config failed, skipping (continueOnError)"
                ),
                Err(error) => return Err(error),
            }
        }
        Ok(clients)
    }
}

pub(crate) fn client_config(server: ResolvedServer, lookup: &EnvLookup) -> Result<McpClientConfig> {
    let mut config = McpClientConfig {
        options: server.options,
        ..Default::default()
    };
    match server.transport {
        ResolvedTransport::Stdio(parameters) => {
            config.transport = Some(McpTransport::stdio_with_env(parameters, lookup));
        }
        ResolvedTransport::Http { url, headers, auth } => {
            config.url = Some(url);
            config.headers = headers;
            config.auth = auth;
        }
        ResolvedTransport::Sse { url, headers } => {
            config.transport = Some(McpTransport::sse(&url, headers.unwrap_or_default())?);
        }
    }
    Ok(config)
}

pub(crate) async fn load_servers_object(
    servers: McpServers,
    lookup: &EnvLookup,
) -> Result<IndexMap<String, JsonValue>> {
    let value = match servers {
        McpServers::Map(map) => {
            return Ok(map
                .into_iter()
                .map(|(name, server)| (name, serde_json::to_value(server).unwrap_or_default()))
                .collect());
        }
        McpServers::Json(value) => value,
        McpServers::Path(path) => {
            let path = expand_home(&path, lookup);
            let text = tokio::fs::read_to_string(&path).await?;
            let mut parsed: JsonValue = serde_json::from_str(&text)?;
            match parsed.get_mut("mcpServers").map(JsonValue::take) {
                Some(servers) if !servers.is_null() => servers,
                _ => parsed,
            }
        }
    };
    match value {
        JsonValue::Object(map) => Ok(map.into_iter().collect()),
        _ => Err(Error::other(CONFIG_SHAPE)),
    }
}

fn expand_home(path: &Path, lookup: &EnvLookup) -> PathBuf {
    let home = lookup("HOME").or_else(|| lookup("USERPROFILE"));
    match (path.to_str().and_then(|path| path.strip_prefix("~/")), home) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => path.to_path_buf(),
    }
}

/// TS `resolveServerConfigs`: one resolved server per enabled entry; per-entry failures are
/// skipped with a warning under `continueOnError`, a non-object entry always fails.
pub(crate) fn resolve_server_configs(
    servers: IndexMap<String, JsonValue>,
    defaults: &McpClientOptions,
    options: McpLoadServersOptions,
    lookup: &EnvLookup,
) -> Result<Vec<ResolvedServer>> {
    let mut resolved = Vec::new();
    for (name, entry) in servers {
        let kind = match &entry {
            JsonValue::Object(_) => None,
            JsonValue::Array(_) => Some("array"),
            JsonValue::String(_) => Some("string"),
            JsonValue::Number(_) => Some("number"),
            JsonValue::Bool(_) => Some("boolean"),
            JsonValue::Null => Some("object"),
        };
        if let Some(kind) = kind {
            return Err(Error::other(format!(
                "Server \"{name}\" must be an object, got {kind}"
            )));
        }
        let continue_on_error = entry
            .get("continueOnError")
            .and_then(JsonValue::as_bool)
            .unwrap_or(defaults.continue_on_error);
        if entry.get("disabled").and_then(JsonValue::as_bool) == Some(true) {
            continue;
        }
        match resolve_server(&name, entry, defaults, options, lookup) {
            Ok(server) => resolved.push(server),
            Err(error) if continue_on_error => {
                tracing::warn!(
                    "server=<{name}>, error=<{error}> | MCP server config failed, skipping (continueOnError)"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(resolved)
}

fn resolve_server(
    name: &str,
    entry: JsonValue,
    defaults: &McpClientOptions,
    options: McpLoadServersOptions,
    lookup: &EnvLookup,
) -> Result<ResolvedServer> {
    let server: McpServerConfig = serde_json::from_value(entry)?;
    if server.tasks_config.is_some() {
        return Err(Error::other(
            "MCP tasks (\"tasksConfig\") are not supported in Rust yet",
        ));
    }
    if server.command.is_some() && server.url.is_some() && server.transport.is_none() {
        return Err(Error::other(
            "Server config has both \"command\" and \"url\" — set \"transport\" explicitly or remove one",
        ));
    }
    let kind = match (&server.transport, &server.command, &server.url) {
        (Some(kind), _, _) => kind.as_str(),
        (None, Some(_), _) => "stdio",
        (None, None, Some(_)) => "streamable-http",
        (None, None, None) => {
            return Err(Error::other(
                "Server config must include either \"command\" (stdio) or \"url\" (http)",
            ));
        }
    };
    let interpolate = |value: &str| interpolate_env(value, lookup);
    let transport = match kind {
        "stdio" => stdio(&server, &interpolate, lookup)?,
        "streamable-http" => http(&server, &interpolate)?,
        "sse" => sse(&server, &interpolate)?,
        other => return Err(Error::other(format!("Unsupported transport type: {other}"))),
    };
    Ok(ResolvedServer {
        transport,
        options: base_options(name, &server, defaults, options, &interpolate)?,
    })
}

type Interpolate<'a> = dyn Fn(&str) -> Result<String> + 'a;

fn map_values(
    values: &IndexMap<String, String>,
    interpolate: &Interpolate<'_>,
) -> Result<IndexMap<String, String>> {
    values
        .iter()
        .map(|(key, value)| Ok((key.clone(), interpolate(value)?)))
        .collect()
}

fn stdio(
    server: &McpServerConfig,
    interpolate: &Interpolate<'_>,
    lookup: &EnvLookup,
) -> Result<ResolvedTransport> {
    let Some(command) = &server.command else {
        return Err(Error::other("Stdio transport requires \"command\" field"));
    };
    let args = match &server.args {
        Some(args) => args.iter().map(|arg| interpolate(arg)).collect::<Result<_>>()?,
        None => Vec::new(),
    };
    let env = match &server.env {
        Some(env) => {
            let mut merged = super::transport::default_environment(lookup);
            merged.extend(map_values(env, interpolate)?);
            Some(merged)
        }
        None => None,
    };
    let cwd = server
        .cwd
        .as_deref()
        .map(interpolate)
        .transpose()?
        .map(PathBuf::from);
    Ok(ResolvedTransport::Stdio(StdioServerParameters {
        command: interpolate(command)?,
        args,
        env,
        cwd,
    }))
}

fn http(server: &McpServerConfig, interpolate: &Interpolate<'_>) -> Result<ResolvedTransport> {
    let Some(url) = &server.url else {
        return Err(Error::other("Streamable HTTP transport requires \"url\" field"));
    };
    let headers = server
        .headers
        .as_ref()
        .map(|headers| map_values(headers, interpolate))
        .transpose()?;
    let auth = match &server.auth {
        Some(auth) => Some(McpClientCredentials {
            client_id: interpolate(&auth.client_id)?,
            client_secret: interpolate(&auth.client_secret)?,
            scopes: auth
                .scopes
                .as_ref()
                .map(|scopes| {
                    scopes
                        .iter()
                        .map(|scope| interpolate(scope))
                        .collect::<Result<_>>()
                })
                .transpose()?,
        }),
        None => None,
    };
    Ok(ResolvedTransport::Http {
        url: interpolate(url)?,
        headers,
        auth,
    })
}

fn sse(server: &McpServerConfig, interpolate: &Interpolate<'_>) -> Result<ResolvedTransport> {
    let Some(url) = &server.url else {
        return Err(Error::other("SSE transport requires \"url\" field"));
    };
    if server.auth.is_some() {
        return Err(Error::other(
            "SSE transport does not support auth — use streamable-http or provide a pre-configured transport",
        ));
    }
    let headers = server
        .headers
        .as_ref()
        .map(|headers| map_values(headers, interpolate))
        .transpose()?;
    Ok(ResolvedTransport::Sse {
        url: interpolate(url)?,
        headers,
    })
}

/// TS `baseOptions`: defaults, then the server's key as the handshake name, then the entry's
/// overrides.
fn base_options(
    name: &str,
    server: &McpServerConfig,
    defaults: &McpClientOptions,
    options: McpLoadServersOptions,
    interpolate: &Interpolate<'_>,
) -> Result<McpClientOptions> {
    let mut resolved = defaults.clone();
    resolved.application_name = Some(
        defaults
            .application_name
            .clone()
            .unwrap_or_else(|| name.to_owned()),
    );
    if options.prefix_with_server_name {
        resolved.prefix = Some(name.chars().map(sanitize).collect());
    }
    if let Some(continue_on_error) = server.continue_on_error {
        resolved.continue_on_error = continue_on_error;
    }
    if let Some(prefix) = &server.prefix {
        resolved.prefix = Some(interpolate(prefix)?);
    }
    if let Some(filters) = &server.tool_filters {
        resolved.tool_filters = Some(compile_tool_filters(name, filters, interpolate)?);
    }
    Ok(resolved)
}

fn sanitize(c: char) -> char {
    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
        c
    } else {
        '_'
    }
}

fn compile_tool_filters(
    name: &str,
    filters: &SerializableMcpToolFilters,
    interpolate: &Interpolate<'_>,
) -> Result<McpToolFilters> {
    let compile = |key: &str, patterns: &Option<Vec<String>>| -> Result<Option<Vec<McpToolMatcher>>> {
        let Some(patterns) = patterns else { return Ok(None) };
        patterns
            .iter()
            .map(|pattern| {
                let pattern = interpolate(pattern)?;
                Regex::new(&pattern).map(McpToolMatcher::Pattern).map_err(|error| {
                    Error::other(format!(
                        "Server \"{name}\": invalid regex in toolFilters.{key}: \"{pattern}\": SyntaxError: {error}"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    };
    Ok(McpToolFilters {
        allowed: compile("allowed", &filters.allowed)?,
        rejected: compile("rejected", &filters.rejected)?,
    })
}

static VARIABLE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\$\{(?:env:)?([A-Za-z_][A-Za-z0-9_]*)\}").ok());

/// Replaces `${VAR}` and `${env:VAR}` with the variable's value (TS `interpolateEnv`).
pub(crate) fn interpolate_env(value: &str, lookup: &EnvLookup) -> Result<String> {
    let Some(variable) = VARIABLE.as_ref() else {
        return Ok(value.to_owned());
    };
    let mut out = String::new();
    let mut last = 0;
    for captures in variable.captures_iter(value) {
        let (Some(whole), Some(key)) = (captures.get(0), captures.get(1)) else {
            continue;
        };
        let resolved = lookup(key.as_str())
            .ok_or_else(|| Error::other(format!("Environment variable \"{}\" is not set", key.as_str())))?;
        out.push_str(value.get(last..whole.start()).unwrap_or_default());
        out.push_str(&resolved);
        last = whole.end();
    }
    out.push_str(value.get(last..).unwrap_or_default());
    Ok(out)
}

/// An env lookup over a fixed map, for tests.
#[cfg(test)]
pub(crate) fn fixed_env(pairs: &[(&str, &str)]) -> EnvLookup {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    std::sync::Arc::new(move |key| map.get(key).cloned())
}
