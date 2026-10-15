//! The A2A HTTP server: an agent card, a JSON-RPC 2.0 endpoint and an SSE stream over `axum`.
//!
//! Mirrors `strands-ts/src/a2a/server.ts` and `express-server.ts`, with the request handling
//! `@a2a-js/sdk`'s `DefaultRequestHandler` performs implemented in-tree over [`TaskStore`].

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;

use super::executor::{A2AExecutor, EventBus, RequestContext};
use super::task_store::{TaskStore, DEFAULT_MAX_TASKS};
use super::types::{
    A2AEventData, AgentCard, AgentSkill, JsonRpcError, JsonRpcRequest, JsonRpcResponse, Message,
    MessageSendParams, Part, Role, Task, TaskIdParams, TaskQueryParams, TaskState, TaskStatus,
    TaskStatusUpdateEvent, AGENT_CARD_PATH, METHOD_MESSAGE_SEND, METHOD_MESSAGE_STREAM, METHOD_TASKS_CANCEL,
    METHOD_TASKS_GET, METHOD_TASKS_RESUBSCRIBE, PREV_AGENT_CARD_PATH,
};
use crate::agent::Agent;
use crate::{Error, Result};

/// Serves one Strands agent over the A2A protocol.
///
/// Build it with [`A2AServer::builder`], then either run it with [`A2AServer::serve`] or mount
/// [`A2AServer::router`] into a larger `axum` application.
///
/// # Example
///
/// ```no_run
/// # async fn example(agent: strands_agents::Agent) -> strands_agents::Result<()> {
/// use strands_agents::a2a::A2AServer;
///
/// let server = A2AServer::builder("My Agent")
///     .description("An agent that helps with tasks")
///     .agent(agent)
///     .port(9000)
///     .build()?;
/// let handle = server.serve().await?;
/// println!("listening on {}", handle.local_addr());
/// handle.shutdown().await;
/// # Ok(())
/// # }
/// ```
pub struct A2AServer {
    card: AgentCard,
    executor: Arc<A2AExecutor>,
    store: Arc<TaskStore>,
    mount_path: String,
    host: String,
    port: u16,
    listener: Option<TcpListener>,
}

impl A2AServer {
    /// A builder for a server advertising itself as `name`.
    pub fn builder(name: impl Into<String>) -> A2AServerBuilder {
        A2AServerBuilder {
            name: name.into(),
            description: None,
            http_url: None,
            version: None,
            skills: Vec::new(),
            card: None,
            serve_at_root: false,
            host: "127.0.0.1".to_owned(),
            port: 0,
            listener: None,
            source: None,
            store: None,
            max_tasks: DEFAULT_MAX_TASKS,
        }
    }

    /// The card this server serves at `/.well-known/agent-card.json`.
    pub fn agent_card(&self) -> &AgentCard {
        &self.card
    }

    /// The tasks this server has served.
    pub fn task_store(&self) -> &Arc<TaskStore> {
        &self.store
    }

    /// The routes of this server, for mounting into an existing `axum` application.
    ///
    /// Nested under the path of the configured `http_url` unless the server serves at root; the
    /// agent card is served at the root well-known paths either way, so a client can discover it.
    pub fn router(&self) -> Router {
        let state = Arc::new(ServerState {
            card: self.card.clone(),
            executor: Arc::clone(&self.executor),
            store: Arc::clone(&self.store),
            runs: Mutex::new(HashMap::new()),
        });
        let routes = Router::new()
            .route("/", post(json_rpc))
            .route(AGENT_CARD_PATH, get(agent_card))
            .route(PREV_AGENT_CARD_PATH, get(agent_card))
            .with_state(Arc::clone(&state));
        if self.mount_path.is_empty() {
            routes
        } else {
            // The card stays at the root well-known paths too, where a client looks for it first.
            let well_known = Router::new()
                .route(AGENT_CARD_PATH, get(agent_card))
                .route(PREV_AGENT_CARD_PATH, get(agent_card))
                .with_state(state);
            Router::new().nest(&self.mount_path, routes).merge(well_known)
        }
    }

    /// Binds and starts serving, returning a handle to the running server.
    ///
    /// When no `http_url` was configured, the agent card's URL is set to the bound address.
    pub async fn serve(mut self) -> Result<ServerHandle> {
        let listener = match self.listener.take() {
            Some(listener) => listener,
            None => TcpListener::bind((self.host.as_str(), self.port))
                .await
                .map_err(Error::other)?,
        };
        let addr = listener.local_addr().map_err(Error::other)?;
        if self.card.url.is_empty() {
            self.card.url = format!("http://{addr}");
        }
        let card = self.card.clone();
        let router = self.router();
        let (shutdown, signal) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = signal.await;
                })
                .await;
        });
        tracing::info!(%addr, "a2a server listening");
        Ok(ServerHandle {
            addr,
            card,
            shutdown: Some(shutdown),
            task,
        })
    }
}

/// Builder for an [`A2AServer`].
pub struct A2AServerBuilder {
    name: String,
    description: Option<String>,
    http_url: Option<String>,
    version: Option<String>,
    skills: Vec<AgentSkill>,
    card: Option<AgentCard>,
    serve_at_root: bool,
    host: String,
    port: u16,
    listener: Option<TcpListener>,
    source: Option<Source>,
    store: Option<Arc<TaskStore>>,
    max_tasks: usize,
}

enum Source {
    Agent(Agent),
    Executor(A2AExecutor),
}

impl A2AServerBuilder {
    /// Serve `agent`, sharing it across every A2A context (see [`A2AExecutor::new`]).
    ///
    /// Replaces any previously configured agent or executor.
    #[must_use]
    pub fn agent(mut self, agent: Agent) -> Self {
        self.source = Some(Source::Agent(agent));
        self
    }

    /// Serve through `executor`, which chooses how contexts map onto agents.
    ///
    /// Replaces any previously configured agent or executor.
    #[must_use]
    pub fn executor(mut self, executor: A2AExecutor) -> Self {
        self.source = Some(Source::Executor(executor));
        self
    }

    /// What the agent does, for the agent card (default: the agent's own description).
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The public URL to advertise; its path also becomes the mount path of the routes.
    #[must_use]
    pub fn http_url(mut self, http_url: impl Into<String>) -> Self {
        self.http_url = Some(http_url.into());
        self
    }

    /// The agent's own version string for the agent card (default: `0.0.1`).
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// The skills to advertise on the agent card.
    #[must_use]
    pub fn skills(mut self, skills: impl IntoIterator<Item = AgentSkill>) -> Self {
        self.skills = skills.into_iter().collect();
        self
    }

    /// The whole agent card, overriding every other card setting.
    #[must_use]
    pub fn agent_card(mut self, card: AgentCard) -> Self {
        self.card = Some(card);
        self
    }

    /// Serve at the root path even when `http_url` has one (for a prefix-stripping proxy).
    #[must_use]
    pub fn serve_at_root(mut self, serve_at_root: bool) -> Self {
        self.serve_at_root = serve_at_root;
        self
    }

    /// The interface to bind (default: `127.0.0.1`).
    #[must_use]
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// The port to bind; zero picks a free one (default: zero).
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Serve on an already-bound listener, instead of binding host and port.
    #[must_use]
    pub fn listener(mut self, listener: TcpListener) -> Self {
        self.listener = Some(listener);
        self
    }

    /// Serve through `store`, so a host can share or inspect the tasks (default: a fresh store).
    ///
    /// Takes precedence over [`A2AServerBuilder::max_tasks`].
    #[must_use]
    pub fn task_store(mut self, store: Arc<TaskStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Cap on tasks the default store retains (default: [`DEFAULT_MAX_TASKS`]).
    #[must_use]
    pub fn max_tasks(mut self, max_tasks: usize) -> Self {
        self.max_tasks = max_tasks;
        self
    }

    /// Build the server.
    ///
    /// Fails when no agent or executor was configured, or when a shared agent cannot be served
    /// (see [`A2AExecutor::new`]).
    pub fn build(self) -> Result<A2AServer> {
        let source = self.source.ok_or_else(|| {
            Error::other("A2AServer requires an agent or an executor: call .agent() or .executor()")
        })?;
        let (executor, agent_description) = match source {
            Source::Agent(agent) => {
                let description = agent.description().map(str::to_owned);
                (A2AExecutor::new(agent)?, description)
            }
            Source::Executor(executor) => (executor, None),
        };

        let http_url = self.http_url.unwrap_or_default();
        let card = self.card.unwrap_or_else(|| {
            let description = self.description.or(agent_description).unwrap_or_default();
            let mut card = AgentCard::new(self.name, description, http_url.clone());
            if let Some(version) = self.version {
                card.version = version;
            }
            card.skills = self.skills;
            card
        });

        Ok(A2AServer {
            card,
            executor: Arc::new(executor),
            store: self
                .store
                .unwrap_or_else(|| Arc::new(TaskStore::new().with_max_tasks(self.max_tasks))),
            mount_path: if self.serve_at_root {
                String::new()
            } else {
                mount_path(&http_url)
            },
            host: self.host,
            port: self.port,
            listener: self.listener,
        })
    }
}

/// A running [`A2AServer`].
pub struct ServerHandle {
    addr: SocketAddr,
    card: AgentCard,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    /// The address the server is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The card the server serves, with its URL resolved to the bound address.
    pub fn agent_card(&self) -> &AgentCard {
        &self.card
    }

    /// Stop serving, waiting for in-flight requests to finish.
    pub async fn shutdown(mut self) {
        drop(self.shutdown.take());
        let _ = self.task.await;
    }
}

/// What the HTTP handlers need to answer a request.
struct ServerState {
    card: AgentCard,
    executor: Arc<A2AExecutor>,
    store: Arc<TaskStore>,
    runs: Mutex<HashMap<String, Run>>,
}

/// A task executing right now: the bus its executor publishes to, and who is listening.
struct Run {
    bus: EventBus,
    subscribers: Subscribers,
}

/// The senders feeding every live subscriber of one run (its SSE response, any resubscription).
type Subscribers = Arc<Mutex<Vec<mpsc::UnboundedSender<A2AEventData>>>>;

/// The path component of `http_url`, without a trailing slash, as a mount path.
fn mount_path(http_url: &str) -> String {
    let after_scheme = http_url.split_once("://").map_or(http_url, |(_, rest)| rest);
    let path = after_scheme.find('/').map_or("", |index| &after_scheme[index..]);
    let trimmed = path.trim_end_matches('/');
    if trimmed == "/" {
        String::new()
    } else {
        trimmed.to_owned()
    }
}

async fn agent_card(State(state): State<Arc<ServerState>>) -> Json<AgentCard> {
    Json(state.card.clone())
}

async fn json_rpc(State(state): State<Arc<ServerState>>, body: Bytes) -> Response {
    let request: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return failure(None, JsonRpcError::parse_error(error.to_string()));
        }
    };
    if request.jsonrpc != "2.0" {
        return failure(
            request.id,
            JsonRpcError::invalid_request("jsonrpc must be \"2.0\""),
        );
    }

    let id = request.id.clone();
    let params = request.params;
    match request.method.as_str() {
        METHOD_MESSAGE_STREAM => match decode(params) {
            Ok(params) => stream(state, id, params),
            Err(error) => failure(id, error),
        },
        METHOD_MESSAGE_SEND => {
            let outcome = match decode(params) {
                Ok(params) => send(&state, params).await,
                Err(error) => Err(error),
            };
            respond(id, outcome)
        }
        METHOD_TASKS_GET => {
            let outcome = decode(params).and_then(|params| get_task(&state, params));
            respond(id, outcome)
        }
        METHOD_TASKS_CANCEL => {
            let outcome = match decode(params) {
                Ok(params) => cancel_task(&state, params).await,
                Err(error) => Err(error),
            };
            respond(id, outcome)
        }
        METHOD_TASKS_RESUBSCRIBE => match decode(params) {
            Ok(params) => resubscribe(&state, id, params),
            Err(error) => failure(id, error),
        },
        method => failure(id, JsonRpcError::method_not_found(method)),
    }
}

/// Run the agent to completion and answer with the task it produced.
///
/// An executor failure is published as a `failed` task and returned as a successful result, as
/// `DefaultRequestHandler.sendMessage` does; only request-level problems are JSON-RPC errors.
async fn send(
    state: &Arc<ServerState>,
    params: MessageSendParams,
) -> std::result::Result<JsonValue, JsonRpcError> {
    let context = request_context(state, params.message)?;
    let mut events = start_run(state, &context);

    // The run's events reach the store before they reach us, so the task is final once it ends.
    let mut answer = None;
    while let Some(event) = events.next().await {
        if let A2AEventData::Message(message) = event {
            answer = Some(message);
        }
    }

    if let Some(message) = answer {
        return encode(&message);
    }
    let task = state
        .store
        .get(&context.task_id)
        .ok_or_else(|| JsonRpcError::internal_error("the agent produced no task"))?;
    encode(&task)
}

/// Run the agent in the background and stream its events as SSE.
fn stream(state: Arc<ServerState>, id: Option<JsonValue>, params: MessageSendParams) -> Response {
    let context = match request_context(&state, params.message) {
        Ok(context) => context,
        Err(error) => return failure(id, error),
    };
    sse(id, start_run(&state, &context))
}

/// Replay the stored task, then follow its live run when one is still going.
///
/// Mirrors `DefaultRequestHandler.resubscribe`: an unknown task is `-32001`, a terminal one is
/// answered with its snapshot alone.
fn resubscribe(state: &Arc<ServerState>, id: Option<JsonValue>, params: TaskIdParams) -> Response {
    let Some(task) = state.store.get(&params.id) else {
        return failure(id, JsonRpcError::task_not_found(&params.id));
    };
    let live = state
        .runs
        .lock()
        .get(&params.id)
        .map(|run| subscribe(&run.subscribers));
    if live.is_none() && !task.status.state.is_terminal() {
        tracing::warn!(task_id = %params.id, "a2a resubscribe: the task has no live run");
    }

    let snapshot = futures::stream::once(async move { A2AEventData::Task(task) });
    match live {
        Some(live) => sse(id, snapshot.chain(live)),
        None => sse(id, snapshot),
    }
}

/// Execute `context` in the background, returning a subscription to its events.
///
/// The events are folded into the task store by a task of their own, so a client that disconnects
/// mid-run cannot leave the task stuck `working`.
fn start_run(state: &Arc<ServerState>, context: &RequestContext) -> UnboundedReceiverStream<A2AEventData> {
    let (bus, mut events) = EventBus::channel();
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));
    let subscription = subscribe(&subscribers);
    state.runs.lock().insert(
        context.task_id.clone(),
        Run {
            bus: bus.clone(),
            subscribers: Arc::clone(&subscribers),
        },
    );

    let executing = Arc::clone(state);
    let running = context.clone();
    tokio::spawn(async move {
        if let Err(error) = executing.executor.execute(&running, &bus).await {
            tracing::warn!(task_id = %running.task_id, %error, "an a2a execution failed");
            bus.publish(failed(&running, &to_json_rpc(error).message));
        }
        // Drop the registration, and with it this bus clone, so the fold below ends.
        executing.runs.lock().remove(&running.task_id);
    });

    let folding = Arc::clone(state);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            folding.store.apply(&event);
            subscribers
                .lock()
                .retain(|subscriber| subscriber.send(event.clone()).is_ok());
        }
    });
    subscription
}

/// Add one listener to a run's events.
fn subscribe(subscribers: &Subscribers) -> UnboundedReceiverStream<A2AEventData> {
    let (tx, rx) = mpsc::unbounded_channel();
    subscribers.lock().push(tx);
    UnboundedReceiverStream::new(rx)
}

/// The stored task, with its history truncated to the requested length.
fn get_task(state: &ServerState, params: TaskQueryParams) -> std::result::Result<JsonValue, JsonRpcError> {
    let mut task = state
        .store
        .get(&params.id)
        .ok_or_else(|| JsonRpcError::task_not_found(&params.id))?;
    if let Some(length) = params.history_length {
        truncate_history(&mut task, length as usize);
    }
    encode(&task)
}

/// Cancel a task: through the executor while it runs, in the store once it no longer does.
async fn cancel_task(
    state: &Arc<ServerState>,
    params: TaskIdParams,
) -> std::result::Result<JsonValue, JsonRpcError> {
    let task = state
        .store
        .get(&params.id)
        .ok_or_else(|| JsonRpcError::task_not_found(&params.id))?;
    if task.status.state.is_terminal() {
        return Err(JsonRpcError::task_not_cancelable(&params.id));
    }

    let running = state.runs.lock().get(&params.id).map(|run| run.bus.clone());
    match running {
        Some(bus) => {
            tracing::debug!(task_id = %params.id, "delegating an a2a cancellation to the executor");
            state
                .executor
                .cancel_task(&params.id, &bus)
                .await
                .map_err(to_json_rpc)?;
        }
        None => {
            tracing::debug!(task_id = %params.id, "canceling a stored a2a task");
            state.store.apply(&canceled(&task));
        }
    }

    let task = state
        .store
        .get(&params.id)
        .ok_or_else(|| JsonRpcError::task_not_found(&params.id))?;
    if task.status.state != TaskState::Canceled {
        return Err(JsonRpcError::task_not_cancelable(&params.id));
    }
    encode(&task)
}

/// The context of an incoming message: a fresh task, or the continuation of a stored one.
///
/// Mirrors `DefaultRequestHandler._createRequestContext`: a `taskId` this server does not know is
/// `-32001`, a task in a terminal state is `-32600`, and otherwise the message is appended to the
/// stored task's history and the task is passed to the executor as [`RequestContext::task`]. A
/// message naming no task starts a fresh one, recorded as `submitted`.
fn request_context(
    state: &ServerState,
    mut message: Message,
) -> std::result::Result<RequestContext, JsonRpcError> {
    let existing = match &message.task_id {
        Some(task_id) => {
            let task = state
                .store
                .get(task_id)
                .ok_or_else(|| JsonRpcError::task_not_found(task_id))?;
            if task.status.state.is_terminal() {
                return Err(JsonRpcError::invalid_request(format!(
                    "Task {task_id} is in a terminal state ({}) and cannot be modified.",
                    task.status.state
                )));
            }
            Some(task)
        }
        None => None,
    };

    let task_id = message
        .task_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let context_id = message
        .context_id
        .clone()
        .or_else(|| existing.as_ref().map(|task| task.context_id.clone()))
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    message.task_id = Some(task_id.clone());
    message.context_id = Some(context_id.clone());

    let context = RequestContext::new(task_id, context_id, message.clone());
    Ok(match existing {
        Some(task) => {
            let appended = state.store.append_history(&task.id, message).unwrap_or(task);
            context.with_task(appended)
        }
        None => {
            let mut task = Task::new(
                context.task_id.clone(),
                context.context_id.clone(),
                TaskStatus::new(TaskState::Submitted),
            );
            task.history = Some(vec![context.user_message.clone()]);
            state.store.save(task);
            context
        }
    })
}

/// A terminal `failed` status update carrying `message`.
fn failed(context: &RequestContext, message: &str) -> A2AEventData {
    let mut status = TaskStatus::new(TaskState::Failed);
    status.message = Some(Message::new(Role::Agent, vec![Part::text(message)]));
    A2AEventData::StatusUpdate(
        TaskStatusUpdateEvent::new(context.task_id.clone(), context.context_id.clone(), status)
            .with_final(true),
    )
}

/// A terminal `canceled` status update for a task no longer running (the SDK's wording).
fn canceled(task: &Task) -> A2AEventData {
    let mut status = TaskStatus::new(TaskState::Canceled);
    status.message = Some(Message::new(
        Role::Agent,
        vec![Part::text("Task cancellation requested by user.")],
    ));
    A2AEventData::StatusUpdate(
        TaskStatusUpdateEvent::new(task.id.clone(), task.context_id.clone(), status).with_final(true),
    )
}

/// One JSON-RPC response per event, as an SSE body.
fn sse<S>(id: Option<JsonValue>, events: S) -> Response
where
    S: Stream<Item = A2AEventData> + Send + 'static,
{
    let body = events.map(move |event| {
        let response = encode(&event)
            .map(|result| JsonRpcResponse::success(id.clone(), result))
            .unwrap_or_else(|error| JsonRpcResponse::error(id.clone(), error));
        Ok::<Event, Infallible>(
            Event::default().data(serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_owned())),
        )
    });
    Sse::new(body).into_response()
}

/// Keep only the last `length` history messages.
fn truncate_history(task: &mut Task, length: usize) {
    if let Some(history) = &mut task.history {
        let excess = history.len().saturating_sub(length);
        history.drain(..excess);
    }
}

fn decode<T: DeserializeOwned>(params: Option<JsonValue>) -> std::result::Result<T, JsonRpcError> {
    serde_json::from_value(params.unwrap_or(JsonValue::Null))
        .map_err(|error| JsonRpcError::invalid_params(error.to_string()))
}

fn encode<T: serde::Serialize>(value: &T) -> std::result::Result<JsonValue, JsonRpcError> {
    serde_json::to_value(value).map_err(|error| JsonRpcError::internal_error(error.to_string()))
}

/// The JSON-RPC error an agent or executor failure maps to.
fn to_json_rpc(error: Error) -> JsonRpcError {
    match &error {
        Error::Other(inner) => inner.downcast_ref::<JsonRpcError>().cloned(),
        _ => None,
    }
    .unwrap_or_else(|| JsonRpcError::internal_error(error.to_string()))
}

fn respond(id: Option<JsonValue>, outcome: std::result::Result<JsonValue, JsonRpcError>) -> Response {
    match outcome {
        Ok(result) => Json(JsonRpcResponse::success(id, result)).into_response(),
        Err(error) => failure(id, error),
    }
}

fn failure(id: Option<JsonValue>, error: JsonRpcError) -> Response {
    Json(JsonRpcResponse::error(id, error)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::{error_codes, METHOD_TASKS_RESUBSCRIBE, PROTOCOL_VERSION};
    use crate::testing::{MockMessageModel, MockResponse};
    use crate::types::{ContentBlock, Message as StrandsMessage, Role as StrandsRole};
    use serde_json::json;

    fn agent_saying(text: &str) -> Agent {
        let model = MockMessageModel::shared(vec![MockResponse::message(StrandsMessage::new(
            StrandsRole::Assistant,
            vec![ContentBlock::text(text)],
        ))]);
        Agent::builder().model(model).build().unwrap()
    }

    fn server() -> A2AServer {
        A2AServer::builder("Test Agent")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .build()
            .unwrap()
    }

    async fn call(base: &str, method: &str, params: JsonValue) -> JsonValue {
        reqwest::Client::new()
            .post(base)
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    #[test]
    fn builds_agent_card_with_provided_values() {
        let server = A2AServer::builder("Base Agent")
            .agent(agent_saying("Hello"))
            .description("A base agent")
            .http_url("http://example.com")
            .version("2.0.0")
            .build()
            .unwrap();

        let card = server.agent_card();
        assert_eq!(card.name, "Base Agent");
        assert_eq!(card.description, "A base agent");
        assert_eq!(card.version, "2.0.0");
        assert_eq!(card.url, "http://example.com");
        assert_eq!(card.protocol_version, PROTOCOL_VERSION);
        assert_eq!(card.default_input_modes, vec!["text/plain".to_owned()]);
        assert_eq!(card.default_output_modes, vec!["text/plain".to_owned()]);
        assert_eq!(card.capabilities.streaming, Some(true));
        assert!(card.skills.is_empty());
    }

    #[test]
    fn uses_default_values_when_optional_config_is_omitted() {
        let card = server().agent_card().clone();
        assert_eq!(card.description, "");
        assert_eq!(card.version, "0.0.1");
        assert_eq!(card.url, "");
        assert!(card.skills.is_empty());
    }

    #[test]
    fn describes_itself_with_the_agents_own_description() {
        let model = MockMessageModel::shared(vec![]);
        let agent = Agent::builder()
            .model(model)
            .description("agent-provided")
            .build()
            .unwrap();
        let server = A2AServer::builder("Named").agent(agent).build().unwrap();
        assert_eq!(server.agent_card().description, "agent-provided");
    }

    #[test]
    fn advertises_the_configured_skills() {
        let server = A2AServer::builder("Skilled")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .skills([AgentSkill::new("search", "Search", "Finds things", ["search"])])
            .build()
            .unwrap();
        assert_eq!(server.agent_card().skills[0].id, "search");
    }

    #[test]
    fn an_explicit_agent_card_overrides_the_card_settings() {
        let server = A2AServer::builder("Ignored")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .version("9.9.9")
            .agent_card(AgentCard::new("Override", "Overridden", "http://card"))
            .build()
            .unwrap();
        assert_eq!(server.agent_card().name, "Override");
        assert_eq!(server.agent_card().version, "0.0.1");
    }

    #[test]
    fn requires_an_agent_or_an_executor() {
        assert!(A2AServer::builder("Nothing").build().is_err());
    }

    #[test]
    fn derives_the_mount_path_from_the_http_url() {
        assert_eq!(mount_path("http://example.com"), "");
        assert_eq!(mount_path("http://example.com/"), "");
        assert_eq!(mount_path("http://example.com/agents/mine"), "/agents/mine");
        assert_eq!(mount_path("http://example.com/agents/mine/"), "/agents/mine");
    }

    #[tokio::test]
    async fn serves_the_agent_card_at_both_well_known_paths() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        for path in [AGENT_CARD_PATH, PREV_AGENT_CARD_PATH] {
            let card: AgentCard = reqwest::get(format!("{base}{path}"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(card.name, "Test Agent");
            assert_eq!(card.url, base);
        }
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn message_send_returns_the_completed_task() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response = call(
            &base,
            METHOD_MESSAGE_SEND,
            json!({ "message": Message::user("hi") }),
        )
        .await;
        let task: Task = serde_json::from_value(response["result"].clone()).unwrap();

        assert_eq!(task.status.state, TaskState::Completed);
        let artifacts = task.artifacts.unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].parts[0].as_text(), Some("Hello"));
        assert_eq!(task.history.unwrap()[0].text(), "hi");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn message_send_continues_an_unfinished_task() {
        let server = A2AServer::builder("Test Agent")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .build()
            .unwrap();
        // A task the agent left waiting for more input, as a multi-turn executor would.
        let mut waiting = Task::new("task-1", "ctx-1", TaskStatus::new(TaskState::InputRequired));
        waiting.history = Some(vec![Message::user("first")]);
        server.task_store().save(waiting);
        let store = Arc::clone(server.task_store());
        let handle = server.serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let mut second = Message::user("second");
        second.task_id = Some("task-1".to_owned());
        let response = call(&base, METHOD_MESSAGE_SEND, json!({ "message": second })).await;

        let task: Task = serde_json::from_value(response["result"].clone()).unwrap();
        assert_eq!(task.id, "task-1");
        assert_eq!(task.context_id, "ctx-1");
        let history = task.history.unwrap();
        assert_eq!(
            history.iter().map(Message::text).collect::<Vec<_>>(),
            vec!["first".to_owned(), "second".to_owned()]
        );
        assert_eq!(store.get("task-1").unwrap().status.state, TaskState::Completed);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn message_send_reports_an_unknown_task_id() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let mut message = Message::user("hi");
        message.task_id = Some("never-seen".to_owned());
        let response = call(&base, METHOD_MESSAGE_SEND, json!({ "message": message })).await;

        assert_eq!(response["error"]["code"], error_codes::TASK_NOT_FOUND);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn message_send_refuses_to_modify_a_terminal_task() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let sent = call(
            &base,
            METHOD_MESSAGE_SEND,
            json!({ "message": Message::user("hi") }),
        )
        .await;
        let id = sent["result"]["id"].as_str().unwrap().to_owned();

        let mut second = Message::user("again");
        second.task_id = Some(id.clone());
        let response = call(&base, METHOD_MESSAGE_SEND, json!({ "message": second })).await;

        assert_eq!(response["error"]["code"], error_codes::INVALID_REQUEST);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("terminal state"));
        handle.shutdown().await;
    }

    fn state_with(store: Arc<TaskStore>) -> ServerState {
        ServerState {
            card: AgentCard::new("Test Agent", "", ""),
            executor: Arc::new(A2AExecutor::with_factory(|_| agent_saying("Hello"))),
            store,
            runs: Mutex::new(HashMap::new()),
        }
    }

    fn waiting_task(id: &str, context_id: &str) -> Task {
        let mut task = Task::new(id, context_id, TaskStatus::new(TaskState::InputRequired));
        task.history = Some(vec![Message::user("first")]);
        task
    }

    #[test]
    fn a_continued_request_appends_to_the_task_and_passes_it_through() {
        let state = state_with(Arc::new(TaskStore::new()));
        state.store.save(waiting_task("task-2", "ctx-2"));
        let mut message = Message::user("second");
        message.task_id = Some("task-2".to_owned());

        let context = request_context(&state, message).unwrap();

        assert_eq!(context.task.as_ref().unwrap().id, "task-2");
        assert_eq!(context.context_id, "ctx-2");
        assert_eq!(context.user_message.task_id.as_deref(), Some("task-2"));
        let history = state.store.get("task-2").unwrap().history.unwrap();
        assert_eq!(
            history.iter().map(Message::text).collect::<Vec<_>>(),
            vec!["first".to_owned(), "second".to_owned()]
        );
        assert_eq!(
            state.store.get("task-2").unwrap().status.state,
            TaskState::InputRequired,
            "continuing a task never resets its status"
        );
    }

    #[test]
    fn a_request_naming_no_task_records_a_fresh_submitted_one() {
        let state = state_with(Arc::new(TaskStore::new()));

        let context = request_context(&state, Message::user("hi")).unwrap();

        assert!(context.task.is_none());
        let task = state.store.get(&context.task_id).unwrap();
        assert_eq!(task.status.state, TaskState::Submitted);
        assert_eq!(task.history.unwrap()[0].text(), "hi");
    }

    #[tokio::test]
    async fn tasks_get_returns_a_previously_served_task() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let sent = call(
            &base,
            METHOD_MESSAGE_SEND,
            json!({ "message": Message::user("hi") }),
        )
        .await;
        let id = sent["result"]["id"].as_str().unwrap().to_owned();
        let got = call(&base, METHOD_TASKS_GET, json!({ "id": id })).await;

        assert_eq!(got["result"]["id"], JsonValue::String(id));
        assert_eq!(got["result"]["status"]["state"], "completed");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn tasks_get_truncates_the_history_to_the_requested_length() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let sent = call(
            &base,
            METHOD_MESSAGE_SEND,
            json!({ "message": Message::user("hi") }),
        )
        .await;
        let id = sent["result"]["id"].as_str().unwrap().to_owned();
        let got = call(&base, METHOD_TASKS_GET, json!({ "id": id, "historyLength": 0 })).await;

        assert_eq!(got["result"]["history"], json!([]));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn tasks_get_reports_an_unknown_task() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response = call(&base, METHOD_TASKS_GET, json!({ "id": "missing" })).await;

        assert_eq!(response["error"]["code"], error_codes::TASK_NOT_FOUND);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn tasks_cancel_reports_an_unknown_task() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response = call(&base, METHOD_TASKS_CANCEL, json!({ "id": "missing" })).await;

        assert_eq!(response["error"]["code"], error_codes::TASK_NOT_FOUND);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn tasks_cancel_reports_a_finished_task_as_not_cancelable() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let sent = call(
            &base,
            METHOD_MESSAGE_SEND,
            json!({ "message": Message::user("hi") }),
        )
        .await;
        let id = sent["result"]["id"].as_str().unwrap().to_owned();
        let response = call(&base, METHOD_TASKS_CANCEL, json!({ "id": id })).await;

        assert_eq!(response["error"]["code"], error_codes::TASK_NOT_CANCELABLE);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn message_stream_emits_the_task_artifacts_and_final_status_as_sse() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let body = reqwest::Client::new()
            .post(&base)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": "stream-1",
                "method": METHOD_MESSAGE_STREAM,
                "params": { "message": Message::user("hi") },
            }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();

        let events: Vec<JsonValue> = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect();

        assert!(events.len() >= 3);
        assert_eq!(events[0]["id"], "stream-1");
        assert_eq!(events[0]["result"]["kind"], "task");
        let last = events.last().unwrap();
        assert_eq!(last["result"]["kind"], "status-update");
        assert_eq!(last["result"]["status"]["state"], "completed");
        assert_eq!(last["result"]["final"], true);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn reports_an_unknown_method() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response = call(&base, "tasks/explode", json!({})).await;

        assert_eq!(response["error"]["code"], error_codes::METHOD_NOT_FOUND);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn reports_invalid_params() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response = call(&base, METHOD_MESSAGE_SEND, json!({ "nope": true })).await;

        assert_eq!(response["error"]["code"], error_codes::INVALID_PARAMS);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn reports_a_malformed_body_as_a_parse_error() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response: JsonValue = reqwest::Client::new()
            .post(&base)
            .header("content-type", "application/json")
            .body("{ not json")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(response["error"]["code"], error_codes::PARSE_ERROR);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn reports_a_wrong_protocol_version_as_an_invalid_request() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response: JsonValue = reqwest::Client::new()
            .post(&base)
            .json(&json!({ "jsonrpc": "1.0", "id": 1, "method": METHOD_TASKS_GET }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(response["error"]["code"], error_codes::INVALID_REQUEST);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn serves_under_the_path_of_the_http_url() {
        let server = A2AServer::builder("Mounted")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .http_url("http://127.0.0.1/agents/mine")
            .build()
            .unwrap();
        let handle = server.serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let mounted = reqwest::get(format!("{base}/agents/mine{AGENT_CARD_PATH}"))
            .await
            .unwrap();
        let at_root = reqwest::get(format!("{base}{AGENT_CARD_PATH}")).await.unwrap();
        let rpc_at_root = reqwest::Client::new()
            .post(&base)
            .json(&json!({}))
            .send()
            .await
            .unwrap();

        assert!(mounted.status().is_success());
        assert!(
            at_root.status().is_success(),
            "the card is discoverable at the root"
        );
        assert_eq!(
            rpc_at_root.status(),
            reqwest::StatusCode::NOT_FOUND,
            "only the card is served outside the mount path"
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn serve_at_root_ignores_the_http_url_path() {
        let server = A2AServer::builder("Mounted")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .http_url("http://127.0.0.1/agents/mine")
            .serve_at_root(true)
            .build()
            .unwrap();
        let handle = server.serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let at_root = reqwest::get(format!("{base}{AGENT_CARD_PATH}")).await.unwrap();

        assert!(at_root.status().is_success());
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn serves_on_a_provided_listener_and_stops_on_shutdown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = A2AServer::builder("Listening")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .listener(listener)
            .build()
            .unwrap();

        let handle = server.serve().await.unwrap();
        assert_eq!(handle.local_addr(), addr);
        assert_eq!(handle.agent_card().url, format!("http://{addr}"));
        handle.shutdown().await;

        assert!(reqwest::get(format!("http://{addr}{AGENT_CARD_PATH}"))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_client_disconnecting_mid_stream_still_lets_the_task_finish() {
        let server = A2AServer::builder("Test Agent")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .build()
            .unwrap();
        server.task_store().save(Task::new(
            "task-4",
            "ctx-4",
            TaskStatus::new(TaskState::Submitted),
        ));
        let store = Arc::clone(server.task_store());
        let handle = server.serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let mut message = Message::user("hi");
        message.task_id = Some("task-4".to_owned());
        let response = reqwest::Client::new()
            .post(&base)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": "stream-2",
                "method": METHOD_MESSAGE_STREAM,
                "params": { "message": message },
            }))
            .send()
            .await
            .unwrap();
        // Hang up without reading the body.
        drop(response);

        let finished = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if store
                    .get("task-4")
                    .is_some_and(|task| task.status.state.is_terminal())
                {
                    return store.get("task-4").unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the task converged to a terminal state");

        assert_eq!(finished.status.state, TaskState::Completed);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn message_send_returns_a_failed_agent_run_as_a_successful_result() {
        let server = A2AServer::builder("Failing")
            .executor(A2AExecutor::with_factory(|_| {
                let model = MockMessageModel::shared(vec![MockResponse::Error(Error::other("boom"))]);
                Agent::builder().model(model).build().unwrap()
            }))
            .build()
            .unwrap();
        let handle = server.serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response = call(
            &base,
            METHOD_MESSAGE_SEND,
            json!({ "message": Message::user("hi") }),
        )
        .await;

        assert!(response["error"].is_null(), "{response}");
        let task: Task = serde_json::from_value(response["result"].clone()).unwrap();
        assert_eq!(task.status.state, TaskState::Failed);
        assert!(task.status.message.unwrap().text().contains("boom"));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn tasks_cancel_cancels_a_stored_task_that_is_not_running() {
        let server = server();
        server.task_store().save(Task::new(
            "task-5",
            "ctx-5",
            TaskStatus::new(TaskState::Submitted),
        ));
        let handle = server.serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let response = call(&base, METHOD_TASKS_CANCEL, json!({ "id": "task-5" })).await;

        assert!(response["error"].is_null(), "{response}");
        let task: Task = serde_json::from_value(response["result"].clone()).unwrap();
        assert_eq!(task.status.state, TaskState::Canceled);
        assert!(task.status.message.unwrap().text().contains("cancellation"));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn tasks_resubscribe_replays_a_finished_task_and_reports_an_unknown_one() {
        let handle = server().serve().await.unwrap();
        let base = format!("http://{}", handle.local_addr());

        let sent = call(
            &base,
            METHOD_MESSAGE_SEND,
            json!({ "message": Message::user("hi") }),
        )
        .await;
        let id = sent["result"]["id"].as_str().unwrap().to_owned();

        let body = reqwest::Client::new()
            .post(&base)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": "resub-1",
                "method": METHOD_TASKS_RESUBSCRIBE,
                "params": { "id": id },
            }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let events: Vec<JsonValue> = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect();

        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["result"]["kind"], "task");
        assert_eq!(events[0]["result"]["status"]["state"], "completed");

        let unknown = call(&base, METHOD_TASKS_RESUBSCRIBE, json!({ "id": "nope" })).await;
        assert_eq!(unknown["error"]["code"], error_codes::TASK_NOT_FOUND);
        handle.shutdown().await;
    }

    #[test]
    fn the_task_store_can_be_shared_with_the_host() {
        let store = Arc::new(TaskStore::new().with_max_tasks(5));
        let server = A2AServer::builder("Shared store")
            .executor(A2AExecutor::with_factory(|_| agent_saying("Hello")))
            .task_store(Arc::clone(&store))
            .build()
            .unwrap();
        store.save(Task::new(
            "task-6",
            "ctx-6",
            TaskStatus::new(TaskState::Submitted),
        ));
        assert!(server.task_store().get("task-6").is_some());
    }
}
