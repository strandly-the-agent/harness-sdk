//! The agent loop (TS `agent.ts` `_streamInvocation`, `_stream`, `_invokeModel`, `executeTools`).
//!
//! Each step fires its hook event, then hands the event's record to the stream. The assistant
//! message that requested tools is appended only after its tools finish, together with their
//! results, so history is always re-invokable.

use std::sync::Arc;

use futures::StreamExt;
use indexmap::IndexMap;

use crate::agent::Agent;
use crate::agent::stream::Emitter;
use crate::error::{Error, Result};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolsEvent, AgentResultEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolsEvent, ContentBlockEvent, HookRegistry, InterruptEvent,
    MessageAddedEvent, ModelMessageEvent, ModelStopData, ModelStreamUpdateEvent, ToolResultEvent,
};
use crate::interrupt::Interrupt;
use crate::models::model::{
    AggregatedEvent, CountTokensOptions, Model, ModelRequest, StreamAggregatedResult,
};
use crate::tools::executors::executor::{
    ToolExecutionInput, ToolExecutorEvent, ToolExecutorItem, error_text_result,
};
use crate::tools::tool::is_valid_tool_name;
use crate::types::agent::{AgentResult, AgentStreamEvent, InvocationState, InvokeArgs, InvokeOptions};
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, TextBlock, ToolResultBlock, ToolUseBlock,
};
use crate::types::tools::ToolSpec;

/// TS `INVALID_TOOL_NAME_PLACEHOLDER`.
const INVALID_TOOL_NAME_PLACEHOLDER: &str = "INVALID_TOOL_NAME";
/// TS `BLANK_TEXT_PLACEHOLDER`.
const BLANK_TEXT_PLACEHOLDER: &str = "[blank text]";

/// How a cycle ended.
enum Cycle {
    /// Tools ran; call the model again.
    Continue,
    /// The invocation is over.
    Done(Box<AgentResult>),
}

/// What a tool batch produced.
struct ToolsOutcome {
    message: Message,
    interrupts: Vec<Interrupt>,
}

impl Agent {
    /// One invocation (TS `_streamInvocation`).
    pub(crate) async fn run(
        &mut self,
        args: InvokeArgs,
        options: InvokeOptions,
        out: &Emitter,
    ) -> Result<()> {
        self.initialize().await?;
        let hooks = self.local.hooks().clone();
        let state = options.invocation_state;
        self.local.set_invocation_cancel(Default::default());

        if matches!(args, InvokeArgs::InterruptResponses(_)) {
            return Err(Error::other(
                "resuming from interrupt responses is not supported yet",
            ));
        }
        let input = normalize_input(args)?;

        let mut before = BeforeInvocationEvent::new(&mut self.local, state.clone(), input);
        hooks
            .invoke::<BeforeInvocationEvent<'static>>(&mut before)
            .await?;
        out.emit(AgentStreamEvent::BeforeInvocation((&before).into()))
            .await;
        let (input, cancel) = before.into_parts();

        let outcome = match cancel {
            Some(cancel) => {
                // TS skips `MessageAddedEvent` and `AgentResultEvent` hooks here (see ts-bugs).
                let message = assistant_text(cancel.message().unwrap_or("invocation denied by hook"));
                self.append_message(message.clone(), &state, &hooks, out).await?;
                Ok(self.result(StopReason::EndTurn, message, &state))
            }
            None => self.run_loop(input, &state, &hooks, out).await,
        };
        self.after_invocation(&hooks, &state, out).await?;
        let result = outcome?;

        let mut event = AgentResultEvent::new(&mut self.local, result, state);
        hooks.invoke::<AgentResultEvent<'static>>(&mut event).await?;
        out.emit(AgentStreamEvent::AgentResult(event.result().clone()))
            .await;
        Ok(())
    }

    async fn after_invocation(
        &mut self,
        hooks: &HookRegistry,
        state: &InvocationState,
        out: &Emitter,
    ) -> Result<()> {
        let mut event = AfterInvocationEvent::new(&mut self.local, state.clone());
        hooks.invoke::<AfterInvocationEvent<'static>>(&mut event).await?;
        out.emit(AgentStreamEvent::AfterInvocation((&event).into())).await;
        Ok(())
    }

    /// The cycles of one invocation (TS `_stream`).
    async fn run_loop(
        &mut self,
        input: Vec<Message>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<AgentResult> {
        if self.local.interrupt_state().lock().activated {
            return Err(Error::other(
                "Agent is in an interrupted state. Resume by invoking with interruptResponse content blocks.",
            ));
        }
        {
            let mut telemetry = self.telemetry.lock();
            telemetry.meter.start_new_invocation();
            telemetry.tracer.reset();
        }
        let mut input = Some(input);
        loop {
            let started = {
                let mut telemetry = self.telemetry.lock();
                let (cycle_id, started) = telemetry.meter.start_cycle();
                telemetry.tracer.start_cycle(&cycle_id);
                started
            };
            let cycle = self.run_cycle(input.take(), state, hooks, out).await;
            {
                let mut telemetry = self.telemetry.lock();
                telemetry.meter.end_cycle(started);
                telemetry.tracer.end_cycle();
            }
            match cycle? {
                Cycle::Continue => {}
                Cycle::Done(result) => return Ok(self.with_telemetry(*result)),
            }
        }
    }

    async fn run_cycle(
        &mut self,
        input: Option<Vec<Message>>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<Cycle> {
        for message in input.into_iter().flatten() {
            self.append_message(message, state, hooks, out).await?;
        }

        let model_result = self.invoke_model(state, hooks, out).await?;
        let stop_reason = model_result.stop_reason().clone();
        let assistant = model_result.into_message();
        if stop_reason != StopReason::ToolUse {
            self.append_message(assistant.clone(), state, hooks, out).await?;
            return Ok(Cycle::Done(Box::new(AgentResult::new(
                stop_reason,
                assistant,
                state.clone(),
            ))));
        }

        let tools = self.execute_tools(&assistant, state, hooks, out).await?;
        if !tools.interrupts.is_empty() {
            return self
                .interrupted(tools.interrupts, state, hooks, out)
                .await
                .map(|result| Cycle::Done(Box::new(result)));
        }
        self.append_message(assistant, state, hooks, out).await?;
        self.append_message(tools.message, state, hooks, out).await?;
        let mut interrupt_state = self.local.interrupt_state().lock();
        interrupt_state.pending_tool_execution = None;
        interrupt_state.activated = false;
        Ok(Cycle::Continue)
    }

    /// TS `_stream`'s `InterruptError` handler: fires one `InterruptEvent` per interrupt.
    async fn interrupted(
        &mut self,
        interrupts: Vec<Interrupt>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<AgentResult> {
        {
            let mut interrupt_state = self.local.interrupt_state().lock();
            for interrupt in &interrupts {
                interrupt_state
                    .interrupts
                    .entry(interrupt.id().to_owned())
                    .or_insert_with(|| interrupt.clone());
            }
            interrupt_state.activated = true;
        }
        for interrupt in interrupts {
            let mut event = InterruptEvent::new(&mut self.local, interrupt, state.clone());
            hooks.invoke::<InterruptEvent<'static>>(&mut event).await?;
            out.emit(AgentStreamEvent::Interrupt((&event).into())).await;
        }
        let last_message = self
            .local
            .messages()
            .last()
            .cloned()
            .unwrap_or_else(|| assistant_text("Interrupted"));
        let unanswered = self
            .local
            .interrupt_state()
            .lock()
            .unanswered()
            .cloned()
            .collect();
        Ok(AgentResult::new(StopReason::Interrupt, last_message, state.clone()).with_interrupts(unanswered))
    }

    /// One model call with its hooks, repeated while an `AfterModelCallEvent` hook asks for a
    /// retry (TS `_invokeModel`).
    async fn invoke_model(
        &mut self,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<StreamAggregatedResult> {
        let tool_specs = self.local.tool_registry().tool_specs();
        let mut attempt_count = 1_u32;
        loop {
            let model = Arc::clone(self.local.model());
            let projected = self.estimate_input_tokens(&tool_specs).await;
            let mut before =
                BeforeModelCallEvent::new(&mut self.local, Arc::clone(&model), state.clone(), projected);
            hooks.invoke::<BeforeModelCallEvent<'static>>(&mut before).await?;
            out.emit(AgentStreamEvent::BeforeModelCall((&before).into()))
                .await;

            if let Some(cancel) = before.take_cancel() {
                let message = assistant_text(cancel.message().unwrap_or("model call denied by hook"));
                let stop_data = ModelStopData::new(message.clone(), StopReason::EndTurn);
                let mut after =
                    AfterModelCallEvent::new(&mut self.local, model, state.clone(), attempt_count)
                        .with_stop_data(stop_data);
                hooks.invoke::<AfterModelCallEvent<'static>>(&mut after).await?;
                out.emit(AgentStreamEvent::AfterModelCall((&after).into())).await;
                if after.retry() {
                    attempt_count = attempt_count.saturating_add(1);
                    continue;
                }
                return Ok(StreamAggregatedResult::new(message, StopReason::EndTurn));
            }

            match self
                .stream_from_model(&model, tool_specs.clone(), state, hooks, out)
                .await
            {
                Ok(result) => {
                    self.telemetry.lock().meter.update_cycle(result.metadata());
                    let message = result.message().clone();
                    let stop_reason = result.stop_reason().clone();
                    let mut event = ModelMessageEvent::new(
                        &mut self.local,
                        message.clone(),
                        stop_reason.clone(),
                        state.clone(),
                    );
                    hooks.invoke::<ModelMessageEvent<'static>>(&mut event).await?;
                    out.emit(AgentStreamEvent::ModelMessage((&event).into())).await;
                    if let Some(redaction) = result.redaction() {
                        self.redact_last_message(redaction.user_message());
                    }

                    let mut stop_data = ModelStopData::new(message, stop_reason);
                    if let Some(redaction) = result.redaction() {
                        stop_data = stop_data.with_redaction(redaction.clone());
                    }
                    let mut after =
                        AfterModelCallEvent::new(&mut self.local, model, state.clone(), attempt_count)
                            .with_stop_data(stop_data);
                    hooks.invoke::<AfterModelCallEvent<'static>>(&mut after).await?;
                    out.emit(AgentStreamEvent::AfterModelCall((&after).into())).await;
                    if after.retry() {
                        attempt_count = attempt_count.saturating_add(1);
                        continue;
                    }
                    return Ok(result);
                }
                Err(error) => {
                    let mut after =
                        AfterModelCallEvent::new(&mut self.local, model, state.clone(), attempt_count)
                            .with_error(&error);
                    hooks.invoke::<AfterModelCallEvent<'static>>(&mut after).await?;
                    out.emit(AgentStreamEvent::AfterModelCall((&after).into())).await;
                    if after.retry() {
                        attempt_count = attempt_count.saturating_add(1);
                        continue;
                    }
                    return Err(error);
                }
            }
        }
    }

    /// Streams one model call, firing the stream and content block events (TS `_streamFromModel`).
    async fn stream_from_model(
        &mut self,
        model: &Arc<dyn Model>,
        tool_specs: Vec<ToolSpec>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<StreamAggregatedResult> {
        // The provider writes to a snapshot, committed only when the call succeeds.
        let model_state = self.local.model_state().snapshot();
        let request = ModelRequest::builder()
            .messages(fill_empty_assistant_messages(normalize_tool_use_names(
                self.local.messages(),
            )))
            .maybe_system_prompt(self.local.system_prompt().cloned())
            .tool_specs(tool_specs)
            .model_state(model_state.clone())
            .cancel_signal(self.local.cancel_signal().clone())
            .build();
        self.telemetry.lock().tracer.start_model();
        let mut stream = model.stream_aggregated(request);
        let mut result = None;
        while let Some(item) = stream.next().await {
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    self.telemetry.lock().tracer.end_model(None);
                    return Err(error);
                }
            };
            match item {
                AggregatedEvent::ModelStreamEvent(event) => {
                    let mut event = ModelStreamUpdateEvent::new(&mut self.local, event, state.clone());
                    hooks
                        .invoke::<ModelStreamUpdateEvent<'static>>(&mut event)
                        .await?;
                    out.emit(AgentStreamEvent::ModelStreamUpdate((&event).into()))
                        .await;
                }
                AggregatedEvent::ContentBlock(block) => {
                    let mut event = ContentBlockEvent::new(&mut self.local, block, state.clone());
                    hooks.invoke::<ContentBlockEvent<'static>>(&mut event).await?;
                    out.emit(AgentStreamEvent::ContentBlock((&event).into())).await;
                }
                AggregatedEvent::Done(done) => result = Some(*done),
            }
        }
        let result = result.ok_or_else(|| Error::other("the model stream ended without a result"))?;
        self.telemetry.lock().tracer.end_model(Some(result.message()));
        self.local.model_state().load_json(model_state.to_json());
        Ok(result)
    }

    /// Runs the tool calls of `assistant` (TS `executeTools`). `AfterToolsEvent` fires even when
    /// the batch fails.
    async fn execute_tools(
        &mut self,
        assistant: &Message,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<ToolsOutcome> {
        let mut before = BeforeToolsEvent::new(&mut self.local, assistant.clone(), state.clone());
        hooks.invoke::<BeforeToolsEvent<'static>>(&mut before).await?;
        let hook_interrupts = before.take_interrupts();
        if !hook_interrupts.is_empty() {
            // TS throws from the hook before yielding the event; the batch is stored for resume.
            self.store_pending(assistant);
            return Ok(ToolsOutcome {
                message: Message::new(Role::User, Vec::new()),
                interrupts: hook_interrupts,
            });
        }
        let cancel = before.take_cancel();
        out.emit(AgentStreamEvent::BeforeTools((&before).into())).await;

        let tool_uses: Vec<ToolUseBlock> = assistant
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tool_use) => Some(tool_use.clone()),
                _ => None,
            })
            .collect();
        let mut results: Vec<ToolResultBlock> = Vec::new();
        let mut interrupts = Vec::new();
        let mut failure = None;
        if tool_uses.is_empty() {
            failure = Some(Error::other(
                "Model indicated toolUse but no tool use blocks found in message",
            ));
        } else if let Some(cancel) = cancel {
            let text = cancel.message().unwrap_or("Tool cancelled by hook");
            let agent = self.local.tool_agent();
            for tool_use in &tool_uses {
                let result = error_text_result(tool_use.tool_use_id(), text);
                let mut event = ToolResultEvent::new(agent.clone(), result.clone(), state.clone());
                hooks.invoke::<ToolResultEvent>(&mut event).await?;
                out.emit(AgentStreamEvent::ToolResult((&event).into())).await;
                results.push(result);
            }
        } else {
            let input = ToolExecutionInput {
                agent: self.local.tool_agent(),
                hooks: hooks.clone(),
                tool_uses,
                invocation_state: state.clone(),
                assistant_message: assistant.clone(),
                completed_tool_results: IndexMap::new(),
                telemetry: self.telemetry.clone(),
            };
            let mut stream = self.tool_executor.execute(input);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(ToolExecutorItem::Event(event)) => out.emit(stream_event(&event)).await,
                    Ok(ToolExecutorItem::Done(batch)) => {
                        results = batch.results;
                        interrupts = batch.interrupts;
                    }
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
        }

        let message = Message::new(
            Role::User,
            results.into_iter().map(ContentBlock::ToolResult).collect(),
        );
        let mut after = AfterToolsEvent::new(&mut self.local, message, state.clone());
        hooks.invoke::<AfterToolsEvent<'static>>(&mut after).await?;
        out.emit(AgentStreamEvent::AfterTools((&after).into())).await;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(ToolsOutcome {
            message: after.message().clone(),
            interrupts,
        })
    }

    fn store_pending(&self, assistant: &Message) {
        self.local.interrupt_state().lock().pending_tool_execution =
            Some(crate::interrupt::state::PendingToolExecution {
                assistant_message_data: assistant.clone(),
                completed_tool_results: IndexMap::new(),
            });
    }

    /// Appends a message and fires `MessageAddedEvent` (TS `_appendMessage`).
    async fn append_message(
        &mut self,
        message: Message,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<()> {
        self.local.messages_mut().push(message.clone());
        let mut event = MessageAddedEvent::new(&mut self.local, message, state.clone());
        hooks.invoke::<MessageAddedEvent<'static>>(&mut event).await?;
        out.emit(AgentStreamEvent::MessageAdded((&event).into())).await;
        Ok(())
    }

    /// The input-token estimate for the next call (TS `_estimateInputTokens`): the last
    /// assistant message's reported usage plus a count of what followed it, else a full count.
    async fn estimate_input_tokens(&self, tool_specs: &[ToolSpec]) -> Option<u64> {
        let messages = self.local.messages();
        let model = self.local.model();
        let baseline = messages.iter().enumerate().rev().find_map(|(index, message)| {
            let usage = message.metadata()?.usage()?;
            (message.role() == Role::Assistant).then(|| {
                (
                    index,
                    usage.total_prompt_tokens().saturating_add(usage.output_tokens()),
                )
            })
        });
        let estimate = match baseline {
            Some((index, known)) => match messages.get(index.saturating_add(1)..) {
                Some(newer) if !newer.is_empty() => model
                    .count_tokens(newer, Default::default())
                    .await
                    .map(|count| known.saturating_add(count)),
                _ => Ok(known),
            },
            None => {
                let options = CountTokensOptions {
                    system_prompt: self.local.system_prompt().cloned(),
                    tool_specs: tool_specs.to_vec(),
                };
                model.count_tokens(messages, options).await
            }
        };
        estimate
            .inspect_err(|error| {
                tracing::debug!("error=<{error}> | token estimation failed, proceeding without estimate");
            })
            .ok()
    }

    /// Replaces the last user message with a guardrail's redaction (TS `_redactLastMessage`).
    fn redact_last_message(&mut self, redact_message: &str) {
        let Some(last) = self.local.messages_mut().last_mut() else {
            return;
        };
        if last.role() != Role::User {
            tracing::warn!(
                "role=<{}> | received input redaction but last message is not from user | redaction skipped",
                last.role()
            );
            return;
        }
        let mut content: Vec<ContentBlock> = last
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult(result) => Some(
                    ToolResultBlock::new(
                        result.tool_use_id(),
                        result.status().clone(),
                        vec![redact_message.into()],
                    )
                    .into(),
                ),
                _ => None,
            })
            .collect();
        if content.is_empty() {
            content.push(TextBlock::new(redact_message).into());
        }
        *last = Message::new(Role::User, content).with_tracking_id(last.tracking_id());
    }

    fn result(&self, stop_reason: StopReason, last_message: Message, state: &InvocationState) -> AgentResult {
        self.with_telemetry(AgentResult::new(stop_reason, last_message, state.clone()))
    }

    fn with_telemetry(&self, result: AgentResult) -> AgentResult {
        let telemetry = self.telemetry.lock();
        result.with_telemetry(
            telemetry.meter.metrics().clone(),
            telemetry.tracer.traces().to_vec(),
        )
    }
}

/// The record of a per-tool event; the executor already ran its hooks.
fn stream_event(event: &ToolExecutorEvent) -> AgentStreamEvent {
    match event {
        ToolExecutorEvent::BeforeToolCall(event) => AgentStreamEvent::BeforeToolCall(event.into()),
        ToolExecutorEvent::ToolStreamUpdate(event) => AgentStreamEvent::ToolStreamUpdate(event.into()),
        ToolExecutorEvent::AfterToolCall(event) => AgentStreamEvent::AfterToolCall(event.into()),
        ToolExecutorEvent::ToolResult(event) => AgentStreamEvent::ToolResult(event.into()),
    }
}

fn assistant_text(text: &str) -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new(text).into()])
}

/// The messages `args` append (TS `_normalizeInput`).
fn normalize_input(args: InvokeArgs) -> Result<Vec<Message>> {
    let messages = match args {
        InvokeArgs::Text(text) => vec![Message::new(Role::User, vec![TextBlock::new(text).into()])],
        InvokeArgs::Content(content) if content.is_empty() => Vec::new(),
        InvokeArgs::Content(content) => vec![Message::new(Role::User, content)],
        InvokeArgs::Messages(messages) => messages,
        InvokeArgs::InterruptResponses(_) => Vec::new(),
    };
    let blank = |message: &Message| {
        message.role() == Role::User
            && message
                .content()
                .iter()
                .all(|block| matches!(block, ContentBlock::Text(text) if text.text().trim().is_empty()))
    };
    if messages.iter().any(blank) {
        return Err(Error::other(
            "Input prompt is blank. Provide non-blank text or at least one non-text content block.",
        ));
    }
    Ok(messages)
}

/// Replaces tool-use names providers would reject (TS `normalizeToolUseNames`).
fn normalize_tool_use_names(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|message| {
            let invalid = |block: &ContentBlock| matches!(block, ContentBlock::ToolUse(tool_use) if !is_valid_tool_name(tool_use.name()));
            if message.role() != Role::Assistant || !message.content().iter().any(invalid) {
                return message.clone();
            }
            let content = message
                .content()
                .iter()
                .map(|block| match block {
                    ContentBlock::ToolUse(tool_use) if !is_valid_tool_name(tool_use.name()) => {
                        tracing::debug!(
                            "tool_name=<{}> | replacing invalid tool name with {INVALID_TOOL_NAME_PLACEHOLDER}",
                            tool_use.name()
                        );
                        let renamed = ToolUseBlock::new(
                            INVALID_TOOL_NAME_PLACEHOLDER,
                            tool_use.tool_use_id(),
                            tool_use.input().clone(),
                        );
                        match tool_use.reasoning_signature() {
                            Some(signature) => renamed.with_reasoning_signature(signature).into(),
                            None => renamed.into(),
                        }
                    }
                    _ => block.clone(),
                })
                .collect();
            rebuild(message, content)
        })
        .collect()
}

/// Sends empty assistant messages as a blank text placeholder (TS `fillEmptyAssistantMessages`).
fn fill_empty_assistant_messages(messages: Vec<Message>) -> Vec<Message> {
    let empty = |message: &Message| message.role() == Role::Assistant && message.content().is_empty();
    if !messages.iter().any(empty) {
        return messages;
    }
    tracing::debug!("replacing empty assistant message content with blank text placeholder");
    messages
        .into_iter()
        .map(|message| {
            if empty(&message) {
                rebuild(&message, vec![TextBlock::new(BLANK_TEXT_PLACEHOLDER).into()])
            } else {
                message
            }
        })
        .collect()
}

/// `message` with new content, keeping its role and metadata (TS `new Message({role, content,
/// metadata})`, which mints a new tracking id).
fn rebuild(message: &Message, content: Vec<ContentBlock>) -> Message {
    let rebuilt = Message::new(message.role(), content);
    match message.metadata() {
        Some(metadata) => rebuilt.with_metadata(metadata.clone()),
        None => rebuilt,
    }
}

#[cfg(test)]
mod tests;
