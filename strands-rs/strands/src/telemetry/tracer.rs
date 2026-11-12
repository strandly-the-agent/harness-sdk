//! Local agent traces (the non-OpenTelemetry part of TS `telemetry/tracer.ts`).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::Serialize;

use crate::types::Message;

/// Milliseconds since the Unix epoch, as TS `Date.now()`.
pub(crate) fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    })
}

/// One node of an invocation's trace tree: a cycle, a model call or a tool call (TS
/// `AgentTrace`). Times are milliseconds since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentTrace {
    /// A random id.
    pub id: String,
    /// `Cycle <n>`, `stream_messages` or `Tool: <name>`.
    pub name: String,
    /// The parent node's id.
    pub parent_id: Option<String>,
    /// When the node started.
    pub start_time: u64,
    /// When the node ended.
    pub end_time: Option<u64>,
    /// `end_time - start_time` in milliseconds.
    pub duration: u64,
    /// Child nodes.
    pub children: Vec<AgentTrace>,
    /// Extra attributes (`cycleId`, `toolUseId`, `toolName`).
    pub metadata: IndexMap<String, String>,
    /// The model's message, on model call nodes.
    pub message: Option<Message>,
}

impl AgentTrace {
    pub(crate) fn new(name: impl Into<String>, parent_id: Option<String>, start_time: u64) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            parent_id,
            start_time,
            end_time: None,
            duration: 0,
            children: Vec::new(),
            metadata: IndexMap::new(),
            message: None,
        }
    }

    fn end(&mut self, end_time: u64) {
        self.end_time = Some(end_time);
        self.duration = end_time.saturating_sub(self.start_time);
    }

    /// The node's duration.
    pub fn elapsed(&self) -> Duration {
        Duration::from_millis(self.duration)
    }
}

/// Builds the trace tree of the running invocation.
#[derive(Debug, Default)]
pub(crate) struct LocalTracer {
    traces: Vec<AgentTrace>,
    cycle_open: bool,
    model_open: bool,
}

impl LocalTracer {
    pub(crate) fn traces(&self) -> Vec<AgentTrace> {
        self.traces.clone()
    }

    pub(crate) fn start_invocation(&mut self) {
        *self = Self::default();
    }

    fn current_cycle(&mut self) -> Option<&mut AgentTrace> {
        if !self.cycle_open {
            return None;
        }
        self.traces.last_mut()
    }

    pub(crate) fn start_cycle(&mut self, cycle_id: &str) {
        let mut cycle = AgentTrace::new(
            format!("Cycle {}", self.traces.len().saturating_add(1)),
            None,
            now_millis(),
        );
        cycle.metadata.insert("cycleId".into(), cycle_id.to_owned());
        self.traces.push(cycle);
        self.cycle_open = true;
    }

    pub(crate) fn end_cycle(&mut self) {
        if let Some(cycle) = self.current_cycle() {
            cycle.end(now_millis());
        }
        self.cycle_open = false;
        self.model_open = false;
    }

    pub(crate) fn start_model(&mut self) {
        let Some(cycle) = self.current_cycle() else {
            return;
        };
        let model = AgentTrace::new("stream_messages", Some(cycle.id.clone()), now_millis());
        cycle.children.push(model);
        self.model_open = true;
    }

    pub(crate) fn end_model(&mut self, message: Option<&Message>) {
        if !self.model_open {
            return;
        }
        self.model_open = false;
        if let Some(model) = self.current_cycle().and_then(|cycle| cycle.children.last_mut()) {
            model.message = message.cloned();
            model.end(now_millis());
        }
    }

    pub(crate) fn record_tool(&mut self, tool_name: &str, tool_use_id: &str, start_time: u64, end_time: u64) {
        let Some(cycle) = self.current_cycle() else {
            return;
        };
        let mut tool = AgentTrace::new(format!("Tool: {tool_name}"), Some(cycle.id.clone()), start_time);
        tool.metadata.insert("toolUseId".into(), tool_use_id.to_owned());
        tool.metadata.insert("toolName".into(), tool_name.to_owned());
        tool.end(end_time);
        cycle.children.push(tool);
    }
}
