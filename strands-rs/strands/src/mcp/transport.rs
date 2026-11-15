//! How an [`McpClient`](super::McpClient) reaches its server (TS `McpTransport` and the
//! `@modelcontextprotocol/client` transports).

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use process_wrap::tokio::{CommandWrap, KillOnDrop};
use rmcp::RoleClient;
use rmcp::service::{ClientLifecycleMode, RunningService, serve_client_with_lifecycle};
use rmcp::transport::{IntoTransport, TokioChildProcess};

use super::error::McpError;
use super::handler::McpHandler;
use super::sse::SseClientTransport;

pub(crate) type Running = RunningService<RoleClient, McpHandler>;
pub(crate) type ConnectFn = Arc<
    dyn Fn(McpHandler, ClientLifecycleMode) -> BoxFuture<'static, Result<Running, McpError>> + Send + Sync,
>;
pub(crate) type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The variables a stdio child inherits from this process (the MCP SDK
/// `DEFAULT_INHERITED_ENV_VARS`).
#[cfg(not(windows))]
pub(crate) const DEFAULT_INHERITED_ENV_VARS: &[&str] = &["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER"];
/// The variables a stdio child inherits from this process (the MCP SDK
/// `DEFAULT_INHERITED_ENV_VARS`).
#[cfg(windows)]
pub(crate) const DEFAULT_INHERITED_ENV_VARS: &[&str] = &[
    "APPDATA",
    "COMSPEC",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "PATH",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "USERNAME",
    "USERPROFILE",
    "WINDIR",
];

/// The inherited variables that are set, skipping values that start with `()` (exported shell
/// functions), as the MCP SDK `getDefaultEnvironment` does.
pub(crate) fn default_environment(lookup: &EnvLookup) -> IndexMap<String, String> {
    DEFAULT_INHERITED_ENV_VARS
        .iter()
        .filter_map(|key| lookup(key).map(|value| ((*key).to_owned(), value)))
        .filter(|(_, value)| !value.starts_with("()"))
        .collect()
}

pub(crate) fn process_env() -> EnvLookup {
    Arc::new(|key| std::env::var(key).ok())
}

/// A server spawned as a child process, spoken to over stdin and stdout (the MCP SDK
/// `StdioServerParameters`).
///
/// The child gets the default environment (`HOME`, `LOGNAME`, `PATH`, `SHELL`, `TERM`, `USER`
/// on Unix) with `env` merged over it, runs in its own process group, and is killed with that
/// group when the client disconnects or is dropped. Its stderr is inherited.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into))]
pub struct StdioServerParameters {
    /// The executable to run.
    pub command: String,
    /// Its arguments.
    #[builder(default)]
    pub args: Vec<String>,
    /// Variables merged over the default environment.
    pub env: Option<IndexMap<String, String>>,
    /// The working directory; defaults to this process's.
    pub cwd: Option<PathBuf>,
}

/// A transport an [`McpClient`](super::McpClient) connects through (TS `McpTransport`).
///
/// It is a recipe rather than a live connection, so the client can connect again after a
/// failure or a [`disconnect`](super::McpClient::disconnect).
#[derive(Clone)]
pub struct McpTransport {
    kind: &'static str,
    connect: ConnectFn,
}

impl McpTransport {
    /// A child process over stdio (TS `StdioClientTransport`).
    pub fn stdio(parameters: StdioServerParameters) -> Self {
        Self::stdio_with_env(parameters, &process_env())
    }

    pub(crate) fn stdio_with_env(parameters: StdioServerParameters, lookup: &EnvLookup) -> Self {
        let mut env = default_environment(lookup);
        env.extend(parameters.env.clone().unwrap_or_default());
        Self::from_fallible_factory("stdio", move || spawn_child(&parameters, &env))
    }

    /// The legacy HTTP+SSE transport (protocol revision 2024-11-05; TS `SSEClientTransport`),
    /// sending `headers` with the event-stream `GET` and every `POST`.
    ///
    /// # Errors
    ///
    /// The url doesn't parse, or a header name or value is invalid.
    pub fn sse(url: &str, headers: IndexMap<String, String>) -> crate::Result<Self> {
        let url = reqwest::Url::parse(url).map_err(|_| crate::Error::other("Invalid URL"))?;
        let headers = header_map(headers)?;
        Ok(Self::from_fallible_factory("sse", move || {
            Ok(SseClientTransport::start(url.clone(), headers.clone()))
        }))
    }

    /// A transport built by `factory` for every connection attempt, from any `rmcp` client
    /// transport (for example `rmcp::transport::StreamableHttpClientTransport`, or an in-memory
    /// pipe).
    pub fn from_factory<T, E, A>(factory: impl Fn() -> T + Send + Sync + 'static) -> Self
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::from_fallible_factory("custom", move || Ok(factory()))
    }

    pub(crate) fn from_fallible_factory<T, E, A>(
        kind: &'static str,
        factory: impl Fn() -> Result<T, McpError> + Send + Sync + 'static,
    ) -> Self
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let factory = Arc::new(factory);
        Self {
            kind,
            connect: Arc::new(move |handler, lifecycle| {
                let transport = factory();
                async move {
                    let running = serve_client_with_lifecycle(handler, transport?, lifecycle).await?;
                    Ok(running)
                }
                .boxed()
            }),
        }
    }

    /// A transport whose connection is produced by `connect`; the url-configured client uses
    /// this to run the OAuth setup inside the connection attempt.
    pub(crate) fn from_connect(kind: &'static str, connect: ConnectFn) -> Self {
        Self { kind, connect }
    }

    pub(crate) fn connect(
        &self,
        handler: McpHandler,
        lifecycle: ClientLifecycleMode,
    ) -> BoxFuture<'static, Result<Running, McpError>> {
        (self.connect)(handler, lifecycle)
    }
}

impl fmt::Debug for McpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpTransport")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

fn spawn_child(
    parameters: &StdioServerParameters,
    env: &IndexMap<String, String>,
) -> Result<TokioChildProcess, McpError> {
    let mut command = CommandWrap::with_new(&parameters.command, |command| {
        command.args(&parameters.args).env_clear().envs(env);
        if let Some(cwd) = &parameters.cwd {
            command.current_dir(cwd);
        }
    });
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    command.wrap(KillOnDrop);
    TokioChildProcess::new(command).map_err(|error| McpError::sdk(error.to_string()))
}

/// Typed headers from a string map.
pub(crate) fn header_map(headers: IndexMap<String, String>) -> crate::Result<http::HeaderMap> {
    let mut map = http::HeaderMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::try_from(name.as_str()).map_err(|error| {
            crate::Error::other(format!("header=<{name}> | invalid header name: {error}"))
        })?;
        let value = http::HeaderValue::try_from(value.as_str()).map_err(|error| {
            crate::Error::other(format!("header=<{name}> | invalid header value: {error}"))
        })?;
        map.insert(name, value);
    }
    Ok(map)
}
