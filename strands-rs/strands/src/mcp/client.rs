//! `McpClient` (TS `mcp/client.ts`).

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use indexmap::IndexMap;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientRequest, Implementation,
    ListToolsRequest, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerResult,
};
use rmcp::service::{ClientLifecycleMode, PeerRequestOptions};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{Peer, RoleClient};
use tracing::Instrument;

use super::error::McpError;
use super::handler::McpHandler;
use super::mcp_tool::McpTool;
use super::transport::{McpTransport, Running};
use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::types::json::{JsonObject, JsonValue};

/// The client name sent in the MCP handshake when `application_name` is unset.
pub const DEFAULT_CLIENT_NAME: &str = "strands-agents-rs-sdk";
/// The MCP SDK default page cap for the no-token list methods.
const DEFAULT_LIST_MAX_PAGES: u32 = 64;
/// TS `MAX_TIMER_DELAY_MS`.
const MAX_TIMER_DELAY: Duration = Duration::from_millis(2_147_483_647);
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

/// Behavioural options shared by every client configuration (TS `McpClientOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct McpClientOptions {
    /// The client name sent in the handshake; defaults to `strands-agents-rs-sdk`.
    pub application_name: Option<String>,
    /// The client version sent in the handshake; defaults to this crate's version.
    pub application_version: Option<String>,
    /// Skips injecting W3C trace context into tool-call arguments.
    #[builder(default)]
    pub disable_mcp_instrumentation: bool,
    /// When `true`, connection failures and overlong prefixed names during tool listing are
    /// skipped with warnings.
    #[builder(default)]
    pub continue_on_error: bool,
    /// Page cap for the no-token list methods, which aggregate every page; `0` disables the cap.
    /// Defaults to 64.
    pub list_max_pages: Option<u32>,
}

/// Everything [`McpClient::new`] takes (TS `McpClientConfig`): a transport or a url, and the
/// shared options.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct McpClientConfig {
    /// A transport recipe. Mutually exclusive with `url`.
    pub transport: Option<McpTransport>,
    /// A server url; the client builds a Streamable HTTP transport for it.
    pub url: Option<String>,
    /// HTTP headers sent with every request. Requires `url`.
    pub headers: Option<IndexMap<String, String>>,
    /// The shared options.
    #[builder(default)]
    pub options: McpClientOptions,
}

/// Per-call options for [`McpClient::call_tool_with`] (TS `McpCallToolOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct McpCallToolOptions {
    /// Cancels the in-flight request.
    pub cancel_signal: Option<CancelSignal>,
    /// Overall time limit for this call; progress notifications reset the idle timer. Without
    /// it, the default request timeout applies.
    pub timeout: Option<Duration>,
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
        let transport = resolve_transport(config.transport, config.url.as_deref(), config.headers)?;
        let options = config.options;
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
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        *self.inner.connecting() = None;
        let previous = std::mem::replace(&mut *self.inner.state(), Conn::Disconnected);
        if let Conn::Connected(running) = previous {
            let _closed = running.cancel().await;
        }
        Ok(())
    }

    /// The server's tools as [`McpTool`]s.
    ///
    /// # Errors
    ///
    /// Connecting or listing failed.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        self.connect(false).await?;
        let Some(peer) = self.connected_peer()? else {
            return Ok(Vec::new());
        };
        if self
            .inner
            .server_info()
            .is_some_and(|info| info.capabilities.tools.is_none())
        {
            return Ok(Vec::new());
        }
        let max_pages = self.inner.list_max_pages();
        let result = aggregate("tools/list", max_pages, |cursor| {
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
        Ok(result
            .tools
            .into_iter()
            .map(|tool| McpTool::from_server_tool(tool.name.to_string(), &tool, self.clone()))
            .collect())
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
        let mut params = CallToolRequestParams::new(tool.server_name().to_owned());
        params.arguments = Some(arguments);
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let handle = peer
            .send_request_with_option(request, call_options(options.timeout))
            .await?;
        let id = handle.id.clone();
        let response = match signal {
            None => handle.await_response().await,
            Some(signal) => {
                tokio::select! {
                    response = handle.await_response() => response,
                    () = signal.cancelled() => {
                        notify_cancelled(&peer, id, signal).await;
                        return Err(abort_error(signal));
                    }
                }
            }
        };
        call_result(response?)
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
        let task_inner = Arc::clone(inner);
        let task = tokio::spawn(
            async move {
                let result = establish(&task_inner, reconnect).await;
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

    fn capabilities(&self) -> ClientCapabilities {
        ClientCapabilities::default()
    }
}

async fn establish(inner: &Arc<Inner>, reconnect: bool) -> Result<(), McpError> {
    let generation = inner.generation();
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
            if inner.generation() != generation {
                let _closed = running.cancel().await;
                return Err(McpError::disconnected());
            }
            *inner.state() = Conn::Connected(running);
            Ok(())
        }
        Err(error) => {
            if !inner.options.continue_on_error {
                return Err(error);
            }
            *inner.state() = Conn::Failed;
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

fn resolve_transport(
    transport: Option<McpTransport>,
    url: Option<&str>,
    headers: Option<IndexMap<String, String>>,
) -> Result<McpTransport> {
    match (transport, url) {
        (Some(_), Some(_)) => Err(Error::other(
            "McpClientConfig: provide either \"transport\" or \"url\", not both",
        )),
        (None, None) => Err(Error::other(
            "McpClientConfig: either \"transport\" or \"url\" must be provided",
        )),
        (Some(transport), None) => {
            if headers.is_some() {
                return Err(Error::other(
                    "McpClientConfig: \"auth\", \"authProvider\", and \"headers\" require \"url\" (not compatible with \"transport\")",
                ));
            }
            Ok(transport)
        }
        (None, Some(url)) => streamable_http(url, headers.unwrap_or_default()),
    }
}

fn streamable_http(url: &str, headers: IndexMap<String, String>) -> Result<McpTransport> {
    reqwest::Url::parse(url).map_err(|_| Error::other("Invalid URL"))?;
    let mut custom_headers = std::collections::HashMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::try_from(name.as_str())
            .map_err(|error| Error::other(format!("header=<{name}> | invalid header name: {error}")))?;
        let value = http::HeaderValue::try_from(value.as_str())
            .map_err(|error| Error::other(format!("header=<{name}> | invalid header value: {error}")))?;
        custom_headers.insert(name, value);
    }
    let config = StreamableHttpClientTransportConfig::with_uri(url).custom_headers(custom_headers);
    Ok(McpTransport::from_factory(move || {
        StreamableHttpClientTransport::from_config(config.clone())
    }))
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
pub(crate) trait Paged {
    type Item: serde::Serialize;
    fn items(&mut self) -> &mut Vec<Self::Item>;
    fn next_cursor(&mut self) -> &mut Option<String>;
}

macro_rules! paged {
    ($($result:ty => $items:ident: $item:ty),* $(,)?) => {$(
        impl Paged for $result {
            type Item = $item;
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
    rmcp::model::ListToolsResult => tools: rmcp::model::Tool,
    rmcp::model::ListPromptsResult => prompts: rmcp::model::Prompt,
    rmcp::model::ListResourcesResult => resources: rmcp::model::Resource,
    rmcp::model::ListResourceTemplatesResult => resource_templates: rmcp::model::ResourceTemplate,
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

/// The TS call options: a caller timeout bounds the whole call and resets on progress;
/// otherwise the SDK default request timeout applies.
fn call_options(timeout: Option<Duration>) -> PeerRequestOptions {
    let Some(timeout) = timeout else {
        return default_options();
    };
    let mut options = PeerRequestOptions::no_options();
    options.timeout = Some(timeout);
    options.max_total_timeout = Some(timeout);
    options.reset_timeout_on_progress = true;
    options
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
