//! Agent metrics (`strands-ts/src/telemetry/meter.ts`).

use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::types::{
    now_ms, AgentMetrics, CycleMetrics, InvocationMetrics, ToolMetrics, MAX_RETAINED_INVOCATIONS,
};
use crate::types::Usage;

/// An open cycle returned by [`Meter::start_cycle`].
#[derive(Clone, Debug)]
pub struct CycleHandle {
    /// Cycle id, also set as `agent_loop.cycle_id` on the cycle span.
    pub cycle_id: String,
    /// Start time in milliseconds since the Unix epoch.
    pub start_time: u64,
}

#[derive(Default)]
struct State {
    cycle_count: u64,
    accumulated_usage: Usage,
    accumulated_latency_ms: u64,
    invocations: Vec<InvocationMetrics>,
    tool_metrics: BTreeMap<String, ToolMetrics>,
    latest_context_size: Option<u64>,
    projected_context_size: Option<u64>,
    total_duration: u64,
    current_cycles: Vec<CycleMetrics>,
}

/// Records agent metrics locally and, with the `telemetry` feature, to OTel instruments.
#[derive(Clone, Default)]
pub struct Meter {
    state: Arc<Mutex<State>>,
    #[cfg(feature = "telemetry")]
    instruments: Option<Arc<Instruments>>,
}

impl std::fmt::Debug for Meter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meter").finish_non_exhaustive()
    }
}

impl Meter {
    /// A meter bound to the global OTel meter provider (a no-op provider by default).
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            #[cfg(feature = "telemetry")]
            instruments: Some(Arc::new(Instruments::new())),
        }
    }

    /// Begin a new invocation (TS `startNewInvocation`).
    pub fn start_new_invocation(&self) {
        let mut state = self.state.lock();
        state.current_cycles.clear();
        #[cfg(feature = "telemetry")]
        if let Some(i) = &self.instruments {
            i.invocation_count.add(1, &[]);
        }
    }

    /// Open a cycle (TS `startCycle`).
    pub fn start_cycle(&self) -> CycleHandle {
        #[cfg(feature = "telemetry")]
        if let Some(i) = &self.instruments {
            i.cycle_count.add(1, &[]);
        }
        let mut state = self.state.lock();
        state.cycle_count += 1;
        CycleHandle {
            cycle_id: uuid::Uuid::new_v4().to_string(),
            start_time: now_ms(),
        }
    }

    /// Close a cycle, recording its duration (TS `endCycle`).
    pub fn end_cycle(&self, cycle: &CycleHandle) {
        let duration = now_ms().saturating_sub(cycle.start_time);
        #[cfg(feature = "telemetry")]
        if let Some(i) = &self.instruments {
            i.cycle_duration.record(duration as f64, &[]);
        }
        let mut state = self.state.lock();
        state.total_duration += duration;
        state.current_cycles.push(CycleMetrics {
            cycle_id: cycle.cycle_id.clone(),
            duration,
            usage: Usage::default(),
        });
    }

    /// Record model usage and latency against the open cycle (TS `updateCycle`).
    pub fn update_cycle(&self, usage: &Usage, latency_ms: u64, time_to_first_byte_ms: Option<u64>) {
        #[cfg(feature = "telemetry")]
        if let Some(i) = &self.instruments {
            i.tokens_input.add(usage.input_tokens, &[]);
            i.tokens_output.add(usage.output_tokens, &[]);
            i.model_latency.record(latency_ms as f64, &[]);
            if let Some(ttfb) = time_to_first_byte_ms.filter(|v| *v > 0) {
                i.time_to_first_token.record(ttfb as f64 / 1000.0, &[]);
            }
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = time_to_first_byte_ms;

        let mut state = self.state.lock();
        accumulate(&mut state.accumulated_usage, usage);
        state.accumulated_latency_ms += latency_ms;
        if let Some(cycle) = state.current_cycles.last_mut() {
            accumulate(&mut cycle.usage, usage);
        }
        let context = usage.input_tokens
            + usage.cache_read_input_tokens.unwrap_or(0)
            + usage.cache_write_input_tokens.unwrap_or(0);
        state.latest_context_size = Some(context);
        state.projected_context_size = Some(context + usage.output_tokens);
    }

    /// Record a finished tool call (TS `endToolCall`).
    pub fn end_tool_call(&self, tool: &str, duration_ms: u64, success: bool) {
        #[cfg(feature = "telemetry")]
        if let Some(i) = &self.instruments {
            let attrs = [opentelemetry::KeyValue::new("gen_ai.tool.name", tool.to_string())];
            i.tool_call_count.add(1, &attrs);
            i.tool_duration.record(duration_ms as f64, &attrs);
            if !success {
                i.tool_error_count.add(1, &attrs);
            }
        }
        let mut state = self.state.lock();
        let entry = state.tool_metrics.entry(tool.to_string()).or_default();
        entry.call_count += 1;
        entry.total_time += duration_ms;
        if success {
            entry.success_count += 1;
        } else {
            entry.error_count += 1;
        }
    }

    /// Close the current invocation, retaining the most recent 50 (TS `endInvocation`).
    pub fn end_invocation(&self) {
        let mut state = self.state.lock();
        let cycles = std::mem::take(&mut state.current_cycles);
        let mut usage = Usage::default();
        for cycle in &cycles {
            accumulate(&mut usage, &cycle.usage);
        }
        state.invocations.push(InvocationMetrics { cycles, usage });
        if state.invocations.len() > MAX_RETAINED_INVOCATIONS {
            let excess = state.invocations.len() - MAX_RETAINED_INVOCATIONS;
            state.invocations.drain(0..excess);
        }
    }

    /// A snapshot of all metrics collected so far (TS `get metrics`).
    pub fn metrics(&self) -> AgentMetrics {
        let state = self.state.lock();
        let mut invocations = state.invocations.clone();
        if !state.current_cycles.is_empty() {
            let cycles = state.current_cycles.clone();
            let mut usage = Usage::default();
            for cycle in &cycles {
                accumulate(&mut usage, &cycle.usage);
            }
            invocations.push(InvocationMetrics { cycles, usage });
        }
        AgentMetrics {
            cycle_count: state.cycle_count,
            accumulated_usage: state.accumulated_usage.clone(),
            accumulated_metrics: crate::types::Metrics {
                latency_ms: state.accumulated_latency_ms,
                time_to_first_byte_ms: None,
            },
            agent_invocations: invocations,
            tool_metrics: state.tool_metrics.clone(),
            latest_context_size: state.latest_context_size,
            projected_context_size: state.projected_context_size,
            total_duration: Some(state.total_duration),
        }
    }
}

fn accumulate(target: &mut Usage, source: &Usage) {
    target.input_tokens += source.input_tokens;
    target.output_tokens += source.output_tokens;
    target.total_tokens += source.total_tokens;
    if let Some(read) = source.cache_read_input_tokens {
        target.cache_read_input_tokens = Some(target.cache_read_input_tokens.unwrap_or(0) + read);
    }
    if let Some(write) = source.cache_write_input_tokens {
        target.cache_write_input_tokens = Some(target.cache_write_input_tokens.unwrap_or(0) + write);
    }
}

#[cfg(feature = "telemetry")]
struct Instruments {
    cycle_count: opentelemetry::metrics::Counter<u64>,
    invocation_count: opentelemetry::metrics::Counter<u64>,
    cycle_duration: opentelemetry::metrics::Histogram<f64>,
    tool_call_count: opentelemetry::metrics::Counter<u64>,
    tool_error_count: opentelemetry::metrics::Counter<u64>,
    tool_duration: opentelemetry::metrics::Histogram<f64>,
    tokens_input: opentelemetry::metrics::Counter<u64>,
    tokens_output: opentelemetry::metrics::Counter<u64>,
    model_latency: opentelemetry::metrics::Histogram<f64>,
    time_to_first_token: opentelemetry::metrics::Histogram<f64>,
}

#[cfg(feature = "telemetry")]
impl Instruments {
    fn new() -> Self {
        let meter = opentelemetry::global::meter_with_scope(
            opentelemetry::InstrumentationScope::builder(super::service_name()).build(),
        );
        Self {
            cycle_count: meter
                .u64_counter("gen_ai.agent.cycle.count")
                .with_description("Number of agent loop cycles executed")
                .build(),
            invocation_count: meter
                .u64_counter("gen_ai.agent.invocation.count")
                .with_description("Number of agent invocations")
                .build(),
            cycle_duration: meter
                .f64_histogram("gen_ai.agent.cycle.duration")
                .with_unit("ms")
                .with_description("Duration of agent loop cycles in milliseconds")
                .build(),
            tool_call_count: meter
                .u64_counter("gen_ai.agent.tool.call.count")
                .with_description("Number of tool calls")
                .build(),
            tool_error_count: meter
                .u64_counter("gen_ai.agent.tool.error.count")
                .with_description("Number of failed tool calls")
                .build(),
            tool_duration: meter
                .f64_histogram("gen_ai.agent.tool.duration")
                .with_unit("ms")
                .with_description("Duration of tool calls in milliseconds")
                .build(),
            tokens_input: meter
                .u64_counter("gen_ai.agent.tokens.input")
                .with_description("Input tokens consumed")
                .build(),
            tokens_output: meter
                .u64_counter("gen_ai.agent.tokens.output")
                .with_description("Output tokens consumed")
                .build(),
            model_latency: meter
                .f64_histogram("gen_ai.agent.model.latency")
                .with_unit("ms")
                .with_description("Model invocation latency in milliseconds")
                .build(),
            time_to_first_token: meter
                .f64_histogram("gen_ai.server.time_to_first_token")
                .with_unit("s")
                .with_description("Time to generate first token for successful responses")
                .build(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
            ..Usage::default()
        }
    }

    #[test]
    fn tracks_cycles_and_usage() {
        let meter = Meter::new();
        meter.start_new_invocation();
        let cycle = meter.start_cycle();
        meter.end_cycle(&cycle);
        meter.update_cycle(&usage(10, 4), 120, Some(50));
        let metrics = meter.metrics();
        assert_eq!(metrics.cycle_count, 1);
        assert_eq!(metrics.accumulated_usage.input_tokens, 10);
        assert_eq!(metrics.accumulated_metrics.latency_ms, 120);
        assert_eq!(metrics.latest_context_size, Some(10));
        assert_eq!(metrics.projected_context_size, Some(14));
    }

    #[test]
    fn tracks_tool_success_and_errors() {
        let meter = Meter::new();
        meter.end_tool_call("calc", 5, true);
        meter.end_tool_call("calc", 7, false);
        let metrics = meter.metrics();
        let tool = &metrics.tool_metrics["calc"];
        assert_eq!((tool.call_count, tool.success_count, tool.error_count), (2, 1, 1));
        assert_eq!(tool.total_time, 12);
    }

    #[test]
    fn retains_only_the_most_recent_invocations() {
        let meter = Meter::new();
        for _ in 0..MAX_RETAINED_INVOCATIONS + 5 {
            meter.start_new_invocation();
            let cycle = meter.start_cycle();
            meter.end_cycle(&cycle);
            meter.end_invocation();
        }
        assert_eq!(meter.metrics().agent_invocations.len(), MAX_RETAINED_INVOCATIONS);
    }
}
