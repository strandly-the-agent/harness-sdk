//! The agent loop (TS `agent.ts` `_streamInvocation`, `_stream`, `_invokeModel`, `executeTools`).
//!
//! Each step fires its hook event, then hands the event's record to the stream. The assistant
//! message that requested tools is appended only after its tools finish, together with their
//! results, so history is always re-invokable.

use std::sync::Arc;

use futures::StreamExt;
use indexmap::IndexMap;

use crate::agent::stream::Emitter;
use crate::agent::{Agent, InvocationContext};
use crate::error::{Error, Result};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolsEvent, AgentResultEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolsEvent, ContentBlockEvent, EndTurn, HookRegistry, InterruptEvent,
    MessageAddedEvent, ModelMessageEvent, ModelStopData, ModelStreamUpdateEvent, ToolResultEvent,
};
use crate::interrupt::Interrupt;
use crate::models::model::{
    AggregatedEvent, CountTokensOptions, Model, ModelRequest, StreamAggregatedResult,
};
use crate::tools::executors::executor::{
    ToolExecutionInput, ToolExecutorEvent, ToolExecutorItem, error_text_result,
};
use crate::tools::structured_output::{
    STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema, StructuredOutputTool, structured_output_tool_use_ids,
};
use crate::tools::tool::is_valid_tool_name;
use crate::types::agent::{AgentResult, AgentStreamEvent, InvocationState, InvokeArgs, InvokeOptions};
use crate::types::json::JsonValue;
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

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

/// The structured output state of one invocation.
struct Structured {
    tool: Option<StructuredOutputTool>,
    /// Set once a text turn forces the structured output tool; stays for the invocation.
    choice: Option<ToolChoice>,
}

/// The structured output retry's request fields: the forced tool and the schema for providers
/// with native structured output.
#[derive(Clone, Default)]
struct Forcing {
    tool_choice: Option<ToolChoice>,
    structured_output_schema: Option<JsonValue>,
}

/// How a turn that ended without a tool call continues (TS `TextTurnOutcome`).
enum TextTurn {
    /// Retry with the structured output tool forced.
    Retry,
    /// End with this stop reason and any structured output.
    End(StopReason, Option<JsonValue>),
}

/// What a tool batch produced.
struct ToolsOutcome {
    message: Message,
    interrupts: Vec<Interrupt>,
    end_turn: Option<EndTurn>,
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
        let schema = options
            .structured_output_schema
            .or_else(|| self.structured_output_schema.clone());
        let context = InvocationContext::resolve(options.invocation_context, options.limits);
        self.local.set_invocation_context(Some(context.clone()));

        // A hook's `AfterInvocationEvent::set_resume` runs another pass (TS `stream`'s loop).
        let mut args = args;
        let result = loop {
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

            if let Some(cancel) = cancel {
                // TS skips `MessageAddedEvent` and `AgentResultEvent` hooks here (see ts-bugs).
                let message = assistant_text(cancel.message().unwrap_or("invocation denied by hook"));
                self.append_message(message.clone(), &state, &hooks, out).await?;
                let result = self.result(StopReason::EndTurn, message, &state);
                // TS returns here without reading `resume`.
                self.after_invocation(&hooks, &state, out).await?;
                break result;
            }
            let outcome = self.run_loop(input, &state, &hooks, out, schema.clone()).await;
            let resume = self.after_invocation(&hooks, &state, out).await?;
            let result = outcome?;
            match resume {
                Some(resume) if should_resume(&context) => args = resume,
                _ => break result,
            }
        };

        let mut event = AgentResultEvent::new(&mut self.local, result, state);
        hooks.invoke::<AgentResultEvent<'static>>(&mut event).await?;
        out.emit(AgentStreamEvent::AgentResult(event.result().clone()))
            .await;
        Ok(())
    }

    /// Fires `AfterInvocationEvent`; returns the input a hook asked to resume with.
    async fn after_invocation(
        &mut self,
        hooks: &HookRegistry,
        state: &InvocationState,
        out: &Emitter,
    ) -> Result<Option<InvokeArgs>> {
        let mut event = AfterInvocationEvent::new(&mut self.local, state.clone());
        hooks.invoke::<AfterInvocationEvent<'static>>(&mut event).await?;
        out.emit(AgentStreamEvent::AfterInvocation((&event).into())).await;
        Ok(event.take_resume())
    }

    /// The cycles of one invocation (TS `_stream`).
    async fn run_loop(
        &mut self,
        input: Vec<Message>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
        schema: Option<StructuredOutputSchema>,
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
        let mut structured = Structured {
            tool: schema.map(StructuredOutputTool::new),
            choice: None,
        };
        if let Some(tool) = &structured.tool {
            self.local.tool_registry_mut().add(Arc::new(tool.clone()))?;
        }
        let outcome = self.cycles(input, state, hooks, out, &mut structured).await;
        if structured.tool.is_some() {
            self.local.tool_registry_mut().remove(STRUCTURED_OUTPUT_TOOL_NAME);
        }
        outcome
    }

    async fn cycles(
        &mut self,
        input: Vec<Message>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
        structured: &mut Structured,
    ) -> Result<AgentResult> {
        let context = self
            .local
            .invocation_context()
            .cloned()
            .unwrap_or_else(|| InvocationContext::new(None, None, false));
        let message_count_at_start = self.local.messages().len();
        let mut input = Some(input);
        loop {
            if let Some(stop_reason) = context.reached_limit() {
                let last_message = self
                    .local
                    .messages()
                    .get(message_count_at_start..)
                    .and_then(<[Message]>::last)
                    .cloned()
                    .unwrap_or_else(|| Message::new(Role::Assistant, Vec::new()));
                return Ok(self.result(stop_reason, last_message, state));
            }
            // Counted before the turn runs, so nested calls started by its tools see it.
            context.record_turn();
            let started = {
                let mut telemetry = self.telemetry.lock();
                let (cycle_id, started) = telemetry.meter.start_cycle();
                telemetry.tracer.start_cycle(&cycle_id);
                started
            };
            let cycle = self
                .run_cycle(input.take(), state, hooks, out, structured, &context)
                .await;
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
        structured: &mut Structured,
        context: &InvocationContext,
    ) -> Result<Cycle> {
        for message in input.into_iter().flatten() {
            self.append_message(message, state, hooks, out).await?;
        }

        let forced = structured.choice.clone();
        let native_schema = forced
            .as_ref()
            .and(structured.tool.as_ref())
            .map(|tool| tool.schema().json_schema().clone());
        let forcing = Forcing {
            tool_choice: forced.clone(),
            structured_output_schema: native_schema,
        };
        let model_result = self.invoke_model(state, hooks, out, forcing).await?;
        let stop_reason = model_result.stop_reason().clone();
        let assistant = model_result.into_message();
        if stop_reason != StopReason::ToolUse {
            let (stop_reason, output) = match resolve_text_turn(
                &assistant,
                stop_reason,
                structured,
                forced.is_some(),
                context,
            )? {
                TextTurn::Retry => {
                    // Appending the text turn would end history on an assistant message,
                    // which providers reject as prefill.
                    structured.choice = Some(ToolChoice::tool(STRUCTURED_OUTPUT_TOOL_NAME));
                    tracing::debug!(
                        "structured output schema set but model responded with plain text; forcing tool use on next cycle"
                    );
                    return Ok(Cycle::Continue);
                }
                TextTurn::End(stop_reason, output) => (stop_reason, output),
            };
            self.append_message(assistant.clone(), state, hooks, out).await?;
            let mut result = AgentResult::new(stop_reason, assistant, state.clone());
            if let Some(output) = output {
                result = result.with_structured_output(output);
            }
            return Ok(Cycle::Done(Box::new(result)));
        }
        // Counted when the response arrives, so an interrupt before its tools run keeps the count.
        if forced.is_some() && structured_output_tool_use_ids(&assistant).is_empty() {
            context.record_structured_output_failure();
        }

        let tools = self.execute_tools(&assistant, state, hooks, out).await?;
        if !tools.interrupts.is_empty() {
            return self
                .interrupted(tools.interrupts, state, hooks, out)
                .await
                .map(|result| Cycle::Done(Box::new(result)));
        }
        // Extracted before the appends: message-added hooks may rewrite the JSON block.
        let output = structured
            .tool
            .as_ref()
            .and_then(|_| extract_structured_output(&assistant, &tools.message));
        self.append_message(assistant.clone(), state, hooks, out).await?;
        self.append_message(tools.message, state, hooks, out).await?;
        {
            let mut interrupt_state = self.local.interrupt_state().lock();
            interrupt_state.pending_tool_execution = None;
            interrupt_state.activated = false;
        }
        if structured.tool.is_some()
            && output.is_none()
            && !structured_output_tool_use_ids(&assistant).is_empty()
        {
            context.record_structured_output_failure();
        }

        if let Some(content) = tools.end_turn.and_then(end_turn_content) {
            let last_message = Message::new(Role::Assistant, content);
            self.append_message(last_message.clone(), state, hooks, out)
                .await?;
            return Ok(Cycle::Done(Box::new(AgentResult::new(
                StopReason::EndTurn,
                last_message,
                state.clone(),
            ))));
        }
        match output {
            Some(output) => Ok(Cycle::Done(Box::new(
                AgentResult::new(StopReason::ToolUse, assistant, state.clone())
                    .with_structured_output(output),
            ))),
            None => Ok(Cycle::Continue),
        }
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
        forcing: Forcing,
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
                .stream_from_model(&model, tool_specs.clone(), forcing.clone(), state, hooks, out)
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
        forcing: Forcing,
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
            .maybe_tool_choice(forcing.tool_choice)
            .maybe_structured_output_schema(forcing.structured_output_schema)
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
        if let (Some(usage), Some(context)) = (
            result.metadata().and_then(|metadata| metadata.usage()),
            self.local.invocation_context(),
        ) {
            context.record_usage(usage);
        }
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
                end_turn: None,
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
            end_turn: after.take_end_turn(),
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
        let result = result.with_telemetry(
            telemetry.meter.metrics().clone(),
            telemetry.tracer.traces().to_vec(),
        );
        match self.local.invocation_context() {
            Some(context) => result.with_invocation_context(context.clone()),
            None => result,
        }
    }
}

/// TS `_shouldResume`: a resume never starts another pass once a limit is reached.
fn should_resume(context: &InvocationContext) -> bool {
    let Some(limit) = context.reached_limit() else {
        return true;
    };
    tracing::debug!("limit=<{limit}> | ignoring resume because the request reached its limit");
    false
}

/// The final message content an `AfterToolsEvent::end_turn` asks for; an empty text or block
/// list doesn't end the turn, as in TS.
fn end_turn_content(end_turn: EndTurn) -> Option<Vec<ContentBlock>> {
    match end_turn {
        EndTurn::Message(text) if text.is_empty() => None,
        EndTurn::Content(content) if content.is_empty() => None,
        EndTurn::Message(text) => Some(vec![TextBlock::new(text).into()]),
        EndTurn::Content(content) => Some(content),
        _ => Some(vec![
            TextBlock::new("Turn ended early by hook after tool execution").into(),
        ]),
    }
}

/// TS `_resolveTextTurn`: with a schema set and no structured output yet, retry with the tool
/// forced unless a limit is reached, which ends the turn with that limit's stop reason.
fn resolve_text_turn(
    message: &Message,
    stop_reason: StopReason,
    structured: &Structured,
    forced: bool,
    context: &InvocationContext,
) -> Result<TextTurn> {
    let Some(tool) = &structured.tool else {
        return Ok(TextTurn::End(stop_reason, None));
    };
    if forced {
        // The provider may return the forced output as JSON text.
        match tool.result_from_text(message) {
            Ok(output) => return Ok(TextTurn::End(stop_reason, Some(output))),
            Err(error) if context.has_structured_output_attempts_limit() => {
                context.record_structured_output_failure();
                tracing::debug!(
                    "error=<{error}> | structured output retry failed | counted toward the structured output attempt limit"
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(match context.reached_limit() {
        Some(limit) => TextTurn::End(limit, None),
        None => TextTurn::Retry,
    })
}

/// The JSON of the first successful structured output tool result (TS `_extractStructuredOutput`).
fn extract_structured_output(tool_use_message: &Message, tool_result_message: &Message) -> Option<JsonValue> {
    let ids = structured_output_tool_use_ids(tool_use_message);
    let result = tool_result_message
        .content()
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult(result)
                if ids.contains(&result.tool_use_id()) && result.status() == &ToolResultStatus::Success =>
            {
                Some(result)
            }
            _ => None,
        })?;
    match result.content().first()? {
        ToolResultContent::Json(json) => Some(json.json().clone()),
        _ => None,
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
