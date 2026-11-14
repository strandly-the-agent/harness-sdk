//! The agent loop (TS `Agent._streamInvocation`, `_stream`, `_invokeModel`, `executeTools`).

use std::sync::{Arc, PoisonError};

use futures::StreamExt;
use indexmap::IndexMap;

use crate::agent::stream::Emitter;
use crate::agent::{Agent, AgentResult, InvocationContext, InvocationState, InvokeArgs, InvokeOptions};
use crate::cancel::CancelSignal;
use crate::error::{AgentError, Error, Result, TypeError};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolsEvent, AgentResultEvent, BeforeInvocationEvent,
    BeforeInvocationEventData, BeforeModelCallEvent, BeforeToolsEvent, ContentBlockEvent, InterruptEvent,
    MessageAddedEvent, ModelMessageEvent, ModelStopData, ModelStreamUpdateEvent, kinds,
};
use crate::interrupt::Interrupt;
use crate::models::model_proxy;
use crate::models::{AggregatedEvent, CountTokensOptions, ModelRequest, StreamAggregatedResult};
use crate::tools::executors::{ToolBatch, ToolRun, check_duplicate_interrupts};
use crate::tools::structured_output::{STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputTool};
use crate::tools::tool::{error_result, is_valid_tool_name};
use crate::tools::{Tool, ToolChoice};
use crate::types::{
    ContentBlock, JsonValue, Message, Role, StopReason, TextBlock, ToolResultBlock, ToolUseBlock,
};

const CANCELLED_TEXT: &str = "Cancelled by user";
const INVALID_TOOL_NAME_PLACEHOLDER: &str = "INVALID_TOOL_NAME";
const BLANK_TEXT_PLACEHOLDER: &str = "[blank text]";

/// Why the loop stopped early, as TS uses exceptions for it.
enum Stop {
    Cancelled,
    Interrupt(Vec<Interrupt>),
    Error(Error),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Self::Error(error)
    }
}

#[expect(clippy::large_enum_variant, reason = "built once per cycle")]
enum Cycle {
    Continue,
    Done(AgentResult),
}

/// Per-invocation values the loop threads through.
struct Run<'e> {
    emitter: &'e Emitter,
    state: InvocationState,
    context: InvocationContext,
    structured_output: Option<Arc<StructuredOutputTool>>,
    structured_output_schema: Option<JsonValue>,
    forced_choice: Option<ToolChoice>,
    message_count_at_start: usize,
}

impl Agent {
    pub(crate) async fn run(
        &mut self,
        args: InvokeArgs,
        options: InvokeOptions,
        emitter: Emitter,
    ) -> Result<AgentResult> {
        self.initialize().await?;
        let state = options.invocation_state.clone().unwrap_or_default();
        let context = InvocationContext::resolve(options.invocation_context.as_ref(), options.limits)?;
        let mut current = args;
        loop {
            self.local.cancel_signal = options
                .cancel_signal
                .clone()
                .unwrap_or_else(|| CancelSignal::new().0);
            let responses = current.interrupt_responses();
            if !responses.is_empty() {
                self.local
                    .interrupt_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .resume(responses)
                    .map_err(TypeError::new)?;
            }
            let input = current.to_messages()?;
            let hooks = self.local.hooks.clone();
            let mut before =
                BeforeInvocationEvent::new(&mut self.local, state.clone(), Some(context.clone()), input);
            hooks.fire::<kinds::BeforeInvocation>(&mut before).await;
            let (input, cancel) = before.into_parts();
            emitter.emit(BeforeInvocationEventData {}).await;
            if let Some(text) = cancel.and_then(|cancel| cancel.resolve("invocation denied by hook")) {
                let message = Message::new(Role::Assistant, vec![ContentBlock::from(text.as_str())]);
                self.append(message.clone(), &state, &context, &emitter).await;
                self.after_invocation(&state, &context, &emitter).await;
                return Ok(self.result(StopReason::EndTurn, message, &state, &context));
            }
            let outcome = self
                .stream_core(&current, input, &state, &context, &options, &emitter)
                .await;
            let resume = self.after_invocation(&state, &context, &emitter).await;
            let result = outcome?;
            if let Some(resume) = resume {
                match context.reached_limit() {
                    None => {
                        current = resume;
                        continue;
                    }
                    Some(limit) => {
                        tracing::debug!(limit = %limit, "ignoring resume because the request reached its limit");
                    }
                }
            }
            let mut event =
                AgentResultEvent::new(&mut self.local, state.clone(), Some(context.clone()), result);
            hooks.fire::<kinds::AgentResult>(&mut event).await;
            let result = event.into_data();
            if self.printer.is_some() {
                emitter.print(&crate::AgentStreamEvent::AgentResult(result.clone()));
            }
            return Ok(result);
        }
    }

    async fn after_invocation(
        &mut self,
        state: &InvocationState,
        context: &InvocationContext,
        emitter: &Emitter,
    ) -> Option<InvokeArgs> {
        let hooks = self.local.hooks.clone();
        let mut after = AfterInvocationEvent::new(&mut self.local, state.clone(), Some(context.clone()));
        hooks.fire::<kinds::AfterInvocation>(&mut after).await;
        let resume = after.take_resume();
        emitter.emit(after.into_data()).await;
        resume
    }

    pub(crate) fn result(
        &self,
        stop_reason: StopReason,
        last_message: Message,
        state: &InvocationState,
        context: &InvocationContext,
    ) -> AgentResult {
        AgentResult::new(stop_reason, last_message, state.clone())
            .with_traces(self.tracer.traces())
            .with_metrics(self.meter.metrics())
            .with_invocation_context(context.clone())
    }

    /// One invocation without its before/after events (TS `_stream`).
    async fn stream_core(
        &mut self,
        args: &InvokeArgs,
        input: Vec<Message>,
        state: &InvocationState,
        context: &InvocationContext,
        options: &InvokeOptions,
        emitter: &Emitter,
    ) -> Result<AgentResult> {
        let has_responses = !args.interrupt_responses().is_empty();
        let activated = self
            .local
            .interrupt_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .activated;
        if activated && !has_responses {
            return Err(TypeError::new(
                "Agent is in an interrupted state. Resume by invoking with interruptResponse content blocks.",
            )
            .into());
        }
        let schema = options
            .structured_output_schema
            .clone()
            .or_else(|| self.structured_output_schema.clone());
        let structured_output = schema
            .clone()
            .map(|schema| Arc::new(StructuredOutputTool::new(schema)));
        self.meter.start_new_invocation();
        self.tracer.start_invocation();
        if let Some(tool) = &structured_output {
            self.local.tool_registry.add(Arc::clone(tool) as Arc<dyn Tool>)?;
        }
        let mut run = Run {
            emitter,
            state: state.clone(),
            context: context.clone(),
            structured_output,
            structured_output_schema: schema,
            forced_choice: None,
            message_count_at_start: self.local.messages.len(),
        };
        let pending_input = (!has_responses).then_some(input);
        let outcome = self.cycles(&mut run, pending_input).await;
        if run.structured_output.is_some() {
            self.local.tool_registry.remove(STRUCTURED_OUTPUT_TOOL_NAME);
        }
        match outcome {
            Ok(result) => Ok(result),
            Err(Stop::Error(error)) => Err(error),
            Err(Stop::Cancelled) => {
                let message = self.cancel_message();
                if self.has_open_user_turn() {
                    self.append(message.clone(), &run.state, &run.context, emitter)
                        .await;
                }
                Ok(self.result(StopReason::Cancelled, message, &run.state, &run.context))
            }
            Err(Stop::Interrupt(interrupts)) => Ok(self.interrupt_result(interrupts, &run).await),
        }
    }

    async fn interrupt_result(&mut self, interrupts: Vec<Interrupt>, run: &Run<'_>) -> AgentResult {
        {
            let mut interrupt_state = self
                .local
                .interrupt_state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for interrupt in &interrupts {
                interrupt_state.register_interrupt(interrupt);
            }
            interrupt_state.activate();
        }
        let hooks = self.local.hooks.clone();
        for interrupt in interrupts {
            let mut event = InterruptEvent::new(
                &mut self.local,
                run.state.clone(),
                Some(run.context.clone()),
                interrupt,
            );
            hooks.fire::<kinds::Interrupt>(&mut event).await;
            run.emitter.emit(event.into_data()).await;
        }
        let last = self
            .local
            .messages
            .last()
            .cloned()
            .unwrap_or_else(|| Message::new(Role::Assistant, vec![ContentBlock::from("Interrupted")]));
        let unanswered = self
            .local
            .interrupt_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_unanswered_interrupts();
        self.result(StopReason::Interrupt, last, &run.state, &run.context)
            .with_interrupts(unanswered)
    }

    async fn cycles(
        &mut self,
        run: &mut Run<'_>,
        mut pending_input: Option<Vec<Message>>,
    ) -> std::result::Result<AgentResult, Stop> {
        loop {
            if self.local.cancel_signal.is_cancelled() {
                return Err(Stop::Cancelled);
            }
            if let Some(limit) = run.context.reached_limit() {
                let last = self
                    .local
                    .messages
                    .get(run.message_count_at_start..)
                    .and_then(<[Message]>::last)
                    .cloned()
                    .unwrap_or_else(|| Message::new(Role::Assistant, Vec::new()));
                return Ok(self.result(limit, last, &run.state, &run.context));
            }
            run.context.record_turn();
            let (cycle_id, start) = self.meter.start_cycle();
            self.tracer.start_cycle(&cycle_id);
            let outcome = self.cycle(run, &mut pending_input).await;
            self.meter.end_cycle(start);
            self.tracer.end_cycle();
            match outcome? {
                Cycle::Continue => {}
                Cycle::Done(result) => {
                    return Ok(result
                        .with_traces(self.tracer.traces())
                        .with_metrics(self.meter.metrics()));
                }
            }
        }
    }

    async fn cycle(
        &mut self,
        run: &mut Run<'_>,
        pending_input: &mut Option<Vec<Message>>,
    ) -> std::result::Result<Cycle, Stop> {
        if let Some(messages) = pending_input.take() {
            for message in messages {
                self.append(message, &run.state, &run.context, run.emitter).await;
            }
        }
        let pending = self
            .local
            .interrupt_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_pending_execution()?;
        let (assistant, completed) = match pending {
            Some(pending) => (pending.assistant_message, pending.completed_tool_results),
            None => {
                let result = self.invoke_model(run).await?;
                if result.stop_reason != StopReason::ToolUse {
                    return self.text_turn(run, result).await;
                }
                if run.forced_choice.is_some() && structured_output_tool_use_ids(&result.message).is_empty() {
                    run.context.record_structured_output_failure();
                }
                if self.local.cancel_signal.is_cancelled() {
                    return Ok(Cycle::Done(self.cancel_before_tools(run, result.message).await));
                }
                (result.message, IndexMap::new())
            }
        };
        let Some((results, end_turn, skipped)) = self.execute_tools(run, &assistant, completed).await? else {
            return Ok(Cycle::Continue);
        };
        let pending_execution = self
            .local
            .interrupt_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending_tool_execution
            .is_some();
        if self.local.cancel_signal.is_cancelled() && skipped && pending_execution {
            return Ok(Cycle::Continue);
        }
        let structured_output = run
            .structured_output
            .as_ref()
            .and_then(|_| extract_structured_output(&assistant, &results));
        self.append(assistant.clone(), &run.state, &run.context, run.emitter)
            .await;
        self.append(results, &run.state, &run.context, run.emitter).await;
        {
            let mut interrupt_state = self
                .local
                .interrupt_state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            interrupt_state.clear_pending_tool_execution();
            if interrupt_state.activated {
                interrupt_state.deactivate();
            }
        }
        if run.structured_output.is_some()
            && structured_output.is_none()
            && !structured_output_tool_use_ids(&assistant).is_empty()
        {
            run.context.record_structured_output_failure();
        }
        if let Some(content) = end_turn {
            let message = Message::new(Role::Assistant, content);
            self.append(message.clone(), &run.state, &run.context, run.emitter)
                .await;
            return Ok(Cycle::Done(self.result(
                StopReason::EndTurn,
                message,
                &run.state,
                &run.context,
            )));
        }
        if self.local.cancel_signal.is_deferred() || self.local.cancel_signal.is_cancelled() {
            let message = self.cancel_message();
            self.append(message.clone(), &run.state, &run.context, run.emitter)
                .await;
            let result = self
                .result(StopReason::Cancelled, message, &run.state, &run.context)
                .with_structured_output(structured_output);
            return Ok(Cycle::Done(result));
        }
        if structured_output.is_some() {
            let result = self
                .result(StopReason::ToolUse, assistant, &run.state, &run.context)
                .with_structured_output(structured_output);
            return Ok(Cycle::Done(result));
        }
        Ok(Cycle::Continue)
    }

    /// A turn that ended without tool calls (TS `_resolveTextTurn`).
    async fn text_turn(
        &mut self,
        run: &mut Run<'_>,
        result: StreamAggregatedResult,
    ) -> std::result::Result<Cycle, Stop> {
        let mut stop_reason = result.stop_reason.clone();
        let mut structured_output = None;
        if let Some(tool) = run.structured_output.clone() {
            if run.forced_choice.is_some() {
                structured_output = self.structured_output_from_forced_text(run, &tool, &result.message)?;
            }
            if structured_output.is_none() {
                match run.context.reached_limit() {
                    Some(limit) => stop_reason = limit,
                    None => {
                        run.forced_choice = Some(ToolChoice::tool(STRUCTURED_OUTPUT_TOOL_NAME));
                        tracing::debug!(
                            "structured output schema set but model responded with plain text; forcing tool use on next cycle"
                        );
                        return Ok(Cycle::Continue);
                    }
                }
            }
        }
        self.append(result.message.clone(), &run.state, &run.context, run.emitter)
            .await;
        let result = self
            .result(stop_reason, result.message, &run.state, &run.context)
            .with_structured_output(structured_output);
        Ok(Cycle::Done(result))
    }

    fn structured_output_from_forced_text(
        &self,
        run: &Run<'_>,
        tool: &StructuredOutputTool,
        message: &Message,
    ) -> std::result::Result<Option<JsonValue>, Stop> {
        match tool.result_from_text(message) {
            Ok(value) => Ok(Some(value)),
            Err(Error::StructuredOutput(_)) if run.context.has_structured_output_attempts_limit() => {
                run.context.record_structured_output_failure();
                tracing::debug!(
                    "structured output retry failed | counted toward the structured output attempt limit"
                );
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn cancel_before_tools(&mut self, run: &Run<'_>, assistant: Message) -> AgentResult {
        let results: Vec<ContentBlock> = tool_use_blocks(&assistant)
            .iter()
            .map(|block| {
                ContentBlock::ToolResult(error_result(&block.tool_use_id, "Tool execution cancelled"))
            })
            .collect();
        self.append(assistant, &run.state, &run.context, run.emitter)
            .await;
        self.append(
            Message::new(Role::User, results),
            &run.state,
            &run.context,
            run.emitter,
        )
        .await;
        let message = self.cancel_message();
        self.append(message.clone(), &run.state, &run.context, run.emitter)
            .await;
        self.result(StopReason::Cancelled, message, &run.state, &run.context)
    }

    pub(crate) fn cancel_message(&self) -> Message {
        let text = self
            .local
            .cancel_signal
            .reason()
            .filter(|reason| !reason.is_empty())
            .unwrap_or_else(|| CANCELLED_TEXT.to_owned());
        Message::new(Role::Assistant, vec![ContentBlock::from(text.as_str())])
    }

    fn has_open_user_turn(&self) -> bool {
        self.local
            .messages
            .last()
            .is_some_and(|message| message.role == Role::User)
    }

    pub(crate) async fn append(
        &mut self,
        message: Message,
        state: &InvocationState,
        context: &InvocationContext,
        emitter: &Emitter,
    ) {
        self.local.messages.push(message.clone());
        let hooks = self.local.hooks.clone();
        let mut event =
            MessageAddedEvent::new(&mut self.local, state.clone(), Some(context.clone()), message);
        hooks.fire::<kinds::MessageAdded>(&mut event).await;
        emitter.emit(event.into_data()).await;
    }

    /// Calls the model, retrying as the `AfterModelCallEvent` hooks ask (TS `_invokeModel`).
    async fn invoke_model(&mut self, run: &Run<'_>) -> std::result::Result<StreamAggregatedResult, Stop> {
        let hooks = self.local.hooks.clone();
        let mut attempt: u32 = 1;
        loop {
            if self.local.cancel_signal.is_cancelled() {
                return Err(Stop::Cancelled);
            }
            let model = Arc::clone(&self.local.model);
            let projected = self.estimate_input_tokens().await;
            let mut before = BeforeModelCallEvent::new(
                &mut self.local,
                run.state.clone(),
                Some(run.context.clone()),
                Arc::clone(&model),
                projected,
            );
            hooks.fire::<kinds::BeforeModelCall>(&mut before).await;
            let cancel = before.take_cancel();
            run.emitter.emit(before.into_data()).await;
            if let Some(text) = cancel.and_then(|cancel| cancel.resolve("model call denied by hook")) {
                let message = Message::new(Role::Assistant, vec![ContentBlock::from(text.as_str())]);
                let stop = ModelStopData::new(message.clone(), StopReason::EndTurn);
                if self.after_model_call(run, &model, attempt, Ok(stop)).await.0 {
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                return Ok(StreamAggregatedResult::new(message, StopReason::EndTurn));
            }
            match self.call_model(run, &model).await {
                Ok(result) => {
                    self.meter.update_cycle(result.metadata.as_ref());
                    let hooks = self.local.hooks.clone();
                    let mut event = ModelMessageEvent::new(
                        &mut self.local,
                        run.state.clone(),
                        Some(run.context.clone()),
                        result.message.clone(),
                        result.stop_reason.clone(),
                    );
                    hooks.fire::<kinds::ModelMessage>(&mut event).await;
                    run.emitter.emit(event.into_data()).await;
                    if let Some(redaction) = &result.redaction {
                        self.redact_last_message(&redaction.user_message);
                    }
                    let mut stop = ModelStopData::new(result.message.clone(), result.stop_reason.clone());
                    stop.redaction.clone_from(&result.redaction);
                    if self.after_model_call(run, &model, attempt, Ok(stop)).await.0 {
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
                    return Ok(result);
                }
                Err(Stop::Error(error)) => {
                    let (retry, error) = self.after_model_call(run, &model, attempt, Err(error)).await;
                    if self.local.cancel_signal.is_cancelled() {
                        return Err(Stop::Cancelled);
                    }
                    if retry {
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
                    return Err(error.map_or(Stop::Cancelled, Stop::Error));
                }
                Err(stop) => {
                    let error = AgentError::new("model call cancelled");
                    let _ = self
                        .after_model_call(run, &model, attempt, Err(error.into()))
                        .await;
                    return Err(stop);
                }
            }
        }
    }

    async fn after_model_call(
        &mut self,
        run: &Run<'_>,
        model: &Arc<dyn crate::models::Model>,
        attempt: u32,
        outcome: std::result::Result<ModelStopData, Error>,
    ) -> (bool, Option<Error>) {
        let hooks = self.local.hooks.clone();
        let mut event = AfterModelCallEvent::new(
            &mut self.local,
            run.state.clone(),
            Some(run.context.clone()),
            Arc::clone(model),
            attempt,
            outcome,
        );
        hooks.fire::<kinds::AfterModelCall>(&mut event).await;
        let retry = event.retry();
        let error = event.take_error();
        run.emitter.emit(event.into_data()).await;
        (retry, error)
    }

    /// Streams one model call (TS `_invokeModelWithMiddleware` and `_streamFromModel`).
    async fn call_model(
        &mut self,
        run: &Run<'_>,
        model: &Arc<dyn crate::models::Model>,
    ) -> std::result::Result<StreamAggregatedResult, Stop> {
        let model_state = self.local.model_state.snapshot();
        let request = ModelRequest {
            messages: fill_empty_assistant_messages(normalize_tool_use_names(self.local.messages.clone())),
            system_prompt: self.local.system_prompt.clone(),
            tool_specs: self
                .local
                .tool_registry
                .list()
                .iter()
                .map(|tool| tool.tool_spec())
                .collect(),
            tool_choice: run.forced_choice.clone(),
            model_state: model_state.clone(),
            dynamic_trailing_blocks: None,
            agent_metadata: None,
            structured_output_schema: run
                .forced_choice
                .as_ref()
                .and(run.structured_output_schema.clone()),
            cancel_signal: Some(self.local.cancel_signal.clone()),
        };
        self.tracer.start_model();
        let mut stream =
            model_proxy::stream_aggregated_recorded(model.as_ref(), request, Some(run.context.clone()));
        let hooks = self.local.hooks.clone();
        let mut result = None;
        while let Some(item) = stream.next().await {
            if self.local.cancel_signal.is_cancelled() {
                self.tracer.end_model(None);
                return Err(Stop::Cancelled);
            }
            match item {
                Ok(AggregatedEvent::ModelStreamEvent(event)) => {
                    let mut event = ModelStreamUpdateEvent::new(
                        &mut self.local,
                        run.state.clone(),
                        Some(run.context.clone()),
                        event,
                    );
                    hooks.fire::<kinds::ModelStreamUpdate>(&mut event).await;
                    run.emitter.emit(event.into_data()).await;
                }
                Ok(AggregatedEvent::ContentBlock(block)) => {
                    let mut event = ContentBlockEvent::new(
                        &mut self.local,
                        run.state.clone(),
                        Some(run.context.clone()),
                        block,
                    );
                    hooks.fire::<kinds::ContentBlock>(&mut event).await;
                    run.emitter.emit(event.into_data()).await;
                }
                Ok(AggregatedEvent::Done(done)) => result = Some(done),
                Err(error) => {
                    self.tracer.end_model(None);
                    if self.local.cancel_signal.is_cancelled() {
                        return Err(Stop::Cancelled);
                    }
                    return Err(Stop::Error(error));
                }
            }
        }
        let Some(result) = result else {
            self.tracer.end_model(None);
            return Err(Stop::Error(
                AgentError::new("model stream ended without a result").into(),
            ));
        };
        self.tracer.end_model(Some(&result.message));
        self.local.model_state.load_json(model_state.to_json());
        Ok(*result)
    }

    /// The projected input tokens of the next request (TS `_estimateInputTokens`).
    async fn estimate_input_tokens(&self) -> Option<u64> {
        let messages = &self.local.messages;
        let model = &self.local.model;
        let last_usage = messages.iter().enumerate().rev().find_map(|(index, message)| {
            (message.role == Role::Assistant)
                .then(|| {
                    message
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.usage.as_ref())
                })
                .flatten()
                .map(|usage| (index, usage))
        });
        let estimate = match last_usage {
            Some((index, usage)) => {
                let baseline = usage.total_prompt_tokens().saturating_add(usage.output_tokens);
                let newer = messages.get(index.saturating_add(1)..).unwrap_or_default();
                if newer.is_empty() {
                    Ok(baseline)
                } else {
                    let (prepared, _) = model_proxy::prepare(model.as_ref(), newer.to_vec());
                    model
                        .count_tokens(&prepared, CountTokensOptions::default())
                        .await
                        .map(|count| baseline.saturating_add(count))
                }
            }
            None => {
                let (prepared, _) = model_proxy::prepare(model.as_ref(), messages.clone());
                let options = CountTokensOptions {
                    system_prompt: self.local.system_prompt.clone(),
                    tool_specs: self
                        .local
                        .tool_registry
                        .list()
                        .iter()
                        .map(|tool| tool.tool_spec())
                        .collect(),
                };
                model.count_tokens(&prepared, options).await
            }
        };
        estimate
            .inspect_err(|error| tracing::debug!(error = %error, "token estimation failed, proceeding without estimate"))
            .ok()
    }

    pub(crate) fn redact_last_message(&mut self, redact_message: &str) {
        let Some(last) = self.local.messages.last_mut() else {
            return;
        };
        if last.role != Role::User {
            tracing::warn!(role = ?last.role, "received input redaction but last message is not from user | redaction skipped");
            return;
        }
        let mut content: Vec<ContentBlock> = last
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult(result) => Some(ContentBlock::ToolResult(ToolResultBlock::new(
                    &result.tool_use_id,
                    result.status.clone(),
                    vec![crate::types::ToolResultContent::Text(TextBlock::new(
                        redact_message,
                    ))],
                ))),
                _ => None,
            })
            .collect();
        if content.is_empty() {
            content.push(ContentBlock::from(redact_message));
        }
        *last = Message::new(Role::User, content).with_tracking_id(last.tracking_id());
    }

    /// Runs a batch of tool calls (TS `executeTools`). `None` means the batch stopped to
    /// continue the loop; otherwise the tool result message, the end-turn content, and whether
    /// the tools were skipped.
    async fn execute_tools(
        &mut self,
        run: &Run<'_>,
        assistant: &Message,
        completed: IndexMap<String, ToolResultBlock>,
    ) -> std::result::Result<Option<(Message, Option<Vec<ContentBlock>>, bool)>, Stop> {
        let hooks = self.local.hooks.clone();
        let mut before = BeforeToolsEvent::new(
            &mut self.local,
            run.state.clone(),
            Some(run.context.clone()),
            assistant.clone(),
        );
        hooks.fire::<kinds::BeforeTools>(&mut before).await;
        let (data, cancel, interrupts) = before.into_parts();
        if !interrupts.is_empty() {
            let interrupts = check_duplicate_interrupts(interrupts)?;
            self.store_pending_before_tools(assistant)?;
            return Err(Stop::Interrupt(interrupts));
        }
        run.emitter.emit(data).await;
        let blocks = tool_use_blocks(assistant);
        let tool_agent = self.local.tool_agent(Some(run.context.clone()));
        let tool_run = ToolRun {
            agent: tool_agent,
            invocation_state: run.state.clone(),
            cancel_signal: self.local.cancel_signal.clone(),
        };
        let mut skipped = false;
        let mut interrupts = Vec::new();
        let mut failure = None;
        let results: Vec<ToolResultBlock> = if blocks.is_empty() {
            failure = Some(
                AgentError::new("Model indicated toolUse but no tool use blocks found in message").into(),
            );
            Vec::new()
        } else if let Some(message) = cancel.or_else(|| {
            self.local
                .cancel_signal
                .is_cancelled()
                .then(|| "Tool execution cancelled".to_owned())
        }) {
            skipped = true;
            let results: Vec<ToolResultBlock> = blocks
                .iter()
                .map(|block| error_result(&block.tool_use_id, message.clone()))
                .collect();
            for result in &results {
                emit_tool_result(&tool_run, run.emitter, result.clone()).await;
            }
            results
        } else {
            let batch = ToolBatch {
                run: tool_run.clone(),
                assistant_message: assistant,
                blocks: blocks.clone(),
                completed,
            };
            match batch.execute(self.tool_executor, run.emitter).await {
                Ok(outcome) => {
                    for record in &outcome.records {
                        self.meter
                            .end_tool_call(&record.name, record.duration, record.success);
                        self.tracer.record_tool(
                            &record.name,
                            &record.tool_use_id,
                            record.start_time,
                            record.end_time,
                        );
                    }
                    interrupts = outcome.interrupts;
                    outcome.results
                }
                Err(error) => {
                    failure = Some(error);
                    Vec::new()
                }
            }
        };
        let message = Message::new(
            Role::User,
            results.into_iter().map(ContentBlock::ToolResult).collect(),
        );
        let mut after = AfterToolsEvent::new(
            &mut self.local,
            run.state.clone(),
            Some(run.context.clone()),
            message.clone(),
        );
        hooks.fire::<kinds::AfterTools>(&mut after).await;
        let end_turn = after
            .take_end_turn()
            .and_then(|end_turn| end_turn.resolve("Turn ended early by hook after tool execution"));
        run.emitter.emit(after.into_data()).await;
        if let Some(error) = failure {
            return Err(Stop::Error(error));
        }
        if !interrupts.is_empty() {
            return Err(Stop::Interrupt(interrupts));
        }
        Ok(Some((message, end_turn, skipped)))
    }

    fn store_pending_before_tools(&self, assistant: &Message) -> Result<()> {
        let pending = crate::interrupt::PendingToolExecution {
            assistant_message_data: serde_json::to_value(assistant)?,
            completed_tool_results: JsonValue::Object(serde_json::Map::new()),
        };
        self.local
            .interrupt_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_pending_tool_execution(pending);
        Ok(())
    }
}

async fn emit_tool_result(run: &ToolRun, emitter: &Emitter, result: ToolResultBlock) {
    let mut event =
        crate::hooks::ToolResultEvent::new(run.agent.clone(), run.invocation_state.clone(), result);
    run.agent
        .hooks()
        .fire::<crate::hooks::ToolResultEvent>(&mut event)
        .await;
    emitter.emit(event.into_data()).await;
}

pub(crate) fn tool_use_blocks(message: &Message) -> Vec<ToolUseBlock> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => Some(tool_use.clone()),
            _ => None,
        })
        .collect()
}

fn structured_output_tool_use_ids(message: &Message) -> Vec<String> {
    tool_use_blocks(message)
        .into_iter()
        .filter(|block| block.name == STRUCTURED_OUTPUT_TOOL_NAME)
        .map(|block| block.tool_use_id)
        .collect()
}

/// The JSON of the successful structured output call, if any (TS `_extractStructuredOutput`).
fn extract_structured_output(assistant: &Message, results: &Message) -> Option<JsonValue> {
    let ids = structured_output_tool_use_ids(assistant);
    results.content.iter().find_map(|block| match block {
        ContentBlock::ToolResult(result)
            if ids.contains(&result.tool_use_id)
                && result.status == crate::tools::ToolResultStatus::Success =>
        {
            match result.content.first() {
                Some(crate::types::ToolResultContent::Json(json)) => Some(json.json.clone()),
                _ => None,
            }
        }
        _ => None,
    })
}

/// Replaces tool names models reject (TS `normalizeToolUseNames`).
fn normalize_tool_use_names(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .map(|mut message| {
            if message.role != Role::Assistant {
                return message;
            }
            for block in &mut message.content {
                if let ContentBlock::ToolUse(tool_use) = block
                    && !is_valid_tool_name(&tool_use.name)
                {
                    tracing::debug!(
                        tool_name = %tool_use.name,
                        "replacing invalid tool name with {INVALID_TOOL_NAME_PLACEHOLDER}"
                    );
                    tool_use.name = INVALID_TOOL_NAME_PLACEHOLDER.to_owned();
                }
            }
            message
        })
        .collect()
}

/// Gives empty assistant messages placeholder text (TS `fillEmptyAssistantMessages`).
fn fill_empty_assistant_messages(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .map(|mut message| {
            if message.role == Role::Assistant && message.content.is_empty() {
                tracing::debug!("replacing empty assistant message content with blank text placeholder");
                message.content = vec![ContentBlock::from(BLANK_TEXT_PLACEHOLDER)];
            }
            message
        })
        .collect()
}
