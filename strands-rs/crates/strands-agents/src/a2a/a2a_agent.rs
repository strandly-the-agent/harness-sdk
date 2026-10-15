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
use crate::agent::{
    AgentAsToolOptions, AgentResult, InvocationState, InvokableAgent, InvokeInput, InvokeOptions,
};
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
/// Fetches the remote agent's card on first use (for its name and description), sends the input as
/// one A2A `message/stream` request, and assembles the streamed protocol events into an
/// [`AgentResult`]. Cheap to clone: clones share one HTTP client, card and conversation context.
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

    /// The A2A `contextId` this agent is continuing, once the remote has assigned one.
    pub fn context_id(&self) -> Option<String> {
        self.inner.context_id.lock().clone()
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
            agent.agent_card().await?;

            let mut message = A2AMessage::user(text);
            message.context_id = agent.context_id();
            let request = JsonRpcRequest::new(METHOD_MESSAGE_STREAM, MessageSendParams::new(message))
                .map_err(|error| remote_error("encoding the A2A request failed", error))?;

            let inner = &agent.inner;
            let response = inner
                .request(inner.http.post(&inner.url).json(&request))
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

    /// Remember the remote's `contextId` so the next turn continues the same conversation.
    fn remember_context(&self, event: &A2AEventData) {
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

#[async_trait]
impl InvokableAgent for A2AAgent {
    fn id(&self) -> &str {
        A2AAgent::id(self)
    }

    fn name(&self) -> &str {
        A2AAgent::name(self)
    }

    fn description(&self) -> Option<&str> {
        A2AAgent::description(self)
    }

    async fn invoke_with(&self, input: InvokeInput, options: InvokeOptions) -> Result<AgentResult> {
        A2AAgent::invoke_with(self, input, options).await
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
                context_id: Mutex::new(None),
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
    if blocks.iter().any(|block| !matches!(block, ContentBlock::Text(_))) {
        return Err(Error::model("an A2A agent does not yet support non-text content"));
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
