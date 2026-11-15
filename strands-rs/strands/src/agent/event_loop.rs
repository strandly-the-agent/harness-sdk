//! The agent loop (TS `Agent._streamInvocation`, `_stream`, `_invokeModel` and `executeTools` in
//! `agent/agent.ts`).

use std::future::Future;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::agent::agent::Agent;
use crate::agent::invocation_context::InvocationContext;
use crate::agent::invocation_state::InvocationState;
use crate::agent::invoke::{InvokeArgs, InvokeOptions};
use crate::agent::stream::Yielder;
use crate::agent::stream_event::{
    AfterInvocationEventData, AfterModelCallEventData, AfterToolsEventData, AgentStreamEvent,
    BeforeInvocationEventData, BeforeModelCallEventData, BeforeToolsEventData, ContentBlockEventData,
    EventError, InterruptEventData, MessageAddedEventData, ModelMessageEventData, ModelStreamUpdateEventData,
};
use crate::agent::tool_agent::{ToolAgent, ToolAgentParts};
use crate::cancel::CancelSignal;
use crate::error::{DataError, Error, InvalidInputError, ModelError, Result, UnsupportedError};
use crate::hooks::events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolsEvent, AgentResultEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolsEvent, ContentBlockEvent, EndTurn, InterruptEvent, MessageAddedEvent,
    ModelMessageEvent, ModelStopData, ModelStreamUpdateEvent,
};
use crate::hooks::registry::HookRegistry;
use crate::interrupt::Interrupt;
use crate::models::model::{
    AggregatedEvent, CountTokensOptions, Model, ModelRequest, StreamAggregatedResult,
};
use crate::models::model_proxy;
use crate::text::js_trim;
use crate::tools::executors::ToolExecutor;
use crate::tools::executors::executor::{
    PipelineToolCaller, TOOL_EXECUTION_CANCELLED, ToolBatch, error_result,
};
use crate::tools::is_valid_tool_name;
use crate::tools::structured_output::{
    STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema, StructuredOutputTool, structured_output_tool_use_ids,
};
use crate::types::agent::AgentResult;
use crate::types::json::JsonValue;
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, TextBlock, ToolResultBlock, ToolResultContent,
};
use crate::types::tools::{ToolChoice, ToolUse};

const BLANK_PROMPT: &str =
    "Input prompt is blank. Provide non-blank text or at least one non-text content block.";
const INVOCATION_DENIED_BY_HOOK: &str = "invocation denied by hook";
const MODEL_CALL_DENIED_BY_HOOK: &str = "model call denied by hook";
const TOOL_CANCELLED_BY_HOOK: &str = "Tool cancelled by hook";
const TURN_ENDED_BY_HOOK: &str = "Turn ended early by hook after tool execution";
const CANCELLED_BY_USER: &str = "Cancelled by user";
const TOOL_EXECUTION_INTERRUPTED: &str = "Tool execution interrupted";
const NO_TOOL_USE_BLOCKS: &str = "Model indicated toolUse but no tool use blocks found in message";
const INVALID_TOOL_NAME_PLACEHOLDER: &str = "INVALID_TOOL_NAME";
const BLANK_TEXT_PLACEHOLDER: &str = "[blank text]";
const RECORD_BUFFER: usize = 16;

/// What one pass of the invocation loop shares.
struct Invocation {
    state: InvocationState,
    context: InvocationContext,
    cancel: CancelSignal,
}

/// Why a step stopped early: a cancel, or an error that fails the invocation.
enum Stop {
    Cancelled,
    Failed(Error),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

/// How one pass of the invocation loop ended.
enum Pass {
    Done(Box<AgentResult>),
    Resume(InvokeArgs),
}

/// How a turn without tool use ended (TS `TextTurnOutcome`).
enum TextTurn {
    Retry,
    Done {
        stop_reason: StopReason,
        structured_output: Option<JsonValue>,
    },
}

/// How a turn's tools ended (TS `ToolsExecutionResult`).
struct ToolsOutcome {
    message: Message,
    end_turn: Option<EndTurn>,
    interrupts: Vec<Interrupt>,
}

impl Agent {
    pub(crate) async fn run_invocation(
        &mut self,
        args: InvokeArgs,
        options: InvokeOptions,
        yielder: Yielder,
    ) -> Result<AgentResult> {
        self.initialize().await?;
        let state = options.invocation_state.clone().unwrap_or_default();
        let context = InvocationContext::resolve(options.invocation_context.as_ref(), options.limits);
        let schema = options
            .structured_output_schema
            .clone()
            .or_else(|| self.structured_output_schema.clone());
        let mut args = args;
        let outcome = loop {
            let invocation = Invocation {
                state: state.clone(),
                context: context.clone(),
                cancel: self.cancel.begin(options.cancel_signal.as_ref()),
            };
            match self.run_pass(args, &invocation, schema.as_ref(), &yielder).await {
                Ok(Pass::Resume(next)) => args = next,
                Ok(Pass::Done(result)) => break Ok(*result),
                Err(error) => break Err(error),
            }
        };
        self.cancel.end();
        outcome
    }

    fn hooks(&self) -> Arc<HookRegistry> {
        Arc::clone(self.local.hooks())
    }

    /// One pass of TS `_streamInvocation`'s loop: before-invocation hooks, the turns, then
    /// after-invocation hooks and their `resume`.
    async fn run_pass(
        &mut self,
        args: InvokeArgs,
        invocation: &Invocation,
        schema: Option<&StructuredOutputSchema>,
        yielder: &Yielder,
    ) -> Result<Pass> {
        let input = normalize_input(args)?;
        let hooks = self.hooks();
        let mut before = BeforeInvocationEvent::new(
            &mut self.local,
            invocation.state.clone(),
            Some(invocation.context.clone()),
            input,
        );
        hooks.invoke::<BeforeInvocationEvent<'static>>(&mut before).await;
        let (input, cancel) = before.into_parts();
        yielder
            .emit(AgentStreamEvent::BeforeInvocation(
                BeforeInvocationEventData::new(),
            ))
            .await;
        if let Some(cancel) = cancel {
            let message = assistant_text(cancel.message().unwrap_or(INVOCATION_DENIED_BY_HOOK));
            self.append_message(message.clone(), invocation, yielder).await;
            self.after_invocation(invocation, yielder).await;
            return Ok(Pass::Done(Box::new(result(
                StopReason::EndTurn,
                message,
                invocation,
            ))));
        }
        let outcome = self.run_turns(input, invocation, schema, yielder).await;
        let resume = self.after_invocation(invocation, yielder).await;
        let result = outcome?;
        if let Some(resume) = resume {
            match invocation.context.reached_limit() {
                None => return Ok(Pass::Resume(resume)),
                Some(limit) => tracing::debug!(
                    "limit=<{}> | ignoring resume because the request reached its limit",
                    limit.as_str()
                ),
            }
        }
        let mut event = AgentResultEvent::new(
            &mut self.local,
            result.clone(),
            invocation.state.clone(),
            Some(invocation.context.clone()),
        );
        hooks.invoke::<AgentResultEvent<'static>>(&mut event).await;
        yielder.emit(AgentStreamEvent::AgentResult(result.clone())).await;
        Ok(Pass::Done(Box::new(result)))
    }

    async fn after_invocation(&mut self, invocation: &Invocation, yielder: &Yielder) -> Option<InvokeArgs> {
        let hooks = self.hooks();
        let mut event = AfterInvocationEvent::new(
            &mut self.local,
            invocation.state.clone(),
            Some(invocation.context.clone()),
        );
        hooks.invoke::<AfterInvocationEvent<'static>>(&mut event).await;
        let resume = event.into_resume();
        yielder
            .emit(AgentStreamEvent::AfterInvocation(AfterInvocationEventData::new()))
            .await;
        resume
    }

    /// TS `_stream`: the turns of one pass, with the structured output tool registered for them.
    async fn run_turns(
        &mut self,
        input: Vec<Message>,
        invocation: &Invocation,
        schema: Option<&StructuredOutputSchema>,
        yielder: &Yielder,
    ) -> Result<AgentResult> {
        let Some(schema) = schema else {
            return self.turn_loop(input, invocation, None, yielder).await;
        };
        let tool = StructuredOutputTool::new(schema.clone());
        self.local
            .tool_registry_mut()
            .add_or_replace([Arc::new(tool.clone()) as Arc<dyn crate::tools::Tool>])?;
        let outcome = self.turn_loop(input, invocation, Some(&tool), yielder).await;
        self.local.tool_registry_mut().remove(STRUCTURED_OUTPUT_TOOL_NAME);
        outcome
    }

    async fn turn_loop(
        &mut self,
        input: Vec<Message>,
        invocation: &Invocation,
        structured_output: Option<&StructuredOutputTool>,
        yielder: &Yielder,
    ) -> Result<AgentResult> {
        let message_count_at_start = self.local.messages().len();
        let mut pending_input = Some(input);
        let mut forced_choice: Option<ToolChoice> = None;
        loop {
            if invocation.cancel.is_cancelled() {
                return Ok(self.cancelled(invocation, yielder).await);
            }
            if let Some(limit) = invocation.context.reached_limit() {
                return Ok(self.limit_result(limit, message_count_at_start, invocation));
            }
            invocation.context.record_turn();
            for message in pending_input.take().unwrap_or_default() {
                self.append_message(message, invocation, yielder).await;
            }
            let schema = forced_choice
                .as_ref()
                .and(structured_output)
                .map(|tool| tool.input_schema().clone());
            let model_result = match self
                .invoke_model(invocation, forced_choice.clone(), schema, yielder)
                .await
            {
                Ok(model_result) => model_result,
                Err(Stop::Cancelled) => return Ok(self.cancelled(invocation, yielder).await),
                Err(Stop::Failed(error)) => return Err(error),
            };
            if model_result.stop_reason != StopReason::ToolUse {
                match resolve_text_turn(
                    &model_result,
                    structured_output,
                    forced_choice.is_some(),
                    invocation,
                )? {
                    TextTurn::Retry => {
                        forced_choice = Some(ToolChoice::tool(STRUCTURED_OUTPUT_TOOL_NAME));
                        tracing::debug!(
                            "structured output schema set but model responded with plain text; forcing tool use on next cycle"
                        );
                        continue;
                    }
                    TextTurn::Done {
                        stop_reason,
                        structured_output,
                    } => {
                        self.append_message(model_result.message.clone(), invocation, yielder)
                            .await;
                        let mut done = result(stop_reason, model_result.message, invocation);
                        if let Some(output) = structured_output {
                            done = done.with_structured_output(output);
                        }
                        return Ok(done);
                    }
                }
            }
            let assistant = model_result.message;
            if forced_choice.is_some() && structured_output_tool_use_ids(&assistant).is_empty() {
                invocation.context.record_structured_output_failure();
            }
            if invocation.cancel.is_cancelled() {
                return Ok(self.cancel_tool_turn(assistant, invocation, yielder).await);
            }
            let tools = self.execute_tools(&assistant, invocation, yielder).await?;
            if !tools.interrupts.is_empty() {
                return Ok(self.interrupted(tools.interrupts, invocation, yielder).await);
            }
            let output =
                structured_output.and_then(|_| extract_structured_output(&assistant, &tools.message));
            let output_requested = !structured_output_tool_use_ids(&assistant).is_empty();
            self.append_message(assistant.clone(), invocation, yielder).await;
            self.append_message(tools.message, invocation, yielder).await;
            if structured_output.is_some() && output.is_none() && output_requested {
                invocation.context.record_structured_output_failure();
            }
            if let Some(end_turn) = tools.end_turn {
                let message = Message::new(Role::Assistant, end_turn_content(end_turn));
                self.append_message(message.clone(), invocation, yielder).await;
                return Ok(result(StopReason::EndTurn, message, invocation));
            }
            if invocation.cancel.is_deferred() || invocation.cancel.is_cancelled() {
                let message = cancel_message(&invocation.cancel);
                self.append_message(message.clone(), invocation, yielder).await;
                let mut done = result(StopReason::Cancelled, message, invocation);
                if let Some(output) = output {
                    done = done.with_structured_output(output);
                }
                return Ok(done);
            }
            if let Some(output) = output {
                return Ok(result(StopReason::ToolUse, assistant, invocation).with_structured_output(output));
            }
        }
    }

    /// Appends a message to the history and fires `MessageAddedEvent` (TS `_appendMessage`).
    async fn append_message(&mut self, message: Message, invocation: &Invocation, yielder: &Yielder) {
        self.local.messages_mut().push(message.clone());
        let hooks = self.hooks();
        let mut event = MessageAddedEvent::new(
            &mut self.local,
            message.clone(),
            invocation.state.clone(),
            Some(invocation.context.clone()),
        );
        hooks.invoke::<MessageAddedEvent<'static>>(&mut event).await;
        yielder
            .emit(AgentStreamEvent::MessageAdded(MessageAddedEventData::new(
                message,
            )))
            .await;
    }

    /// TS's `CancelledError` handler: closes an open user turn with the cancel message.
    async fn cancelled(&mut self, invocation: &Invocation, yielder: &Yielder) -> AgentResult {
        let message = cancel_message(&invocation.cancel);
        if self
            .local
            .messages()
            .last()
            .is_some_and(|last| last.role == Role::User)
        {
            self.append_message(message.clone(), invocation, yielder).await;
        }
        result(StopReason::Cancelled, message, invocation)
    }

    /// A cancel that arrived while the model asked for tools: every call gets the cancelled
    /// result and the turn closes with the cancel message.
    async fn cancel_tool_turn(
        &mut self,
        assistant: Message,
        invocation: &Invocation,
        yielder: &Yielder,
    ) -> AgentResult {
        let results = tool_uses(&assistant)
            .iter()
            .map(|tool_use| ContentBlock::from(error_result(&tool_use.tool_use_id, TOOL_EXECUTION_CANCELLED)))
            .collect();
        self.append_message(assistant, invocation, yielder).await;
        self.append_message(Message::new(Role::User, results), invocation, yielder)
            .await;
        let message = cancel_message(&invocation.cancel);
        self.append_message(message.clone(), invocation, yielder).await;
        result(StopReason::Cancelled, message, invocation)
    }

    fn limit_result(
        &self,
        limit: StopReason,
        message_count_at_start: usize,
        invocation: &Invocation,
    ) -> AgentResult {
        let last_message = self
            .local
            .messages()
            .get(message_count_at_start..)
            .and_then(<[Message]>::last)
            .cloned()
            .unwrap_or_else(|| Message::new(Role::Assistant, Vec::new()));
        result(limit, last_message, invocation)
    }

    async fn interrupted(
        &mut self,
        interrupts: Vec<Interrupt>,
        invocation: &Invocation,
        yielder: &Yielder,
    ) -> AgentResult {
        let hooks = self.hooks();
        for interrupt in &interrupts {
            let mut event = InterruptEvent::new(
                &mut self.local,
                interrupt.clone(),
                invocation.state.clone(),
                Some(invocation.context.clone()),
            );
            hooks.invoke::<InterruptEvent<'static>>(&mut event).await;
            yielder
                .emit(AgentStreamEvent::Interrupt(InterruptEventData::new(
                    interrupt.clone(),
                )))
                .await;
        }
        let last_message = self
            .local
            .messages()
            .last()
            .cloned()
            .unwrap_or_else(|| assistant_text("Interrupted"));
        result(StopReason::Interrupt, last_message, invocation).with_interrupts(interrupts)
    }

    /// One model call with its hooks, retried while an `AfterModelCallEvent` hook asks to (TS
    /// `_invokeModel`).
    async fn invoke_model(
        &mut self,
        invocation: &Invocation,
        tool_choice: Option<ToolChoice>,
        structured_output_schema: Option<JsonValue>,
        yielder: &Yielder,
    ) -> Result<StreamAggregatedResult, Stop> {
        let hooks = self.hooks();
        let mut attempt_count = 1;
        loop {
            if invocation.cancel.is_cancelled() {
                return Err(Stop::Cancelled);
            }
            let model = Arc::clone(self.local.model());
            let projected_input_tokens = self.estimate_input_tokens().await;
            let mut before = BeforeModelCallEvent::new(
                &mut self.local,
                Arc::clone(&model),
                invocation.state.clone(),
                Some(invocation.context.clone()),
            );
            let mut data = BeforeModelCallEventData::new();
            if let Some(tokens) = projected_input_tokens {
                before = before.with_projected_input_tokens(tokens);
                data = data.with_projected_input_tokens(tokens);
            }
            hooks.invoke::<BeforeModelCallEvent<'static>>(&mut before).await;
            let cancel = before.cancel().cloned();
            yielder.emit(AgentStreamEvent::BeforeModelCall(data)).await;
            if let Some(cancel) = cancel {
                let message = assistant_text(cancel.message().unwrap_or(MODEL_CALL_DENIED_BY_HOOK));
                let stop_data = ModelStopData::new(message.clone(), StopReason::EndTurn);
                if self
                    .after_model_call(&model, attempt_count, Ok(stop_data), invocation, yielder)
                    .await
                {
                    attempt_count += 1;
                    continue;
                }
                return Ok(StreamAggregatedResult::new(message, StopReason::EndTurn));
            }
            let request =
                self.model_request(invocation, tool_choice.clone(), structured_output_schema.clone());
            let model_state = request.model_state.clone();
            match self.stream_from_model(&model, request, invocation, yielder).await {
                Ok(model_result) => {
                    model_proxy::record(&model_result, Some(&invocation.context));
                    self.local
                        .model_state()
                        .load_state_from_json(model_state.state_to_json());
                    let mut event = ModelMessageEvent::new(
                        &mut self.local,
                        model_result.message.clone(),
                        model_result.stop_reason.clone(),
                        invocation.state.clone(),
                        Some(invocation.context.clone()),
                    );
                    hooks.invoke::<ModelMessageEvent<'static>>(&mut event).await;
                    yielder
                        .emit(AgentStreamEvent::ModelMessage(ModelMessageEventData::new(
                            model_result.message.clone(),
                            model_result.stop_reason.clone(),
                        )))
                        .await;
                    if let Some(redaction) = &model_result.redaction {
                        self.redact_last_message(&redaction.user_message);
                    }
                    let mut stop_data =
                        ModelStopData::new(model_result.message.clone(), model_result.stop_reason.clone());
                    if let Some(redaction) = &model_result.redaction {
                        stop_data = stop_data.with_redaction(redaction.clone());
                    }
                    if self
                        .after_model_call(&model, attempt_count, Ok(stop_data), invocation, yielder)
                        .await
                    {
                        attempt_count += 1;
                        continue;
                    }
                    return Ok(model_result);
                }
                Err(Stop::Cancelled) => return Err(Stop::Cancelled),
                Err(Stop::Failed(error)) => {
                    if self
                        .after_model_call(&model, attempt_count, Err(&error), invocation, yielder)
                        .await
                    {
                        attempt_count += 1;
                        continue;
                    }
                    return Err(Stop::Failed(error));
                }
            }
        }
    }

    /// Fires `AfterModelCallEvent` and returns whether a hook asked to retry.
    async fn after_model_call(
        &mut self,
        model: &Arc<dyn Model>,
        attempt_count: u32,
        outcome: std::result::Result<ModelStopData, &Error>,
        invocation: &Invocation,
        yielder: &Yielder,
    ) -> bool {
        let hooks = self.hooks();
        let mut event = AfterModelCallEvent::new(
            &mut self.local,
            Arc::clone(model),
            invocation.state.clone(),
            Some(invocation.context.clone()),
            attempt_count,
        );
        let data = match outcome {
            Ok(stop_data) => {
                event = event.with_stop_data(stop_data.clone());
                AfterModelCallEventData::new(attempt_count, stop_data)
            }
            Err(error) => {
                event = event.with_error(error);
                AfterModelCallEventData::failed(attempt_count, EventError::new(error.to_string()))
            }
        };
        hooks.invoke::<AfterModelCallEvent<'static>>(&mut event).await;
        let retry = event.retry();
        yielder.emit(AgentStreamEvent::AfterModelCall(data)).await;
        retry
    }

    fn model_request(
        &self,
        invocation: &Invocation,
        tool_choice: Option<ToolChoice>,
        structured_output_schema: Option<JsonValue>,
    ) -> ModelRequest {
        let messages = fill_empty_assistant_messages(normalize_tool_use_names(self.local.messages()));
        let rendered = model_proxy::prepare(&messages);
        ModelRequest {
            messages: rendered.messages,
            system_prompt: self.local.system_prompt().cloned(),
            tool_specs: tool_specs(self),
            tool_choice,
            model_state: self.local.model_state().snapshot(),
            dynamic_trailing_blocks: model_proxy::dynamic_trailing_blocks(
                rendered.dynamic_trailing_blocks,
                None,
            ),
            agent_metadata: None,
            structured_output_schema,
            cancel_signal: Some(invocation.cancel.clone()),
        }
    }

    /// Streams one model call, firing the stream and content block events (TS `_streamFromModel`).
    async fn stream_from_model(
        &mut self,
        model: &Arc<dyn Model>,
        request: ModelRequest,
        invocation: &Invocation,
        yielder: &Yielder,
    ) -> Result<StreamAggregatedResult, Stop> {
        let hooks = self.hooks();
        let mut stream = model.stream_aggregated(request);
        loop {
            let item = tokio::select! {
                biased;
                () = invocation.cancel.cancelled() => return Err(Stop::Cancelled),
                item = stream.next() => item,
            };
            let Some(item) = item else {
                return Err(Error::from(ModelError::new("the model stream ended without a result")).into());
            };
            let item = match item {
                Ok(item) => item,
                Err(_) if invocation.cancel.is_cancelled() => return Err(Stop::Cancelled),
                Err(error) => return Err(error.into()),
            };
            match item {
                AggregatedEvent::ModelStreamEvent(event) => {
                    let mut hook_event = ModelStreamUpdateEvent::new(
                        &mut self.local,
                        event.clone(),
                        invocation.state.clone(),
                        Some(invocation.context.clone()),
                    );
                    hooks
                        .invoke::<ModelStreamUpdateEvent<'static>>(&mut hook_event)
                        .await;
                    yielder
                        .emit(AgentStreamEvent::ModelStreamUpdate(
                            ModelStreamUpdateEventData::new(event),
                        ))
                        .await;
                }
                AggregatedEvent::ContentBlock(block) => {
                    let mut hook_event = ContentBlockEvent::new(
                        &mut self.local,
                        block.clone(),
                        invocation.state.clone(),
                        Some(invocation.context.clone()),
                    );
                    hooks.invoke::<ContentBlockEvent<'static>>(&mut hook_event).await;
                    yielder
                        .emit(AgentStreamEvent::ContentBlock(ContentBlockEventData::new(block)))
                        .await;
                }
                AggregatedEvent::Done(result) => return Ok(*result),
            }
        }
    }

    /// The projected input tokens of the next call (TS `_estimateInputTokens`); `None` when the
    /// estimate fails.
    async fn estimate_input_tokens(&self) -> Option<u64> {
        let messages = self.local.messages();
        let model = self.local.model();
        let last_with_usage = messages.iter().rposition(|message| {
            message.role == Role::Assistant
                && message
                    .metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.usage.is_some())
        });
        let estimate = match last_with_usage {
            Some(index) => {
                let usage = messages[index]
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.usage)?;
                let baseline = usage.total_prompt_tokens().saturating_add(usage.output_tokens);
                let newer = &messages[index + 1..];
                if newer.is_empty() {
                    Ok(baseline)
                } else {
                    model_proxy::count_tokens(model.as_ref(), newer, CountTokensOptions::default())
                        .await
                        .map(|tokens| baseline.saturating_add(tokens))
                }
            }
            None => {
                let options = CountTokensOptions {
                    system_prompt: self.local.system_prompt().cloned(),
                    tool_specs: tool_specs(self),
                };
                model_proxy::count_tokens(model.as_ref(), messages, options).await
            }
        };
        estimate
            .inspect_err(|error| {
                tracing::debug!("error=<{error}> | token estimation failed, proceeding without estimate");
            })
            .ok()
    }

    /// Replaces the last user message's content with a guardrail's redaction text (TS
    /// `_redactLastMessage`).
    fn redact_last_message(&mut self, redaction: &str) {
        let Some(last) = self.local.messages_mut().last_mut() else {
            return;
        };
        if last.role != Role::User {
            tracing::warn!(
                "role=<assistant> | received input redaction but last message is not from user | redaction skipped"
            );
            return;
        }
        let mut content: Vec<ContentBlock> = last
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult(result) => Some(
                    ToolResultBlock::new(
                        result.tool_use_id.clone(),
                        result.status.clone(),
                        vec![ToolResultContent::from(redaction)],
                    )
                    .into(),
                ),
                _ => None,
            })
            .collect();
        if content.is_empty() {
            content.push(TextBlock::new(redaction).into());
        }
        last.content = content;
    }

    /// Runs the tools the model asked for (TS `executeTools`).
    async fn execute_tools(
        &mut self,
        assistant: &Message,
        invocation: &Invocation,
        yielder: &Yielder,
    ) -> Result<ToolsOutcome> {
        let hooks = self.hooks();
        let mut before = BeforeToolsEvent::new(
            &mut self.local,
            assistant.clone(),
            invocation.state.clone(),
            Some(invocation.context.clone()),
        );
        hooks.invoke::<BeforeToolsEvent<'static>>(&mut before).await;
        let raised = before.take_interrupts();
        let cancel = before.cancel().cloned();
        yielder
            .emit(AgentStreamEvent::BeforeTools(BeforeToolsEventData::new(
                assistant.clone(),
            )))
            .await;
        let interrupts = raised?;
        if !interrupts.is_empty() {
            return Ok(ToolsOutcome {
                message: Message::new(Role::User, Vec::new()),
                end_turn: None,
                interrupts,
            });
        }
        let tool_uses = tool_uses(assistant);
        let cancel_text = match cancel {
            Some(cancel) => Some(cancel.message().unwrap_or(TOOL_CANCELLED_BY_HOOK).to_owned()),
            None if invocation.cancel.is_cancelled() => Some(TOOL_EXECUTION_CANCELLED.to_owned()),
            None => None,
        };
        let (results, interrupts) = if tool_uses.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let (batch, mut records) = self.tool_batch(invocation);
            let executor = self.tool_executor;
            let batch_run = async {
                match cancel_text {
                    Some(text) => (cancel_all(&batch, &tool_uses, &text).await, Vec::new()),
                    None => execute_batch(executor, &batch, &tool_uses).await,
                }
            };
            forward_records(batch_run, &mut records, yielder).await
        };
        let message = Message::new(Role::User, results.into_iter().map(ContentBlock::from).collect());
        let mut after = AfterToolsEvent::new(
            &mut self.local,
            message.clone(),
            invocation.state.clone(),
            Some(invocation.context.clone()),
        );
        hooks.invoke::<AfterToolsEvent<'static>>(&mut after).await;
        let end_turn = after.end_turn().cloned().filter(ends_turn);
        yielder
            .emit(AgentStreamEvent::AfterTools(AfterToolsEventData::new(
                message.clone(),
            )))
            .await;
        if tool_uses.is_empty() {
            return Err(DataError::new(NO_TOOL_USE_BLOCKS).into());
        }
        Ok(ToolsOutcome {
            message,
            end_turn,
            interrupts,
        })
    }

    /// The shared state of one tool batch, and the receiver of its calls' stream records.
    fn tool_batch(&self, invocation: &Invocation) -> (Arc<ToolBatch>, mpsc::Receiver<AgentStreamEvent>) {
        let (sender, receiver) = mpsc::channel(RECORD_BUFFER);
        let hooks = self.hooks();
        let caller = PipelineToolCaller::new(
            Arc::clone(&hooks),
            invocation.state.clone(),
            invocation.cancel.clone(),
        );
        let agent = ToolAgent::new(ToolAgentParts {
            id: self.local.id().to_owned(),
            name: self.local.name().to_owned(),
            app_state: self.local.app_state().clone(),
            model: Arc::clone(self.local.model()),
            aux_model: Arc::clone(self.local.aux_model()),
            cancel_handle: invocation.cancel.handle(),
            messages: Arc::from(self.local.messages()),
            tool_registry: Arc::new(self.local.tool_registry().clone()),
            invocation_context: Some(invocation.context.clone()),
            tool_caller: caller,
        });
        let batch = ToolBatch::new(
            agent,
            hooks,
            invocation.state.clone(),
            invocation.cancel.clone(),
            Some(sender),
        );
        (batch, receiver)
    }
}

/// Runs `work` while forwarding the records its tool calls send, in order.
async fn forward_records<T>(
    work: impl Future<Output = T>,
    records: &mut mpsc::Receiver<AgentStreamEvent>,
    yielder: &Yielder,
) -> T {
    let mut work = std::pin::pin!(work);
    loop {
        tokio::select! {
            biased;
            Some(record) = records.recv() => yielder.emit(record).await,
            output = &mut work => {
                while let Ok(record) = records.try_recv() {
                    yielder.emit(record).await;
                }
                return output;
            }
        }
    }
}

async fn execute_batch(
    executor: ToolExecutor,
    batch: &Arc<ToolBatch>,
    tool_uses: &[ToolUse],
) -> (Vec<ToolResultBlock>, Vec<Interrupt>) {
    match executor {
        ToolExecutor::Sequential(sequential) => {
            let outcome = sequential.execute(batch, tool_uses).await;
            (
                outcome.results.into_iter().flatten().collect(),
                outcome.interrupts,
            )
        }
        ToolExecutor::Concurrent(concurrent) => {
            let outcome = concurrent.execute(batch, tool_uses).await;
            let results = outcome
                .results
                .into_iter()
                .zip(tool_uses)
                .map(|(result, tool_use)| {
                    result.unwrap_or_else(|| error_result(&tool_use.tool_use_id, TOOL_EXECUTION_INTERRUPTED))
                })
                .collect();
            (results, outcome.interrupts)
        }
    }
}

/// Every call of a skipped batch gets `text` as its error result (TS `_cancelAllAsResults`).
async fn cancel_all(batch: &ToolBatch, tool_uses: &[ToolUse], text: &str) -> Vec<ToolResultBlock> {
    let mut results = Vec::with_capacity(tool_uses.len());
    for tool_use in tool_uses {
        let result = error_result(&tool_use.tool_use_id, text);
        batch.finish(&result).await;
        results.push(result);
    }
    results
}

fn result(stop_reason: StopReason, last_message: Message, invocation: &Invocation) -> AgentResult {
    AgentResult::new(stop_reason, last_message)
        .with_invocation_context(invocation.context.clone())
        .with_invocation_state(invocation.state.clone())
}

fn assistant_text(text: &str) -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new(text).into()])
}

/// The final message of a cancelled invocation (TS `cancelResultMessage` or `Cancelled by user`).
fn cancel_message(cancel: &CancelSignal) -> Message {
    let reason = cancel.reason().filter(|reason| !reason.is_empty());
    assistant_text(reason.as_deref().unwrap_or(CANCELLED_BY_USER))
}

/// The messages an invocation's input adds (TS `_normalizeInput`).
fn normalize_input(args: InvokeArgs) -> Result<Vec<Message>> {
    let messages = match args {
        InvokeArgs::Text(text) => vec![Message::new(Role::User, vec![TextBlock::new(text).into()])],
        InvokeArgs::ContentBlocks(content) if content.is_empty() => Vec::new(),
        InvokeArgs::ContentBlocks(content) => vec![Message::new(Role::User, content)],
        InvokeArgs::Messages(messages) => messages,
        InvokeArgs::InterruptResponses(responses) if responses.is_empty() => Vec::new(),
        InvokeArgs::InterruptResponses(_) => {
            return Err(
                UnsupportedError::new("resuming from interrupts is not supported in Rust yet").into(),
            );
        }
    };
    let blank = messages
        .iter()
        .any(|message| message.role == Role::User && message.content.iter().all(is_blank_text));
    if blank {
        return Err(InvalidInputError::new(BLANK_PROMPT).into());
    }
    Ok(messages)
}

fn is_blank_text(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Text(text) if js_trim(&text.text).is_empty())
}

/// TS `_resolveTextTurn`: a turn without tool use either ends the invocation or, when structured
/// output is still missing, forces the structured output tool on the next turn.
fn resolve_text_turn(
    model_result: &StreamAggregatedResult,
    structured_output: Option<&StructuredOutputTool>,
    forced: bool,
    invocation: &Invocation,
) -> Result<TextTurn> {
    let done = |stop_reason: StopReason, structured_output| TextTurn::Done {
        stop_reason,
        structured_output,
    };
    let Some(tool) = structured_output else {
        return Ok(done(model_result.stop_reason.clone(), None));
    };
    if forced {
        match tool.result_from_text(&model_result.message) {
            Ok(output) => return Ok(done(model_result.stop_reason.clone(), Some(output))),
            Err(error) if !invocation.context.has_structured_output_attempts_limit() => {
                return Err(error.into());
            }
            Err(_) => {
                invocation.context.record_structured_output_failure();
                tracing::debug!(
                    "structured output retry failed | counted toward the structured output attempt limit"
                );
            }
        }
    }
    Ok(match invocation.context.reached_limit() {
        Some(limit) => done(limit, None),
        None => TextTurn::Retry,
    })
}

/// The structured output tool's successful JSON result (TS `_extractStructuredOutput`).
fn extract_structured_output(tool_use_message: &Message, tool_result_message: &Message) -> Option<JsonValue> {
    let ids = structured_output_tool_use_ids(tool_use_message);
    tool_result_message.content.iter().find_map(|block| match block {
        ContentBlock::ToolResult(result)
            if ids.contains(&result.tool_use_id.as_str())
                && result.status == crate::types::tools::ToolResultStatus::Success =>
        {
            match result.content.first() {
                Some(ToolResultContent::Json(json)) => Some(json.json.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}

/// Whether an `AfterToolsEvent` `end_turn` ends the turn (TS `true`, a non-empty string or block
/// list).
fn ends_turn(end_turn: &EndTurn) -> bool {
    match end_turn {
        EndTurn::Message(text) => !text.is_empty(),
        EndTurn::Content(content) => !content.is_empty(),
        EndTurn::DefaultMessage => true,
    }
}

fn end_turn_content(end_turn: EndTurn) -> Vec<ContentBlock> {
    match end_turn {
        EndTurn::Content(content) => content,
        EndTurn::Message(text) => vec![TextBlock::new(text).into()],
        EndTurn::DefaultMessage => vec![TextBlock::new(TURN_ENDED_BY_HOOK).into()],
    }
}

fn tool_uses(message: &Message) -> Vec<ToolUse> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => Some(ToolUse::new(
                tool_use.name.clone(),
                tool_use.tool_use_id.clone(),
                tool_use.input.clone(),
            )),
            _ => None,
        })
        .collect()
}

fn tool_specs(agent: &Agent) -> Vec<crate::types::tools::ToolSpec> {
    agent
        .local
        .tool_registry()
        .list()
        .iter()
        .map(|tool| tool.tool_spec())
        .collect()
}

/// Replaces tool names a provider would reject (TS `normalizeToolUseNames`).
fn normalize_tool_use_names(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|message| {
            if message.role != Role::Assistant {
                return message.clone();
            }
            let mut message = message.clone();
            for block in &mut message.content {
                if let ContentBlock::ToolUse(tool_use) = block
                    && !is_valid_tool_name(&tool_use.name)
                {
                    tracing::debug!(
                        "tool_name=<{}> | replacing invalid tool name with {INVALID_TOOL_NAME_PLACEHOLDER}",
                        tool_use.name
                    );
                    INVALID_TOOL_NAME_PLACEHOLDER.clone_into(&mut tool_use.name);
                }
            }
            message
        })
        .collect()
}

/// Gives empty assistant messages a placeholder text block (TS `fillEmptyAssistantMessages`).
fn fill_empty_assistant_messages(mut messages: Vec<Message>) -> Vec<Message> {
    for message in &mut messages {
        if message.role == Role::Assistant && message.content.is_empty() {
            tracing::debug!("replacing empty assistant message content with blank text placeholder");
            message.content = vec![TextBlock::new(BLANK_TEXT_PLACEHOLDER).into()];
        }
    }
    messages
}
