//! Ported from strands-ts/src/mcp/__tests__/config.test.node.ts
//!
//! The TS tests mock the transports and inspect their constructor arguments; these inspect the
//! resolved transport parts and, for tool listing, run the resolved options against the
//! in-process test server.

use indexmap::IndexMap;
use serde_json::json;

use super::server::{TestServer, tool};
use crate::mcp::config::{
    ResolvedServer, ResolvedTransport, fixed_env, interpolate_env, load_servers_object,
    resolve_server_configs,
};
use crate::mcp::transport::EnvLookup;
use crate::mcp::{
    McpClient, McpClientConfig, McpClientCredentials, McpClientOptions, McpLoadServersOptions,
    McpServerConfig, McpServers, McpToolFilters, StdioServerParameters,
};
use crate::tools::Tool;

fn env() -> EnvLookup {
    fixed_env(&[("PATH", "/usr/bin"), ("HOME", "/home/user")])
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "call sites pass fresh option values"
)]
fn resolve_with(
    servers: serde_json::Value,
    defaults: &McpClientOptions,
    options: McpLoadServersOptions,
    lookup: &EnvLookup,
) -> crate::Result<Vec<ResolvedServer>> {
    let map = match servers {
        serde_json::Value::Object(map) => map.into_iter().collect(),
        _ => IndexMap::new(),
    };
    resolve_server_configs(map, defaults, &options, lookup)
}

fn resolve(servers: serde_json::Value) -> crate::Result<Vec<ResolvedServer>> {
    resolve_with(
        servers,
        &McpClientOptions::default(),
        McpLoadServersOptions::default(),
        &env(),
    )
}

fn error(servers: serde_json::Value) -> String {
    match resolve(servers) {
        Ok(_) => panic!("expected an error"),
        Err(error) => error.to_string(),
    }
}

fn stdio(command: &str) -> StdioServerParameters {
    StdioServerParameters {
        command: command.to_owned(),
        ..Default::default()
    }
}

/// Tool names the resolved options expose for a server with `names`.
async fn listed(server: &ResolvedServer, names: &[&str]) -> Vec<String> {
    let test_server = TestServer::with_tools(names.iter().map(|name| tool(name, None)).collect());
    let client = McpClient::new(McpClientConfig {
        transport: Some(test_server.transport()),
        options: server.options.clone(),
        ..Default::default()
    })
    .expect("client");
    client
        .list_tools()
        .await
        .expect("tools")
        .iter()
        .map(|tool| Tool::name(tool).to_owned())
        .collect()
}

#[tokio::test]
async fn skips_invalid_task_settings_when_that_server_allows_errors_and_keeps_valid_servers() {
    let servers = resolve(json!({
        "invalid": {"url": "https://invalid.example/mcp", "tasksConfig": {"ttl": 0}, "continueOnError": true},
        "valid": {"url": "https://valid.example/mcp"}
    }))
    .expect("servers");
    assert_eq!(servers.len(), 1);
    assert!(!servers[0].options.continue_on_error);
}

#[test]
fn rejects_invalid_task_settings_when_that_server_overrides_lenient_defaults() {
    let defaults = McpClientOptions {
        continue_on_error: true,
        ..Default::default()
    };
    let servers = json!({"invalid": {"url": "https://invalid.example/mcp", "tasksConfig": {"ttl": 0}, "continueOnError": false}});
    let result = resolve_with(servers, &defaults, McpLoadServersOptions::default(), &env());
    assert_eq!(
        result.err().map(|error| error.to_string()).as_deref(),
        Some("MCP tasks (\"tasksConfig\") are not supported in Rust yet")
    );
}

mod transport_detection {
    use super::*;

    #[test]
    fn creates_stdio_client_transport_when_command_is_present() {
        let servers =
            resolve(json!({"my-server": {"command": "node", "args": ["server.js"]}})).expect("servers");
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers[0].transport,
            ResolvedTransport::Stdio(StdioServerParameters {
                args: vec!["server.js".to_owned()],
                ..stdio("node")
            })
        );
    }

    #[test]
    fn creates_mcp_client_with_url_when_url_is_present() {
        let servers = resolve(json!({"remote-server": {"url": "https://example.com/mcp"}})).expect("servers");
        assert_eq!(
            servers[0].transport,
            ResolvedTransport::Http {
                url: "https://example.com/mcp".to_owned(),
                headers: None,
                auth: None
            }
        );
    }

    #[test]
    fn creates_sse_client_transport_when_transport_is_sse() {
        let servers = resolve(json!({"sse-server": {"url": "https://example.com/sse", "transport": "sse"}}))
            .expect("servers");
        assert_eq!(
            servers[0].transport,
            ResolvedTransport::Sse {
                url: "https://example.com/sse".to_owned(),
                headers: None
            }
        );
    }

    #[test]
    fn explicit_transport_overrides_auto_detection() {
        let servers = resolve(json!({"server": {"url": "https://example.com/mcp", "transport": "sse"}}))
            .expect("servers");
        assert!(matches!(servers[0].transport, ResolvedTransport::Sse { .. }));
    }

    #[test]
    fn interpolates_auth_credentials_for_streamable_http() {
        let lookup = fixed_env(&[("CLIENT_ID", "my-id"), ("CLIENT_SECRET", "my-secret")]);
        let servers = json!({"server": {"url": "https://example.com/mcp", "auth": {"clientId": "${CLIENT_ID}", "clientSecret": "${CLIENT_SECRET}"}}});
        let servers = resolve_with(
            servers,
            &McpClientOptions::default(),
            McpLoadServersOptions::default(),
            &lookup,
        )
        .expect("servers");
        let ResolvedTransport::Http { auth, .. } = &servers[0].transport else {
            panic!("http expected");
        };
        assert_eq!(
            auth.as_ref(),
            Some(&McpClientCredentials {
                client_id: "my-id".to_owned(),
                client_secret: "my-secret".to_owned(),
                scopes: None
            })
        );
    }

    #[test]
    fn throws_when_auth_credential_references_missing_env_var() {
        assert_eq!(
            error(
                json!({"server": {"url": "https://example.com/mcp", "auth": {"clientId": "${MISSING_ID}", "clientSecret": "literal"}}})
            ),
            "Environment variable \"MISSING_ID\" is not set"
        );
    }
}

mod env_interpolation {
    use super::*;

    fn with(pairs: &[(&str, &str)], servers: serde_json::Value) -> ResolvedTransport {
        let mut all = vec![("PATH", "/usr/bin"), ("HOME", "/home/user")];
        all.extend_from_slice(pairs);
        let servers = resolve_with(
            servers,
            &McpClientOptions::default(),
            McpLoadServersOptions::default(),
            &fixed_env(&all),
        )
        .expect("servers");
        servers[0].transport.clone()
    }

    #[test]
    fn interpolates_var_in_env_values() {
        let transport = with(
            &[("MY_KEY", "secret-123")],
            json!({"server": {"command": "node", "env": {"API_KEY": "${MY_KEY}"}}}),
        );
        let ResolvedTransport::Stdio(parameters) = transport else {
            panic!("stdio expected")
        };
        assert_eq!(
            parameters.env.expect("env").get("API_KEY").map(String::as_str),
            Some("secret-123")
        );
    }

    #[test]
    fn interpolates_var_in_headers() {
        let transport = with(
            &[("TOKEN", "abc")],
            json!({"server": {"url": "https://example.com/mcp", "headers": {"Authorization": "Bearer ${TOKEN}"}}}),
        );
        let ResolvedTransport::Http { headers, .. } = transport else {
            panic!("http expected")
        };
        assert_eq!(
            headers.expect("headers").get("Authorization").map(String::as_str),
            Some("Bearer abc")
        );
    }

    #[test]
    fn interpolates_var_in_url() {
        let transport = with(
            &[("HOST", "example.com")],
            json!({"server": {"url": "https://${HOST}/mcp"}}),
        );
        let ResolvedTransport::Http { url, .. } = transport else {
            panic!("http expected")
        };
        assert_eq!(url, "https://example.com/mcp");
    }

    #[test]
    fn throws_when_env_var_is_not_set() {
        assert_eq!(
            error(json!({"server": {"command": "node", "env": {"VAL": "${NONEXISTENT_VAR}"}}})),
            "Environment variable \"NONEXISTENT_VAR\" is not set"
        );
    }

    #[test]
    fn skips_server_with_missing_env_var_when_continue_on_error_is_true() {
        let servers = resolve(json!({
            "broken": {"command": "node", "env": {"VAL": "${NONEXISTENT_VAR}"}, "continueOnError": true},
            "working": {"command": "node"}
        }))
        .expect("servers");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].options.application_name.as_deref(), Some("working"));
    }

    #[test]
    fn throws_on_a_non_object_entry_even_when_continue_on_error_is_true() {
        let defaults = McpClientOptions {
            continue_on_error: true,
            ..Default::default()
        };
        let result = resolve_with(
            json!({"bad": "oops"}),
            &defaults,
            McpLoadServersOptions::default(),
            &env(),
        );
        assert_eq!(
            result.err().map(|error| error.to_string()).as_deref(),
            Some("Server \"bad\" must be an object, got string")
        );
    }

    #[test]
    fn interpolates_var_in_cwd() {
        let transport = with(
            &[("PROJECT_DIR", "/home/user/projects")],
            json!({"server": {"command": "node", "cwd": "${PROJECT_DIR}/my-server"}}),
        );
        assert_eq!(
            transport,
            ResolvedTransport::Stdio(StdioServerParameters {
                cwd: Some("/home/user/projects/my-server".into()),
                ..stdio("node")
            })
        );
    }

    #[test]
    fn merges_env_with_default_environment() {
        let transport = with(
            &[("OTHER", "x")],
            json!({"server": {"command": "node", "env": {"CUSTOM": "value"}}}),
        );
        let ResolvedTransport::Stdio(parameters) = transport else {
            panic!("stdio expected")
        };
        let env: Vec<(String, String)> = parameters.env.expect("env").into_iter().collect();
        let expected: Vec<(String, String)> =
            [("HOME", "/home/user"), ("PATH", "/usr/bin"), ("CUSTOM", "value")]
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect();
        assert_eq!(env, expected);
    }

    #[test]
    fn supports_env_var_namespaced_syntax() {
        let lookup = fixed_env(&[("TOKEN", "t1")]);
        assert_eq!(
            interpolate_env("Bearer ${env:TOKEN}", &lookup).expect("value"),
            "Bearer t1"
        );
    }

    #[test]
    fn interpolates_var_in_command_and_args() {
        let transport = with(
            &[("NODE_BIN", "/usr/local/bin/node"), ("SCRIPT", "server.js")],
            json!({"server": {"command": "${NODE_BIN}", "args": ["${SCRIPT}", "--flag"]}}),
        );
        assert_eq!(
            transport,
            ResolvedTransport::Stdio(StdioServerParameters {
                args: vec!["server.js".to_owned(), "--flag".to_owned()],
                ..stdio("/usr/local/bin/node")
            })
        );
    }
}

mod file_config_loading {
    use super::*;

    async fn load(contents: &str, name: &str) -> crate::Result<IndexMap<String, serde_json::Value>> {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join(name);
        tokio::fs::write(&path, contents).await.expect("write");
        load_servers_object(McpServers::from(path), &env()).await
    }

    #[tokio::test]
    async fn reads_and_parses_a_json_file() {
        let servers = load(
            r#"{"my-server": {"command": "node", "args": ["server.js"]}}"#,
            "config.json",
        )
        .await
        .expect("servers");
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["my-server"]);
    }

    #[tokio::test]
    async fn extracts_mcp_servers_key_when_present() {
        let servers = load(
            r#"{"mcpServers": {"server-a": {"command": "node"}, "server-b": {"url": "https://example.com"}}}"#,
            "config.json",
        )
        .await
        .expect("servers");
        let resolved = resolve(serde_json::Value::Object(servers.into_iter().collect())).expect("resolved");
        assert_eq!(resolved[0].transport, ResolvedTransport::Stdio(stdio("node")));
        assert!(matches!(resolved[1].transport, ResolvedTransport::Http { .. }));
    }

    #[tokio::test]
    async fn uses_whole_object_when_mcp_servers_key_is_absent() {
        let servers = load(r#"{"server-a": {"command": "node"}}"#, "config.json")
            .await
            .expect("servers");
        assert_eq!(servers.len(), 1);
    }

    #[tokio::test]
    async fn expands_to_home_directory() {
        let home = tempfile::tempdir().expect("home");
        tokio::fs::create_dir_all(home.path().join("config"))
            .await
            .expect("dir");
        tokio::fs::write(
            home.path().join("config/mcp.json"),
            r#"{"server": {"command": "node"}}"#,
        )
        .await
        .expect("write");
        let home_path = home.path().display().to_string();
        let lookup = fixed_env(&[("HOME", home_path.as_str())]);
        let servers = load_servers_object(McpServers::from("~/config/mcp.json"), &lookup)
            .await
            .expect("servers");
        assert_eq!(servers.len(), 1);
    }

    #[tokio::test]
    async fn throws_on_invalid_file_path() {
        let error = load_servers_object(McpServers::from("/nonexistent/path.json"), &env())
            .await
            .expect_err("missing");
        assert!(
            error.to_string().contains("No such file") || error.to_string().contains("cannot find"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn throws_on_malformed_json() {
        assert!(load("{not json", "bad.json").await.is_err());
    }

    #[tokio::test]
    async fn throws_on_invalid_config_shape() {
        let error = load(r#"["not", "an", "object"]"#, "bad.json")
            .await
            .expect_err("shape");
        assert!(
            error.to_string().starts_with("MCP config must be a JSON object"),
            "{error}"
        );
    }
}

mod defaults_and_per_server_overrides {
    use regex::Regex;

    use super::*;

    fn lenient() -> McpClientOptions {
        McpClientOptions {
            continue_on_error: true,
            ..Default::default()
        }
    }

    fn named(
        options: McpLoadServersOptions,
        defaults: &McpClientOptions,
        servers: serde_json::Value,
    ) -> Vec<ResolvedServer> {
        resolve_with(servers, defaults, options, &env()).expect("servers")
    }

    fn by_name() -> McpLoadServersOptions {
        McpLoadServersOptions {
            prefix_with_server_name: true,
        }
    }

    #[test]
    fn per_server_continue_on_error_overrides_defaults() {
        let servers = named(
            McpLoadServersOptions::default(),
            &lenient(),
            json!({"server": {"command": "node", "continueOnError": false}}),
        );
        assert!(!servers[0].options.continue_on_error);
    }

    #[test]
    fn applies_default_continue_on_error_when_server_does_not_override() {
        let servers = named(
            McpLoadServersOptions::default(),
            &lenient(),
            json!({"server": {"command": "node"}}),
        );
        assert!(servers[0].options.continue_on_error);
    }

    #[tokio::test]
    async fn compiles_config_filter_strings_as_regexes_and_preserves_prefix() {
        let servers = resolve(json!({"server": {
            "command": "node", "prefix": "configured",
            "toolFilters": {"allowed": ["search_.*"], "rejected": ["^search_delete$"]}
        }}))
        .expect("servers");
        assert_eq!(
            listed(&servers[0], &["search_docs", "xsearch_docs", "search_delete"]).await,
            ["configured_search_docs"]
        );
    }

    #[tokio::test]
    async fn interpolates_environment_variables_in_prefix_and_filter_patterns() {
        let lookup = fixed_env(&[("MCP_PREFIX", "docs"), ("MCP_PATTERN", "search_.*")]);
        let servers = json!({"server": {"command": "node", "prefix": "${MCP_PREFIX}", "toolFilters": {"allowed": ["${env:MCP_PATTERN}"]}}});
        let servers = resolve_with(
            servers,
            &McpClientOptions::default(),
            McpLoadServersOptions::default(),
            &lookup,
        )
        .expect("servers");
        assert_eq!(
            listed(&servers[0], &["search_docs", "read_docs"]).await,
            ["docs_search_docs"]
        );
    }

    #[test]
    fn reports_and_optionally_skips_missing_environment_variables_in_new_config_fields() {
        assert_eq!(
            error(json!({"bad": {"command": "node", "prefix": "${NONEXISTENT_PREFIX}"}})),
            "Environment variable \"NONEXISTENT_PREFIX\" is not set"
        );
        let servers = resolve(json!({
            "bad": {"command": "node", "toolFilters": {"allowed": ["${NONEXISTENT_PATTERN}"]}, "continueOnError": true},
            "good": {"command": "node"}
        }))
        .expect("servers");
        let names: Vec<_> = servers
            .iter()
            .map(|server| server.options.application_name.clone())
            .collect();
        assert_eq!(names, [Some("good".to_owned())]);
    }

    #[tokio::test]
    async fn preserves_explicit_empty_server_prefix_and_filters_over_defaults() {
        let defaults = McpClientOptions {
            prefix: Some("default".to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(vec![]),
                rejected: None,
            }),
            ..Default::default()
        };
        let servers = named(
            McpLoadServersOptions::default(),
            &defaults,
            json!({"server": {"command": "node", "prefix": "", "toolFilters": {}}}),
        );
        assert_eq!(listed(&servers[0], &["echo"]).await, ["echo"]);
    }

    #[tokio::test]
    async fn preserves_continue_on_error_while_applying_filter_and_prefix_defaults() {
        let defaults = McpClientOptions {
            prefix: Some("default".to_owned()),
            tool_filters: Some(McpToolFilters {
                allowed: Some(vec!["echo".into()]),
                rejected: None,
            }),
            ..Default::default()
        };
        let servers = named(
            McpLoadServersOptions::default(),
            &defaults,
            json!({"server": {"command": "node", "continueOnError": true}}),
        );
        assert!(servers[0].options.continue_on_error);
        assert_eq!(listed(&servers[0], &["echo"]).await, ["default_echo"]);
    }

    #[tokio::test]
    async fn prefixes_tools_with_the_server_name_when_prefix_with_server_name_is_set() {
        let servers = named(
            by_name(),
            &McpClientOptions::default(),
            json!({"slack": {"command": "node"}}),
        );
        assert_eq!(listed(&servers[0], &["search"]).await, ["slack_search"]);
    }

    #[tokio::test]
    async fn replaces_unsupported_tool_name_characters_in_the_server_name_when_prefix_with_server_name_is_set()
     {
        let servers = named(
            by_name(),
            &McpClientOptions::default(),
            json!({"awslabs.aws-docs mcp/server": {"command": "node"}}),
        );
        assert_eq!(
            listed(&servers[0], &["search"]).await,
            ["awslabs_aws-docs_mcp_server_search"]
        );
    }

    #[tokio::test]
    async fn prefers_the_server_name_over_a_default_prefix_when_prefix_with_server_name_is_set() {
        let defaults = McpClientOptions {
            prefix: Some("global".to_owned()),
            ..Default::default()
        };
        let servers = named(by_name(), &defaults, json!({"slack": {"command": "node"}}));
        assert_eq!(listed(&servers[0], &["search"]).await, ["slack_search"]);
    }

    #[tokio::test]
    async fn lets_an_explicit_server_prefix_win_over_prefix_with_server_name() {
        let servers = named(
            by_name(),
            &McpClientOptions::default(),
            json!({"slack": {"command": "node", "prefix": "chat.v2"}, "chorus": {"command": "node", "prefix": ""}}),
        );
        assert_eq!(listed(&servers[0], &["search"]).await, ["chat.v2_search"]);
        assert_eq!(listed(&servers[1], &["search"]).await, ["search"]);
    }

    #[test]
    fn uses_server_name_as_application_name_when_not_in_defaults() {
        let servers = resolve(json!({"my-named-server": {"command": "node"}})).expect("servers");
        assert_eq!(
            servers[0].options.application_name.as_deref(),
            Some("my-named-server")
        );
    }

    #[test]
    fn uses_defaults_application_name_over_server_name() {
        let defaults = McpClientOptions {
            application_name: Some("my-app".to_owned()),
            ..Default::default()
        };
        let servers = named(
            McpLoadServersOptions::default(),
            &defaults,
            json!({"server": {"command": "node"}}),
        );
        assert_eq!(servers[0].options.application_name.as_deref(), Some("my-app"));
    }

    #[test]
    fn compiled_patterns_are_regexes() {
        let servers = resolve(json!({"server": {"command": "node", "toolFilters": {"rejected": ["a+"]}}}))
            .expect("servers");
        let filters = servers[0].options.tool_filters.clone().expect("filters");
        let rejected = filters.rejected.expect("rejected");
        let crate::mcp::McpToolMatcher::Pattern(pattern) = &rejected[0] else {
            panic!("pattern expected")
        };
        assert_eq!(pattern.as_str(), Regex::new("a+").expect("regex").as_str());
    }
}

mod error_cases {
    use super::*;

    #[test]
    fn throws_a_contextual_syntax_error_for_an_invalid_config_regex() {
        let message = error(json!({"bad": {"command": "node", "toolFilters": {"allowed": ["([unclosed"]}}}));
        assert!(
            message.starts_with("Server \"bad\": invalid regex in toolFilters.allowed: \"([unclosed\""),
            "{message}"
        );
    }

    #[test]
    fn skips_an_invalid_config_regex_when_continue_on_error_is_true() {
        let servers = resolve(json!({
            "bad": {"command": "node", "toolFilters": {"allowed": ["([unclosed"]}, "continueOnError": true},
            "good": {"command": "node"}
        }))
        .expect("servers");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].options.application_name.as_deref(), Some("good"));
    }

    #[test]
    fn throws_when_server_has_neither_command_nor_url() {
        assert_eq!(
            error(json!({"bad": {}})),
            "Server config must include either \"command\" (stdio) or \"url\" (http)"
        );
    }

    #[test]
    fn throws_when_stdio_transport_specified_without_command() {
        assert_eq!(
            error(json!({"bad": {"transport": "stdio"}})),
            "Stdio transport requires \"command\" field"
        );
    }

    #[test]
    fn throws_when_streamable_http_transport_specified_without_url() {
        assert_eq!(
            error(json!({"bad": {"transport": "streamable-http"}})),
            "Streamable HTTP transport requires \"url\" field"
        );
    }

    #[test]
    fn throws_when_sse_transport_specified_without_url() {
        assert_eq!(
            error(json!({"bad": {"transport": "sse"}})),
            "SSE transport requires \"url\" field"
        );
    }

    #[test]
    fn throws_when_auth_is_used_with_sse_transport() {
        let message = error(
            json!({"bad": {"url": "https://example.com/sse", "transport": "sse", "auth": {"clientId": "id", "clientSecret": "secret"}}}),
        );
        assert!(
            message.starts_with("SSE transport does not support auth"),
            "{message}"
        );
    }

    #[test]
    fn throws_when_server_has_both_command_and_url_without_explicit_transport() {
        assert_eq!(
            error(json!({"bad": {"command": "node", "url": "https://example.com"}})),
            "Server config has both \"command\" and \"url\" — set \"transport\" explicitly or remove one"
        );
    }

    #[test]
    fn rejects_an_unsupported_transport_type() {
        assert_eq!(
            error(json!({"bad": {"url": "https://example.com", "transport": "websocket"}})),
            "Unsupported transport type: websocket"
        );
    }
}

mod disabled {
    use super::*;

    #[test]
    fn skips_disabled_servers() {
        let servers =
            resolve(json!({"off": {"command": "node", "disabled": true}, "active": {"command": "node"}}))
                .expect("servers");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].options.application_name.as_deref(), Some("active"));
    }
}

mod load_servers {
    use super::*;

    #[tokio::test]
    async fn builds_clients_from_a_typed_map() {
        let mut servers = IndexMap::new();
        servers.insert(
            "remote".to_owned(),
            crate::mcp::McpServerConfig {
                url: Some("https://example.com/mcp".to_owned()),
                ..Default::default()
            },
        );
        servers.insert(
            "sse".to_owned(),
            crate::mcp::McpServerConfig {
                url: Some("https://example.com/sse".to_owned()),
                transport: Some("sse".to_owned()),
                ..Default::default()
            },
        );
        let clients = McpClient::load_servers(
            servers,
            McpClientOptions::default(),
            McpLoadServersOptions::default(),
        )
        .await
        .expect("clients");
        let names: Vec<&str> = clients.iter().map(McpClient::client_name).collect();
        assert_eq!(names, ["remote", "sse"]);
    }

    #[tokio::test]
    async fn skips_a_client_that_fails_to_build_under_continue_on_error() {
        let servers = json!({
            "bad": {"url": "not a url", "continueOnError": true},
            "good": {"url": "https://example.com/mcp"}
        });
        let clients = McpClient::load_servers(
            servers,
            McpClientOptions::default(),
            McpLoadServersOptions::default(),
        )
        .await
        .expect("clients");
        let names: Vec<&str> = clients.iter().map(McpClient::client_name).collect();
        assert_eq!(names, ["good"]);
    }
}

// Rust-only: `Debug` output reaches logs and error messages.
#[test]
fn debug_output_names_env_and_headers_without_their_values() {
    let config = McpServerConfig {
        command: Some("server".to_owned()),
        env: Some(IndexMap::from([(
            "API_TOKEN".to_owned(),
            "env-value-1".to_owned(),
        )])),
        headers: Some(IndexMap::from([(
            "Authorization".to_owned(),
            "Bearer header-value-2".to_owned(),
        )])),
        auth: Some(McpClientCredentials {
            client_id: "id".to_owned(),
            client_secret: "auth-value-3".to_owned(),
            scopes: None,
        }),
        ..Default::default()
    };
    let servers = McpServers::Map(IndexMap::from([("one".to_owned(), config.clone())]));
    let raw =
        McpServers::Json(json!({ "one": { "command": "server", "env": { "API_TOKEN": "env-value-1" } } }));
    let builder = McpServerConfig::builder().env(IndexMap::from([(
        "API_TOKEN".to_owned(),
        "env-value-1".to_owned(),
    )]));
    for printed in [
        format!("{config:?}"),
        format!("{servers:?}"),
        format!("{raw:?}"),
        format!("{builder:?}"),
    ] {
        for value in ["env-value-1", "header-value-2", "auth-value-3"] {
            assert!(!printed.contains(value), "{printed}");
        }
    }
    let printed = format!("{config:?}");
    assert!(
        printed.contains(r#"env: Some({"API_TOKEN": "<redacted>"})"#),
        "{printed}"
    );
    assert!(
        printed.contains(r#"headers: Some({"Authorization": "<redacted>"})"#),
        "{printed}"
    );
    assert!(printed.contains(r#"command: Some("server")"#), "{printed}");
}

// Rust-only: a key can sit in a URL's userinfo, query string or fragment, or in an argument.
#[test]
fn debug_output_prints_a_url_without_userinfo_query_or_fragment_and_args_as_a_count() {
    let remote = McpServerConfig {
        url: Some(
            "https://user:url-value-1@example.com:8443/mcp/v1?key=query-value-2#fragment-value-3".to_owned(),
        ),
        ..Default::default()
    };
    let local = McpServerConfig {
        command: Some("server".to_owned()),
        args: Some(vec![
            "--token".to_owned(),
            "arg-value-4".to_owned(),
            "--verbose".to_owned(),
        ]),
        ..Default::default()
    };
    let servers = McpServers::Map(IndexMap::from([
        ("remote".to_owned(), remote.clone()),
        ("local".to_owned(), local.clone()),
    ]));
    let raw = McpServers::Json(json!({
        "remote": { "url": remote.url },
        "local": { "command": "server", "args": local.args }
    }));
    let builder = McpServerConfig::builder()
        .url("https://user:url-value-1@example.com/mcp?key=query-value-2".to_owned())
        .args(vec!["--token".to_owned(), "arg-value-4".to_owned()]);
    for printed in [
        format!("{remote:?}"),
        format!("{local:?}"),
        format!("{servers:?}"),
        format!("{servers:#?}"),
        format!("{raw:?}"),
        format!("{builder:?}"),
    ] {
        for value in [
            "user",
            "url-value-1",
            "query-value-2",
            "fragment-value-3",
            "arg-value-4",
            "--token",
        ] {
            assert!(!printed.contains(value), "{printed}");
        }
    }
    let printed = format!("{remote:?}");
    assert!(
        printed.contains(r#"url: Some("https://example.com:8443/mcp/v1")"#),
        "{printed}"
    );
    let printed = format!("{local:?}");
    assert!(printed.contains("args: Some(<3 redacted>)"), "{printed}");
    assert!(printed.contains(r#"command: Some("server")"#), "{printed}");
}

// Rust-only: what is printed for a URL that has no host, or does not parse, holds none of it.
#[test]
fn debug_output_prints_a_plain_url_whole_and_an_unparseable_one_not_at_all() {
    let printed = |url: &str| {
        format!(
            "{:?}",
            McpServerConfig {
                url: Some(url.to_owned()),
                ..Default::default()
            }
        )
    };
    assert!(printed("http://localhost:3000/sse").contains(r#"url: Some("http://localhost:3000/sse")"#));
    assert!(printed("https://[::1]/mcp").contains(r#"url: Some("https://[::1]/mcp")"#));
    for url in ["not a url key-value-5", "data:text/plain,key-value-5"] {
        let printed = printed(url);
        assert!(printed.contains(r#"url: Some("<redacted>")"#), "{printed}");
    }
    assert!(printed("https://example.com").contains(r#"url: Some("https://example.com/")"#));
}

// Rust-only: the parameters of a spawned server are printed without what can hold a key.
#[test]
fn debug_output_of_stdio_parameters_names_env_without_its_values_and_counts_args() {
    let parameters = StdioServerParameters {
        command: "server".to_owned(),
        args: vec!["--token".to_owned(), "arg-value-1".to_owned()],
        env: Some(IndexMap::from([(
            "API_TOKEN".to_owned(),
            "env-value-2".to_owned(),
        )])),
        cwd: Some("/srv".into()),
    };
    let builder = StdioServerParameters::builder()
        .command("server")
        .args(parameters.args.clone())
        .maybe_env(parameters.env.clone());
    assert_eq!(
        format!("{parameters:?}"),
        r#"StdioServerParameters { command: "server", args: <2 redacted>, env: Some({"API_TOKEN": "<redacted>"}), cwd: Some("/srv") }"#
    );
    for printed in [
        format!("{parameters:#?}"),
        format!("{builder:?}"),
        format!("{:?}", ResolvedTransport::Stdio(parameters)),
    ] {
        for value in ["arg-value-1", "env-value-2", "--token"] {
            assert!(!printed.contains(value), "{printed}");
        }
    }
}
