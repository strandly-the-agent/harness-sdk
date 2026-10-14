//! The agent event loop (§9–§11): `invoke`, `stream`, and the model↔tool cycle.

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Agent, AgentResult, InvocationState, InvokeInput};
use crate::error::{Error, Result};
use crate::hooks::events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolsEvent, AgentResultEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolsEvent, Cancel, ContentBlockEvent, EndTurn, MessageAddedEvent,
    ModelMessageEvent, ModelStopData, ModelStreamUpdateEvent, ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::hooks::HookEvent;
use crate::models::{AggregatedItem, CountTokensOptions, Model, StreamAggregatedResult, StreamOptions};
use crate::tools::{
    ExecuteToolsInput, StructuredOutputTool, ToolExecutorEvent, EXECUTION_CANCELLED_MESSAGE,
    STRUCTURED_OUTPUT_TOOL_NAME,
};
use crate::types::{
    ContentBlock, JsonValue, Message, Role, StopReason, ToolChoice, ToolResultBlock, ToolResultContent,
    ToolResultStatus, ToolUseBlock, Usage,
};

/// Error text when `invoke`/`stream` is called while another invocation runs.
#[doc(hidden)]
pub const CONCURRENT_INVOCATION_MESSAGE: &str = "Agent is already processing an invocation. Wait for the \
     current invoke() or stream() call to complete before invoking again.";

const INVOCATION_DENIED_MESSAGE: &str = "invocation denied by hook";
const MODEL_CALL_DENIED_MESSAGE: &str = "model call denied by hook";
const HOOK_END_TURN_MESSAGE: &str = "Turn ended early by hook after tool execution";
const CANCELLED_MESSAGE: &str = "Cancelled by user";
const NO_TOOL_USE_BLOCKS_MESSAGE: &str = "Model indicated toolUse but no tool use blocks found in message";

/// Error text when a forced structured-output tool call is still not made.
#[doc(hidden)]
pub const STRUCTURED_OUTPUT_FORCED_FAILURE_MESSAGE: &str =
    "The model failed to invoke the structured output tool even after it was forced.";

const NO_STRUCTURED_OUTPUT_MESSAGE: &str = "The agent stopped without producing structured output.";

/// Per-invocation caps (§9.4 step 9). A `None` field is uncapped; comparison is "at most".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum model↔tool cycles in this invocation.
    pub turns: Option<u64>,
    /// Maximum output tokens accumulated in this invocation.
    pub output_tokens: Option<u64>,
    /// Maximum total tokens accumulated in this invocation.
    pub total_tokens: Option<u64>,
}

impl Limits {
    /// Cap the number of model↔tool cycles.
    #[must_use]
    pub fn with_turns(mut self, turns: u64) -> Self {
        self.turns = Some(turns);
        self
    }

    /// Cap accumulated output tokens.
    #[must_use]
    pub fn with_output_tokens(mut self, tokens: u64) -> Self {
        self.output_tokens = Some(tokens);
        self
    }

    /// Cap accumulated total tokens.
    #[must_use]
    pub fn with_total_tokens(mut self, tokens: u64) -> Self {
        self.total_tokens = Some(tokens);
        self
    }

    /// `_validateLimits`: every present cap must be positive.
    pub fn validate(&self) -> Result<()> {
        for (key, value) in [
            ("turns", self.turns),
            ("outputTokens", self.output_tokens),
            ("totalTokens", self.total_tokens),
        ] {
            if let Some(0) = value {
                return Err(Error::other(format!(
                    "limits.{key} must be a positive finite number, got 0"
                )));
            }
        }
        Ok(())
    }

    /// `_checkLimits`: `turns` wins over `totalTokens`, which wins over `outputTokens`.
    fn check(&self, turns: u64, usage: &Usage) -> Option<StopReason> {
        if self.turns.is_some_and(|cap| turns >= cap) {
            return Some(StopReason::LimitTurns);
        }
        if self.total_tokens.is_some_and(|cap| usage.total_tokens >= cap) {
            return Some(StopReason::LimitTotalTokens);
        }
        if self.output_tokens.is_some_and(|cap| usage.output_tokens >= cap) {
            return Some(StopReason::LimitOutputTokens);
        }
        None
    }
}

/// Options for one invocation.
///
/// Build from [`InvokeOptions::default`] and the `with_*` setters.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct InvokeOptions {
    /// Shared per-invocation state; every event carries this handle.
    pub invocation_state: Option<InvocationState>,
    /// External cancellation. The agent's own [`Agent::cancel`] works regardless.
    pub cancel: Option<CancellationToken>,
    /// Caps that stop the loop with a `limit*` stop reason.
    pub limits: Option<Limits>,
    /// JSON Schema the final answer must satisfy, via the structured-output tool.
    pub structured_output_schema: Option<JsonValue>,
}

impl InvokeOptions {
    /// Set the shared per-invocation state.
    #[must_use]
    pub fn with_invocation_state(mut self, state: InvocationState) -> Self {
        self.invocation_state = Some(state);
        self
    }

    /// Set external cancellation; the loop's own token is a child of it.
    #[must_use]
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Set the per-invocation caps.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set the JSON Schema the final answer must satisfy.
    #[must_use]
    pub fn with_structured_output_schema(mut self, schema: JsonValue) -> Self {
        self.structured_output_schema = Some(schema);
        self
    }
}

/// An event yielded by [`Agent::stream`]; `AgentResult` is always last.
#[derive(Clone)]
#[non_exhaustive]
pub enum AgentStreamEvent {
    /// Before the invocation starts.
    BeforeInvocation(BeforeInvocationEvent),
    /// A message was appended to the conversation.
    MessageAdded(MessageAddedEvent),
    /// Before a model call.
    BeforeModelCall(BeforeModelCallEvent),
    /// A raw provider stream event.
    ModelStreamUpdate(ModelStreamUpdateEvent),
    /// A fully assembled content block.
    ContentBlock(ContentBlockEvent),
    /// The model's complete message.
    ModelMessage(ModelMessageEvent),
    /// After a model call (success, hook-cancel or error).
    AfterModelCall(AfterModelCallEvent),
    /// Before a batch of tool calls.
    BeforeTools(BeforeToolsEvent),
    /// Before one tool call.
    BeforeToolCall(crate::hooks::events::BeforeToolCallEvent),
    /// A running tool reported progress.
    ToolStreamUpdate(ToolStreamUpdateEvent),
    /// After one tool call.
    AfterToolCall(crate::hooks::events::AfterToolCallEvent),
    /// A tool's final result.
    ToolResult(ToolResultEvent),
    /// After a batch of tool calls.
    AfterTools(AfterToolsEvent),
    /// After the invocation.
    AfterInvocation(AfterInvocationEvent),
    /// The invocation's result; always the last event.
    AgentResult(AgentResultEvent),
}

impl AgentStreamEvent {
    /// Short event name, matching the TS `type` discriminator.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::BeforeInvocation(_) => "beforeInvocation",
            Self::MessageAdded(_) => "messageAdded",
            Self::BeforeModelCall(_) => "beforeModelCall",
            Self::ModelStreamUpdate(_) => "modelStreamUpdate",
            Self::ContentBlock(_) => "contentBlock",
            Self::ModelMessage(_) => "modelMessage",
            Self::AfterModelCall(_) => "afterModelCall",
            Self::BeforeTools(_) => "beforeTools",
            Self::BeforeToolCall(_) => "beforeToolCall",
            Self::ToolStreamUpdate(_) => "toolStreamUpdate",
            Self::AfterToolCall(_) => "afterToolCall",
            Self::ToolResult(_) => "toolResult",
            Self::AfterTools(_) => "afterTools",
            Self::AfterInvocation(_) => "afterInvocation",
            Self::AgentResult(_) => "agentResult",
        }
    }

    /// The final result, when this is the `AgentResult` event.
    pub fn result(&self) -> Option<&AgentResult> {
        match self {
            Self::AgentResult(e) => Some(&e.result),
            _ => None,
        }
    }
}

impl std::fmt::Debug for AgentStreamEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AgentStreamEvent")
            .field(&self.type_name())
            .finish()
    }
}

/// Stream of [`AgentStreamEvent`]s produced by a running invocation.
#[must_use = "an AgentStream does nothing unless polled"]
pub struct AgentStream {
    rx: mpsc::Receiver<Result<AgentStreamEvent>>,
}

impl Stream for AgentStream {
    type Item = Result<AgentStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

type Sink = mpsc::Sender<Result<AgentStreamEvent>>;

/// Dispatches `event` to hooks, then yields it; returns the (possibly mutated) event.
async fn emit<E, F>(agent: &Agent, tx: &Sink, mut event: E, wrap: F) -> Result<E>
where
    E: HookEvent + Clone,
    F: FnOnce(E) -> AgentStreamEvent,
{
    agent.hooks().invoke_callbacks(&mut event).await?;
    let _ = tx.send(Ok(wrap(event.clone()))).await;
    Ok(event)
}

async fn append_message(agent: &Agent, tx: &Sink, state: &InvocationState, message: Message) -> Result<()> {
    agent.with_messages(|messages| messages.push(message.clone()));
    emit(
        agent,
        tx,
        MessageAddedEvent {
            agent: agent.clone(),
            message,
            invocation_state: state.clone(),
        },
        AgentStreamEvent::MessageAdded,
    )
    .await?;
    Ok(())
}

impl Agent {
    /// Run the loop to completion and return its result.
    pub async fn invoke(&self, input: impl Into<InvokeInput>) -> Result<AgentResult> {
        self.invoke_with(input, InvokeOptions::default()).await
    }

    /// [`Agent::invoke`] with options.
    pub async fn invoke_with(
        &self,
        input: impl Into<InvokeInput>,
        options: InvokeOptions,
    ) -> Result<AgentResult> {
        let mut stream = self.stream_with(input, options);
        let mut result = None;
        while let Some(event) = stream.next().await {
            if let Some(agent_result) = event?.result() {
                result = Some(agent_result.clone());
            }
        }
        result.ok_or_else(|| Error::other("agent stream ended without a result"))
    }

    /// Run the loop and deserialize the structured output the model produced.
    pub async fn invoke_structured<T>(&self, input: impl Into<InvokeInput>) -> Result<T>
    where
        T: serde::de::DeserializeOwned + schemars::JsonSchema,
    {
        let result = self
            .invoke_with(
                input,
                InvokeOptions::default().with_structured_output_schema(crate::tools::input_schema_for::<T>()),
            )
            .await?;
        let value = result
            .structured_output
            .ok_or_else(|| Error::StructuredOutput(NO_STRUCTURED_OUTPUT_MESSAGE.to_owned()))?;
        serde_json::from_value(value).map_err(|error| Error::StructuredOutput(error.to_string()))
    }

    /// Run the loop, streaming every event as it happens.
    ///
    /// The loop runs in a spawned task, so cleanup events are emitted even when the consumer stops
    /// reading. The invocation is only released when the stream is dropped or finished.
    pub fn stream(&self, input: impl Into<InvokeInput>) -> AgentStream {
        self.stream_with(input, InvokeOptions::default())
    }

    /// [`Agent::stream`] with options.
    pub fn stream_with(&self, input: impl Into<InvokeInput>, options: InvokeOptions) -> AgentStream {
        let (tx, rx) = mpsc::channel(64);
        if self.inner.invoking.swap(true, Ordering::SeqCst) {
            let _ = tx.try_send(Err(Error::ConcurrentInvocation(
                CONCURRENT_INVOCATION_MESSAGE.to_owned(),
            )));
            return AgentStream { rx };
        }

        let agent = self.clone();
        let input = input.into();
        tokio::spawn(async move {
            let outcome = run_invocation(&agent, input, options, &tx).await;
            if let Err(error) = outcome {
                let _ = tx.send(Err(error)).await;
            }
            agent.inner.invoking.store(false, Ordering::SeqCst);
        });
        AgentStream { rx }
    }
}

/// §9.1 — the outer (resume) loop.
async fn run_invocation(agent: &Agent, input: InvokeInput, options: InvokeOptions, tx: &Sink) -> Result<()> {
    agent.initialize().await?;
    let state = options.invocation_state.clone().unwrap_or_default();
    let mut current = input;

    loop {
        let cancel = match &options.cancel {
            Some(external) => external.child_token(),
            None => CancellationToken::new(),
        };
        *agent.inner.cancel.write() = cancel.clone();

        let before = emit(
            agent,
            tx,
            BeforeInvocationEvent {
                agent: agent.clone(),
                invocation_state: state.clone(),
                cancel: Cancel::No,
            },
            AgentStreamEvent::BeforeInvocation,
        )
        .await?;

        if let Some(text) = before.cancel.message_or(INVOCATION_DENIED_MESSAGE) {
            let message = Message::assistant(text);
            append_message(agent, tx, &state, message.clone()).await?;
            emit(
                agent,
                tx,
                AfterInvocationEvent {
                    agent: agent.clone(),
                    invocation_state: state.clone(),
                    resume: None,
                },
                AgentStreamEvent::AfterInvocation,
            )
            .await?;
            return finish(
                agent,
                tx,
                &state,
                AgentResult {
                    stop_reason: StopReason::EndTurn,
                    last_message: message,
                    invocation_state: state.clone(),
                    structured_output: None,
                },
            )
            .await;
        }

        let outcome = run_cycles(agent, current.clone(), &state, &cancel, tx, &options).await;
        let after = emit(
            agent,
            tx,
            AfterInvocationEvent {
                agent: agent.clone(),
                invocation_state: state.clone(),
                resume: None,
            },
            AgentStreamEvent::AfterInvocation,
        )
        .await?;

        let result = outcome?;
        let resumable = matches!(result.stop_reason, StopReason::EndTurn | StopReason::StopSequence);
        match after.resume {
            Some(resume) if resumable => current = resume,
            _ => return finish(agent, tx, &state, result).await,
        }
    }
}

async fn finish(agent: &Agent, tx: &Sink, state: &InvocationState, result: AgentResult) -> Result<()> {
    emit(
        agent,
        tx,
        AgentResultEvent {
            agent: agent.clone(),
            result,
            invocation_state: state.clone(),
        },
        AgentStreamEvent::AgentResult,
    )
    .await?;
    Ok(())
}

/// §9.4 — the model↔tool cycle, with the structured-output tool registered for its duration.
async fn run_cycles(
    agent: &Agent,
    input: InvokeInput,
    state: &InvocationState,
    cancel: &CancellationToken,
    tx: &Sink,
    options: &InvokeOptions,
) -> Result<AgentResult> {
    if let Some(limits) = &options.limits {
        limits.validate()?;
    }
    let structured = options.structured_output_schema.is_some();
    if let Some(schema) = options.structured_output_schema.clone() {
        agent.with_tool_registry(|registry| {
            registry.remove(STRUCTURED_OUTPUT_TOOL_NAME);
            registry.add(Arc::new(StructuredOutputTool::from_schema(schema)))
        })?;
    }

    let outcome = run_cycles_inner(agent, input, state, cancel, tx, options, structured).await;

    if structured {
        agent.with_tool_registry(|registry| registry.remove(STRUCTURED_OUTPUT_TOOL_NAME));
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn run_cycles_inner(
    agent: &Agent,
    input: InvokeInput,
    state: &InvocationState,
    cancel: &CancellationToken,
    tx: &Sink,
    options: &InvokeOptions,
    structured: bool,
) -> Result<AgentResult> {
    let mut pending_input = Some(input);
    let mut usage = Usage::default();
    let mut turns = 0u64;
    let mut forced_choice: Option<ToolChoice> = None;

    loop {
        if cancel.is_cancelled() {
            return cancelled_result(agent, tx, state).await;
        }

        if let Some(stop_reason) = options.limits.as_ref().and_then(|l| l.check(turns, &usage)) {
            return Ok(AgentResult {
                stop_reason,
                last_message: agent
                    .with_messages(|m| m.last().cloned())
                    .unwrap_or_else(|| Message::assistant("")),
                invocation_state: state.clone(),
                structured_output: None,
            });
        }
        turns += 1;

        if let Some(input) = pending_input.take() {
            for message in input.into_messages() {
                append_message(agent, tx, state, message).await?;
            }
        }

        let model_result = match invoke_model(agent, state, cancel, tx, forced_choice.clone()).await {
            Ok(result) => result,
            Err(Error::Cancelled) => return cancelled_result(agent, tx, state).await,
            Err(error) => return Err(error),
        };
        if let Some(cycle_usage) = &model_result.usage {
            usage.input_tokens += cycle_usage.input_tokens;
            usage.output_tokens += cycle_usage.output_tokens;
            usage.total_tokens += cycle_usage.total_tokens;
        }

        if model_result.stop_reason != StopReason::ToolUse {
            if structured {
                if forced_choice.is_some() {
                    return Err(Error::StructuredOutput(
                        STRUCTURED_OUTPUT_FORCED_FAILURE_MESSAGE.to_owned(),
                    ));
                }
                forced_choice = Some(ToolChoice::Tool {
                    name: STRUCTURED_OUTPUT_TOOL_NAME.to_owned(),
                });
                continue;
            }
            append_message(agent, tx, state, model_result.message.clone()).await?;
            return Ok(AgentResult {
                stop_reason: model_result.stop_reason,
                last_message: model_result.message,
                invocation_state: state.clone(),
                structured_output: None,
            });
        }

        let assistant_message = model_result.message;

        if cancel.is_cancelled() {
            let cancelled = Message::new(
                Role::User,
                tool_use_blocks(&assistant_message)
                    .iter()
                    .map(|tool_use| {
                        ContentBlock::ToolResult(ToolResultBlock::error(
                            &tool_use.tool_use_id,
                            EXECUTION_CANCELLED_MESSAGE,
                        ))
                    })
                    .collect(),
            );
            append_message(agent, tx, state, assistant_message.clone()).await?;
            append_message(agent, tx, state, cancelled).await?;
            return Ok(AgentResult {
                stop_reason: StopReason::Cancelled,
                last_message: assistant_message,
                invocation_state: state.clone(),
                structured_output: None,
            });
        }

        let tools = match execute_tools(agent, &assistant_message, state, cancel, tx).await {
            Ok(tools) => tools,
            Err(Error::Cancelled) => return cancelled_result(agent, tx, state).await,
            Err(error) => return Err(error),
        };

        append_message(agent, tx, state, assistant_message.clone()).await?;
        append_message(agent, tx, state, tools.message.clone()).await?;

        match tools.end_turn {
            EndTurn::No => {}
            EndTurn::Yes | EndTurn::Text(_) | EndTurn::Blocks(_) => {
                let content = match tools.end_turn {
                    EndTurn::Text(text) if !text.is_empty() => vec![ContentBlock::Text(text)],
                    EndTurn::Blocks(blocks) if !blocks.is_empty() => blocks,
                    EndTurn::Text(_) | EndTurn::Blocks(_) => continue,
                    _ => vec![ContentBlock::Text(HOOK_END_TURN_MESSAGE.to_owned())],
                };
                let last = Message::new(Role::Assistant, content);
                append_message(agent, tx, state, last.clone()).await?;
                return Ok(AgentResult {
                    stop_reason: StopReason::EndTurn,
                    last_message: last,
                    invocation_state: state.clone(),
                    structured_output: None,
                });
            }
        }

        if structured {
            if let Some(output) = extract_structured_output(&assistant_message, &tools.message) {
                return Ok(AgentResult {
                    stop_reason: StopReason::ToolUse,
                    last_message: assistant_message,
                    invocation_state: state.clone(),
                    structured_output: Some(output),
                });
            }
        }
    }
}

fn tool_use_blocks(message: &Message) -> Vec<ToolUseBlock> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => Some(tool_use.clone()),
            _ => None,
        })
        .collect()
}

/// `_extractStructuredOutput`: the JSON of a *successful* structured-output tool result.
fn extract_structured_output(assistant: &Message, tool_results: &Message) -> Option<JsonValue> {
    let tool_use = assistant.content.iter().find_map(|block| match block {
        ContentBlock::ToolUse(tool_use) if tool_use.name == STRUCTURED_OUTPUT_TOOL_NAME => Some(tool_use),
        _ => None,
    })?;
    let result = tool_results.content.iter().find_map(|block| match block {
        ContentBlock::ToolResult(result)
            if result.tool_use_id == tool_use.tool_use_id && result.status == ToolResultStatus::Success =>
        {
            Some(result)
        }
        _ => None,
    })?;
    match result.content.first()? {
        ToolResultContent::Json(value) => Some(value.clone()),
        _ => None,
    }
}

async fn cancelled_result(agent: &Agent, tx: &Sink, state: &InvocationState) -> Result<AgentResult> {
    let message = Message::assistant(CANCELLED_MESSAGE);
    let open_user_turn = agent.with_messages(|m| m.last().map(|m| m.role == Role::User).unwrap_or(false));
    if open_user_turn {
        append_message(agent, tx, state, message.clone()).await?;
    }
    Ok(AgentResult {
        stop_reason: StopReason::Cancelled,
        last_message: message,
        invocation_state: state.clone(),
        structured_output: None,
    })
}

/// §10 — one model call with the retry loop driven by `AfterModelCallEvent::retry`.
async fn invoke_model(
    agent: &Agent,
    state: &InvocationState,
    cancel: &CancellationToken,
    tx: &Sink,
    tool_choice: Option<ToolChoice>,
) -> Result<StreamAggregatedResult> {
    let mut attempt_count = 1;

    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let model = agent.model();
        let count_options = CountTokensOptions {
            system_prompt: agent.system_prompt(),
            tool_specs: agent.tools().iter().map(|t| t.spec().clone()).collect(),
        };
        let projected_input_tokens = estimate_input_tokens(agent, &model, &count_options).await.ok();
        let before = emit(
            agent,
            tx,
            BeforeModelCallEvent {
                agent: agent.clone(),
                model: Arc::clone(&model),
                invocation_state: state.clone(),
                projected_input_tokens,
                cancel: Cancel::No,
            },
            AgentStreamEvent::BeforeModelCall,
        )
        .await?;

        if let Some(text) = before.cancel.message_or(MODEL_CALL_DENIED_MESSAGE) {
            let denied = StreamAggregatedResult {
                message: Message::assistant(text),
                stop_reason: StopReason::EndTurn,
                usage: None,
                metrics: None,
                redaction: None,
            };
            let after = emit(
                agent,
                tx,
                AfterModelCallEvent {
                    agent: agent.clone(),
                    model: Arc::clone(&model),
                    stop_data: Some(ModelStopData {
                        message: denied.message.clone(),
                        stop_reason: denied.stop_reason.clone(),
                        redaction: None,
                    }),
                    error: None,
                    invocation_state: state.clone(),
                    attempt_count,
                    retry: false,
                },
                AgentStreamEvent::AfterModelCall,
            )
            .await?;
            if after.retry {
                attempt_count += 1;
                continue;
            }
            return Ok(denied);
        }

        let options = StreamOptions {
            cancel: Some(cancel.clone()),
            system_prompt: count_options.system_prompt.clone(),
            tool_specs: count_options.tool_specs.clone(),
            tool_choice: tool_choice.clone(),
            ..StreamOptions::default()
        };

        match stream_from_model(agent, &model, &options, state, cancel, tx).await {
            Ok(result) => {
                emit(
                    agent,
                    tx,
                    ModelMessageEvent {
                        agent: agent.clone(),
                        message: result.message.clone(),
                        stop_reason: result.stop_reason.clone(),
                        invocation_state: state.clone(),
                    },
                    AgentStreamEvent::ModelMessage,
                )
                .await?;
                if let Some(redaction) = &result.redaction {
                    redact_last_message(agent, &redaction.user_message);
                }
                let after = emit(
                    agent,
                    tx,
                    AfterModelCallEvent {
                        agent: agent.clone(),
                        model: Arc::clone(&model),
                        stop_data: Some(ModelStopData {
                            message: result.message.clone(),
                            stop_reason: result.stop_reason.clone(),
                            redaction: result.redaction.as_ref().map(|r| crate::models::Redaction {
                                user_message: r.user_message.clone(),
                            }),
                        }),
                        error: None,
                        invocation_state: state.clone(),
                        attempt_count,
                        retry: false,
                    },
                    AgentStreamEvent::AfterModelCall,
                )
                .await?;
                if after.retry {
                    attempt_count += 1;
                    continue;
                }
                return Ok(result);
            }
            Err(error) => {
                let shared = Arc::new(error);
                let after = emit(
                    agent,
                    tx,
                    AfterModelCallEvent {
                        agent: agent.clone(),
                        model: Arc::clone(&model),
                        stop_data: None,
                        error: Some(Arc::clone(&shared)),
                        invocation_state: state.clone(),
                        attempt_count,
                        retry: false,
                    },
                    AgentStreamEvent::AfterModelCall,
                )
                .await?;
                if matches!(*shared, Error::Cancelled) {
                    return Err(Error::Cancelled);
                }
                if after.retry {
                    attempt_count += 1;
                    continue;
                }
                return Err(unwrap_shared_error(shared));
            }
        }
    }
}

/// Recover the owned error after it has been shared with hooks, preserving the loop-relevant
/// variants (`Error` is not `Clone`, and emitted events keep a handle).
fn unwrap_shared_error(shared: Arc<Error>) -> Error {
    Arc::try_unwrap(shared).unwrap_or_else(|shared| match &*shared {
        Error::ContextWindowOverflow(message) => Error::ContextWindowOverflow(message.clone()),
        Error::MaxTokens {
            message,
            partial_message,
        } => Error::MaxTokens {
            message: message.clone(),
            partial_message: partial_message.clone(),
        },
        Error::ModelThrottled { message, .. } => Error::ModelThrottled {
            message: message.clone(),
            source: None,
        },
        Error::Model { message, .. } => Error::Model {
            message: message.clone(),
            source: None,
        },
        Error::Cancelled => Error::Cancelled,
        other => Error::other(other.to_string()),
    })
}

/// §10 step 6 — `_estimateInputTokens`: reuse the last reported usage, else count everything.
async fn estimate_input_tokens(
    agent: &Agent,
    model: &Arc<dyn Model>,
    options: &CountTokensOptions,
) -> Result<u64> {
    let messages = agent.messages();
    let anchor = messages.iter().rposition(|message| {
        message.role == Role::Assistant
            && message
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.usage.as_ref())
                .is_some()
    });

    let Some(index) = anchor else {
        return model.count_tokens(&messages, options).await;
    };
    let usage = messages[index]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.usage.as_ref())
        .expect("anchor message has usage");
    let baseline = usage.input_tokens
        + usage.cache_read_input_tokens.unwrap_or(0)
        + usage.cache_write_input_tokens.unwrap_or(0)
        + usage.output_tokens;
    let rest = &messages[index + 1..];
    if rest.is_empty() {
        return Ok(baseline);
    }
    Ok(baseline + model.count_tokens(rest, options).await?)
}

async fn stream_from_model(
    agent: &Agent,
    model: &Arc<dyn crate::models::Model>,
    options: &StreamOptions,
    state: &InvocationState,
    cancel: &CancellationToken,
    tx: &Sink,
) -> Result<StreamAggregatedResult> {
    let messages = agent.messages();
    let mut stream = model.stream_aggregated(&messages, options);
    let mut aggregated = None;

    while let Some(item) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match item {
            Ok(AggregatedItem::Event(event)) => {
                emit(
                    agent,
                    tx,
                    ModelStreamUpdateEvent {
                        agent: agent.clone(),
                        event,
                        invocation_state: state.clone(),
                    },
                    AgentStreamEvent::ModelStreamUpdate,
                )
                .await?;
            }
            Ok(AggregatedItem::ContentBlock(content_block)) => {
                emit(
                    agent,
                    tx,
                    ContentBlockEvent {
                        agent: agent.clone(),
                        content_block,
                        invocation_state: state.clone(),
                    },
                    AgentStreamEvent::ContentBlock,
                )
                .await?;
            }
            Ok(AggregatedItem::Result(result)) => aggregated = Some(result),
            Err(error) => {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                return Err(error);
            }
        }
    }

    aggregated.ok_or_else(|| Error::other("model stream ended without a result"))
}

fn redact_last_message(agent: &Agent, text: &str) {
    agent.with_messages(|messages| {
        let Some(last) = messages.last_mut() else { return };
        if last.role != Role::User {
            tracing::debug!(
                role = ?last.role,
                "received input redaction but last message is not from user | redaction skipped"
            );
            return;
        }
        let tool_results: Vec<ContentBlock> = last
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult(result) => {
                    let mut replaced = result.clone();
                    replaced.content = vec![crate::types::ToolResultContent::Text(text.to_owned())];
                    Some(ContentBlock::ToolResult(replaced))
                }
                _ => None,
            })
            .collect();
        last.content = if tool_results.is_empty() {
            vec![ContentBlock::Text(text.to_owned())]
        } else {
            tool_results
        };
    });
}

struct ToolsOutcome {
    message: Message,
    end_turn: EndTurn,
}

/// §11 — run the assistant message's tool uses and collect them into one user message.
async fn execute_tools(
    agent: &Agent,
    assistant_message: &Message,
    state: &InvocationState,
    cancel: &CancellationToken,
    tx: &Sink,
) -> Result<ToolsOutcome> {
    let before = emit(
        agent,
        tx,
        BeforeToolsEvent {
            agent: agent.clone(),
            message: assistant_message.clone(),
            invocation_state: state.clone(),
            cancel: Cancel::No,
        },
        AgentStreamEvent::BeforeTools,
    )
    .await?;

    let tool_uses: Vec<ToolUseBlock> = assistant_message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => Some(tool_use.clone()),
            _ => None,
        })
        .collect();

    let mut results = Vec::new();
    let mut outcome = Ok(());

    if tool_uses.is_empty() {
        outcome = Err(Error::other(NO_TOOL_USE_BLOCKS_MESSAGE));
    } else if let Some(message) = before
        .cancel
        .message_or(crate::tools::HOOK_CANCELLED_MESSAGE)
        .map(str::to_owned)
        .or_else(|| {
            cancel
                .is_cancelled()
                .then(|| crate::tools::EXECUTION_CANCELLED_MESSAGE.to_owned())
        })
    {
        for tool_use in &tool_uses {
            let result = crate::types::ToolResultBlock::error(&tool_use.tool_use_id, &message);
            results.push(result.clone());
            emit(
                agent,
                tx,
                ToolResultEvent {
                    agent: agent.clone(),
                    result,
                    invocation_state: state.clone(),
                },
                AgentStreamEvent::ToolResult,
            )
            .await?;
        }
    } else {
        outcome = run_executor(agent, tool_uses, state, cancel, tx, &mut results).await;
    }

    let message = Message::new(
        Role::User,
        results.into_iter().map(ContentBlock::ToolResult).collect(),
    );
    let after = emit(
        agent,
        tx,
        AfterToolsEvent {
            agent: agent.clone(),
            message: message.clone(),
            invocation_state: state.clone(),
            end_turn: EndTurn::No,
        },
        AgentStreamEvent::AfterTools,
    )
    .await?;
    outcome?;

    Ok(ToolsOutcome {
        message: after.message,
        end_turn: after.end_turn,
    })
}

async fn run_executor(
    agent: &Agent,
    tool_uses: Vec<ToolUseBlock>,
    state: &InvocationState,
    cancel: &CancellationToken,
    tx: &Sink,
    results: &mut Vec<crate::types::ToolResultBlock>,
) -> Result<()> {
    let (events_tx, mut events_rx) = mpsc::channel::<ToolExecutorEvent>(64);
    let input = ExecuteToolsInput {
        agent: agent.clone(),
        tool_uses,
        invocation_state: state.clone(),
        cancel: cancel.clone(),
    };
    let executor = agent.tool_executor();

    let forward = async {
        while let Some(event) = events_rx.recv().await {
            let wrapped = match event {
                ToolExecutorEvent::BeforeToolCall(e) => AgentStreamEvent::BeforeToolCall(e),
                ToolExecutorEvent::ToolStreamUpdate(e) => AgentStreamEvent::ToolStreamUpdate(e),
                ToolExecutorEvent::AfterToolCall(e) => AgentStreamEvent::AfterToolCall(e),
                ToolExecutorEvent::ToolResult(e) => AgentStreamEvent::ToolResult(e),
            };
            let _ = tx.send(Ok(wrapped)).await;
        }
    };

    let execute = async move {
        let outcome = executor.execute(&input, &events_tx, results).await;
        drop(events_tx);
        outcome
    };

    let (outcome, ()) = futures::future::join(execute, forward).await;
    outcome
}
