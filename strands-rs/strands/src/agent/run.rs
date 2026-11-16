//! The agent loop (TS `agent.ts` `_streamInvocation`, `_stream`, `_invokeModel`, `executeTools`).
//!
//! Each step fires its hook event, then hands the event's record to the stream. The assistant
//! message that requested tools is appended only after its tools finish, together with their
//! results, so history is always re-invokable.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::StreamExt;
use indexmap::IndexMap;

use crate::agent::agent_metadata::AgentMetadata;
use crate::agent::continuation::{self, ContinuationInput, Prepared};
use crate::agent::stream::{Driven, DrivenItem, Emitter};
use crate::agent::{Agent, InvocationContext};
use crate::cancel::CancelOptions;
use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolsEvent, AgentResultEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolsEvent, ContentBlockEvent, EndTurn, HookRegistry, InterruptEvent,
    MessageAddedEvent, ModelMessageEvent, ModelStopData, ModelStreamUpdateEvent, ToolResultEvent,
};
use crate::interrupt::{Interrupt, interrupt_error_message};
use crate::middleware::ended_without_result;
use crate::middleware::{
    AgentStreamContext, AgentStreamResult, AgentStreamStage, InvokeModelContext, InvokeModelResult,
    InvokeModelStage, MiddlewareError, MiddlewareInterrupt, MiddlewareItem, MiddlewareStream,
};
use crate::models::model::{
    AggregatedEvent, CountTokensOptions, Model, ModelRequest, StreamAggregatedResult, js_trim,
};
use crate::models::model_proxy::ModelProxy;
use crate::state_store::StateStore;
use crate::telemetry::LocalTelemetry;
use crate::telemetry::tracer::{AgentSpanStart, ModelSpanEnd};
use crate::tools::executors::executor::{
    BackgroundRoute, ToolExecutionInput, ToolExecutorEvent, ToolExecutorItem, error_text_result,
};
use crate::tools::structured_output::{
    STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema, StructuredOutputTool, structured_output_tool_use_ids,
};
use crate::tools::tool::is_valid_tool_name;
use crate::types::agent::{AgentResult, AgentStreamEvent, InvocationState, InvokeArgs, InvokeOptions};
use crate::types::agent::{ContentBlockEventData, ModelStreamUpdateEventData};
use crate::types::json::JsonValue;
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

/// TS `INVALID_TOOL_NAME_PLACEHOLDER`.
const INVALID_TOOL_NAME_PLACEHOLDER: &str = "INVALID_TOOL_NAME";
/// The error text of a tool call a cancel stopped before it ran.
const TOOL_CANCELLED: &str = "Tool execution cancelled";
/// TS `BLANK_TEXT_PLACEHOLDER`.
const BLANK_TEXT_PLACEHOLDER: &str = "[blank text]";

/// TS `CancelledError`: what `AfterModelCallEvent` hooks see when a cancel stops a model call.
#[derive(Debug, thiserror::Error)]
#[error("Agent invocation cancelled")]
pub(crate) struct CancelledError;

/// Why a loop step stopped before finishing.
enum Stop {
    /// A cancel checkpoint saw the invocation's signal (TS `CancelledError`).
    Cancelled,
    /// A hook, a tool or middleware raised interrupts (TS `InterruptError`).
    Interrupted(Vec<Interrupt>),
    Failed(Error),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

type Step<T> = std::result::Result<T, Stop>;

/// What a dropped invocation left for the next call.
#[derive(Clone)]
pub(crate) struct AbandonedRun {
    state: InvocationState,
    /// The cancel message the drop appended, for its `MessageAddedEvent` hooks.
    repair: Option<Message>,
}

impl AbandonedRun {
    /// Forgets the cancel message, for a caller that replaced the history it was appended to.
    pub(crate) fn clear_repair(&mut self) {
        self.repair = None;
    }
}

/// Lives for one invocation; dropped unfinished, it repairs the agent synchronously.
struct RunGuard<'a> {
    agent: &'a mut Agent,
    state: InvocationState,
    finished: bool,
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.agent.abandon(self.state.clone());
        }
    }
}

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
    /// The batch was cancelled before any tool ran.
    skipped: bool,
}

impl Agent {
    /// One invocation (TS `_streamInvocation`).
    pub(crate) async fn run(
        &mut self,
        args: InvokeArgs,
        options: InvokeOptions,
        out: &Emitter,
    ) -> Result<()> {
        let state = options.invocation_state.clone();
        let mut guard = RunGuard {
            agent: self,
            state,
            finished: false,
        };
        let outcome = guard.agent.run_passes(args, options, out).await;
        guard.finished = true;
        guard.agent.in_pass = false;
        // After `AgentResultEvent`, so its consumers still read the cancel signal (TS `stream`).
        guard.agent.local.end_cancel();
        outcome
    }

    /// Runs the hooks a dropped invocation's stream skipped (#63): `MessageAddedEvent` for the
    /// cancel message the drop appended, then `AfterInvocationEvent`. They see a cancelled
    /// signal, and a run stays pending until its hooks return, so a drop here retries them.
    pub(crate) async fn finish_abandoned_run(&mut self) -> Result<()> {
        if self.abandoned.is_none() {
            return Ok(());
        }
        self.local.begin_cancel(None);
        self.local.cancel(CancelOptions::default());
        let outcome = self.abandoned_hooks().await;
        self.local.end_cancel();
        outcome
    }

    async fn abandoned_hooks(&mut self) -> Result<()> {
        let hooks = self.local.hooks().clone();
        let Some(AbandonedRun { state, repair }) = self.abandoned.clone() else {
            return Ok(());
        };
        if let Some(message) = repair {
            let mut event = MessageAddedEvent::new(&mut self.local, message, state.clone());
            let added = hooks.invoke::<MessageAddedEvent<'static>>(&mut event).await;
            if added.is_err() {
                self.abandoned = None;
                return added;
            }
            if let Some(run) = &mut self.abandoned {
                run.repair = None;
            }
        }
        let mut event = AfterInvocationEvent::new(&mut self.local, state);
        let after = hooks.invoke::<AfterInvocationEvent<'static>>(&mut event).await;
        self.abandoned = None;
        after
    }

    /// The synchronous half of a dropped invocation, in the TS cancel shape: the signal is
    /// cancelled and an open user turn is closed with the cancel message, unless the turn is
    /// paused on an interrupt. The skipped hooks run on the next call.
    fn abandon(&mut self, state: InvocationState) {
        if self.structured_output_registered {
            self.local.tool_registry_mut().remove(STRUCTURED_OUTPUT_TOOL_NAME);
            self.structured_output_registered = false;
        }
        self.local.cancel(CancelOptions::default());
        {
            // TS closes the open spans in its `finally` blocks when the consumer closes the iterator.
            let mut telemetry = self.local.telemetry().lock();
            telemetry.tracer.end_model(&ModelSpanEnd::default());
            telemetry.meter.end_open_cycle();
            telemetry.tracer.end_cycle(None);
            let usage = telemetry.meter.metrics().accumulated_usage().clone();
            telemetry.tracer.end_agent(None, None, &usage, None);
        }
        let in_pass = std::mem::take(&mut self.in_pass);
        let paused = self.local.interrupt_state().lock().activated;
        let open_turn = self
            .local
            .messages()
            .last()
            .is_some_and(|last| last.role() == Role::User);
        let repair = (in_pass && !paused && open_turn).then(|| self.cancel_message());
        if let Some(message) = &repair {
            self.local.messages_mut().push(message.clone());
        }
        self.local.end_cancel();
        if in_pass {
            self.abandoned = Some(AbandonedRun { state, repair });
        }
    }

    async fn run_passes(&mut self, args: InvokeArgs, options: InvokeOptions, out: &Emitter) -> Result<()> {
        self.finish_abandoned_run().await?;
        self.initialize().await?;
        let hooks = self.local.hooks().clone();
        let stage_options = options.clone();
        let state = options.invocation_state;
        let external_cancel = options.cancel_signal;
        let schema = options
            .structured_output_schema
            .or_else(|| self.structured_output_schema.clone());
        let context = InvocationContext::resolve(options.invocation_context, options.limits);
        self.local.set_invocation_context(Some(context.clone()));

        // A hook's `AfterInvocationEvent::set_resume` runs another pass (TS `stream`'s loop), and
        // so does follow-up input a hook contributed (TS `continuations`).
        let mut args = args;
        let mut continuation: Option<Prepared> = None;
        let result = loop {
            self.local.begin_cancel(external_cancel.as_ref());
            // Consumed before the hooks run so their `interrupt()` finds the responses.
            let resuming = match &args {
                InvokeArgs::InterruptResponses(responses) if !responses.is_empty() => {
                    self.local.interrupt_state().lock().resume(responses)?;
                    true
                }
                _ => false,
            };
            let pass_args = args.clone();
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
                if let Some(prepared) = continuation.take() {
                    prepared.abandon(continuation::NOT_INCORPORATED);
                }
                // TS returns here without reading `resume`; its inputs are dropped (abandoned).
                self.after_invocation(&hooks, &state, out).await?;
                break result;
            }
            self.in_pass = true;
            let pass = Pass {
                args: pass_args,
                input,
                resuming,
                state: state.clone(),
                hooks: hooks.clone(),
                schema: schema.clone(),
                continuation: Arc::new(Mutex::new(continuation.take())),
            };
            let unused = Arc::clone(&pass.continuation);
            let outcome = self
                .stream_with_middleware(pass, stage_options.clone(), out)
                .await;
            if let Some(prepared) = lock(&unused).take() {
                prepared.abandon(continuation::NOT_INCORPORATED);
            }
            if outcome
                .as_ref()
                .is_ok_and(|result| result.stop_reason() != StopReason::Interrupt)
            {
                let mut interrupt_state = self.local.interrupt_state().lock();
                if interrupt_state.activated && interrupt_state.pending_tool_execution.is_none() {
                    interrupt_state.deactivate();
                }
            }
            self.in_pass = false;
            let (resume, inputs) = self.after_invocation(&hooks, &state, out).await?;
            let result = outcome?;
            let stop_reason = result.stop_reason();
            let inputs = match &stop_reason {
                StopReason::EndTurn | StopReason::StopSequence | StopReason::Interrupt => inputs,
                other => {
                    continuation::abandon(inputs, &continuation::stopped_after(other));
                    Vec::new()
                }
            };
            continuation = continuation::prepare(
                inputs,
                Some(&mut self.deferred_continuations),
                Some(&stop_reason),
                normalize_input,
            );
            match resume {
                Some(resume) if continuation.is_some() || should_resume(&context) => args = resume,
                None if continuation.is_some() => args = InvokeArgs::Messages(Vec::new()),
                _ => break result,
            }
        };

        let mut event = AgentResultEvent::new(&mut self.local, result, state);
        hooks.invoke::<AgentResultEvent<'static>>(&mut event).await?;
        out.emit(AgentStreamEvent::AgentResult(event.result().clone()))
            .await;
        Ok(())
    }

    /// Runs one pass through the `AgentStreamStage` chain (TS `_streamWithMiddleware`). An
    /// interrupt a handler raises stops the pass with stop reason `interrupt`.
    async fn stream_with_middleware(
        &mut self,
        pass: Pass,
        options: InvokeOptions,
        out: &Emitter,
    ) -> Result<AgentResult> {
        if !self.local.middleware().has::<AgentStreamStage>() {
            let input = pass.input_for(&pass.args)?;
            let Pass {
                resuming,
                state,
                hooks,
                schema,
                ..
            } = pass;
            return self.run_loop(input, resuming, &state, &hooks, out, schema).await;
        }
        let interrupt =
            MiddlewareInterrupt::new(self.local.interrupt_state().clone(), "middleware:agentStream")
                .with_snapshot();
        let context = AgentStreamContext::new(self.local.tool_agent(), pass.args.clone(), options, interrupt);
        let registry = self.local.middleware().clone();
        let state = pass.state.clone();
        let hooks = pass.hooks.clone();
        let terminal = PassTerminal {
            agent: Arc::new(Mutex::new(Some(&mut *self))),
            pass,
        };
        let mut chain = registry.invoke::<AgentStreamStage>(context, move |context| terminal.stream(context));
        let mut outcome = None;
        while let Some(item) = chain.next().await {
            if outcome.is_some() {
                continue;
            }
            match item {
                Ok(MiddlewareItem::Event(event)) => out.forward(event).await,
                Ok(MiddlewareItem::Result(result)) => outcome = Some(Ok(result.result)),
                Err(error) => outcome = Some(Err(error)),
            }
        }
        let outcome = outcome.unwrap_or_else(|| Err(ended_without_result::<AgentStreamStage>().into()));
        drop(chain);
        match outcome {
            Ok(result) => Ok(result),
            Err(MiddlewareError::Interrupt(interrupt)) => {
                self.interrupted(vec![*interrupt], &state, &hooks, out).await
            }
            Err(MiddlewareError::Failed(error)) => Err(error),
        }
    }

    /// Fires `AfterInvocationEvent`; returns the input a hook asked to resume with and the
    /// follow-up input hooks contributed.
    async fn after_invocation(
        &mut self,
        hooks: &HookRegistry,
        state: &InvocationState,
        out: &Emitter,
    ) -> Result<(Option<InvokeArgs>, Vec<ContinuationInput>)> {
        // Pending while the hooks run, so a drop inside them is retried like `abandoned_hooks`.
        self.abandoned = Some(AbandonedRun {
            state: state.clone(),
            repair: None,
        });
        let mut event = AfterInvocationEvent::new(&mut self.local, state.clone());
        let invoked = hooks.invoke::<AfterInvocationEvent<'static>>(&mut event).await;
        self.abandoned = None;
        invoked?;
        out.emit(AgentStreamEvent::AfterInvocation((&event).into())).await;
        let inputs = event.take_continuations();
        Ok((event.take_resume(), inputs))
    }

    /// The cycles of one invocation (TS `_stream`).
    async fn run_loop(
        &mut self,
        input: PassInput,
        resuming: bool,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
        schema: Option<StructuredOutputSchema>,
    ) -> Result<AgentResult> {
        if !resuming && self.local.interrupt_state().lock().activated {
            return Err(Error::other(
                "Agent is in an interrupted state. Resume by invoking with interruptResponse content blocks.",
            ));
        }
        // A resume continues the paused cycle; hook-added input isn't appended.
        let input = if resuming {
            PassInput {
                messages: Vec::new(),
                continuation: input.continuation,
            }
        } else {
            input
        };
        {
            let mut telemetry = self.local.telemetry().lock();
            telemetry.meter.start_new_invocation();
            telemetry.tracer.reset();
        }
        let mut structured = Structured {
            tool: schema.map(StructuredOutputTool::new),
            choice: None,
        };
        // Before the structured-output tool is registered, as in TS: the span doesn't list it.
        self.start_agent_span(&input.messages);
        if let Some(tool) = &structured.tool {
            if let Err(error) = self.local.tool_registry_mut().add(Arc::new(tool.clone())) {
                let outcome = Err(error);
                self.end_agent_span(&outcome);
                return outcome;
            }
            self.structured_output_registered = true;
        }
        let outcome = self.cycles(input, state, hooks, out, &mut structured).await;
        self.end_agent_span(&outcome);
        if structured.tool.is_some() {
            self.local.tool_registry_mut().remove(STRUCTURED_OUTPUT_TOOL_NAME);
            self.structured_output_registered = false;
        }
        outcome
    }

    async fn cycles(
        &mut self,
        input: PassInput,
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
            if self.local.cancel_signal().is_cancelled() {
                return self.cancelled_at_checkpoint(state, hooks, out).await;
            }
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
                let mut telemetry = self.local.telemetry().lock();
                let (cycle_id, started) = telemetry.meter.start_cycle();
                telemetry.tracer.start_cycle(&cycle_id, self.local.messages());
                started
            };
            let cycle = self
                .run_cycle(input.take(), state, hooks, out, structured, &context)
                .await;
            {
                let mut telemetry = self.local.telemetry().lock();
                telemetry.meter.end_cycle(started);
                // TS closes the cycle with whatever error leaves it, a cancel or an interrupt too.
                let error = match &cycle {
                    Err(Stop::Failed(error)) => Some(error.to_string()),
                    Err(Stop::Cancelled) => Some(CancelledError.to_string()),
                    Err(Stop::Interrupted(interrupts)) => Some(interrupt_error_message(interrupts)),
                    Ok(_) => None,
                };
                telemetry.tracer.end_cycle(error.as_deref());
            }
            match cycle {
                Ok(Cycle::Continue) => {}
                Ok(Cycle::Done(result)) => return Ok(self.with_telemetry(*result)),
                Err(Stop::Cancelled) => return self.cancelled_at_checkpoint(state, hooks, out).await,
                Err(Stop::Interrupted(interrupts)) => {
                    let result = self.interrupted(interrupts, state, hooks, out).await?;
                    return Ok(self.with_telemetry(result));
                }
                Err(Stop::Failed(error)) => return Err(error),
            }
        }
    }

    /// TS `_stream`'s `CancelledError` handler: closes an open user turn with the cancel message.
    async fn cancelled_at_checkpoint(
        &mut self,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<AgentResult> {
        let message = self.cancel_message();
        if self
            .local
            .messages()
            .last()
            .is_some_and(|last| last.role() == Role::User)
        {
            self.append_message(message.clone(), state, hooks, out).await?;
        }
        Ok(self.result(StopReason::Cancelled, message, state))
    }

    /// The final message of a cancelled invocation (TS `cancelResultMessage`, else the default).
    fn cancel_message(&self) -> Message {
        let reason = self
            .local
            .cancel_signal()
            .reason()
            .filter(|reason| !reason.is_empty());
        assistant_text(reason.as_deref().unwrap_or("Cancelled by user"))
    }

    /// Ends the cycle as cancelled: appends the cancel message and builds the result.
    async fn cancelled(
        &mut self,
        structured_output: Option<JsonValue>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Step<Cycle> {
        // Always ends on an assistant message, so history stays re-invokable.
        let message = self.cancel_message();
        self.append_message(message.clone(), state, hooks, out).await?;
        let mut result = AgentResult::new(StopReason::Cancelled, message, state.clone());
        if let Some(output) = structured_output {
            result = result.with_structured_output(output);
        }
        Ok(Cycle::Done(Box::new(result)))
    }

    async fn run_cycle(
        &mut self,
        input: Option<PassInput>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
        structured: &mut Structured,
        context: &InvocationContext,
    ) -> Step<Cycle> {
        if let Some(input) = input {
            match input.continuation {
                Some(prepared) => {
                    self.append_continuation_messages(input.messages, prepared, state, hooks, out)
                        .await?;
                }
                None => {
                    for message in input.messages {
                        self.append_message(message, state, hooks, out).await?;
                    }
                }
            }
        }

        let forced = structured.choice.clone();
        // Resuming an interrupted batch skips the model call (TS `getPendingExecution`).
        let pending = self.local.interrupt_state().lock().pending_execution();
        let (assistant, completed) = match pending {
            Some(pending) => (
                pending.assistant_message_data,
                pending
                    .completed_tool_results
                    .into_iter()
                    .map(|(id, entry)| (id, entry.tool_result))
                    .collect(),
            ),
            None => {
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
                if self.local.cancel_signal().is_cancelled() {
                    return self.cancelled_before_tools(assistant, state, hooks, out).await;
                }
                (assistant, IndexMap::new())
            }
        };

        let tools = self
            .execute_tools(&assistant, completed, state, hooks, out)
            .await?;
        if !tools.interrupts.is_empty() {
            // TS throws the batch's `InterruptError` out of the cycle, which closes its span with it.
            return Err(Stop::Interrupted(tools.interrupts));
        }
        // A skipped resume keeps its pending batch; the next cycle's checkpoint stops the loop.
        if tools.skipped
            && self.local.cancel_signal().is_cancelled()
            && self
                .local
                .interrupt_state()
                .lock()
                .pending_tool_execution
                .is_some()
        {
            return Ok(Cycle::Continue);
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
            if interrupt_state.activated {
                interrupt_state.deactivate();
            }
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
        let cancel = self.local.cancel_signal();
        if cancel.is_deferred() || cancel.is_cancelled() {
            // Wins over a structured-output stop, which still carries its output.
            return self.cancelled(output, state, hooks, out).await;
        }
        match output {
            Some(output) => Ok(Cycle::Done(Box::new(
                AgentResult::new(StopReason::ToolUse, assistant, state.clone())
                    .with_structured_output(output),
            ))),
            None => Ok(Cycle::Continue),
        }
    }

    /// The cancel checkpoint between the model's tool use and its tools: every call gets the
    /// `Tool execution cancelled` result without running.
    async fn cancelled_before_tools(
        &mut self,
        assistant: Message,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Step<Cycle> {
        let results = tool_uses(&assistant)
            .map(|tool_use| {
                ContentBlock::ToolResult(error_text_result(tool_use.tool_use_id(), TOOL_CANCELLED))
            })
            .collect();
        self.append_message(assistant, state, hooks, out).await?;
        self.append_message(Message::new(Role::User, results), state, hooks, out)
            .await?;
        self.cancelled(None, state, hooks, out).await
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
    ) -> Step<StreamAggregatedResult> {
        let tool_specs = self.local.tool_registry().tool_specs();
        let mut attempt_count = NonZeroU32::MIN;
        loop {
            // A cancel during a retry backoff stops before the next attempt.
            if self.local.cancel_signal().is_cancelled() {
                return Err(Stop::Cancelled);
            }
            let model = self
                .routed_model()
                .unwrap_or_else(|| Arc::clone(self.local.model()));
            let projected = self.estimate_input_tokens(&tool_specs).await;
            let mut before =
                BeforeModelCallEvent::new(&mut self.local, Arc::clone(&model), state.clone(), projected);
            hooks.invoke::<BeforeModelCallEvent<'static>>(&mut before).await?;
            let raised = before.take_raised_interrupts();
            if !raised.is_empty() {
                // TS throws from the hook, before the event is yielded.
                return Err(Stop::Interrupted(raised));
            }
            out.emit(AgentStreamEvent::BeforeModelCall((&before).into()))
                .await;
            let inputs = before.take_continuations();
            let cancel = before.take_cancel();
            let model_continuation = continuation::prepare(inputs, None, None, normalize_input);

            if let Some(cancel) = cancel {
                if let Some(prepared) = model_continuation {
                    prepared.abandon(continuation::CANCELLED_BY_MODEL_HOOK);
                }
                let message = assistant_text(cancel.message().unwrap_or("model call denied by hook"));
                let stop_data = ModelStopData::new(message.clone(), StopReason::EndTurn);
                let mut after = AfterModelCallEvent::new(
                    &mut self.local,
                    Arc::clone(&model),
                    state.clone(),
                    attempt_count,
                )
                .with_stop_data(stop_data);
                hooks.invoke::<AfterModelCallEvent<'static>>(&mut after).await?;
                out.emit(AgentStreamEvent::AfterModelCall((&after).into())).await;
                if after.retry() {
                    attempt_count = self.next_attempt_count(&model, attempt_count);
                    continue;
                }
                return Ok(StreamAggregatedResult::new(message, StopReason::EndTurn));
            }
            let mut projected = projected;
            if let Some(mut prepared) = model_continuation {
                let messages = std::mem::take(&mut prepared.messages);
                self.append_continuation_messages(messages, prepared, state, hooks, out)
                    .await?;
                projected = self.estimate_input_tokens(&tool_specs).await;
            }

            let outcome = self
                .invoke_model_with_middleware(&model, forcing.clone(), projected, state, hooks, out)
                .await;
            let routed = self.routed_model().unwrap_or_else(|| Arc::clone(&model));
            match outcome {
                Ok((result, model)) => {
                    self.local
                        .telemetry()
                        .lock()
                        .meter
                        .update_cycle(result.metadata());
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
                        attempt_count = self.next_attempt_count(&routed, attempt_count);
                        continue;
                    }
                    return Ok(result);
                }
                Err((stop, model)) => {
                    let (error, stop) = match stop {
                        Stop::Failed(error) => (error, None),
                        Stop::Cancelled => (Error::other(CancelledError), Some(Stop::Cancelled)),
                        Stop::Interrupted(interrupts) => {
                            let names: Vec<&str> = interrupts.iter().map(Interrupt::name).collect();
                            let error = Error::other(format!("interrupt {}", names.join(", ")));
                            (error, Some(Stop::Interrupted(interrupts)))
                        }
                    };
                    let mut after =
                        AfterModelCallEvent::new(&mut self.local, model, state.clone(), attempt_count)
                            .with_error(&error);
                    hooks.invoke::<AfterModelCallEvent<'static>>(&mut after).await?;
                    out.emit(AgentStreamEvent::AfterModelCall((&after).into())).await;
                    // The event pair stays, but a cancel is never retried.
                    if matches!(stop, Some(Stop::Cancelled)) {
                        return Err(Stop::Cancelled);
                    }
                    if after.retry() {
                        attempt_count = self.next_attempt_count(&routed, attempt_count);
                        continue;
                    }
                    return Err(stop.unwrap_or(Stop::Failed(error)));
                }
            }
        }
    }

    /// Runs one model call through the `InvokeModelStage` chain (TS
    /// `_invokeModelWithMiddleware`), firing hooks on the events the chain yields. Returns the
    /// model the terminal invoked, or `model` when the chain answered without it.
    async fn invoke_model_with_middleware(
        &mut self,
        model: &Arc<dyn Model>,
        forcing: Forcing,
        projected: Option<u64>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> std::result::Result<(StreamAggregatedResult, Arc<dyn Model>), (Stop, Arc<dyn Model>)> {
        let tool_agent = self.local.tool_agent();
        let context = InvokeModelContext::new(
            tool_agent.clone(),
            Arc::clone(model),
            self.local.messages().to_vec(),
            self.local.system_prompt().cloned(),
            // Read here, after the `BeforeModelCallEvent` hooks, as TS does: a tool a hook
            // registers or removes is in this call's request.
            self.local.tool_registry().tool_specs(),
            forcing.tool_choice,
            state.clone(),
            projected,
            forcing.structured_output_schema,
        );
        let terminal = ModelTerminal {
            base_state: self.local.model_state().snapshot(),
            written_state: Arc::default(),
            invoked: Arc::default(),
            cancel: self.local.cancel_signal().clone(),
            telemetry: self.local.telemetry().clone(),
            invocation_context: self.local.invocation_context().cloned(),
            agent_metadata: self.local.agent_metadata().cloned(),
        };
        let written_state = Arc::clone(&terminal.written_state);
        let invoked = Arc::clone(&terminal.invoked);
        let mut chain = self
            .local
            .middleware()
            .invoke::<InvokeModelStage>(context, move |context| terminal.stream(context));
        // Drained past the result, so handler code after its `next` stream still runs.
        let mut outcome = None;
        while let Some(item) = chain.next().await {
            // A handler's write to the history (TS mutates `agent.messages` in place) lands before
            // the hooks of the call's events run.
            if let Some(messages) = tool_agent.take_history_edit() {
                self.local.set_messages(messages);
            }
            if outcome.is_some() {
                continue;
            }
            match item {
                Ok(MiddlewareItem::Event(record)) => match self.record_hooks(record, state, hooks).await {
                    Ok(record) => out.emit(record).await,
                    Err(error) => outcome = Some(Err(Stop::Failed(error))),
                },
                Ok(MiddlewareItem::Result(result)) => outcome = Some(Ok(result.result)),
                Err(error) => outcome = Some(Err(self.classify(error))),
            }
        }
        let outcome =
            outcome.unwrap_or_else(|| Err(Stop::Failed(ended_without_result::<InvokeModelStage>())));
        drop(chain);
        if let Some(messages) = tool_agent.take_history_edit() {
            self.local.set_messages(messages);
        }
        let invoked = lock(&invoked).take();
        match outcome {
            Ok(result) => {
                // Committed after the whole chain, so no handler's write to the state persists.
                if let Some(written) = lock(&written_state).take() {
                    self.local.model_state().load_json(written.to_json());
                }
                Ok((result, invoked.unwrap_or_else(|| Arc::clone(model))))
            }
            // A failure before the terminal ran is attributed to the routed model.
            Err(stop) => {
                let failed = invoked
                    .or_else(|| self.routed_model())
                    .unwrap_or_else(|| Arc::clone(model));
                Err((stop, failed))
            }
        }
    }

    /// The model the router chose for this invocation (TS `getRoutedModel`), once it has.
    fn routed_model(&self) -> Option<Arc<dyn Model>> {
        self.model_router.as_ref()?.routed_model(&self.local)
    }

    /// The next attempt number (TS `_nextAttemptCount`): `1` again when the router moved off
    /// `model`.
    fn next_attempt_count(&self, model: &Arc<dyn Model>, attempt_count: NonZeroU32) -> NonZeroU32 {
        match self.routed_model() {
            Some(next) if !Arc::ptr_eq(&next, model) => NonZeroU32::MIN,
            _ => attempt_count.saturating_add(1),
        }
    }

    /// Why a chain stopped; a failure while the invocation is cancelled is its cancel.
    fn classify(&self, error: MiddlewareError) -> Stop {
        match error {
            MiddlewareError::Interrupt(interrupt) => Stop::Interrupted(vec![*interrupt]),
            _ if self.local.cancel_signal().is_cancelled() => Stop::Cancelled,
            MiddlewareError::Failed(error) => Stop::Failed(error),
        }
    }

    /// Fires the hooks of a model event a chain yielded (TS `_streamCore`'s `_invokeCallbacks`)
    /// and returns its record. Other events pass without hooks.
    async fn record_hooks(
        &mut self,
        record: AgentStreamEvent,
        state: &InvocationState,
        hooks: &HookRegistry,
    ) -> Result<AgentStreamEvent> {
        match record {
            AgentStreamEvent::ModelStreamUpdate(data) => {
                let mut event =
                    ModelStreamUpdateEvent::new(&mut self.local, data.event().clone(), state.clone());
                hooks
                    .invoke::<ModelStreamUpdateEvent<'static>>(&mut event)
                    .await?;
                Ok(AgentStreamEvent::ModelStreamUpdate((&event).into()))
            }
            AgentStreamEvent::ContentBlock(data) => {
                let mut event =
                    ContentBlockEvent::new(&mut self.local, data.content_block().clone(), state.clone());
                hooks.invoke::<ContentBlockEvent<'static>>(&mut event).await?;
                Ok(AgentStreamEvent::ContentBlock((&event).into()))
            }
            other => Ok(other),
        }
    }

    /// Runs the tool calls of `assistant` (TS `executeTools`). `AfterToolsEvent` fires even when
    /// the batch fails.
    async fn execute_tools(
        &mut self,
        assistant: &Message,
        completed_tool_results: IndexMap<String, ToolResultBlock>,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Step<ToolsOutcome> {
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
                skipped: false,
            });
        }
        let record = AgentStreamEvent::BeforeTools((&before).into());
        let cancel = before.take_cancel();
        out.emit(record).await;

        let tool_uses: Vec<ToolUseBlock> = tool_uses(assistant).cloned().collect();
        // Pre-launch cancellation is executor-independent (TS `executeTools`).
        let cancel_text = match &cancel {
            Some(cancel) => Some(cancel.message().unwrap_or("Tool cancelled by hook")),
            None => self
                .local
                .cancel_signal()
                .is_cancelled()
                .then_some(TOOL_CANCELLED),
        };
        let mut results: Vec<ToolResultBlock> = Vec::new();
        let mut interrupts = Vec::new();
        let mut failure = None;
        if tool_uses.is_empty() {
            failure = Some(Error::other(
                "Model indicated toolUse but no tool use blocks found in message",
            ));
        } else if let Some(text) = cancel_text {
            let agent = self.local.tool_agent();
            for tool_use in &tool_uses {
                let result = error_text_result(tool_use.tool_use_id(), text);
                let mut event = ToolResultEvent::new(agent.clone(), result.clone(), state.clone());
                hooks.invoke::<ToolResultEvent>(&mut event).await?;
                out.emit(AgentStreamEvent::ToolResult((&event).into())).await;
                results.push(result);
            }
        } else {
            let tool_agent = self.local.tool_agent();
            let input = ToolExecutionInput {
                agent: tool_agent.clone(),
                hooks: hooks.clone(),
                tool_uses,
                invocation_state: state.clone(),
                assistant_message: assistant.clone(),
                completed_tool_results,
                telemetry: self.local.telemetry().clone(),
                background: self.background_tasks.clone().map(|scheduler| BackgroundRoute {
                    scheduler,
                    pass_id: assistant.tracking_id().to_owned(),
                }),
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
            if let Some(messages) = tool_agent.take_history_edit() {
                self.local.set_messages(messages);
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
            return Err(error.into());
        }
        Ok(ToolsOutcome {
            message: after.message().clone(),
            interrupts,
            end_turn: after.take_end_turn(),
            skipped: cancel_text.is_some(),
        })
    }

    fn store_pending(&self, assistant: &Message) {
        self.local.interrupt_state().lock().pending_tool_execution =
            Some(crate::interrupt::state::PendingToolExecution {
                assistant_message_data: assistant.clone(),
                completed_tool_results: IndexMap::new(),
            });
    }

    /// Appends follow-up input, merging a message into the last one when they share a role, then
    /// runs the inputs' `on_appended` callbacks and fires one `MessageAddedEvent` per message
    /// that changed (TS `_appendContinuationMessages`).
    async fn append_continuation_messages(
        &mut self,
        messages: Vec<Message>,
        prepared: Prepared,
        state: &InvocationState,
        hooks: &HookRegistry,
        out: &Emitter,
    ) -> Result<()> {
        let mut added: Vec<Message> = Vec::new();
        // Whether the last added message is the one now last in history.
        let mut last_is_added = false;
        for message in messages {
            let history = self.local.messages_mut();
            match history.last_mut() {
                Some(last) if last.role() == message.role() => {
                    let mut content = last.content().to_vec();
                    content.extend(message.content().iter().cloned());
                    let mut merged =
                        Message::new(message.role(), content).with_tracking_id(last.tracking_id());
                    if let Some(metadata) = last.metadata() {
                        merged = merged.with_metadata(metadata.clone());
                    }
                    *last = merged.clone();
                    if last_is_added {
                        added.pop();
                    }
                    added.push(merged);
                }
                _ => {
                    history.push(message.clone());
                    added.push(message);
                }
            }
            last_is_added = true;
        }
        prepared.mark_appended();
        for message in added {
            let mut event = MessageAddedEvent::new(&mut self.local, message, state.clone());
            hooks.invoke::<MessageAddedEvent<'static>>(&mut event).await?;
            out.emit(AgentStreamEvent::MessageAdded((&event).into())).await;
        }
        Ok(())
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
                Some(newer) if !newer.is_empty() => ModelProxy::new(model.as_ref())
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
                ModelProxy::new(model.as_ref())
                    .count_tokens(messages, options)
                    .await
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

    /// TS `startAgentSpan` with the invocation's input messages.
    fn start_agent_span(&self, input: &[Message]) {
        let tool_names = self
            .local
            .tool_registry()
            .list()
            .map(|tool| tool.name().to_owned())
            .collect();
        self.local.telemetry().lock().tracer.start_agent(&AgentSpanStart {
            messages: input,
            agent_name: self.local.name(),
            agent_id: self.local.id(),
            model_id: self.local.model().model_id(),
            tool_names,
            tools_config: None,
            system_prompt: self.local.system_prompt(),
        });
    }

    /// TS `endAgentSpan` with the result or the error of the invocation.
    fn end_agent_span(&self, outcome: &Result<AgentResult>) {
        let mut telemetry = self.local.telemetry().lock();
        let usage = telemetry.meter.metrics().accumulated_usage().clone();
        match outcome {
            Ok(result) => telemetry.tracer.end_agent(
                Some(result.last_message()),
                Some(result.stop_reason().as_str()),
                &usage,
                None,
            ),
            Err(error) => telemetry
                .tracer
                .end_agent(None, None, &usage, Some(&error.to_string())),
        }
    }

    fn with_telemetry(&self, result: AgentResult) -> AgentResult {
        let telemetry = self.local.telemetry().lock();
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
/// One pass of an invocation as the loop prepared it.
struct Pass {
    /// The invocation arguments, before the hooks.
    args: InvokeArgs,
    /// The input after `BeforeInvocationEvent` hooks.
    input: Vec<Message>,
    resuming: bool,
    state: InvocationState,
    hooks: HookRegistry,
    schema: Option<StructuredOutputSchema>,
    /// Follow-up input from the previous pass, taken when this pass incorporates it.
    continuation: Arc<Mutex<Option<Prepared>>>,
}

/// What a pass appends before its first model call.
struct PassInput {
    messages: Vec<Message>,
    /// Set when `messages` start with follow-up input; consecutive same-role messages merge.
    continuation: Option<Prepared>,
}

impl Pass {
    /// The pass input for `args` (the stage context's), combined with follow-up input from the
    /// previous pass (TS `continuations.combine` in `_streamWithMiddleware`). Replaced arguments
    /// replace the hook-processed input.
    fn input_for(&self, args: &InvokeArgs) -> Result<PassInput> {
        let replaced = *args != self.args;
        let public = normalize_input(args.clone())?;
        let mut slot = lock(&self.continuation);
        let applies = slot.is_some() && (!public.is_empty() || continuation::is_empty_list(args));
        let input = if replaced { public } else { self.input.clone() };
        let Some(mut prepared) = slot.take().filter(|_| applies) else {
            return Ok(PassInput {
                messages: input,
                continuation: None,
            });
        };
        let mut messages = std::mem::take(&mut prepared.messages);
        messages.extend(input);
        Ok(PassInput {
            messages,
            continuation: Some(prepared),
        })
    }
}

/// The innermost layer of the `AgentStreamStage` chain: the pass's cycles (TS `_streamCore`). It
/// lends the agent to one run at a time.
struct PassTerminal<'a> {
    agent: Arc<Mutex<Option<&'a mut Agent>>>,
    pass: Pass,
}

/// The agent lent to one run of the pass, returned when the run ends or is dropped.
struct Lease<'a> {
    slot: Arc<Mutex<Option<&'a mut Agent>>>,
    agent: Option<&'a mut Agent>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.take() {
            *lock(&self.slot) = Some(agent);
        }
    }
}

impl<'a> PassTerminal<'a> {
    fn stream(&self, context: AgentStreamContext) -> MiddlewareStream<'a, AgentStreamStage> {
        let fail = |message: &str| -> MiddlewareStream<'a, AgentStreamStage> {
            Box::pin(futures::stream::iter([Err(
                Error::other(message.to_owned()).into()
            )]))
        };
        let Some(agent) = lock(&self.agent).take() else {
            return fail("the agent stream is already running; a middleware ran `next` twice at once");
        };
        let mut lease = Lease {
            slot: Arc::clone(&self.agent),
            agent: Some(agent),
        };
        // A handler that replaced the arguments replaces the hook-processed input.
        let input = match self.pass.input_for(&context.args) {
            Ok(input) => input,
            Err(error) => return Box::pin(futures::stream::iter([Err(error.into())])),
        };
        let schema = context
            .options
            .structured_output_schema
            .or_else(|| self.pass.schema.clone());
        let resuming = self.pass.resuming;
        let state = self.pass.state.clone();
        let hooks = self.pass.hooks.clone();
        let printer = lease.agent.as_ref().and_then(|agent| agent.printer.clone());
        let driven = Driven::new(printer, move |emitter| async move {
            let Some(agent) = lease.agent.as_deref_mut() else {
                return Err(Error::other("the agent was not lent to the pass"));
            };
            agent
                .run_loop(input, resuming, &state, &hooks, &emitter, schema)
                .await
        });
        Box::pin(driven.map(|item| match item {
            DrivenItem::Event(event) => Ok(MiddlewareItem::Event(event)),
            DrivenItem::Done(Ok(result)) => Ok(MiddlewareItem::Result(AgentStreamResult::new(result))),
            DrivenItem::Done(Err(error)) => Err(error.into()),
        }))
    }
}

/// The innermost layer of the `InvokeModelStage` chain: the model call itself (TS
/// `_streamFromModel`). Each run starts from the model state as it was before the chain.
struct ModelTerminal {
    base_state: StateStore,
    /// The state the provider wrote in the last run, committed when the chain succeeds.
    written_state: Arc<Mutex<Option<StateStore>>>,
    /// The model the last run invoked.
    invoked: Arc<Mutex<Option<Arc<dyn Model>>>>,
    cancel: CancelSignal,
    telemetry: LocalTelemetry,
    invocation_context: Option<InvocationContext>,
    agent_metadata: Option<AgentMetadata>,
}

impl ModelTerminal {
    fn stream(&self, context: InvokeModelContext) -> MiddlewareStream<'static, InvokeModelStage> {
        *lock(&self.invoked) = Some(Arc::clone(&context.model));
        let model_state = self.base_state.snapshot();
        *lock(&self.written_state) = Some(model_state.clone());
        self.telemetry.lock().tracer.start_model(
            &context.messages,
            context.model.model_id().as_deref(),
            context.system_prompt.as_ref(),
        );
        let request = ModelRequest::builder()
            .messages(fill_empty_assistant_messages(normalize_tool_use_names(
                &context.messages,
            )))
            .maybe_system_prompt(context.system_prompt)
            .tool_specs(context.tool_specs)
            .maybe_tool_choice(context.tool_choice)
            .maybe_structured_output_schema(context.structured_output_schema)
            .maybe_dynamic_trailing_blocks(context.dynamic_trailing_blocks)
            .maybe_agent_metadata(self.agent_metadata.clone())
            .model_state(model_state)
            .cancel_signal(self.cancel.clone())
            .build();
        let telemetry = self.telemetry.clone();
        let cancel = self.cancel.clone();
        let mut stream = ModelProxy::new(context.model.as_ref())
            .stream_aggregated(request, self.invocation_context.clone());
        Box::pin(async_stream::stream! {
            let mut result = None;
            loop {
                // A cancel doesn't wait for a silent model: dropping the stream drops the request.
                let item = tokio::select! {
                    biased;
                    item = stream.next() => item,
                    () = cancel.cancelled() => Some(Err(Error::other(CancelledError))),
                };
                let Some(item) = item else { break };
                let cancelled = cancel.is_cancelled();
                match item {
                    Ok(AggregatedEvent::Done(done)) => result = Some(*done),
                    // Providers surface a cancel as a transport error; the cancel wins.
                    Ok(_) | Err(_) if cancelled => {
                        telemetry.lock().tracer.end_model(&ModelSpanEnd::failed(&CancelledError));
                        yield Err(MiddlewareError::Failed(Error::other(CancelledError)));
                        return;
                    }
                    Err(error) => {
                        telemetry.lock().tracer.end_model(&ModelSpanEnd::failed(&error));
                        yield Err(MiddlewareError::Failed(error));
                        return;
                    }
                    Ok(AggregatedEvent::ModelStreamEvent(event)) => {
                        let record = ModelStreamUpdateEventData::new(event);
                        yield Ok(MiddlewareItem::Event(AgentStreamEvent::ModelStreamUpdate(record)));
                    }
                    Ok(AggregatedEvent::ContentBlock(block)) => {
                        let record = ContentBlockEventData::new(block);
                        yield Ok(MiddlewareItem::Event(AgentStreamEvent::ContentBlock(record)));
                    }
                }
            }
            let Some(result) = result else {
                telemetry.lock().tracer.end_model(&ModelSpanEnd::default());
                yield Err(MiddlewareError::Failed(Error::other("the model stream ended without a result")));
                return;
            };
            telemetry.lock().tracer.end_model(&ModelSpanEnd {
                output: Some(result.message()),
                stop_reason: Some(result.stop_reason().as_str()),
                usage: result.metadata().and_then(|metadata| metadata.usage()),
                metrics: result.metadata().and_then(|metadata| metadata.metrics()),
                error: None,
            });
            yield Ok(MiddlewareItem::Result(InvokeModelResult::new(result)));
        })
    }
}

fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

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
        ToolExecutorEvent::Record(event) => event.clone(),
    }
}

fn tool_uses(message: &Message) -> impl Iterator<Item = &ToolUseBlock> {
    message.content().iter().filter_map(|block| match block {
        ContentBlock::ToolUse(tool_use) => Some(tool_use),
        _ => None,
    })
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
                .all(|block| matches!(block, ContentBlock::Text(text) if js_trim(text.text()).is_empty()))
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
