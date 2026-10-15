//! [`A2AAgent`]: a remote A2A agent used where a local [`Agent`](crate::agent::Agent) is
//! (`strands-ts/src/a2a/a2a-agent.ts`).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use serde_json::json;

use super::events::{A2AResultEvent, A2AStreamEvent, A2AStreamUpdateEvent};
use super::types::{
    A2AEventData, AgentCard, JsonRpcError, JsonRpcRequest, Message as A2AMessage, MessageSendParams,
    METHOD_MESSAGE_STREAM,
};
use crate::agent::{AgentAsToolOptions, AgentResult, InvocationState, InvokeInput, InvokeOptions};
use crate::error::{Error, Result};
use crate::models::sse::SseParser;
use crate::tools::{create_error_result, Tool, ToolContext, ToolRef};
use crate::types::{
    ContentBlock, JsonValue, Message, Role, StopReason, ToolResultBlock, ToolResultContent, ToolSpec,
};

/// Default path of the agent card, relative to the agent's base URL.
pub const DEFAULT_AGENT_CARD_PATH: &str = "/.well-known/agent-card.json";

/// A remote A2A agent, invocable like a local one.
///
/// Fetches the remote agent's card on first use (for its name, description and RPC endpoint),
/// sends the input as one A2A `message/stream` request, and assembles the streamed protocol events
/// into an [`AgentResult`]. Each invocation starts a fresh remote conversation unless
/// [`A2AAgentBuilder::continue_context`] asks otherwise. Cheap to clone: clones share one HTTP
/// client, card and conversation context.
#[derive(Clone, Debug)]
pub struct A2AAgent {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    url: String,
    agent_card_path: String,
    http: reqwest::Client,
    timeout: Option<Duration>,
    id: String,
    name: OnceLock<String>,
    description: OnceLock<String>,
    card: tokio::sync::OnceCell<AgentCard>,
    continue_context: bool,
    context_id: Mutex<Option<String>>,
}

impl A2AAgent {
    /// Start building an agent that talks to the A2A endpoint at `url`.
    pub fn builder(url: impl Into<String>) -> A2AAgentBuilder {
        A2AAgentBuilder {
            url: url.into(),
            agent_card_path: None,
            http: None,
            timeout: None,
            id: None,
            name: None,
            description: None,
            card: None,
            continue_context: false,
            context_id: None,
        }
    }

    /// The remote agent's base URL.
    pub fn url(&self) -> &str {
        &self.inner.url
    }

    /// Unique identifier of this agent; defaults to its URL.
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// The agent's name: the configured override, else the card's name once connected, else the
    /// URL (TS leaves it `undefined` until the card arrives; this crate's name is never absent).
    pub fn name(&self) -> &str {
        self.inner.name.get().map_or(&self.inner.url, String::as_str)
    }

    /// What the agent does: the configured override, else the card's description once connected.
    pub fn description(&self) -> Option<&str> {
        self.inner.description.get().map(String::as_str)
    }

    /// The A2A `contextId` this agent is continuing, when context continuity is on.
    ///
    /// Always `None` unless [`A2AAgentBuilder::continue_context`] or
    /// [`A2AAgentBuilder::context_id`] asked for it.
    pub fn context_id(&self) -> Option<String> {
        self.inner.context_id.lock().clone()
    }

    /// Forget the remote `contextId`, so the next invocation starts a fresh conversation.
    pub fn reset_context(&self) {
        *self.inner.context_id.lock() = None;
    }

    /// The remote agent's card, fetched and cached on first call.
    pub async fn agent_card(&self) -> Result<&AgentCard> {
        let inner = &self.inner;
        let card = inner
            .card
            .get_or_try_init(|| async {
                let url = format!("{}{}", inner.url, inner.agent_card_path);
                let response = inner
                    .request(inner.http.get(url))
                    .send()
                    .await
                    .map_err(|error| remote_error("fetching the A2A agent card failed", error))?
                    .error_for_status()
                    .map_err(|error| remote_error("the A2A agent card request failed", error))?;
                response
                    .json::<AgentCard>()
                    .await
                    .map_err(|error| remote_error("the A2A agent card could not be parsed", error))
            })
            .await?;
        let _ = inner.name.set(card.name.clone());
        if !card.description.is_empty() {
            let _ = inner.description.set(card.description.clone());
        }
        Ok(card)
    }

    /// Invoke the remote agent and wait for its result.
    pub async fn invoke(&self, input: impl Into<InvokeInput>) -> Result<AgentResult> {
        self.invoke_with(input, InvokeOptions::default()).await
    }

    /// [`A2AAgent::invoke`] with options. Only `invocation_state` is used: it is threaded onto the
    /// returned result, since the remote agent runs in another process and cannot read it.
    pub async fn invoke_with(
        &self,
        input: impl Into<InvokeInput>,
        options: InvokeOptions,
    ) -> Result<AgentResult> {
        let mut stream = Box::pin(self.stream_with(input, options));
        let mut result = None;
        while let Some(event) = stream.next().await {
            if let Some(agent_result) = event?.result() {
                result = Some(agent_result.clone());
            }
        }
        result.ok_or_else(|| Error::model("the remote A2A agent produced no result"))
    }

    /// Invoke the remote agent, yielding every A2A protocol event as it arrives.
    ///
    /// Each protocol event becomes an [`A2AStreamEvent::StreamUpdate`]; the last event is always an
    /// [`A2AStreamEvent::Result`] carrying the assembled [`AgentResult`].
    pub fn stream(&self, input: impl Into<InvokeInput>) -> impl Stream<Item = Result<A2AStreamEvent>> + Send {
        self.stream_with(input, InvokeOptions::default())
    }

    /// [`A2AAgent::stream`] with options. See [`A2AAgent::invoke_with`] for which fields are used.
    pub fn stream_with(
        &self,
        input: impl Into<InvokeInput>,
        options: InvokeOptions,
    ) -> impl Stream<Item = Result<A2AStreamEvent>> + Send {
        let agent = self.clone();
        let input = input.into();
        let invocation_state = options.invocation_state.unwrap_or_default();
        try_stream! {
            let text = text_from_input(&input)?;
            let endpoint = agent.endpoint().await?;

            let mut message = A2AMessage::user(text);
            message.context_id = agent.context_id();
            let request = JsonRpcRequest::new(METHOD_MESSAGE_STREAM, MessageSendParams::new(message))
                .map_err(|error| remote_error("encoding the A2A request failed", error))?;

            let inner = &agent.inner;
            let response = inner
                .request(inner.http.post(&endpoint).json(&request))
                .send()
                .await
                .map_err(|error| remote_error("the A2A message/stream request failed", error))?
                .error_for_status()
                .map_err(|error| remote_error("the A2A message/stream request failed", error))?;

            let mut body = response.bytes_stream();
            let mut parser = SseParser::new();
            let mut last: Option<A2AEventData> = None;
            let mut last_complete: Option<A2AEventData> = None;
            let mut artifacts: HashMap<String, String> = HashMap::new();
            let mut order: Vec<String> = Vec::new();

            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|error| remote_error("reading the A2A event stream failed", error))?;
                for sse in parser.push(&chunk) {
                    let event = decode_event(&sse.data)?;
                    if event.is_complete() {
                        last_complete = Some(event.clone());
                    }
                    if let A2AEventData::ArtifactUpdate(update) = &event {
                        let id = update.artifact.artifact_id.clone();
                        if update.append != Some(true) {
                            artifacts.remove(&id);
                            order.retain(|seen| seen != &id);
                        }
                        if !order.contains(&id) {
                            order.push(id.clone());
                        }
                        artifacts
                            .entry(id)
                            .or_default()
                            .push_str(&super::types::text_from_parts(&update.artifact.parts));
                    }
                    agent.remember_context(&event);
                    last = Some(event.clone());
                    yield A2AStreamEvent::StreamUpdate(A2AStreamUpdateEvent { event });
                }
            }

            let accumulated = order
                .iter()
                .filter_map(|id| artifacts.get(id))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            let final_event = last_complete.or(last);
            let result = build_result(final_event.as_ref(), invocation_state, &accumulated);
            yield A2AStreamEvent::Result(A2AResultEvent { result });
        }
    }

    /// Wrap this remote agent as a tool a local agent can call, like
    /// [`Agent::as_tool`](crate::agent::Agent::as_tool).
    pub fn as_tool(&self, options: AgentAsToolOptions) -> ToolRef {
        Arc::new(A2AAgentAsTool::new(self.clone(), options))
    }

    /// The RPC endpoint: the card's own `url`, else the configured base URL.
    ///
    /// Fetches the card first, as the TS client does — it addresses `agentCard.url` rather than
    /// the URL it discovered the card at.
    async fn endpoint(&self) -> Result<String> {
        let card = self.agent_card().await?;
        Ok(if card.url.is_empty() {
            self.inner.url.clone()
        } else {
            card.url.clone()
        })
    }

    /// Remember the remote's `contextId` so the next turn continues the same conversation.
    fn remember_context(&self, event: &A2AEventData) {
        if !self.inner.continue_context {
            return;
        }
        let context_id = match event {
            A2AEventData::Message(message) => message.context_id.clone(),
            A2AEventData::Task(task) => Some(task.context_id.clone()),
            A2AEventData::StatusUpdate(update) => Some(update.context_id.clone()),
            A2AEventData::ArtifactUpdate(update) => Some(update.context_id.clone()),
        };
        if let Some(context_id) = context_id {
            *self.inner.context_id.lock() = Some(context_id);
        }
    }
}

impl Inner {
    /// Apply the configured timeout to a request.
    fn request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.timeout {
            Some(timeout) => builder.timeout(timeout),
            None => builder,
        }
    }
}

/// Builds an [`A2AAgent`]; mirrors [`AgentBuilder`](crate::agent::AgentBuilder).
#[derive(Clone, Debug)]
#[must_use = "an A2AAgentBuilder does nothing until built"]
pub struct A2AAgentBuilder {
    url: String,
    agent_card_path: Option<String>,
    http: Option<reqwest::Client>,
    timeout: Option<Duration>,
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    card: Option<AgentCard>,
    continue_context: bool,
    context_id: Option<String>,
}

impl A2AAgentBuilder {
    /// Path of the agent card relative to the URL (default [`DEFAULT_AGENT_CARD_PATH`]).
    pub fn agent_card_path(mut self, path: impl Into<String>) -> Self {
        self.agent_card_path = Some(path.into());
        self
    }

    /// A pre-fetched card: skips discovery, so the agent never requests it.
    pub fn agent_card(mut self, card: AgentCard) -> Self {
        self.card = Some(card);
        self
    }

    /// The HTTP client to use — the place to add auth headers (TS's `clientFactory`).
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http = Some(client);
        self
    }

    /// Per-request timeout. Unset means the client's own default.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Unique identifier; defaults to the URL.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Name override; otherwise taken from the agent card.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Description override; otherwise taken from the agent card.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Continue one remote conversation across invocations, instead of a fresh one each time.
    ///
    /// Off by default (TS starts a fresh context every turn). Clones share the context, so two
    /// callers holding clones of one agent are in the same remote conversation.
    pub fn continue_context(mut self, continue_context: bool) -> Self {
        self.continue_context = continue_context;
        self
    }

    /// Continue the remote conversation `context_id`, implying
    /// [`A2AAgentBuilder::continue_context`].
    pub fn context_id(mut self, context_id: impl Into<String>) -> Self {
        self.context_id = Some(context_id.into());
        self.continue_context = true;
        self
    }

    /// Build the agent. No request is made until it is invoked.
    pub fn build(self) -> A2AAgent {
        let url = self.url.trim_end_matches('/').to_owned();
        let name = OnceLock::new();
        let description = OnceLock::new();
        let card = tokio::sync::OnceCell::new();
        if let Some(prefetched) = self.card {
            let _ = name.set(prefetched.name.clone());
            if !prefetched.description.is_empty() {
                let _ = description.set(prefetched.description.clone());
            }
            let _ = card.set(prefetched);
        }
        if let Some(configured) = self.name {
            let _ = name.set(configured);
        }
        if let Some(configured) = self.description {
            let _ = description.set(configured);
        }
        A2AAgent {
            inner: Arc::new(Inner {
                id: self.id.unwrap_or_else(|| url.clone()),
                url,
                agent_card_path: self
                    .agent_card_path
                    .unwrap_or_else(|| DEFAULT_AGENT_CARD_PATH.to_owned()),
                http: self.http.unwrap_or_default(),
                timeout: self.timeout,
                name,
                description,
                card,
                continue_context: self.continue_context,
                context_id: Mutex::new(self.context_id),
            }),
        }
    }
}

/// A remote failure, with the underlying cause kept as the error's source.
fn remote_error(message: &str, source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::model_with_source(format!("{message}: {source}"), source)
}

/// One SSE payload: a JSON-RPC response whose result is an A2A event, or a JSON-RPC error.
fn decode_event(data: &str) -> Result<A2AEventData> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        result: Option<JsonValue>,
        #[serde(default)]
        error: Option<JsonRpcError>,
    }

    let envelope: Envelope = serde_json::from_str(data)
        .map_err(|error| remote_error("an A2A event could not be parsed", error))?;
    if let Some(error) = envelope.error {
        return Err(remote_error("the remote A2A agent returned an error", error));
    }
    let result = envelope
        .result
        .ok_or_else(|| Error::model("an A2A event carried neither a result nor an error"))?;
    serde_json::from_value(result).map_err(|error| remote_error("an A2A event could not be parsed", error))
}

/// The text to send to the remote agent: the last user message's text blocks, joined.
fn text_from_input(input: &InvokeInput) -> Result<String> {
    let blocks = match input {
        InvokeInput::None => return Ok(String::new()),
        InvokeInput::Text(text) => return Ok(text.clone()),
        InvokeInput::Blocks(blocks) => blocks.as_slice(),
        InvokeInput::Messages(messages) => match messages.iter().rev().find(|m| m.role == Role::User) {
            Some(message) => message.content.as_slice(),
            None => return Ok(String::new()),
        },
        _ => {
            return Err(Error::model(
                "an A2A agent cannot resume interrupts or checkpoints: it runs in another process",
            ))
        }
    };
    let non_text = blocks
        .iter()
        .filter(|block| !matches!(block, ContentBlock::Text(_)))
        .count();
    if non_text > 0 {
        tracing::warn!(
            non_text_blocks = non_text,
            "stripping non-text content blocks, A2AAgent does not yet support non-text content"
        );
    }
    Ok(blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The result of an invocation: the final event's text, else the accumulated artifact chunks.
fn build_result(
    event: Option<&A2AEventData>,
    invocation_state: InvocationState,
    accumulated: &str,
) -> AgentResult {
    let text = match event.map(A2AEventData::text) {
        Some(text) if !text.is_empty() => text,
        _ => accumulated.to_owned(),
    };
    AgentResult::new(
        StopReason::EndTurn,
        Message::new(Role::Assistant, vec![ContentBlock::text(text)]),
        invocation_state,
    )
}

/// The [`Tool`] returned by [`A2AAgent::as_tool`].
#[derive(Debug)]
struct A2AAgentAsTool {
    agent: A2AAgent,
    name: String,
    description: String,
    spec: ToolSpec,
}

impl A2AAgentAsTool {
    fn new(agent: A2AAgent, options: AgentAsToolOptions) -> Self {
        let name = options.name.unwrap_or_else(|| agent.name().to_owned());
        let description = options
            .description
            .or_else(|| agent.description().map(str::to_owned))
            .unwrap_or_else(|| format!("Use the {name} agent by providing a natural language input"));
        let spec = ToolSpec::new(&name, &description).with_input_schema(json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "The natural language input to send to the agent." }
            },
            "required": ["prompt"]
        }));
        Self {
            agent,
            name,
            description,
            spec,
        }
    }
}

#[async_trait]
impl Tool for A2AAgentAsTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        #[derive(serde::Deserialize)]
        struct Input {
            prompt: String,
        }
        let tool_use_id = ctx.tool_use.tool_use_id.clone();
        let prompt = match ctx.input::<Input>() {
            Ok(input) => input.prompt,
            Err(error) => return Ok(create_error_result(error, &tool_use_id)),
        };
        match self.agent.invoke(prompt).await {
            Ok(result) => Ok(ToolResultBlock::success(
                &tool_use_id,
                vec![ToolResultContent::Text(result.text().to_owned())],
            )),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(error) => Ok(create_error_result(error, &tool_use_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::{
        Artifact, Part, Task, TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent,
    };
    use crate::a2a::{A2AExecutor, A2AServer, ServerHandle};
    use crate::agent::Agent;
    use crate::testing::{MockMessageModel, MockResponse};

    fn agent_saying(replies: &[&str]) -> Agent {
        let responses = replies
            .iter()
            .map(|text| MockResponse::message(Message::new(Role::Assistant, vec![ContentBlock::text(*text)])))
            .collect();
        Agent::builder()
            .model(MockMessageModel::shared(responses))
            .build()
            .unwrap()
    }

    /// An in-process A2A server whose agent answers with `replies`, in order, per context.
    async fn serve(replies: &'static [&'static str]) -> ServerHandle {
        A2AServer::builder("Remote Agent")
            .description("A remote helper")
            .executor(A2AExecutor::with_factory(move |_| agent_saying(replies)))
            .build()
            .unwrap()
            .serve()
            .await
            .unwrap()
    }

    fn remote(handle: &ServerHandle) -> A2AAgent {
        A2AAgent::builder(format!("http://{}", handle.local_addr())).build()
    }

    fn card() -> AgentCard {
        AgentCard::new("Card Agent", "From the card", "http://example.com")
    }

    #[test]
    fn defaults_id_to_the_url_when_not_provided() {
        let agent = A2AAgent::builder("http://localhost:9999/").build();
        assert_eq!(agent.id(), "http://localhost:9999");
        assert_eq!(agent.url(), "http://localhost:9999");
    }

    #[test]
    fn uses_provided_id_from_config() {
        let agent = A2AAgent::builder("http://localhost:9999").id("custom-id").build();
        assert_eq!(agent.id(), "custom-id");
    }

    #[test]
    fn uses_provided_name_and_description_from_config() {
        let agent = A2AAgent::builder("http://localhost:9999")
            .name("My Agent")
            .description("Does things")
            .build();
        assert_eq!(agent.name(), "My Agent");
        assert_eq!(agent.description(), Some("Does things"));
    }

    #[test]
    fn falls_back_to_the_url_as_the_name_before_connecting() {
        let agent = A2AAgent::builder("http://localhost:9999").build();
        assert_eq!(agent.name(), "http://localhost:9999");
        assert_eq!(agent.description(), None);
    }

    #[test]
    fn takes_name_and_description_from_a_prefetched_card() {
        let agent = A2AAgent::builder("http://example.com").agent_card(card()).build();
        assert_eq!(agent.name(), "Card Agent");
        assert_eq!(agent.description(), Some("From the card"));
    }

    #[tokio::test]
    async fn populates_name_and_description_from_agent_card_on_first_connection() {
        let handle = serve(&["Hello"]).await;
        let agent = remote(&handle);

        let fetched = agent.agent_card().await.unwrap().clone();

        assert_eq!(fetched.name, "Remote Agent");
        assert_eq!(agent.name(), "Remote Agent");
        assert_eq!(agent.description(), Some("A remote helper"));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn does_not_overwrite_config_provided_name_and_description_with_agent_card_values() {
        let handle = serve(&["Hello"]).await;
        let agent = A2AAgent::builder(format!("http://{}", handle.local_addr()))
            .name("Configured")
            .description("Configured description")
            .build();

        agent.agent_card().await.unwrap();

        assert_eq!(agent.name(), "Configured");
        assert_eq!(agent.description(), Some("Configured description"));
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn invoke_returns_an_agent_result_with_the_response_text() {
        let handle = serve(&["Hello from the remote"]).await;

        let result = remote(&handle).invoke("hi").await.unwrap();

        assert_eq!(result.text(), "Hello from the remote");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn invoke_threads_the_callers_invocation_state_onto_the_result() {
        let handle = serve(&["Hello"]).await;
        let state = InvocationState::new();
        state.insert("caller", json!("local"));

        let result = remote(&handle)
            .invoke_with("hi", InvokeOptions::default().with_invocation_state(state))
            .await
            .unwrap();

        assert_eq!(
            result.invocation_state.get("caller").as_deref(),
            Some(&json!("local"))
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn auto_connects_on_first_invoke() {
        let handle = serve(&["Hello"]).await;
        let agent = remote(&handle);
        assert_eq!(agent.description(), None);

        agent.invoke("hi").await.unwrap();

        assert_eq!(agent.name(), "Remote Agent");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn uses_a_custom_http_client_when_provided() {
        let handle = serve(&["Hello"]).await;
        let client = reqwest::Client::builder()
            .user_agent("strands-test")
            .build()
            .unwrap();
        let agent = A2AAgent::builder(format!("http://{}", handle.local_addr()))
            .http_client(client)
            .timeout(Duration::from_secs(10))
            .build();

        assert_eq!(agent.invoke("hi").await.unwrap().text(), "Hello");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn stream_yields_update_events_for_each_a2a_event_and_a_result_event_last() {
        let handle = serve(&["Streamed answer"]).await;
        let agent = remote(&handle);

        let events: Vec<_> = Box::pin(agent.stream("hi"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(std::result::Result::unwrap)
            .collect();

        assert!(events.len() >= 2, "expected updates and a result, got {events:?}");
        assert!(events
            .iter()
            .take(events.len() - 1)
            .all(|event| event.type_name() == "a2aStreamUpdateEvent"));
        let last = events.last().unwrap();
        assert_eq!(last.type_name(), "a2aResultEvent");
        assert_eq!(last.result().unwrap().text(), "Streamed answer");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn starts_a_fresh_remote_context_per_invocation_by_default() {
        let handle = serve(&["first", "second"]).await;
        let agent = remote(&handle);

        assert_eq!(agent.invoke("one").await.unwrap().text(), "first");

        assert_eq!(agent.context_id(), None);
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn continues_the_same_remote_context_across_turns_when_asked() {
        let handle = serve(&["first", "second"]).await;
        let agent = A2AAgent::builder(format!("http://{}", handle.local_addr()))
            .continue_context(true)
            .build();

        assert_eq!(agent.invoke("one").await.unwrap().text(), "first");
        let context = agent.context_id().expect("the remote assigned a context");
        assert_eq!(agent.invoke("two").await.unwrap().text(), "second");
        assert_eq!(agent.context_id().as_deref(), Some(context.as_str()));

        agent.reset_context();
        assert_eq!(agent.context_id(), None);
        handle.shutdown().await;
    }

    #[test]
    fn a_configured_context_id_turns_continuity_on() {
        let agent = A2AAgent::builder("http://localhost:9999")
            .context_id("ctx-42")
            .build();
        assert_eq!(agent.context_id().as_deref(), Some("ctx-42"));
    }

    #[tokio::test]
    async fn sends_requests_to_the_endpoint_the_agent_card_advertises() {
        // The card is served by one server and advertises another as the RPC endpoint.
        let answering = serve(&["from the card url"]).await;
        let mut card = AgentCard::new("Carded", "Advertises another endpoint", "");
        card.url = format!("http://{}", answering.local_addr());
        let advertising = A2AServer::builder("Advertising")
            .executor(A2AExecutor::with_factory(|_| agent_saying(&["wrong server"])))
            .agent_card(card)
            .build()
            .unwrap()
            .serve()
            .await
            .unwrap();

        let agent = A2AAgent::builder(format!("http://{}", advertising.local_addr())).build();
        let result = agent.invoke("hi").await.unwrap();

        assert_eq!(result.text(), "from the card url");
        advertising.shutdown().await;
        answering.shutdown().await;
    }

    #[tokio::test]
    async fn surfaces_a_transport_failure_as_a_model_error() {
        let agent = A2AAgent::builder("http://127.0.0.1:1")
            .timeout(Duration::from_millis(500))
            .build();

        let error = agent.invoke("hi").await.unwrap_err();

        assert!(error.is_model_error(), "expected a model error, got {error:?}");
    }

    #[test]
    fn a_json_rpc_error_event_becomes_a_model_error_keeping_the_code() {
        let data = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": JsonRpcError::task_not_found("task-1"),
        })
        .to_string();

        let error = decode_event(&data).unwrap_err();

        assert!(error.is_model_error());
        let source = std::error::Error::source(&error).expect("the JSON-RPC error is the source");
        let rpc = source.downcast_ref::<JsonRpcError>().expect("a JSON-RPC error");
        assert_eq!(rpc.code, crate::a2a::types::error_codes::TASK_NOT_FOUND);
    }

    #[test]
    fn an_event_with_neither_result_nor_error_is_an_error() {
        assert!(decode_event(&json!({ "jsonrpc": "2.0", "id": 1 }).to_string()).is_err());
    }

    #[test]
    fn extracts_the_text_to_send_from_every_kind_of_input() {
        assert_eq!(
            text_from_input(&InvokeInput::Text("hi".to_owned())).unwrap(),
            "hi"
        );
        assert_eq!(text_from_input(&InvokeInput::None).unwrap(), "");
        assert_eq!(
            text_from_input(&InvokeInput::Blocks(vec![
                ContentBlock::text("one"),
                ContentBlock::text("two")
            ]))
            .unwrap(),
            "one\ntwo"
        );
        assert_eq!(
            text_from_input(&InvokeInput::Messages(vec![
                Message::user("first"),
                Message::new(Role::Assistant, vec![ContentBlock::text("reply")]),
                Message::user("last"),
            ]))
            .unwrap(),
            "last"
        );
        assert_eq!(
            text_from_input(&InvokeInput::Messages(vec![Message::new(
                Role::Assistant,
                vec![ContentBlock::text("only assistant")]
            )]))
            .unwrap(),
            ""
        );
    }

    #[test]
    fn strips_non_text_content_instead_of_failing() {
        let input = InvokeInput::Blocks(vec![
            ContentBlock::text("hi"),
            ContentBlock::CachePoint(Default::default()),
            ContentBlock::text("there"),
        ]);
        assert_eq!(text_from_input(&input).unwrap(), "hi\nthere");
    }

    #[test]
    fn builds_an_empty_result_when_no_events_are_received() {
        let result = build_result(None, InvocationState::new(), "");
        assert_eq!(result.text(), "");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn joins_multiple_text_parts_from_task_artifacts() {
        let mut task = Task::new("task-1", "ctx-1", TaskStatus::new(TaskState::Completed));
        task.artifacts = Some(vec![
            Artifact::new("a1", vec![Part::text("one"), Part::text("two")]),
            Artifact::new("a2", vec![Part::text("three")]),
        ]);

        let result = build_result(Some(&A2AEventData::Task(task)), InvocationState::new(), "");

        assert_eq!(result.text(), "one\ntwo\nthree");
    }

    #[test]
    fn falls_back_to_the_task_status_message_when_there_are_no_artifacts() {
        let mut task = Task::new("task-1", "ctx-1", TaskStatus::new(TaskState::Completed));
        task.status.message = Some(A2AMessage::new(
            crate::a2a::types::Role::Agent,
            vec![Part::text("from the status")],
        ));

        let result = build_result(Some(&A2AEventData::Task(task)), InvocationState::new(), "");

        assert_eq!(result.text(), "from the status");
    }

    #[test]
    fn builds_the_result_from_a_completed_status_update() {
        let event = A2AEventData::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "task-1".to_owned(),
            context_id: "ctx-1".to_owned(),
            status: TaskStatus {
                message: Some(A2AMessage::new(
                    crate::a2a::types::Role::Agent,
                    vec![Part::text("done streaming")],
                )),
                ..TaskStatus::new(TaskState::Completed)
            },
            final_: true,
            metadata: None,
        });

        let result = build_result(Some(&event), InvocationState::new(), "");

        assert_eq!(result.text(), "done streaming");
    }

    #[test]
    fn falls_back_to_accumulated_artifact_chunks_when_the_last_event_has_no_text() {
        let event = A2AEventData::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: "task-1".to_owned(),
            context_id: "ctx-1".to_owned(),
            artifact: Artifact::new("a1", vec![]),
            append: Some(true),
            last_chunk: Some(true),
            metadata: None,
        });

        let result = build_result(Some(&event), InvocationState::new(), "Hello world");

        assert_eq!(result.text(), "Hello world");
    }

    #[tokio::test]
    async fn invoke_with_threads_the_invocation_state_onto_the_result() {
        let handle = serve(&["Hello"]).await;
        let agent = remote(&handle);
        let state = InvocationState::new();
        state.insert("caller", "local".to_owned());

        let result = agent
            .invoke_with(
                InvokeInput::Text("hi".to_owned()),
                InvokeOptions::default().with_invocation_state(state),
            )
            .await
            .unwrap();

        assert_eq!(result.text(), "Hello");
        assert_eq!(
            result.invocation_state.get::<String>("caller").as_deref(),
            Some(&"local".to_owned())
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn as_tool_names_and_describes_itself_from_the_remote_card() {
        let handle = serve(&["Hello"]).await;
        let agent = remote(&handle);
        agent.agent_card().await.unwrap();

        let tool = agent.as_tool(AgentAsToolOptions::default());

        assert_eq!(tool.name(), "Remote Agent");
        assert_eq!(tool.description(), "A remote helper");
        assert_eq!(
            tool.spec().input_schema.as_ref().unwrap()["required"][0],
            "prompt"
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn as_tool_overrides_the_name_and_description() {
        let agent = A2AAgent::builder("http://example.com").agent_card(card()).build();

        let tool = agent.as_tool(AgentAsToolOptions::named("ask_remote").with_description("Ask the remote"));

        assert_eq!(tool.name(), "ask_remote");
        assert_eq!(tool.description(), "Ask the remote");
    }
}
