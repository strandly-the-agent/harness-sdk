//! MCP server configuration and the `mcp.json` config-file loader
//! (`strands-ts/src/mcp/config.ts`, `config.node.ts`).

use std::collections::BTreeMap;
use std::path::Path;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::types::JsonValue;

/// Transport to use for a server; absent means "auto-detect from `command`/`url`".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransportKind {
    /// Spawn `command` and talk MCP over its stdio.
    Stdio,
    /// Legacy HTTP+SSE transport. Not supported by this SDK.
    Sse,
    /// Streamable HTTP transport.
    StreamableHttp,
}

/// Tool allow/deny lists; patterns are regexes matched from the start of the server-side name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolFilters {
    /// A tool is kept only when it matches one of these (empty = keep everything).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
    /// A tool matching any of these is dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<String>,
}

impl McpToolFilters {
    /// `true` when neither list has a pattern.
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty() && self.rejected.is_empty()
    }
}

/// One entry of an `mcp.json` `mcpServers` map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// stdio: executable to spawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// stdio: arguments for `command`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// stdio: extra environment variables for the child.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// stdio: working directory for the child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// HTTP: endpoint URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP: extra request headers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Explicit transport; auto-detected from `command`/`url` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<McpTransportKind>,
    /// Prefix for agent-facing tool names (`<prefix>_<tool>`); `Some("")` opts out of defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Tool allow/deny lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_filters: Option<McpToolFilters>,
    /// Skip this server entirely.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// Swallow connect/list failures for this server instead of propagating them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_on_error: Option<bool>,
}

impl McpServerConfig {
    /// A stdio server config.
    pub fn stdio(command: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            command: Some(command.into()),
            args: args.into_iter().map(Into::into).collect(),
            transport: Some(McpTransportKind::Stdio),
            ..Self::default()
        }
    }

    /// A streamable-HTTP server config.
    pub fn streamable_http(url: impl Into<String>) -> Self {
        Self {
            url: Some(url.into()),
            transport: Some(McpTransportKind::StreamableHttp),
            ..Self::default()
        }
    }

    /// Validate the transport fields and pick the transport to build.
    ///
    /// Errors are the TS `config.node.ts` messages verbatim, plus a Rust-only rejection of
    /// `transport: "sse"` (rmcp 3 has no SSE client transport).
    pub fn resolve(&self) -> Result<ResolvedTransport> {
        let kind = match self.transport {
            Some(kind) => kind,
            None => match (self.command.is_some(), self.url.is_some()) {
                (true, true) => {
                    return Err(config_error(
                        "Server config has both \"command\" and \"url\" — set \"transport\" explicitly or remove one",
                    ))
                }
                (true, false) => McpTransportKind::Stdio,
                (false, true) => McpTransportKind::StreamableHttp,
                (false, false) => {
                    return Err(config_error(
                        "Server config must include either \"command\" (stdio) or \"url\" (http)",
                    ))
                }
            },
        };
        match kind {
            McpTransportKind::Stdio => {
                let command = self
                    .command
                    .clone()
                    .ok_or_else(|| config_error("Stdio transport requires \"command\" field"))?;
                Ok(ResolvedTransport::Stdio {
                    command,
                    args: self.args.clone(),
                    env: self.env.clone(),
                    cwd: self.cwd.clone(),
                })
            }
            McpTransportKind::StreamableHttp => {
                let url = self
                    .url
                    .clone()
                    .ok_or_else(|| config_error("Streamable HTTP transport requires \"url\" field"))?;
                Ok(ResolvedTransport::StreamableHttp {
                    url,
                    headers: self.headers.clone(),
                })
            }
            McpTransportKind::Sse => Err(config_error(
                "SSE transport is not supported: rmcp 3 has no SSE client transport — use \"streamable-http\"",
            )),
        }
    }

    /// Replace `${VAR}` / `${env:VAR}` in every string field from the process environment.
    pub fn interpolate_env(&self) -> Result<Self> {
        let mut out = self.clone();
        out.command = out.command.as_deref().map(interpolate).transpose()?;
        out.cwd = out.cwd.as_deref().map(interpolate).transpose()?;
        out.url = out.url.as_deref().map(interpolate).transpose()?;
        out.prefix = out.prefix.as_deref().map(interpolate).transpose()?;
        out.args = out.args.iter().map(|a| interpolate(a)).collect::<Result<_>>()?;
        out.env = interpolate_map(&out.env)?;
        out.headers = interpolate_map(&out.headers)?;
        if let Some(filters) = &mut out.tool_filters {
            filters.allowed = filters
                .allowed
                .iter()
                .map(|p| interpolate(p))
                .collect::<Result<_>>()?;
            filters.rejected = filters
                .rejected
                .iter()
                .map(|p| interpolate(p))
                .collect::<Result<_>>()?;
        }
        Ok(out)
    }
}

/// The transport a validated [`McpServerConfig`] asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResolvedTransport {
    /// Spawn a child process and speak MCP over its stdio.
    Stdio {
        /// Executable.
        command: String,
        /// Arguments.
        args: Vec<String>,
        /// Extra environment variables.
        env: BTreeMap<String, String>,
        /// Working directory.
        cwd: Option<String>,
    },
    /// Streamable HTTP endpoint.
    StreamableHttp {
        /// Endpoint URL.
        url: String,
        /// Extra request headers.
        headers: BTreeMap<String, String>,
    },
}

/// An `mcp.json`-shaped config file: `{ "mcpServers": { "<name>": { ... } } }`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigFile {
    /// Server configs keyed by name.
    #[serde(default)]
    pub mcp_servers: IndexMap<String, McpServerConfig>,
}

/// Where [`crate::mcp::McpClient::load_servers`] reads server configs from.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ConfigSource {
    /// Path to a JSON config file.
    Path(std::path::PathBuf),
    /// Already-parsed server map.
    Inline(IndexMap<String, McpServerConfig>),
    /// Raw JSON: either an `mcp.json` object or a bare server map.
    Json(JsonValue),
}

impl From<&str> for ConfigSource {
    fn from(path: &str) -> Self {
        ConfigSource::Path(path.into())
    }
}

impl From<String> for ConfigSource {
    fn from(path: String) -> Self {
        ConfigSource::Path(path.into())
    }
}

impl From<&Path> for ConfigSource {
    fn from(path: &Path) -> Self {
        ConfigSource::Path(path.to_path_buf())
    }
}

impl From<std::path::PathBuf> for ConfigSource {
    fn from(path: std::path::PathBuf) -> Self {
        ConfigSource::Path(path)
    }
}

impl From<IndexMap<String, McpServerConfig>> for ConfigSource {
    fn from(servers: IndexMap<String, McpServerConfig>) -> Self {
        ConfigSource::Inline(servers)
    }
}

impl From<JsonValue> for ConfigSource {
    fn from(json: JsonValue) -> Self {
        ConfigSource::Json(json)
    }
}

impl ConfigSource {
    /// Read and parse the source into a name → config map.
    pub async fn load(&self) -> Result<IndexMap<String, McpServerConfig>> {
        match self {
            ConfigSource::Inline(servers) => Ok(servers.clone()),
            ConfigSource::Json(json) => parse_servers(json.clone()),
            ConfigSource::Path(path) => {
                let text = tokio::fs::read_to_string(path).await.map_err(|e| {
                    config_error(format!("Failed to read MCP config {}: {e}", path.display()))
                })?;
                let json: JsonValue = serde_json::from_str(&text).map_err(|e| {
                    config_error(format!("Failed to parse MCP config {}: {e}", path.display()))
                })?;
                parse_servers(json)
            }
        }
    }
}

/// Accept either `{ "mcpServers": {...} }` or a bare `{ "<name>": {...} }` map.
fn parse_servers(json: JsonValue) -> Result<IndexMap<String, McpServerConfig>> {
    let map = match json {
        JsonValue::Object(map) => map,
        other => {
            return Err(config_error(format!(
                "MCP config must be an object, got {}",
                json_type(&other)
            )))
        }
    };
    let entries = match map.get("mcpServers") {
        Some(JsonValue::Object(servers)) => servers.clone(),
        Some(other) => {
            return Err(config_error(format!(
                "MCP config \"mcpServers\" must be an object, got {}",
                json_type(other)
            )))
        }
        None => map,
    };
    let mut out = IndexMap::with_capacity(entries.len());
    for (name, value) in entries {
        if !value.is_object() {
            return Err(config_error(format!(
                "Server \"{name}\" must be an object, got {}",
                json_type(&value)
            )));
        }
        let config: McpServerConfig = serde_json::from_value(value)
            .map_err(|e| config_error(format!("Server \"{name}\": invalid config: {e}")))?;
        out.insert(name, config);
    }
    Ok(out)
}

fn json_type(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn interpolate_map(map: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    map.iter()
        .map(|(k, v)| Ok((k.clone(), interpolate(v)?)))
        .collect()
}

/// Expand `${VAR}` and `${env:VAR}`; an unset variable is an error (TS `config.node.ts`).
fn interpolate(input: &str) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let key = after[..end].strip_prefix("env:").unwrap_or(&after[..end]);
        let value = std::env::var(key)
            .map_err(|_| config_error(format!("Environment variable \"{key}\" is not set")))?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn config_error(message: impl Into<String>) -> Error {
    Error::other(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_mcp_json_shape() {
        let servers = parse_servers(json!({
            "mcpServers": {
                "fs": {"command": "uvx", "args": ["mcp-fs"], "env": {"A": "1"}, "cwd": "/tmp"},
                "web": {"url": "https://example.com/mcp", "headers": {"Authorization": "Bearer t"}},
            }
        }))
        .unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(
            servers["fs"].resolve().unwrap(),
            ResolvedTransport::Stdio {
                command: "uvx".to_owned(),
                args: vec!["mcp-fs".to_owned()],
                env: BTreeMap::from([("A".to_owned(), "1".to_owned())]),
                cwd: Some("/tmp".to_owned()),
            }
        );
        assert_eq!(
            servers["web"].resolve().unwrap(),
            ResolvedTransport::StreamableHttp {
                url: "https://example.com/mcp".to_owned(),
                headers: BTreeMap::from([("Authorization".to_owned(), "Bearer t".to_owned())]),
            }
        );
    }

    #[test]
    fn parses_bare_server_map() {
        let servers = parse_servers(json!({"fs": {"command": "echo"}})).unwrap();
        assert!(servers.contains_key("fs"));
    }

    #[test]
    fn rejects_non_object_server() {
        let err = parse_servers(json!({"mcpServers": {"fs": []}}))
            .unwrap_err()
            .to_string();
        assert_eq!(err, "Server \"fs\" must be an object, got array");
    }

    #[test]
    fn rejects_ambiguous_and_empty_transport() {
        let both = McpServerConfig {
            command: Some("x".into()),
            url: Some("https://x".into()),
            ..Default::default()
        };
        assert_eq!(
            both.resolve().unwrap_err().to_string(),
            "Server config has both \"command\" and \"url\" — set \"transport\" explicitly or remove one"
        );
        assert_eq!(
            McpServerConfig::default().resolve().unwrap_err().to_string(),
            "Server config must include either \"command\" (stdio) or \"url\" (http)"
        );
    }

    #[test]
    fn rejects_sse_transport() {
        let config = McpServerConfig {
            url: Some("https://x/sse".into()),
            transport: Some(McpTransportKind::Sse),
            ..Default::default()
        };
        assert_eq!(
            config.resolve().unwrap_err().to_string(),
            "SSE transport is not supported: rmcp 3 has no SSE client transport — use \"streamable-http\""
        );
    }

    #[test]
    fn requires_fields_for_explicit_transport() {
        let stdio = McpServerConfig {
            transport: Some(McpTransportKind::Stdio),
            url: Some("https://x".into()),
            ..Default::default()
        };
        assert_eq!(
            stdio.resolve().unwrap_err().to_string(),
            "Stdio transport requires \"command\" field"
        );
        let http = McpServerConfig {
            transport: Some(McpTransportKind::StreamableHttp),
            command: Some("x".into()),
            ..Default::default()
        };
        assert_eq!(
            http.resolve().unwrap_err().to_string(),
            "Streamable HTTP transport requires \"url\" field"
        );
    }

    #[test]
    fn interpolates_env_vars() {
        std::env::set_var("STRANDS_MCP_TEST_TOKEN", "sekret");
        let config: McpServerConfig = serde_json::from_value(json!({
            "url": "https://example.com/${env:STRANDS_MCP_TEST_TOKEN}/mcp",
            "headers": {"Authorization": "Bearer ${STRANDS_MCP_TEST_TOKEN}"},
            "prefix": "p_${STRANDS_MCP_TEST_TOKEN}",
        }))
        .unwrap();
        let resolved = config.interpolate_env().unwrap();
        assert_eq!(resolved.url.as_deref(), Some("https://example.com/sekret/mcp"));
        assert_eq!(resolved.headers["Authorization"], "Bearer sekret");
        assert_eq!(resolved.prefix.as_deref(), Some("p_sekret"));
    }

    #[test]
    fn unset_env_var_is_an_error() {
        let config = McpServerConfig {
            command: Some("${STRANDS_MCP_DEFINITELY_UNSET}".into()),
            ..Default::default()
        };
        assert_eq!(
            config.interpolate_env().unwrap_err().to_string(),
            "Environment variable \"STRANDS_MCP_DEFINITELY_UNSET\" is not set"
        );
    }

    #[tokio::test]
    async fn loads_config_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"fs":{"command":"echo","disabled":true}}}"#,
        )
        .unwrap();
        let servers = ConfigSource::from(path).load().await.unwrap();
        assert!(servers["fs"].disabled);
    }
}
