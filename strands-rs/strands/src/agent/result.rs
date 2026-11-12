//! The result of an invocation (TS `AgentResult` in `types/agent.ts`).

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};

use crate::agent::{InvocationContext, InvocationState};
use crate::error::{Result, StructuredOutputError};
use crate::interrupt::Interrupt;
use crate::telemetry::{AgentMetrics, AgentTrace};
use crate::tools::structured_output::NO_STRUCTURED_OUTPUT_MESSAGE;
use crate::types::{ContentBlock, JsonValue, Message, StopReason};

/// What an invocation produced (TS `AgentResult`).
///
/// `Display` gives the TS `toString()`: the interrupts as JSON, else the structured output as
/// JSON, else the text of the last message.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AgentResult {
    stop_reason: StopReason,
    last_message: Message,
    traces: Vec<AgentTrace>,
    structured_output: Option<JsonValue>,
    metrics: AgentMetrics,
    invocation_context: Option<InvocationContext>,
    invocation_state: InvocationState,
    interrupts: Vec<Interrupt>,
}

impl AgentResult {
    pub(crate) fn new(
        stop_reason: StopReason,
        last_message: Message,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            stop_reason,
            last_message,
            traces: Vec::new(),
            structured_output: None,
            metrics: AgentMetrics::default(),
            invocation_context: None,
            invocation_state,
            interrupts: Vec::new(),
        }
    }

    pub(crate) fn with_traces(mut self, traces: Vec<AgentTrace>) -> Self {
        self.traces = traces;
        self
    }

    pub(crate) fn with_metrics(mut self, metrics: AgentMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    pub(crate) fn with_invocation_context(mut self, invocation_context: InvocationContext) -> Self {
        self.invocation_context = Some(invocation_context);
        self
    }

    pub(crate) fn with_structured_output(mut self, structured_output: Option<JsonValue>) -> Self {
        self.structured_output = structured_output;
        self
    }

    pub(crate) fn with_interrupts(mut self, interrupts: Vec<Interrupt>) -> Self {
        self.interrupts = interrupts;
        self
    }

    /// Why the invocation stopped.
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason.clone()
    }

    /// The final message.
    pub fn last_message(&self) -> &Message {
        &self.last_message
    }

    /// The trace tree of the invocation's cycles.
    pub fn traces(&self) -> &[AgentTrace] {
        &self.traces
    }

    /// The agent's metrics when the invocation ended.
    pub fn metrics(&self) -> &AgentMetrics {
        &self.metrics
    }

    /// The invocation context.
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// The invocation state the hooks and tools saw.
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The unanswered interrupts, when the invocation stopped on [`StopReason::Interrupt`].
    pub fn interrupts(&self) -> &[Interrupt] {
        &self.interrupts
    }

    /// The structured output as JSON.
    pub fn structured_output_value(&self) -> Option<&JsonValue> {
        self.structured_output.as_ref()
    }

    /// The structured output, deserialized.
    ///
    /// # Errors
    ///
    /// [`StructuredOutputError`] when there is none, [`Error::Json`](crate::Error::Json) when
    /// it doesn't deserialize into `T`.
    pub fn structured_output<T: DeserializeOwned>(&self) -> Result<T> {
        let value = self
            .structured_output
            .clone()
            .ok_or_else(|| StructuredOutputError::new(NO_STRUCTURED_OUTPUT_MESSAGE))?;
        Ok(serde_json::from_value(value)?)
    }

    /// Prompt tokens of the latest model call (TS `contextSize`).
    pub fn context_size(&self) -> Option<u64> {
        self.metrics.latest_context_size
    }

    /// Prompt plus output tokens of the latest model call (TS `projectedContextSize`).
    pub fn projected_context_size(&self) -> Option<u64> {
        self.metrics.projected_context_size
    }
}

impl fmt::Display for AgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.interrupts.is_empty() {
            let json = serde_json::to_string(&self.interrupts).map_err(|_| fmt::Error)?;
            return f.write_str(&json);
        }
        if let Some(output) = &self.structured_output {
            return f.write_str(&output.to_string());
        }
        let mut parts: Vec<String> = Vec::new();
        for block in &self.last_message.content {
            match block {
                ContentBlock::Text(text) => parts.push(text.text.clone()),
                ContentBlock::Reasoning(reasoning) => {
                    if let Some(text) = reasoning.text.as_deref().filter(|text| !text.is_empty()) {
                        parts.push(format!("💭 Reasoning:\n   {}", text.replace('\n', "\n   ")));
                    }
                }
                ContentBlock::Citations(citations) => {
                    parts.extend(citations.content.iter().map(|content| content.text.clone()));
                }
                _ => {}
            }
        }
        f.write_str(&parts.join("\n"))
    }
}

impl Serialize for AgentResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(tag = "type", rename = "agentResult", rename_all = "camelCase")]
        struct Json<'a> {
            stop_reason: &'a StopReason,
            last_message: &'a Message,
            #[serde(skip_serializing_if = "Option::is_none")]
            structured_output: Option<&'a JsonValue>,
        }
        Json {
            stop_reason: &self.stop_reason,
            last_message: &self.last_message,
            structured_output: self.structured_output.as_ref(),
        }
        .serialize(serializer)
    }
}
