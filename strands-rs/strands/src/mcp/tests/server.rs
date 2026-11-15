//! An in-process `rmcp` test server, served over an in-memory pipe or localhost HTTP.
#![allow(deprecated, reason = "the test server exercises MCP logging")]

use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock as Content, ElicitRequestParams,
    GetPromptRequestParams, GetPromptResult, Implementation, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, LoggingLevel, LoggingMessageNotificationParam,
    PaginatedRequestParams, ProgressNotificationParam, Prompt, PromptMessage, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ResourceTemplate, Role,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RunningService};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;

use crate::mcp::{McpClient, McpClientConfig, McpClientOptions, McpTransport};
use crate::types::json::JsonObject;

/// What the server exposes and how it behaves.
#[derive(Clone)]
pub(crate) struct TestServer {
    pub(crate) tools: Arc<Mutex<Vec<Tool>>>,
    /// Tools per `tools/list` page; `None` puts all of them on one page.
    pub(crate) page_size: Option<usize>,
    /// Answer every page with the same cursor (a pagination loop).
    pub(crate) endless_pages: bool,
    /// Answer `server/discover` with method-not-found, as a 2025-era server does.
    pub(crate) legacy_only: bool,
    pub(crate) instructions: Option<String>,
    pub(crate) list_calls: Arc<AtomicUsize>,
    pub(crate) calls: Arc<Mutex<Vec<CallToolRequestParams>>>,
}

pub(crate) fn tool(name: &str, description: Option<&str>) -> Tool {
    let schema = json!({"type": "object", "properties": {"message": {"type": "string"}}});
    let mut tool = Tool::default();
    tool.name = Cow::Owned(name.to_owned());
    tool.description = description.map(|description| Cow::Owned(description.to_owned()));
    tool.input_schema = Arc::new(object(schema));
    tool
}

pub(crate) fn object(value: serde_json::Value) -> JsonObject {
    match value {
        serde_json::Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

impl Default for TestServer {
    fn default() -> Self {
        let names = [
            "echo",
            "echo_args",
            "add",
            "error_tool",
            "slow",
            "progress",
            "elicit",
            "log",
            "change_tools",
        ];
        Self {
            tools: Arc::new(Mutex::new(
                names.iter().map(|name| tool(name, Some("A test tool"))).collect(),
            )),
            page_size: None,
            endless_pages: false,
            legacy_only: false,
            instructions: Some("Use the echo tool.".to_owned()),
            list_calls: Arc::default(),
            calls: Arc::default(),
        }
    }
}

impl TestServer {
    pub(crate) fn with_tools(tools: Vec<Tool>) -> Self {
        Self {
            tools: Arc::new(Mutex::new(tools)),
            ..Self::default()
        }
    }

    /// Serves one connection over an in-memory pipe and returns the client end.
    pub(crate) fn pipe(&self) -> tokio::io::DuplexStream {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let handler = self.clone();
        tokio::spawn(async move {
            if let Ok(running) = handler.serve(server).await {
                let _done: Result<_, _> = RunningService::waiting(running).await;
            }
        });
        client
    }

    pub(crate) fn transport(&self) -> McpTransport {
        let server = self.clone();
        McpTransport::from_factory(move || server.pipe())
    }

    pub(crate) fn client(&self) -> McpClient {
        self.client_with(McpClientOptions::default())
    }

    pub(crate) fn client_with(&self, options: McpClientOptions) -> McpClient {
        McpClient::new(McpClientConfig {
            transport: Some(self.transport()),
            options,
            ..Default::default()
        })
        .expect("valid config")
    }

    fn page(&self, cursor: Option<&str>) -> ListToolsResult {
        let tools = self.tools.lock().expect("tools").clone();
        let mut result = ListToolsResult::default();
        if self.endless_pages {
            result.tools = tools;
            result.next_cursor = Some("again".to_owned());
            return result;
        }
        let Some(size) = self.page_size else {
            result.tools = tools;
            return result;
        };
        let start: usize = cursor.and_then(|cursor| cursor.parse().ok()).unwrap_or(0);
        let end = (start + size).min(tools.len());
        result.tools = tools.get(start..end).map(<[Tool]>::to_vec).unwrap_or_default();
        if end < tools.len() {
            result.next_cursor = Some(end.to_string());
        }
        result
    }
}

/// A 2026-07-28 `input_required` round: ask for confirmation, then echo the answer.
fn input_required(request: &CallToolRequestParams) -> CallToolResponse {
    if let Some(answer) = request
        .input_responses
        .as_ref()
        .and_then(|responses| responses.get("confirm"))
    {
        return text(answer.to_string()).into();
    }
    let elicit: rmcp::model::ElicitRequest = serde_json::from_value(json!({
        "method": "elicitation/create",
        "params": {"mode": "form", "message": "Confirm?", "requestedSchema": {"type": "object", "properties": {}}}
    }))
    .expect("elicit request");
    let requests = [(
        "confirm".to_owned(),
        rmcp::model::InputRequest::Elicitation(elicit),
    )]
    .into_iter()
    .collect();
    rmcp::model::InputRequiredResult::new(Some(requests), Some("state-1".to_owned())).into()
}

fn text(value: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![Content::text(value)])
}

fn argument(request: &CallToolRequestParams, key: &str) -> serde_json::Value {
    request
        .arguments
        .as_ref()
        .and_then(|arguments| arguments.get(key).cloned())
        .unwrap_or_default()
}

async fn run_tool(
    request: &CallToolRequestParams,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    match request.name.as_ref() {
        "echo" => Ok(text(argument(request, "message").as_str().unwrap_or_default())),
        "echo_args" => Ok(text(
            serde_json::to_string(&request.arguments).unwrap_or_default(),
        )),
        "add" => {
            let sum =
                argument(request, "a").as_i64().unwrap_or(0) + argument(request, "b").as_i64().unwrap_or(0);
            Ok(text(sum.to_string()))
        }
        "error_tool" => Ok(CallToolResult::error(vec![Content::text(
            "This tool always fails",
        )])),
        "slow" => {
            context.ct.cancelled().await;
            Ok(text("cancelled"))
        }
        "progress" => {
            let token = context.meta.get_progress_token();
            for step in 1..=3 {
                if let Some(token) = &token {
                    let mut params = ProgressNotificationParam::new(token.clone(), f64::from(step));
                    params.total = Some(3.0);
                    params.message = Some(format!("step {step}"));
                    let _sent = context.peer.notify_progress(params).await;
                }
            }
            Ok(text("done"))
        }
        "elicit" | "elicit_url" => {
            let params = if request.name == "elicit" {
                json!({
                    "mode": "form",
                    "message": "Confirm?",
                    "requestedSchema": {"type": "object", "properties": {"confirmed": {"type": "boolean"}}}
                })
            } else {
                json!({
                    "mode": "url",
                    "message": "Please authenticate",
                    "url": "https://example.com/auth",
                    "elicitationId": "elicit-123"
                })
            };
            let params: ElicitRequestParams = serde_json::from_value(params)
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            let result = context
                .peer
                .create_elicitation(params)
                .await
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            Ok(text(serde_json::to_string(&result).unwrap_or_default()))
        }
        "log" => {
            let params = LoggingMessageNotificationParam::new(LoggingLevel::Warning, json!({"note": "hi"}))
                .with_logger("srv");
            let _sent = context.peer.notify_logging_message(params).await;
            Ok(text("logged"))
        }
        "change_tools" => {
            let _sent = context.peer.notify_tool_list_changed().await;
            Ok(text("changed"))
        }
        "url_elicitation" => Err(ErrorData::new(
            rmcp::model::ErrorCode(-32042),
            "URL elicitation required",
            Some(
                json!({"elicitations": [{"mode": "url", "url": "https://example.com/auth", "elicitationId": "e1", "message": "Authorize"}]}),
            ),
        )),
        other => Err(ErrorData::invalid_params(format!("unknown tool {other}"), None)),
    }
}

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_prompts()
            .enable_resources()
            .enable_logging()
            .build();
        let mut info = ServerConfig::new(capabilities);
        let mut implementation = Implementation::default();
        implementation.name = "test-server".to_owned();
        implementation.version = "1.2.3".to_owned();
        info.server_info = implementation;
        info.instructions = self.instructions.clone();
        info
    }

    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::DiscoverResult, ErrorData> {
        if self.legacy_only {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "Method not found",
                None,
            ));
        }
        Ok(rmcp::model::DiscoverResult::from_server_info(
            ProtocolVersion::KNOWN_VERSIONS.to_vec(),
            self.get_info(),
        ))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        if self
            .tools
            .lock()
            .expect("tools")
            .iter()
            .any(|tool| tool.name == "fail_list")
        {
            return Err(ErrorData::internal_error("server disconnected", None));
        }
        let cursor = request.and_then(|request| request.cursor);
        Ok(self.page(cursor.as_deref()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.calls.lock().expect("calls").push(request.clone());
        if request.name == "elicit_mrtr" {
            return Ok(input_required(&request));
        }
        run_tool(&request, context).await.map(CallToolResponse::from)
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let cursor = request.and_then(|request| request.cursor);
        let mut result = ListPromptsResult::default();
        match cursor.as_deref() {
            None => {
                result.prompts = vec![Prompt::new("greeting", Some("Greets"), None)];
                result.next_cursor = Some("p2".to_owned());
            }
            Some(_) => result.prompts = vec![Prompt::new("farewell", Some("Says bye"), None)],
        }
        Ok(result)
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::GetPromptResponse, ErrorData> {
        let name = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("name"))
            .and_then(|name| name.as_str().map(str::to_owned))
            .unwrap_or_else(|| "world".to_owned());
        let message = PromptMessage::new_text(Role::User, format!("{}: hello {name}", request.name));
        Ok(rmcp::model::GetPromptResponse::Complete(GetPromptResult::new(
            vec![message],
        )))
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let cursor = request.and_then(|request| request.cursor);
        let mut result = ListResourcesResult::default();
        let resource = match cursor.as_deref() {
            None => {
                result.next_cursor = Some("r2".to_owned());
                Resource::new("test://static", "static")
            }
            Some(_) => Resource::new("test://second", "second"),
        };
        result.resources = vec![resource];
        Ok(result)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let mut result = ListResourceTemplatesResult::default();
        let template = ResourceTemplate::new("test://users/{id}", "user");
        result.resource_templates = vec![template];
        Ok(result)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        let contents = ResourceContents::text(format!("contents of {}", request.uri), request.uri);
        Ok(rmcp::model::ReadResourceResponse::Complete(
            ReadResourceResult::new(vec![contents]),
        ))
    }
}

/// Waits (in virtual or real time) until `condition` holds, polling every millisecond.
pub(crate) async fn eventually(mut condition: impl FnMut() -> bool) {
    for _ in 0..5_000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("condition never held");
}

/// A server whose handshakes wait for [`Gate::open`] or fail on [`Gate::fail`], counting the
/// connection attempts.
#[derive(Clone)]
pub(crate) struct Gate {
    pub(crate) attempts: Arc<AtomicUsize>,
    permits: Arc<tokio::sync::Semaphore>,
    failures: Arc<AtomicUsize>,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            attempts: Arc::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(0)),
            failures: Arc::default(),
        }
    }
}

impl Gate {
    pub(crate) fn open(&self) {
        self.permits.add_permits(1);
    }

    pub(crate) fn fail(&self) {
        self.failures.fetch_add(1, Ordering::SeqCst);
        self.permits.add_permits(1);
    }

    pub(crate) fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    pub(crate) fn transport(&self, server: TestServer) -> McpTransport {
        let gate = self.clone();
        McpTransport::from_factory(move || {
            gate.attempts.fetch_add(1, Ordering::SeqCst);
            let (client, stream) = tokio::io::duplex(64 * 1024);
            let gate = gate.clone();
            let server = server.clone();
            tokio::spawn(async move {
                let Ok(permit) = gate.permits.acquire().await else {
                    return;
                };
                permit.forget();
                let failed = gate
                    .failures
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok();
                if failed {
                    drop(stream);
                    return;
                }
                if let Ok(running) = server.serve(stream).await {
                    let _done: Result<_, _> = RunningService::waiting(running).await;
                }
            });
            client
        })
    }
}

/// Serves `server` over Streamable HTTP on a localhost port; returns the `/mcp` url. The headers
/// of every request are recorded in `seen`.
pub(crate) async fn serve_http(server: TestServer, seen: Arc<Mutex<Vec<http::HeaderMap>>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    serve_http_on(listener, server, seen, Arc::default())
}

/// [`serve_http`] on `listener`, plus an OAuth token endpoint at `/token` that records each form
/// body in `tokens`; other `/.well-known` paths answer 404.
pub(crate) fn serve_http_on(
    listener: tokio::net::TcpListener,
    server: TestServer,
    seen: Arc<Mutex<Vec<http::HeaderMap>>>,
    tokens: Arc<Mutex<Vec<String>>>,
) -> String {
    use http_body_util::{BodyExt, Full};
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let service = hyper_util::service::TowerToHyperService::new(service.clone());
            let (seen, tokens) = (Arc::clone(&seen), Arc::clone(&tokens));
            let handler = hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                seen.lock().expect("headers").push(request.headers().clone());
                let (service, tokens) = (service.clone(), Arc::clone(&tokens));
                async move {
                    let path = request.uri().path().to_owned();
                    if path == "/token" {
                        let body = request.into_body().collect().await.map(|body| body.to_bytes());
                        let body = String::from_utf8_lossy(&body.unwrap_or_default()).into_owned();
                        tokens.lock().expect("tokens").push(body);
                        let token = r#"{"access_token":"tok-123","token_type":"Bearer","expires_in":3600}"#;
                        return Ok(http::Response::builder()
                            .header(http::header::CONTENT_TYPE, "application/json")
                            .body(Full::new(hyper::body::Bytes::from_static(token.as_bytes())).boxed())
                            .expect("response"));
                    }
                    if path.starts_with("/.well-known") {
                        return Ok(http::Response::builder()
                            .status(404)
                            .body(Full::new(hyper::body::Bytes::new()).boxed())
                            .expect("response"));
                    }
                    hyper::service::Service::call(&service, request).await
                }
            });
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let _served = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, handler)
                    .await;
            });
        }
    });
    format!("http://{address}/mcp")
}

type SseBody = http_body_util::combinators::BoxBody<hyper::body::Bytes, std::convert::Infallible>;
type Sessions = Arc<
    Mutex<
        std::collections::HashMap<
            String,
            futures::channel::mpsc::Sender<rmcp::service::RxJsonRpcMessage<RoleServer>>,
        >,
    >,
>;

/// Serves `server` over the legacy HTTP+SSE transport (`GET /sse`, `POST /messages`); returns
/// the `/sse` url. `endpoint` overrides the advertised POST url.
pub(crate) async fn serve_sse(
    server: TestServer,
    endpoint: Option<String>,
    seen: Arc<Mutex<Vec<http::HeaderMap>>>,
) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let sessions: Sessions = Arc::default();
    let ids = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (server, endpoint, seen, sessions, ids) = (
                server.clone(),
                endpoint.clone(),
                Arc::clone(&seen),
                Arc::clone(&sessions),
                Arc::clone(&ids),
            );
            let handler = hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                seen.lock().expect("headers").push(request.headers().clone());
                sse_route(
                    request,
                    server.clone(),
                    endpoint.clone(),
                    Arc::clone(&sessions),
                    Arc::clone(&ids),
                )
            });
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let _served = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, handler)
                    .await;
            });
        }
    });
    format!("http://{address}/sse")
}

async fn sse_route(
    request: http::Request<hyper::body::Incoming>,
    server: TestServer,
    endpoint: Option<String>,
    sessions: Sessions,
    ids: Arc<AtomicUsize>,
) -> Result<http::Response<SseBody>, std::convert::Infallible> {
    use futures::{SinkExt, StreamExt};
    use http_body_util::{BodyExt, Full, StreamBody};

    let full = |status: u16, text: &'static str| {
        http::Response::builder()
            .status(status)
            .body(Full::new(hyper::body::Bytes::from_static(text.as_bytes())).boxed())
            .expect("response")
    };
    if request.method() == http::Method::GET && request.uri().path() == "/sse" {
        let id = ids.fetch_add(1, Ordering::SeqCst).to_string();
        let (to_server, from_client) = futures::channel::mpsc::channel(16);
        let (to_client, from_server) =
            futures::channel::mpsc::channel::<rmcp::service::TxJsonRpcMessage<RoleServer>>(16);
        sessions.lock().expect("sessions").insert(id.clone(), to_server);
        tokio::spawn(async move {
            if let Ok(running) = server.serve((to_client, from_client)).await {
                let _done: Result<_, _> = RunningService::waiting(running).await;
            }
        });
        let endpoint = endpoint.unwrap_or_else(|| format!("/messages?sessionId={id}"));
        let first = futures::stream::once(async move { format!("event: endpoint\ndata: {endpoint}\n\n") });
        let rest = from_server.map(|message| {
            format!(
                "event: message\ndata: {}\n\n",
                serde_json::to_string(&message).unwrap_or_default()
            )
        });
        let frames = first.chain(rest).map(|text| {
            Ok::<_, std::convert::Infallible>(hyper::body::Frame::data(hyper::body::Bytes::from(text)))
        });
        let response = http::Response::builder()
            .header("content-type", "text/event-stream")
            .body(BodyExt::boxed(StreamBody::new(frames)))
            .expect("response");
        return Ok(response);
    }
    if request.method() == http::Method::POST && request.uri().path() == "/messages" {
        let id = request
            .uri()
            .query()
            .and_then(|query| query.strip_prefix("sessionId="))
            .unwrap_or_default()
            .to_owned();
        let Some(mut sender) = sessions.lock().expect("sessions").get(&id).cloned() else {
            return Ok(full(404, "unknown session"));
        };
        let Ok(body) = request.into_body().collect().await else {
            return Ok(full(400, "bad body"));
        };
        let Ok(message) = serde_json::from_slice(&body.to_bytes()) else {
            return Ok(full(400, "bad json"));
        };
        let _sent = sender.send(message).await;
        return Ok(full(202, "Accepted"));
    }
    Ok(full(404, "not found"))
}
