//! Local agent traces (TS `telemetry/tracer.ts` `AgentTrace` and the local trace state). OpenTelemetry
//! spans join in the `otel` feature.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::{Serialize, Serializer};
use tokio::time::Instant;

use crate::types::messages::Message;

fn as_ms<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

/// Epoch milliseconds, as TS `Date.now()`.
fn timestamp_ms<S: Serializer>(timestamp: &SystemTime, serializer: S) -> Result<S::Ok, S::Error> {
    let ms = match timestamp.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    };
    serializer.serialize_i64(ms)
}

fn optional_timestamp_ms<S: Serializer>(
    timestamp: &Option<SystemTime>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match timestamp {
        Some(timestamp) => timestamp_ms(timestamp, serializer),
        None => serializer.serialize_none(),
    }
}

/// One node of the local execution trace: a cycle, a model call or a tool call (TS `AgentTrace`).
///
/// Serializes as TS `toJSON()`; times are epoch milliseconds and the duration whole milliseconds.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentTrace {
    id: String,
    name: String,
    parent_id: Option<String>,
    #[serde(serialize_with = "timestamp_ms")]
    start_time: SystemTime,
    #[serde(serialize_with = "optional_timestamp_ms")]
    end_time: Option<SystemTime>,
    #[serde(serialize_with = "as_ms")]
    duration: Duration,
    children: Vec<AgentTrace>,
    metadata: IndexMap<String, String>,
    message: Option<Message>,
    #[serde(skip)]
    started: Instant,
}

impl AgentTrace {
    fn new(name: impl Into<String>, parent_id: Option<String>) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            name: name.into(),
            parent_id,
            start_time: SystemTime::now(),
            end_time: None,
            duration: Duration::ZERO,
            children: Vec::new(),
            metadata: IndexMap::new(),
            message: None,
            started: Instant::now(),
        }
    }

    fn child(&self, name: impl Into<String>) -> Self {
        Self::new(name, Some(self.id.clone()))
    }

    fn end(&mut self) {
        let duration = self.started.elapsed();
        self.duration = duration;
        self.end_time = Some(self.start_time.checked_add(duration).unwrap_or(self.start_time));
    }

    /// A random unique id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `Cycle <n>`, `stream_messages` or `Tool: <name>`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The id of the enclosing trace.
    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    /// When the traced step started.
    pub fn start_time(&self) -> SystemTime {
        self.start_time
    }

    /// When the traced step ended; `None` while it runs.
    pub fn end_time(&self) -> Option<SystemTime> {
        self.end_time
    }

    /// How long the step took; zero while it runs.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// The nested steps, in start order.
    pub fn children(&self) -> &[AgentTrace] {
        &self.children
    }

    /// Step details: `cycleId` on cycles, `toolUseId` and `toolName` on tool calls.
    pub fn metadata(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.metadata
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// One metadata value.
    pub fn metadata_value(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(String::as_str)
    }

    /// The model's output message, on model-call traces.
    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }
}

/// The local trace tree of the current invocation (TS `Tracer` local trace state).
#[derive(Debug, Default)]
pub(crate) struct LocalTracer {
    traces: Vec<AgentTrace>,
    cycle_open: bool,
    model_open: bool,
}

impl LocalTracer {
    pub(crate) fn traces(&self) -> &[AgentTrace] {
        &self.traces
    }

    /// TS `startAgentSpan` resets the local traces for each invocation.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn start_cycle(&mut self, cycle_id: &str) {
        let mut trace = AgentTrace::new(format!("Cycle {}", self.traces.len().saturating_add(1)), None);
        trace.metadata.insert("cycleId".to_owned(), cycle_id.to_owned());
        self.traces.push(trace);
        self.cycle_open = true;
        self.model_open = false;
    }

    pub(crate) fn end_cycle(&mut self) {
        if let Some(cycle) = self.open_cycle() {
            cycle.end();
        }
        self.cycle_open = false;
    }

    /// Model calls outside a cycle aren't recorded, as in TS.
    pub(crate) fn start_model(&mut self) {
        if let Some(cycle) = self.open_cycle() {
            let model = cycle.child("stream_messages");
            cycle.children.push(model);
            self.model_open = true;
        }
    }

    pub(crate) fn end_model(&mut self, output: Option<&Message>) {
        if !std::mem::take(&mut self.model_open) {
            return;
        }
        let model = self.open_cycle().and_then(|cycle| {
            cycle
                .children
                .iter_mut()
                .rev()
                .find(|child| child.name == "stream_messages")
        });
        if let Some(model) = model {
            model.message = output.cloned();
            model.end();
        }
    }

    pub(crate) fn start_tool(&mut self, tool_name: &str, tool_use_id: &str) {
        if let Some(cycle) = self.open_cycle() {
            let mut tool = cycle.child(format!("Tool: {tool_name}"));
            tool.metadata
                .insert("toolUseId".to_owned(), tool_use_id.to_owned());
            tool.metadata.insert("toolName".to_owned(), tool_name.to_owned());
            cycle.children.push(tool);
        }
    }

    pub(crate) fn end_tool(&mut self, tool_use_id: &str) {
        let tool = self.open_cycle().and_then(|cycle| {
            cycle.children.iter_mut().rev().find(|child| {
                child.end_time.is_none() && child.metadata_value("toolUseId") == Some(tool_use_id)
            })
        });
        if let Some(tool) = tool {
            tool.end();
        }
    }

    fn open_cycle(&mut self) -> Option<&mut AgentTrace> {
        if self.cycle_open {
            self.traces.last_mut()
        } else {
            None
        }
    }
}
