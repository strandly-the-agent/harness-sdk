//! `McpClient` (TS `mcp/client.ts`).
#![expect(
    deprecated,
    reason = "MCP logging (SEP-2577) is deprecated in rmcp but part of the TS client"
)]

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use indexmap::IndexMap;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientRequest, ElicitationCapability,
    FormElicitationCapability, GetPromptRequestParams, GetPromptResult, Implementation, InputRequest,
    InputRequests, InputRequiredResult, InputResponses, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsRequest, LoggingMessageNotificationParam, PaginatedRequestParams,
    ProgressToken, ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities,
    ServerResult, UrlElicitationCapability,
};
use rmcp::service::{ClientLifecycleMode, PeerRequestOptions};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{Peer, RoleClient};
use tracing::Instrument;

use super::auth::{ClientCredentialsClient, client_credentials};
use super::elicitation::{ElicitationCallback, ElicitationContext};
use super::error::McpError;
use super::filters::McpToolFilters;
use super::handler::McpHandler;
use super::mcp_tool::McpTool;
use super::trace::{current_trace_context, inject_trace_context};
use super::transport::{McpTransport, Running, header_map, serve};
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::{Error, Result, ToolValidationError};
use crate::tools::Tool;
use crate::types::json::{JsonObject, JsonValue};

/// The client name sent in the MCP handshake when `application_name` is unset.
pub const DEFAULT_CLIENT_NAME: &str = "strands-agents-rs-sdk";
/// The MCP SDK default page cap for the no-token list methods.
const DEFAULT_LIST_MAX_PAGES: u32 = 64;
/// TS `MAX_TIMER_DELAY_MS`.
const MAX_TIMER_DELAY: Duration = Duration::from_millis(2_147_483_647);
const MAX_TOOL_NAME_LENGTH: usize = 64;
/// The MCP SDK `DEFAULT_INPUT_REQUIRED_MAX_ROUNDS`: input-required rounds fulfilled per call.
const MRTR_MAX_ROUNDS: u32 = 10;
/// The MCP SDK `REQUEST_STATE_ONLY_LEG_PACING_MS`.
const REQUEST_STATE_ONLY_LEG_PACING: Duration = Duration::from_millis(250);
const INPUT_REQUIRED_WITHOUT_INPUT: &str = "Invalid result for tools/call: input_required carries neither inputRequests nor requestState (every input_required result must include at least one of the two)";
/// The MCP SDK `listChanged.tools.debounceMs`.
const TOOLS_CHANGED_DEBOUNCE: Duration = Duration::from_millis(300);
/// JSON-RPC `InvalidRequest`.
const ERROR_INVALID_REQUEST: i32 = -32600;
/// JSON-RPC `InvalidParams`.
const ERROR_INVALID_PARAMS: i32 = -32602;
const FAILED_TO_CONNECT: &str = "MCP server failed to connect. Call connect(true) to retry.";

/// The connection state of an [`McpClient`] (TS `McpConnectionState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpConnectionState {
    /// Not connected; the next call connects.
    Disconnected,
    /// Connected.
    Connected,
    /// A connection attempt failed with `continue_on_error`; call `connect(true)` to retry.
    Failed,
}

/// One progress notification for a tool call (TS `McpProgress`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct McpProgress {
    /// Progress so far.
    pub progress: f64,
    /// The total, when the server gives one.
    pub total: Option<f64>,
    /// A message, when the server gives one.
    pub message: Option<String>,
}

impl McpProgress {
    /// A progress value.
    pub fn new(progress: f64, total: Option<f64>, message: Option<String>) -> Self {
        Self {
            progress,
            total,
            message,
        }
    }
}

/// Receives tool-call progress (TS `McpProgressCallback`). A panic is caught and logged.
pub type McpProgressCallback = Arc<dyn Fn(McpProgress) + Send + Sync>;
/// Receives server log messages (TS `logHandler`).
pub type McpLogHandler = Arc<dyn Fn(LoggingMessageNotificationParam) + Send + Sync>;
/// Receives the previously registered tool names and the freshly listed tools after a
/// tools-list change (TS `onToolsChanged`).
pub type McpToolsChangedCallback = Arc<dyn Fn(Vec<String>, Vec<McpTool>) + Send + Sync>;
/// An OAuth-authorized HTTP client for the Streamable HTTP transport (TS `authProvider`).
pub type McpAuthProvider = rmcp::transport::AuthClient<reqwest::Client>;

/// OAuth 2.0 client-credentials for machine-to-machine auth (TS `McpClientCredentials`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct McpClientCredentials {
    /// The client id.
    pub client_id: String,
    /// The client secret.
    pub client_secret: String,
    /// Scopes to request.
    pub scopes: Option<Vec<String>>,
}

impl fmt::Debug for McpClientCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClientCredentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Behavioural options shared by every client configuration (TS `McpClientOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct McpClientOptions {
    /// The client name sent in the handshake; defaults to `strands-agents-rs-sdk`.
    pub application_name: Option<String>,
    /// The client version sent in the handshake; defaults to this crate's version.
    pub application_version: Option<String>,
    /// Skips injecting W3C trace context into tool-call arguments.
    #[builder(default)]
    pub disable_mcp_instrumentation: bool,
    /// Prefix for agent-facing tool names, applied as `<prefix>_<toolName>`.
    pub prefix: Option<String>,
    /// Which tools this client exposes.
    pub tool_filters: Option<McpToolFilters>,
    /// Page cap for the no-token list methods, which aggregate every page; `0` disables the cap.
    /// Defaults to 64.
    pub list_max_pages: Option<u32>,
    /// Progress for every tool call, including calls an agent makes; a per-call callback
    /// takes precedence.
    pub progress_callback: Option<McpProgressCallback>,
    /// Answers server elicitation requests; when set, the client advertises elicitation
    /// support (form and url modes).
    pub elicitation_callback: Option<ElicitationCallback>,
    /// When `true`, connection failures and overlong prefixed names during tool listing are
    /// skipped with warnings.
    #[builder(default)]
    pub continue_on_error: bool,
    /// Receives server log messages; defaults to logging them through `tracing`.
    pub log_handler: Option<McpLogHandler>,
}

impl fmt::Debug for McpClientOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClientOptions")
            .field("application_name", &self.application_name)
            .field("application_version", &self.application_version)
            .field("disable_mcp_instrumentation", &self.disable_mcp_instrumentation)
            .field("prefix", &self.prefix)
            .field("tool_filters", &self.tool_filters)
            .field("list_max_pages", &self.list_max_pages)
            .field("progress_callback", &self.progress_callback.is_some())
            .field("elicitation_callback", &self.elicitation_callback.is_some())
            .field("continue_on_error", &self.continue_on_error)
            .field("log_handler", &self.log_handler.is_some())
            .finish()
    }
}

/// Everything [`McpClient::new`] takes (TS `McpClientConfig`): a transport or a url, and the
/// shared options.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct McpClientConfig {
    /// A transport recipe. Mutually exclusive with `url`.
    pub transport: Option<McpTransport>,
    /// A server url; the client builds a Streamable HTTP transport for it.
    pub url: Option<String>,
    /// Client credentials for OAuth machine-to-machine auth. Requires `url`.
    pub auth: Option<McpClientCredentials>,
    /// An OAuth-authorized HTTP client. Requires `url`; exclusive with `auth`.
    pub auth_provider: Option<McpAuthProvider>,
    /// HTTP headers sent with every request. Requires `url`.
    pub headers: Option<IndexMap<String, String>>,
    /// The shared options.
    #[builder(default)]
    pub options: McpClientOptions,
}

impl fmt::Debug for McpClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClientConfig")
            .field("transport", &self.transport)
            .field("url", &self.url)
            .field("auth", &self.auth)
            .field("auth_provider", &self.auth_provider.is_some())
            .field(
                "headers",
                &self
                    .headers
                    .as_ref()
                    .map(|headers| headers.keys().collect::<Vec<_>>()),
            )
            .field("options", &self.options)
            .finish()
    }
}

/// Per-call options for [`McpClient::call_tool_with`] (TS `McpCallToolOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
pub struct McpCallToolOptions {
    /// Cancels the in-flight request.
    pub cancel_signal: Option<CancelSignal>,
    /// Overall time limit for this call; progress notifications reset the idle timer. Without
    /// it, the default request timeout applies.
    pub timeout: Option<Duration>,
    /// Progress for this call; takes precedence over the client's callback.
    pub progress_callback: Option<McpProgressCallback>,
}

impl fmt::Debug for McpCallToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpCallToolOptions")
            .field("cancel_signal", &self.cancel_signal)
            .field("timeout", &self.timeout)
            .field("progress_callback", &self.progress_callback.is_some())
            .finish()
    }
}

/// Overrides for [`McpClient::list_tools_with`] (TS `McpListToolsOptions`). An unset field uses
/// the client's value; `Some("")` or empty filters disable it.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct McpListToolsOptions {
    /// The prefix for this listing.
    pub prefix: Option<String>,
    /// The filters for this listing.
    pub tool_filters: Option<McpToolFilters>,
}

/// Pagination for the prompt and resource list methods (TS `McpListOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct McpListOptions {
    /// Fetch the single page after this cursor; without it, every page is aggregated.
    pub pagination_token: Option<String>,
}

/// A client for one MCP server (TS `McpClient`).
///
/// Cheap to clone; clones share the connection. It connects lazily on the first call, and an
/// agent that takes it as a tool source lists its tools in `Agent::initialize`. Concurrent
/// calls share one connection without serializing. [`disconnect`](Self::disconnect) closes the
/// connection; dropping the last clone (including the tools it listed) closes it too.
#[derive(Clone)]
pub struct McpClient {
    pub(crate) inner: Arc<Inner>,
}

type SharedConnect = Shared<BoxFuture<'static, Result<(), McpError>>>;

pub(crate) struct Inner {
    name: String,
    version: String,
    pub(crate) options: McpClientOptions,
    transport: McpTransport,
    state: Mutex<Conn>,
    connecting: Mutex<Option<(u64, SharedConnect)>>,
    connect_ids: AtomicU64,
    generation: AtomicU64,
    /// Names from the last listing that used the client's own prefix and filters.
    registered: Mutex<Vec<String>>,
    refresh: Mutex<Refresh>,
    on_tools_changed: Mutex<Option<McpToolsChangedCallback>>,
    /// Bumped by every tools-list-changed notification; a refresh runs once it settles.
    tools_changed: AtomicU64,
    pub(crate) progress: Mutex<ProgressRoutes>,
    pub(crate) trace_context: TraceSource,
}

pub(crate) type TraceSource = Arc<dyn Fn() -> Option<JsonObject> + Send + Sync>;

#[derive(Default)]
struct Refresh {
    running: bool,
    pending: bool,
}

/// Clears `running` when a refresh ends, including by a panic or a dropped future.
struct RefreshRunning<'a>(Option<&'a Mutex<Refresh>>);

impl RefreshRunning<'_> {
    /// Ends the refresh unless a notification arrived during it, in one critical section so
    /// no notification is lost between the check and the reset.
    fn finish_unless_pending(&mut self) -> bool {
        let Some(refresh) = self.0 else { return true };
        let mut state = lock(refresh);
        if state.pending {
            state.pending = false;
            return false;
        }
        state.running = false;
        self.0 = None;
        true
    }
}

impl Drop for RefreshRunning<'_> {
    fn drop(&mut self) {
        if let Some(refresh) = self.0 {
            lock(refresh).running = false;
        }
    }
}

/// Per-call progress callbacks by progress token. Notifications are routed in arrival order on
/// the connection's receive path; ones that arrive before their call registers are held briefly
/// and delivered on registration.
#[derive(Default)]
pub(crate) struct ProgressRoutes {
    routes: std::collections::HashMap<ProgressToken, McpProgressCallback>,
    early: std::collections::VecDeque<(ProgressToken, McpProgress)>,
}

const EARLY_PROGRESS_LIMIT: usize = 32;

impl ProgressRoutes {
    fn take_early(&mut self, token: &ProgressToken) -> Vec<McpProgress> {
        let (early, rest): (Vec<_>, Vec<_>) = self.early.drain(..).partition(|(held, _)| held == token);
        self.early = rest.into_iter().collect();
        early.into_iter().map(|(_, progress)| progress).collect()
    }

    fn hold(&mut self, token: ProgressToken, progress: McpProgress) {
        if self.early.len() >= EARLY_PROGRESS_LIMIT {
            self.early.pop_front();
        }
        self.early.push_back((token, progress));
    }
}

enum Conn {
    Disconnected,
    Connected(Running),
    Failed,
}

impl McpClient {
    /// The MCP SDK default per-request timeout.
    pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

    /// A client for `config`. Nothing connects yet.
    ///
    /// # Errors
    ///
    /// Both or neither of `transport` and `url` are set, `headers` come with a `transport`, or
    /// the url doesn't parse.
    pub fn new(config: McpClientConfig) -> Result<Self> {
        Self::with_trace_source(config, Arc::new(current_trace_context))
    }

    /// [`new`](Self::new) with the trace-context seam the tests set.
    pub(crate) fn with_trace_source(config: McpClientConfig, trace_context: TraceSource) -> Result<Self> {
        let options = config.options.clone();
        let transport = resolve_transport(config)?;
        let name = options
            .application_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| DEFAULT_CLIENT_NAME.to_owned());
        let version = options
            .application_version
            .clone()
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
        Ok(Self {
            inner: Arc::new(Inner {
                name,
                version,
                options,
                transport,
                state: Mutex::new(Conn::Disconnected),
                connecting: Mutex::new(None),
                connect_ids: AtomicU64::new(0),
                generation: AtomicU64::new(0),
                registered: Mutex::default(),
                refresh: Mutex::default(),
                on_tools_changed: Mutex::default(),
                tools_changed: AtomicU64::new(0),
                progress: Mutex::default(),
                trace_context,
            }),
        })
    }

    /// The client name sent in the handshake.
    pub fn client_name(&self) -> &str {
        &self.inner.name
    }

    /// The client version sent in the handshake.
    pub fn client_version(&self) -> &str {
        &self.inner.version
    }

    /// Whether connection failures are skipped with a warning.
    pub fn continue_on_error(&self) -> bool {
        self.inner.options.continue_on_error
    }

    /// The connection state.
    pub fn connection_state(&self) -> McpConnectionState {
        match &*self.inner.state() {
            Conn::Disconnected => McpConnectionState::Disconnected,
            Conn::Connected(_) => McpConnectionState::Connected,
            Conn::Failed => McpConnectionState::Failed,
        }
    }

    /// The `rmcp` peer of the live connection (TS `client`), for requests this client doesn't
    /// wrap.
    pub fn peer(&self) -> Option<Peer<RoleClient>> {
        self.inner.peer()
    }

    /// The server's capabilities, once connected.
    pub fn server_capabilities(&self) -> Option<ServerCapabilities> {
        self.inner.server_info().map(|info| info.capabilities.clone())
    }

    /// The server's name and version, once connected.
    pub fn server_version(&self) -> Option<Implementation> {
        self.inner.server_info().and_then(|info| info.server_info.clone())
    }

    /// The server's instructions, once connected.
    pub fn server_instructions(&self) -> Option<String> {
        self.inner
            .server_info()
            .and_then(|info| info.instructions.clone())
    }

    /// The protocol revision the connection negotiated.
    pub fn negotiated_protocol_version(&self) -> Option<String> {
        self.inner
            .server_info()
            .map(|info| info.protocol_version.to_string())
    }

    /// Connects if not connected yet; with `reconnect`, replaces the current connection.
    ///
    /// Concurrent callers share one attempt. With `continue_on_error`, a failed attempt
    /// leaves the client [`Failed`](McpConnectionState::Failed) instead of returning an error,
    /// and later calls don't retry until `connect(true)`.
    ///
    /// # Errors
    ///
    /// The connection attempt failed (without `continue_on_error`), or the client was
    /// disconnected while connecting.
    pub async fn connect(&self, reconnect: bool) -> Result<()> {
        self.connect_with(reconnect, None).await
    }

    /// [`connect`](Self::connect) that stops waiting when `signal` is cancelled. The shared
    /// attempt keeps running for other callers.
    ///
    /// # Errors
    ///
    /// As [`connect`](Self::connect), or `signal` was cancelled.
    pub async fn connect_with(&self, reconnect: bool, signal: Option<&CancelSignal>) -> Result<()> {
        if let Some(signal) = signal.filter(|signal| signal.is_cancelled()) {
            return Err(abort_error(signal));
        }
        let generation = self.inner.generation();
        if let Some(pending) = self.inner.pending_connect() {
            if let Err(error) = race(pending, signal).await
                && (!reconnect || signal.is_some_and(CancelSignal::is_cancelled))
            {
                return Err(error);
            }
            self.inner.assert_current(generation)?;
            if !reconnect {
                return Ok(());
            }
        }
        if !reconnect && !matches!(*self.inner.state(), Conn::Disconnected) {
            return Ok(());
        }
        let pending = Inner::start_connect(&self.inner, reconnect);
        race(pending, signal).await?;
        self.inner.assert_current(generation)?;
        Ok(())
    }

    /// Closes the connection. The next call connects again.
    ///
    /// # Errors
    ///
    /// Never fails today; the `Result` leaves room for transport close errors.
    pub async fn disconnect(&self) -> Result<()> {
        let previous = {
            let mut state = self.inner.state();
            self.inner.generation.fetch_add(1, Ordering::SeqCst);
            self.inner.tools_changed.fetch_add(1, Ordering::SeqCst);
            std::mem::replace(&mut *state, Conn::Disconnected)
        };
        *self.inner.connecting() = None;
        if let Conn::Connected(running) = previous {
            let _closed = running.cancel().await;
        }
        Ok(())
    }

    /// The server's tools as [`McpTool`]s, with the client's prefix and filters.
    ///
    /// # Errors
    ///
    /// As [`list_tools_with`](Self::list_tools_with).
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        self.list_tools_with(McpListToolsOptions::default()).await
    }

    /// The server's tools, with `options` overriding the client's prefix and filters.
    ///
    /// A prefix renames tools for the agent only; tools are always called, and matched by name
    /// and pattern filters, under their server-side name. Prefixed names longer than 64
    /// characters are skipped with a warning under `continue_on_error`, and fail the listing
    /// otherwise. With `continue_on_error` and a failed connection, the list is empty.
    ///
    /// # Errors
    ///
    /// Connecting or listing failed, or a prefixed name is too long (`ToolValidationError`).
    pub async fn list_tools_with(&self, options: McpListToolsOptions) -> Result<Vec<McpTool>> {
        self.connect(false).await?;
        let Some(peer) = self.connected_peer()? else {
            return Ok(Vec::new());
        };
        self.list_tools_on(&peer, options).await
    }

    async fn list_tools_on(
        &self,
        peer: &Peer<RoleClient>,
        options: McpListToolsOptions,
    ) -> Result<Vec<McpTool>> {
        let defaults = options.prefix.is_none() && options.tool_filters.is_none();
        let prefix = options
            .prefix
            .or_else(|| self.inner.options.prefix.clone())
            .unwrap_or_default();
        let filters = options
            .tool_filters
            .or_else(|| self.inner.options.tool_filters.clone());
        let mut tools = Vec::new();
        for server_tool in self.server_tools(peer).await? {
            let name = if prefix.is_empty() {
                server_tool.name.to_string()
            } else {
                let renamed = format!("{prefix}_{}", server_tool.name);
                tracing::debug!("tool_rename=<{}->{renamed}> | renamed tool", server_tool.name);
                renamed
            };
            let tool = McpTool::from_server_tool(name, &server_tool, self.clone());
            if !filters
                .as_ref()
                .is_none_or(|filters| filters.includes(&tool, &server_tool.name))
            {
                continue;
            }
            if !prefix.is_empty() && Tool::name(&tool).chars().count() > MAX_TOOL_NAME_LENGTH {
                self.overlong(&tool)?;
                continue;
            }
            tools.push(tool);
        }
        if defaults {
            *lock(&self.inner.registered) = tools.iter().map(|tool| Tool::name(tool).to_owned()).collect();
        }
        Ok(tools)
    }

    async fn server_tools(&self, peer: &Peer<RoleClient>) -> Result<Vec<rmcp::model::Tool>> {
        if self
            .inner
            .server_info()
            .is_some_and(|info| info.capabilities.tools.is_none())
        {
            return Ok(Vec::new());
        }
        let result = aggregate("tools/list", self.inner.list_max_pages(), |cursor| {
            let peer = peer.clone();
            async move {
                let request = ListToolsRequest::with_param(paginated(cursor));
                match request_with(&peer, ClientRequest::ListToolsRequest(request), default_options()).await?
                {
                    ServerResult::ListToolsResult(result) => Ok(result),
                    _ => Err(McpError::sdk("Unexpected response type")),
                }
            }
            .boxed()
        })
        .await?;
        Ok(result.tools)
    }

    /// An overlong prefixed name: an error, or a warning under `continue_on_error`.
    fn overlong(&self, tool: &McpTool) -> Result<()> {
        let name = Tool::name(tool);
        let server = self
            .server_version()
            .map_or_else(|| "unknown".to_owned(), |version| version.name);
        let message = format!(
            "server=<{server}>, tool=<{}>, name=<{name}>, length=<{}>, limit=<{MAX_TOOL_NAME_LENGTH}> | tool name exceeds registry limit | use a shorter prefix or tool name",
            tool.server_name(),
            name.chars().count()
        );
        if !self.inner.options.continue_on_error {
            return Err(ToolValidationError::new(message).into());
        }
        tracing::warn!("{message} | skipping tool (continueOnError)");
        Ok(())
    }

    /// Sets the callback a tools-list change notification triggers (TS `onToolsChanged`). An
    /// agent that takes this client as a tool source installs its own on `initialize`.
    pub fn set_on_tools_changed(&self, callback: Option<McpToolsChangedCallback>) {
        *lock(&self.inner.on_tools_changed) = callback;
    }

    /// Re-lists the tools with the client's prefix and filters and reports the change;
    /// notifications during a refresh coalesce into one more refresh (TS `_handleToolsChanged`).
    pub(crate) async fn refresh_tools(&self) {
        {
            let mut refresh = lock(&self.inner.refresh);
            if refresh.running {
                refresh.pending = true;
                return;
            }
            refresh.running = true;
            refresh.pending = false;
        }
        let mut running = RefreshRunning(Some(&self.inner.refresh));
        loop {
            let old = lock(&self.inner.registered).clone();
            // Only the live connection: a refresh never reconnects a disconnected client.
            let Some(peer) = self.inner.peer() else { return };
            let outcome = match self.list_tools_on(&peer, McpListToolsOptions::default()).await {
                Ok(tools) => match lock(&self.inner.on_tools_changed).clone() {
                    Some(callback) => catch_panic(|| callback(old, tools)),
                    None => Ok(()),
                },
                Err(error) => Err(error.to_string()),
            };
            if let Err(error) = outcome {
                tracing::warn!(
                    "client=<{}>, error=<{error}> | failed to refresh tools after toolsChanged notification",
                    self.inner.name
                );
                return;
            }
            if running.finish_unless_pending() {
                return;
            }
        }
    }

    /// Debounces tools-list change notifications (the MCP SDK `listChanged.tools.debounceMs`).
    pub(crate) fn tools_changed(&self) {
        let ticket = self
            .inner
            .tools_changed
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        let client = self.clone();
        tokio::spawn(
            async move {
                tokio::time::sleep(TOOLS_CHANGED_DEBOUNCE).await;
                if client.inner.tools_changed.load(Ordering::SeqCst) == ticket {
                    client.refresh_tools().await;
                }
            }
            .in_current_span(),
        );
    }

    /// The server's prompts (TS `listPrompts`).
    ///
    /// # Errors
    ///
    /// The connection failed earlier under `continue_on_error` (call `connect(true)`), or the
    /// request failed, or pagination passed `list_max_pages`.
    pub async fn list_prompts(&self, options: McpListOptions) -> Result<ListPromptsResult> {
        let peer = self.connected_or_throw().await?;
        Ok(self
            .list_paged(
                "prompts/list",
                &peer,
                options,
                |capabilities| capabilities.prompts.is_some(),
                |params| {
                    ClientRequest::ListPromptsRequest(rmcp::model::ListPromptsRequest::with_param(params))
                },
            )
            .await?)
    }

    /// Renders a prompt (TS `getPrompt`).
    ///
    /// # Errors
    ///
    /// As [`list_prompts`](Self::list_prompts).
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<IndexMap<String, String>>,
    ) -> Result<GetPromptResult> {
        let peer = self.connected_or_throw().await?;
        let mut params = GetPromptRequestParams::new(name.to_owned());
        params.arguments = arguments.map(|arguments| {
            arguments
                .into_iter()
                .map(|(key, value)| (key, JsonValue::String(value)))
                .collect()
        });
        let request = ClientRequest::GetPromptRequest(rmcp::model::GetPromptRequest::new(params));
        match request_with(&peer, request, default_options()).await? {
            ServerResult::GetPromptResult(result) => Ok(result),
            _ => Err(McpError::sdk("Unexpected response type").into()),
        }
    }

    /// The server's resources (TS `listResources`).
    ///
    /// # Errors
    ///
    /// As [`list_prompts`](Self::list_prompts).
    pub async fn list_resources(&self, options: McpListOptions) -> Result<ListResourcesResult> {
        let peer = self.connected_or_throw().await?;
        Ok(self
            .list_paged(
                "resources/list",
                &peer,
                options,
                |capabilities| capabilities.resources.is_some(),
                |params| {
                    ClientRequest::ListResourcesRequest(rmcp::model::ListResourcesRequest::with_param(params))
                },
            )
            .await?)
    }

    /// The server's resource templates (TS `listResourceTemplates`).
    ///
    /// # Errors
    ///
    /// As [`list_prompts`](Self::list_prompts).
    pub async fn list_resource_templates(
        &self,
        options: McpListOptions,
    ) -> Result<ListResourceTemplatesResult> {
        let peer = self.connected_or_throw().await?;
        Ok(self
            .list_paged(
                "resources/templates/list",
                &peer,
                options,
                |capabilities| capabilities.resources.is_some(),
                |params| {
                    ClientRequest::ListResourceTemplatesRequest(
                        rmcp::model::ListResourceTemplatesRequest::with_param(params),
                    )
                },
            )
            .await?)
    }

    /// Reads a resource (TS `readResource`).
    ///
    /// # Errors
    ///
    /// As [`list_prompts`](Self::list_prompts).
    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        let peer = self.connected_or_throw().await?;
        let params = ReadResourceRequestParams::new(uri.to_owned());
        let request = ClientRequest::ReadResourceRequest(rmcp::model::ReadResourceRequest::new(params));
        match request_with(&peer, request, default_options()).await? {
            ServerResult::ReadResourceResult(result) => Ok(result),
            _ => Err(McpError::sdk("Unexpected response type").into()),
        }
    }

    async fn list_paged<R>(
        &self,
        method: &'static str,
        peer: &Peer<RoleClient>,
        options: McpListOptions,
        supported: fn(&ServerCapabilities) -> bool,
        build: fn(PaginatedRequestParams) -> ClientRequest,
    ) -> Result<R, McpError>
    where
        R: Paged + Default + Send + 'static,
    {
        // The MCP SDK answers an empty list when the server lacks the capability.
        if self
            .inner
            .server_info()
            .is_some_and(|info| !supported(&info.capabilities))
        {
            return Ok(R::default());
        }
        let fetch = |cursor: Option<String>| {
            let peer = peer.clone();
            async move {
                let response = request_with(&peer, build(paginated(cursor)), default_options()).await?;
                R::from_result(response).ok_or_else(|| McpError::sdk("Unexpected response type"))
            }
            .boxed()
        };
        match options.pagination_token {
            Some(token) => fetch(Some(token)).await,
            None => aggregate(method, self.inner.list_max_pages(), fetch).await,
        }
    }

    async fn connected_or_throw(&self) -> Result<Peer<RoleClient>> {
        self.connect(false).await?;
        self.connected_peer()?
            .ok_or_else(|| Error::other(FAILED_TO_CONNECT))
    }

    /// Calls `tool` with `args` and returns the raw `CallToolResult` JSON.
    ///
    /// # Errors
    ///
    /// As [`call_tool_with`](Self::call_tool_with).
    pub async fn call_tool(&self, tool: &McpTool, args: JsonValue) -> Result<JsonValue> {
        self.call_tool_with(tool, args, McpCallToolOptions::default())
            .await
    }

    /// [`call_tool`](Self::call_tool) with per-call options.
    ///
    /// # Errors
    ///
    /// The timeout is zero or too large, the arguments aren't a JSON object, the client failed
    /// to connect, the request failed or was cancelled.
    pub async fn call_tool_with(
        &self,
        tool: &McpTool,
        args: JsonValue,
        options: McpCallToolOptions,
    ) -> Result<JsonValue> {
        if let Some(timeout) = options.timeout {
            assert_positive_duration(timeout, "MCP call timeout")?;
        }
        let signal = options.cancel_signal.as_ref();
        self.connect_with(false, signal).await?;
        let Some(peer) = self.connected_peer()? else {
            return Err(Error::other(FAILED_TO_CONNECT));
        };
        let arguments = prepare_arguments(args)?;
        let output_schema = compile_output_schema(tool)?;
        let arguments = if self.inner.options.disable_mcp_instrumentation {
            arguments
        } else {
            inject_trace_context(arguments, (self.inner.trace_context)())
        };
        let mut params = CallToolRequestParams::new(tool.server_name().to_owned());
        params.arguments = Some(arguments);
        let progress = options
            .progress_callback
            .clone()
            .or_else(|| self.inner.options.progress_callback.clone());
        let started = tokio::time::Instant::now();
        let first = call_options(options.timeout, options.timeout);
        let mut response = self
            .call_once(&peer, call_request(&params), first, &options, progress.as_ref())
            .await?;
        let mut round: u32 = 0;
        loop {
            let ServerResult::InputRequiredResult(input) = response else {
                let result = call_result(response)?;
                check_output(tool.server_name(), output_schema.as_ref(), &result)?;
                return Ok(result);
            };
            if input.input_requests.as_ref().is_none_or(InputRequests::is_empty)
                && input.request_state.is_none()
            {
                return Err(McpError::sdk(INPUT_REQUIRED_WITHOUT_INPUT).into());
            }
            round = round.saturating_add(1);
            if round > MRTR_MAX_ROUNDS {
                return Err(McpError::sdk(format!(
                    "Multi-round-trip request 'tools/call' still required input after {MRTR_MAX_ROUNDS} rounds (inputRequired.maxRounds)"
                ))
                .into());
            }
            if let Some(callback) = &progress {
                let message = format!("Fulfilling input required by 'tools/call' (round {round})");
                self.inner
                    .deliver_progress(callback, McpProgress::new(f64::from(round), None, Some(message)));
            }
            let (responses, state) = self.fulfill_inputs(input, options.cancel_signal.as_ref()).await?;
            let remaining = match options.timeout {
                None => None,
                Some(timeout) => {
                    let remaining = timeout.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        return Err(McpError::sdk("Maximum total timeout exceeded").into());
                    }
                    Some(remaining)
                }
            };
            params.input_responses = responses;
            params.request_state = state;
            let leg = call_options(options.timeout, remaining);
            response = self
                .call_once(&peer, call_request(&params), leg, &options, progress.as_ref())
                .await?;
        }
    }

    /// One `tools/call` round with its progress route and cancellation.
    async fn call_once(
        &self,
        peer: &Peer<RoleClient>,
        request: ClientRequest,
        request_options: PeerRequestOptions,
        options: &McpCallToolOptions,
        progress: Option<&McpProgressCallback>,
    ) -> Result<ServerResult> {
        if let Some(signal) = options
            .cancel_signal
            .as_ref()
            .filter(|signal| signal.is_cancelled())
        {
            return Err(abort_error(signal));
        }
        let handle = peer.send_request_with_option(request, request_options).await?;
        let id = handle.id.clone();
        let token = handle.progress_token.clone();
        if let Some(callback) = progress {
            self.inner.register_progress(token.clone(), Arc::clone(callback));
        }
        let response = match &options.cancel_signal {
            None => handle.await_response().await,
            Some(signal) => {
                tokio::select! {
                    response = handle.await_response() => response,
                    () = signal.cancelled() => {
                        lock(&self.inner.progress).routes.remove(&token);
                        notify_cancelled(peer, id, signal).await;
                        return Err(abort_error(signal));
                    }
                }
            }
        };
        lock(&self.inner.progress).routes.remove(&token);
        Ok(response?)
    }

    /// Answers a 2026-07-28 `input_required` result's input requests for the next round (the MCP
    /// SDK `runInputRequiredDriver` round): the elicitations run concurrently under a signal the
    /// call's `cancel` cancels, and a `requestState`-only round is paced first.
    async fn fulfill_inputs(
        &self,
        input: InputRequiredResult,
        cancel: Option<&CancelSignal>,
    ) -> Result<(Option<InputResponses>, Option<String>)> {
        let requests = input.input_requests.unwrap_or_default();
        if requests.is_empty() {
            pause(REQUEST_STATE_ONLY_LEG_PACING, cancel).await?;
            return Ok((None, input.request_state));
        }
        let mut pending = Vec::new();
        for (key, request) in requests {
            let callback = self.inner.options.elicitation_callback.clone();
            match (request, callback) {
                (InputRequest::Elicitation(request), Some(callback)) => {
                    pending.push((key, callback, request.params));
                }
                (request, _) => return Err(no_input_handler(&key, &request).into()),
            }
        }
        let (signal, handle) = CancelSignal::new();
        let answers = futures::future::try_join_all(pending.into_iter().map(|(key, callback, params)| {
            let answer = callback(ElicitationContext::new(signal.clone()), params);
            async move {
                let result = answer.await.map_err(Error::other)?;
                Ok::<_, Error>((key, serde_json::to_value(result)?))
            }
        }));
        let answers = match cancel {
            None => answers.await,
            Some(cancel) => tokio::select! {
                answers = answers => answers,
                () = cancel.cancelled() => Err(abort_error(cancel)),
            },
        };
        let reason = cancel.and_then(CancelSignal::reason);
        let responses = answers.inspect_err(|_| {
            handle.cancel(CancelOptions {
                message: reason,
                ..Default::default()
            });
        })?;
        Ok((Some(responses.into_iter().collect()), input.request_state))
    }

    #[cfg(test)]
    pub(crate) fn inner_capabilities(&self) -> ClientCapabilities {
        self.inner.capabilities()
    }

    fn connected_peer(&self) -> Result<Option<Peer<RoleClient>>> {
        match &*self.inner.state() {
            Conn::Connected(running) => Ok(Some(running.peer().clone())),
            Conn::Failed => Ok(None),
            Conn::Disconnected => Err(McpError::disconnected().into()),
        }
    }
}

impl fmt::Debug for McpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClient")
            .field("client_name", &self.inner.name)
            .field("transport", &self.inner.transport)
            .field("connection_state", &self.connection_state())
            .finish_non_exhaustive()
    }
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, Conn> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn connecting(&self) -> MutexGuard<'_, Option<(u64, SharedConnect)>> {
        self.connecting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn assert_current(&self, generation: u64) -> Result<(), McpError> {
        if self.generation() == generation {
            Ok(())
        } else {
            Err(McpError::disconnected())
        }
    }

    pub(crate) fn peer(&self) -> Option<Peer<RoleClient>> {
        match &*self.state() {
            Conn::Connected(running) => Some(running.peer().clone()),
            Conn::Disconnected | Conn::Failed => None,
        }
    }

    fn server_info(&self) -> Option<Arc<rmcp::model::ServerPeerInfo>> {
        match &*self.state() {
            Conn::Connected(running) => running.peer_info(),
            Conn::Disconnected | Conn::Failed => None,
        }
    }

    pub(crate) fn list_max_pages(&self) -> u32 {
        self.options.list_max_pages.unwrap_or(DEFAULT_LIST_MAX_PAGES)
    }

    fn pending_connect(&self) -> Option<SharedConnect> {
        self.connecting().as_ref().map(|(_, pending)| pending.clone())
    }

    /// Starts a shared connection attempt. It runs on its own task, so it completes even when
    /// every waiter stops waiting, and clears itself from `connecting` when it settles.
    fn start_connect(inner: &Arc<Self>, reconnect: bool) -> SharedConnect {
        let mut connecting = inner.connecting();
        let id = inner.connect_ids.fetch_add(1, Ordering::SeqCst);
        let generation = inner.generation();
        let task_inner = Arc::clone(inner);
        let task = tokio::spawn(
            async move {
                let result = establish(&task_inner, reconnect, generation).await;
                let mut connecting = task_inner.connecting();
                if connecting.as_ref().is_some_and(|(current, _)| *current == id) {
                    *connecting = None;
                }
                result
            }
            .in_current_span(),
        );
        let pending = async move {
            task.await
                .unwrap_or_else(|error| Err(McpError::sdk(error.to_string())))
        }
        .boxed()
        .shared();
        *connecting = Some((id, pending.clone()));
        pending
    }

    /// Stores `conn` if no `disconnect` ran since `generation`, under the same lock `disconnect`
    /// bumps the generation in; otherwise hands `conn` back.
    fn settle(&self, generation: u64, conn: Conn) -> Option<Conn> {
        let mut state = self.state();
        if self.generation() != generation {
            return Some(conn);
        }
        *state = conn;
        None
    }

    /// Delivers one progress notification to its call's callback. Delivery happens under the
    /// routes lock, so held and live notifications keep arrival order.
    pub(crate) fn route_progress(&self, token: &ProgressToken, progress: McpProgress) {
        let mut routes = lock(&self.progress);
        match routes.routes.get(token).cloned() {
            Some(callback) => self.deliver_progress(&callback, progress),
            None => routes.hold(token.clone(), progress),
        }
    }

    fn register_progress(&self, token: ProgressToken, callback: McpProgressCallback) {
        let mut routes = lock(&self.progress);
        for progress in routes.take_early(&token) {
            self.deliver_progress(&callback, progress);
        }
        routes.routes.insert(token, callback);
    }

    fn deliver_progress(&self, callback: &McpProgressCallback, progress: McpProgress) {
        if let Err(error) = catch_panic(|| callback(progress)) {
            tracing::warn!(
                "client=<{}>, error=<{error}> | progress callback failed",
                self.name
            );
        }
    }

    fn capabilities(&self) -> ClientCapabilities {
        let mut capabilities = ClientCapabilities::default();
        if self.options.elicitation_callback.is_some() {
            let mut elicitation = ElicitationCapability::default();
            elicitation.form = Some(FormElicitationCapability::default());
            elicitation.url = Some(UrlElicitationCapability::default());
            capabilities.elicitation = Some(elicitation);
        }
        capabilities
    }
}

/// Connects for the attempt started at `generation`; a `disconnect` since then wins.
async fn establish(inner: &Arc<Inner>, reconnect: bool, generation: u64) -> Result<(), McpError> {
    if reconnect {
        let previous = {
            let mut state = inner.state();
            match std::mem::replace(&mut *state, Conn::Disconnected) {
                Conn::Connected(running) => Some(running),
                other => {
                    *state = other;
                    None
                }
            }
        };
        if let Some(running) = previous {
            let _closed = running.cancel().await;
            inner.assert_current(generation)?;
        }
    }
    let handler = McpHandler::new(
        Arc::downgrade(inner),
        &inner.name,
        &inner.version,
        inner.capabilities(),
    );
    let attempt = tokio::time::timeout(
        McpClient::DEFAULT_REQUEST_TIMEOUT,
        inner.transport.connect(handler, lifecycle()),
    )
    .await
    .unwrap_or_else(|_| Err(McpError::sdk("Request timed out")));
    match attempt {
        Ok(running) => {
            let Some(stale) = inner.settle(generation, Conn::Connected(running)) else {
                return Ok(());
            };
            if let Conn::Connected(running) = stale {
                let _closed = running.cancel().await;
            }
            Err(McpError::disconnected())
        }
        Err(error) => {
            if !inner.options.continue_on_error {
                return Err(error);
            }
            if inner.settle(generation, Conn::Failed).is_some() {
                return Err(McpError::disconnected());
            }
            tracing::warn!(
                "client=<{}>, error=<{}> | MCP server failed to connect, continuing (continueOnError)",
                inner.name,
                error
            );
            Ok(())
        }
    }
}

/// Probe for the 2026-07-28 revision and fall back to the `initialize` handshake (TS
/// `versionNegotiation: { mode: 'auto' }`).
fn lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    }
}

const URL_ONLY: &str = "McpClientConfig: \"auth\", \"authProvider\", and \"headers\" require \"url\" (not compatible with \"transport\")";

/// TS `_resolveTransport`: exactly one of `transport` and `url`; auth and headers need `url`.
fn resolve_transport(config: McpClientConfig) -> Result<McpTransport> {
    let McpClientConfig {
        transport,
        url,
        auth,
        auth_provider,
        headers,
        ..
    } = config;
    match (transport, url) {
        (Some(_), Some(_)) => Err(Error::other(
            "McpClientConfig: provide either \"transport\" or \"url\", not both",
        )),
        (None, None) => Err(Error::other(
            "McpClientConfig: either \"transport\" or \"url\" must be provided",
        )),
        (Some(transport), None) => {
            if auth.is_some() || auth_provider.is_some() || headers.is_some() {
                return Err(Error::other(URL_ONLY));
            }
            Ok(transport)
        }
        (None, Some(url)) => {
            if auth.is_some() && auth_provider.is_some() {
                return Err(Error::other(
                    "McpClientConfig: provide either \"auth\" or \"authProvider\", not both",
                ));
            }
            streamable_http(&url, auth, auth_provider, headers.unwrap_or_default())
        }
    }
}

fn streamable_http(
    url: &str,
    auth: Option<McpClientCredentials>,
    auth_provider: Option<McpAuthProvider>,
    headers: IndexMap<String, String>,
) -> Result<McpTransport> {
    reqwest::Url::parse(url).map_err(|_| Error::other("Invalid URL"))?;
    let authorized = auth.is_some() || auth_provider.is_some();
    // With auth, the token is the only `Authorization` header, as in the MCP SDK.
    let custom_headers = header_map(headers)?
        .into_iter()
        .filter_map(|(name, value)| Some((name?, value)))
        .filter(|(name, _)| !(authorized && *name == http::header::AUTHORIZATION))
        .collect();
    let config = StreamableHttpClientTransportConfig::with_uri(url).custom_headers(custom_headers);
    if let Some(provider) = auth_provider {
        return Ok(McpTransport::from_factory(move || {
            StreamableHttpClientTransport::with_client(provider.clone(), config.clone())
        }));
    }
    let Some(credentials) = auth else {
        return Ok(McpTransport::from_factory(move || {
            StreamableHttpClientTransport::from_config(config.clone())
        }));
    };
    let url = url.to_owned();
    Ok(McpTransport::from_connect(
        "streamable-http",
        Arc::new(move |handler, lifecycle| {
            let (url, credentials, config) = (url.clone(), credentials.clone(), config.clone());
            async move {
                let provider = client_credentials(&url, credentials.clone())
                    .await
                    .map_err(|error| McpError::sdk(error.to_string()))?;
                let client = ClientCredentialsClient::new(&url, credentials, provider);
                let transport = StreamableHttpClientTransport::with_client(client, config);
                serve(handler, transport, lifecycle).await
            }
            .boxed()
        }),
    ))
}

/// Walks every page of a list method, as the MCP SDK `_listAllPages` does: a page repeating the
/// previous cursor and items ends the walk, and passing `max_pages` (unless `0`) is an error.
pub(crate) async fn aggregate<R: Paged>(
    method: &str,
    max_pages: u32,
    fetch: impl Fn(Option<String>) -> BoxFuture<'static, Result<R, McpError>>,
) -> Result<R, McpError> {
    let mut acc = fetch(None).await?;
    let mut cursor = acc.next_cursor().take();
    let mut previous = serde_json::to_value(acc.items()).ok();
    let mut pages: u32 = 1;
    while let Some(current) = cursor {
        if max_pages != 0 && pages >= max_pages {
            return Err(McpError::sdk(format!(
                "{method}: exceeded listMaxPages ({max_pages}); server pagination did not terminate"
            )));
        }
        let mut page = fetch(Some(current.clone())).await?;
        let next = page.next_cursor().take();
        let items = serde_json::to_value(page.items()).ok();
        if next.as_ref() == Some(&current) && items == previous {
            break;
        }
        acc.items().append(page.items());
        previous = items;
        cursor = next;
        pages = pages.saturating_add(1);
    }
    Ok(acc)
}

/// A paginated MCP list result.
pub(crate) trait Paged: Sized {
    type Item: serde::Serialize;
    fn from_result(result: ServerResult) -> Option<Self>;
    fn items(&mut self) -> &mut Vec<Self::Item>;
    fn next_cursor(&mut self) -> &mut Option<String>;
}

macro_rules! paged {
    ($($result:ident => $items:ident: $item:ty),* $(,)?) => {$(
        impl Paged for rmcp::model::$result {
            type Item = $item;
            fn from_result(result: ServerResult) -> Option<Self> {
                match result {
                    ServerResult::$result(result) => Some(result),
                    _ => None,
                }
            }
            fn items(&mut self) -> &mut Vec<$item> {
                &mut self.$items
            }
            fn next_cursor(&mut self) -> &mut Option<String> {
                &mut self.next_cursor
            }
        }
    )*};
}

paged!(
    ListToolsResult => tools: rmcp::model::Tool,
    ListPromptsResult => prompts: rmcp::model::Prompt,
    ListResourcesResult => resources: rmcp::model::Resource,
    ListResourceTemplatesResult => resource_templates: rmcp::model::ResourceTemplate,
);

pub(crate) fn paginated(cursor: Option<String>) -> PaginatedRequestParams {
    let mut params = PaginatedRequestParams::default();
    params.cursor = cursor;
    params
}

pub(crate) fn default_options() -> PeerRequestOptions {
    let mut options = PeerRequestOptions::no_options();
    options.timeout = Some(McpClient::DEFAULT_REQUEST_TIMEOUT);
    options
}

/// The TS call options: a caller timeout bounds the whole call (`max_total`, shrinking across
/// input-required rounds) and resets on progress; otherwise the SDK default request timeout applies.
fn call_options(timeout: Option<Duration>, max_total: Option<Duration>) -> PeerRequestOptions {
    let Some(timeout) = timeout else {
        return default_options();
    };
    let mut options = PeerRequestOptions::no_options();
    options.timeout = Some(timeout);
    options.max_total_timeout = max_total.or(Some(timeout));
    options.reset_timeout_on_progress = true;
    options
}

fn call_request(params: &CallToolRequestParams) -> ClientRequest {
    ClientRequest::CallToolRequest(CallToolRequest::new(params.clone()))
}

/// The MCP SDK text for an input request this client has no handler for.
fn no_input_handler(key: &str, request: &InputRequest) -> McpError {
    let method = serde_json::to_value(request)
        .ok()
        .and_then(|request| request.get("method")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    McpError::sdk(format!(
        "Cannot fulfil input request '{key}': no handler is registered for '{method}' on this client. Declare the corresponding capability and register a handler, or handle input_required results manually."
    ))
}

/// Waits `delay`, or fails with the abort error when `cancel` fires first.
async fn pause(delay: Duration, cancel: Option<&CancelSignal>) -> Result<()> {
    let Some(cancel) = cancel else {
        tokio::time::sleep(delay).await;
        return Ok(());
    };
    tokio::select! {
        () = tokio::time::sleep(delay) => Ok(()),
        () = cancel.cancelled() => Err(abort_error(cancel)),
    }
}

pub(crate) async fn request_with(
    peer: &Peer<RoleClient>,
    request: ClientRequest,
    options: PeerRequestOptions,
) -> Result<ServerResult, McpError> {
    let handle = peer.send_request_with_option(request, options).await?;
    Ok(handle.await_response().await?)
}

fn call_result(response: ServerResult) -> Result<JsonValue> {
    match response {
        ServerResult::CallToolResult(result) => Ok(serde_json::to_value(result)?),
        ServerResult::CustomResult(result) => Ok(result.0),
        _ => Err(McpError::sdk("Unexpected response type").into()),
    }
}

/// The tool's listed `outputSchema`, compiled (the MCP SDK `callTool` output validator).
fn compile_output_schema(tool: &McpTool) -> Result<Option<jsonschema::Validator>, McpError> {
    let Some(schema) = tool.output_schema() else {
        return Ok(None);
    };
    jsonschema::validator_for(schema).map(Some).map_err(|error| {
        let message: String = error.to_string().chars().take(200).collect();
        McpError::protocol(
            ERROR_INVALID_PARAMS,
            format!(
                "Tool '{}' has an invalid outputSchema: {message}",
                tool.server_name()
            ),
            None,
        )
    })
}

/// The MCP SDK `callTool` check of a result against the tool's output schema; error results
/// are not checked.
fn check_output(
    name: &str,
    schema: Option<&jsonschema::Validator>,
    result: &JsonValue,
) -> Result<(), McpError> {
    let Some(schema) = schema else { return Ok(()) };
    if result.get("isError").and_then(JsonValue::as_bool) == Some(true) {
        return Ok(());
    }
    let Some(structured) = result.get("structuredContent") else {
        return Err(McpError::protocol(
            ERROR_INVALID_REQUEST,
            format!("Tool {name} has an output schema but did not return structured content"),
            None,
        ));
    };
    let errors: Vec<String> = schema
        .iter_errors(structured)
        .map(|error| format!("data{} {error}", error.instance_path()))
        .collect();
    if errors.is_empty() {
        return Ok(());
    }
    Err(McpError::protocol(
        ERROR_INVALID_PARAMS,
        format!(
            "Structured content does not match the tool's output schema: {}",
            errors.join(", ")
        ),
        None,
    ))
}

/// `null` becomes `{}`; anything but an object is rejected (TS `_prepareToolCall`).
fn prepare_arguments(args: JsonValue) -> Result<JsonObject> {
    match args {
        JsonValue::Null => Ok(JsonObject::new()),
        JsonValue::Object(object) => Ok(object),
        other => {
            let kind = match other {
                JsonValue::Array(_) => "Array",
                JsonValue::String(_) => "string",
                JsonValue::Number(_) => "number",
                _ => "boolean",
            };
            Err(Error::other(format!(
                "MCP Protocol Error: Tool arguments must be a JSON Object (named parameters). Received: {kind}"
            )))
        }
    }
}

fn assert_positive_duration(value: Duration, name: &str) -> Result<()> {
    if value.is_zero() || value > MAX_TIMER_DELAY {
        return Err(Error::other(format!(
            "{name} must be a positive safe integer no greater than {}",
            MAX_TIMER_DELAY.as_millis()
        )));
    }
    Ok(())
}

async fn race(pending: SharedConnect, signal: Option<&CancelSignal>) -> Result<()> {
    let Some(signal) = signal else {
        return Ok(pending.await?);
    };
    tokio::select! {
        result = pending => Ok(result?),
        () = signal.cancelled() => Err(abort_error(signal)),
    }
}

/// TS `abortReason`: the cancel message, or the DOM `AbortError` text.
fn abort_error(signal: &CancelSignal) -> Error {
    Error::other(
        signal
            .reason()
            .unwrap_or_else(|| "The operation was aborted".to_owned()),
    )
}

async fn notify_cancelled(peer: &Peer<RoleClient>, id: rmcp::model::RequestId, signal: &CancelSignal) {
    let params = rmcp::model::CancelledNotificationParam::new(Some(id), signal.reason());
    let _sent = peer.notify_cancelled(params).await;
}

/// Runs a user callback; a panic becomes its message.
fn catch_panic(callback: impl FnOnce()) -> Result<(), String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)).map_err(|panic| {
        panic
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_owned())
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
