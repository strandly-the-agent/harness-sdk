//! Ports `strands-ts/src/telemetry/__tests__/meter.test.ts`. The clock is tokio's paused clock;
//! the OpenTelemetry cases read the instruments back through an `opentelemetry_sdk` meter provider
//! with a recording exporter.

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use super::{AgentLoopMetricsData, AgentMetrics, InvocationMetricsData, Meter, ToolMetricsData};
use crate::models::streaming::{Metrics, ModelMetadataEvent, Usage};

fn usage(input: u64, output: u64, total: u64) -> ModelMetadataEvent {
    ModelMetadataEvent::new().with_usage(Usage::new(input, output, total))
}

fn latency(latency_ms: u64) -> ModelMetadataEvent {
    ModelMetadataEvent::new().with_metrics(Metrics::new(latency_ms))
}

fn tool_metrics(calls: u64, successes: u64, errors: u64, total_time: Duration) -> ToolMetricsData {
    ToolMetricsData {
        call_count: calls,
        success_count: successes,
        error_count: errors,
        total_time,
    }
}

fn tools(metrics: &AgentMetrics) -> Vec<(&str, ToolMetricsData)> {
    metrics
        .tool_metrics()
        .map(|(name, data)| (name, data.clone()))
        .collect()
}

fn cycle(cycle_id: &str, duration_ms: u64, usage: Usage) -> AgentLoopMetricsData {
    AgentLoopMetricsData {
        cycle_id: cycle_id.to_owned(),
        duration: Duration::from_millis(duration_ms),
        usage,
    }
}

const MICROS_500: Duration = Duration::from_micros(500);
const MICROS_1500: Duration = Duration::from_micros(1500);

mod metrics_getter {
    use super::*;

    #[test]
    fn returns_an_agent_metrics_instance() {
        let meter = Meter::default();
        let _: &AgentMetrics = meter.metrics();
    }

    #[test]
    fn returns_zeroed_snapshot_for_fresh_instance() {
        assert_eq!(Meter::default().metrics(), &AgentMetrics::default());
        assert_eq!(Meter::default().metrics().cycle_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn returns_complete_snapshot_after_a_realistic_agent_execution() {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        let (_, c1) = meter.start_cycle();
        meter.update_cycle(Some(&usage(10, 5, 15).with_metrics(Metrics::new(100))));
        meter.end_tool_call("search", MICROS_500, true);
        tokio::time::advance(Duration::from_secs(3)).await;
        meter.end_cycle(c1);

        tokio::time::advance(Duration::from_secs(97)).await;
        let (_, c2) = meter.start_cycle();
        meter.update_cycle(Some(&usage(20, 10, 30).with_metrics(Metrics::new(250))));
        meter.end_tool_call("search", MICROS_1500, false);
        tokio::time::advance(Duration::from_secs(5)).await;
        meter.end_cycle(c2);

        let snapshot = meter.metrics();
        assert_eq!(snapshot.cycle_count(), 2);
        assert_eq!(snapshot.total_duration(), Duration::from_millis(8000));
        assert_eq!(snapshot.accumulated_usage(), &Usage::new(30, 15, 45));
        assert_eq!(snapshot.accumulated_metrics(), &Metrics::new(350));
        assert_eq!(
            tools(snapshot),
            [("search", tool_metrics(2, 1, 1, Duration::from_millis(2)))]
        );
        assert_eq!(
            serde_json::to_value(snapshot.agent_invocations()).unwrap(),
            json!([{
                "usage": { "inputTokens": 30, "outputTokens": 15, "totalTokens": 45 },
                "cycles": [
                    { "cycleId": "cycle-1", "duration": 3000, "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } },
                    { "cycleId": "cycle-2", "duration": 5000, "usage": { "inputTokens": 20, "outputTokens": 10, "totalTokens": 30 } },
                ],
            }])
        );
    }

    #[test]
    fn tracks_multiple_invocations_independently() {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        meter.start_cycle();
        meter.update_cycle(Some(&usage(10, 5, 15)));
        meter.start_new_invocation();
        meter.start_cycle();
        meter.update_cycle(Some(&usage(20, 10, 30)));

        assert_eq!(
            serde_json::to_value(meter.metrics().agent_invocations()).unwrap(),
            json!([
                {
                    "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
                    "cycles": [{ "cycleId": "cycle-1", "duration": 0, "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } }],
                },
                {
                    "usage": { "inputTokens": 20, "outputTokens": 10, "totalTokens": 30 },
                    "cycles": [{ "cycleId": "cycle-2", "duration": 0, "usage": { "inputTokens": 20, "outputTokens": 10, "totalTokens": 30 } }],
                },
            ])
        );
    }
}

mod start_new_invocation {
    use super::*;

    #[test]
    fn appends_an_invocation_with_empty_cycles_and_zeroed_usage() {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        assert_eq!(
            serde_json::to_value(meter.metrics().agent_invocations()).unwrap(),
            json!([{ "cycles": [], "usage": { "inputTokens": 0, "outputTokens": 0, "totalTokens": 0 } }])
        );
    }

    #[test]
    fn latest_agent_invocation_returns_the_most_recently_added_invocation() {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        meter.start_new_invocation();
        let snapshot = meter.metrics();
        assert_eq!(snapshot.agent_invocations().len(), 2);
        assert!(std::ptr::eq(
            snapshot.latest_agent_invocation().unwrap(),
            &snapshot.agent_invocations()[1]
        ));
    }

    #[test]
    fn evicts_oldest_entries_when_exceeding_max_history_cap() {
        let mut meter = Meter::default();
        for _ in 0..60 {
            meter.start_new_invocation();
            meter.start_cycle();
        }
        let invocations = meter.metrics().agent_invocations();
        assert_eq!(invocations.len(), 50);
        assert_eq!(invocations[0].cycles()[0].cycle_id(), "cycle-11");
        assert_eq!(invocations[49].cycles()[0].cycle_id(), "cycle-60");
    }
}

mod start_cycle {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn returns_cycle_id_and_start_time() {
        let mut meter = Meter::default();
        let now = Instant::now();
        let (cycle_id, start_time) = meter.start_cycle();
        assert_eq!(cycle_id, "cycle-1");
        assert_eq!(start_time, now);
        assert_eq!(meter.metrics().cycle_count(), 1);
    }

    #[test]
    fn adds_cycle_entry_to_the_latest_invocation() {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        meter.start_cycle();
        assert_eq!(
            meter.metrics().latest_agent_invocation().unwrap().cycles(),
            [cycle("cycle-1", 0, Usage::default())]
        );
    }

    #[test]
    fn does_not_fail_when_no_invocation_exists() {
        let mut meter = Meter::default();
        let (cycle_id, _) = meter.start_cycle();
        assert_eq!(cycle_id, "cycle-1");
        assert!(meter.metrics().agent_invocations().is_empty());
    }
}

mod end_cycle {
    use super::*;

    const ELAPSED: Duration = Duration::from_millis(100_000);

    #[tokio::test(start_paused = true)]
    async fn records_duration_on_the_latest_cycle() {
        let mut meter = Meter::default();
        let start = Instant::now();
        tokio::time::advance(ELAPSED).await;
        meter.start_new_invocation();
        meter.start_cycle();
        meter.end_cycle(start);
        let cycles = meter.metrics().latest_agent_invocation().unwrap().cycles();
        assert_eq!(cycles[0].duration(), ELAPSED);
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_fail_when_no_invocation_exists() {
        let mut meter = Meter::default();
        let start = Instant::now();
        tokio::time::advance(ELAPSED).await;
        meter.start_cycle();
        meter.end_cycle(start);
        assert!(meter.metrics().agent_invocations().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_fail_when_invocation_has_no_cycles() {
        let mut meter = Meter::default();
        let start = Instant::now();
        tokio::time::advance(ELAPSED).await;
        meter.start_new_invocation();
        meter.end_cycle(start);
        assert!(
            meter
                .metrics()
                .latest_agent_invocation()
                .unwrap()
                .cycles()
                .is_empty()
        );
    }
}

mod end_tool_call {
    use super::*;

    #[test]
    fn records_success() {
        let mut meter = Meter::default();
        meter.end_tool_call("myTool", MICROS_1500, true);
        assert_eq!(
            tools(meter.metrics()),
            [("myTool", tool_metrics(1, 1, 0, MICROS_1500))]
        );
    }

    #[test]
    fn records_failure() {
        let mut meter = Meter::default();
        meter.end_tool_call("myTool", MICROS_500, false);
        assert_eq!(
            tools(meter.metrics()),
            [("myTool", tool_metrics(1, 0, 1, MICROS_500))]
        );
    }

    #[test]
    fn accumulates_across_multiple_calls_to_the_same_tool() {
        let mut meter = Meter::default();
        meter.end_tool_call("myTool", Duration::from_millis(1), true);
        meter.end_tool_call("myTool", Duration::from_millis(2), false);
        assert_eq!(
            tools(meter.metrics()),
            [("myTool", tool_metrics(2, 1, 1, Duration::from_millis(3)))]
        );
    }

    #[test]
    fn tracks_different_tools_independently() {
        let mut meter = Meter::default();
        meter.end_tool_call("toolA", Duration::from_millis(1), true);
        meter.end_tool_call("toolB", Duration::from_millis(2), false);
        assert_eq!(
            tools(meter.metrics()),
            [
                ("toolA", tool_metrics(1, 1, 0, Duration::from_millis(1))),
                ("toolB", tool_metrics(1, 0, 1, Duration::from_millis(2))),
            ]
        );
    }
}

mod latest_context_size {
    use super::*;

    #[test]
    fn is_undefined_when_no_invocations_have_occurred() {
        assert_eq!(Meter::default().metrics().latest_context_size(), None);
    }

    #[test]
    fn returns_the_most_recent_input_tokens_after_a_model_call() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&usage(100, 50, 150)));
        assert_eq!(meter.metrics().latest_context_size(), Some(100));
    }

    #[test]
    fn updates_across_multiple_cycles() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&usage(100, 50, 150)));
        meter.update_cycle(Some(&usage(200, 80, 280)));
        assert_eq!(meter.metrics().latest_context_size(), Some(200));
    }

    #[test]
    fn updates_across_multiple_invocations() {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        meter.start_cycle();
        meter.update_cycle(Some(&usage(100, 50, 150)));
        meter.start_new_invocation();
        meter.start_cycle();
        meter.update_cycle(Some(&usage(300, 100, 400)));
        assert_eq!(meter.metrics().latest_context_size(), Some(300));
    }

    #[test]
    fn remains_undefined_when_metadata_has_no_usage() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&latency(100)));
        assert_eq!(meter.metrics().latest_context_size(), None);
    }

    #[test]
    fn remains_undefined_when_update_cycle_is_called_with_undefined() {
        let mut meter = Meter::default();
        meter.update_cycle(None);
        assert_eq!(meter.metrics().latest_context_size(), None);
    }

    #[test]
    fn counts_cache_reads_on_disjoint_providers_where_they_add_to_input_tokens() {
        let mut meter = Meter::default();
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_usage(usage)));
        assert_eq!(meter.metrics().latest_context_size(), Some(5858));
    }

    #[test]
    fn does_not_double_count_cache_reads_on_subset_providers() {
        let mut meter = Meter::default();
        let usage = Usage::new(12936, 10, 12946).with_cache_read_input_tokens(6457);
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_usage(usage)));
        assert_eq!(meter.metrics().latest_context_size(), Some(12936));
    }
}

mod projected_context_size {
    use super::*;

    #[test]
    fn is_undefined_when_no_invocations_have_occurred() {
        assert_eq!(Meter::default().metrics().projected_context_size(), None);
    }

    #[test]
    fn returns_input_tokens_output_tokens_after_a_model_call() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&usage(100, 50, 150)));
        assert_eq!(meter.metrics().projected_context_size(), Some(150));
    }

    #[test]
    fn updates_across_multiple_cycles() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&usage(100, 50, 150)));
        meter.update_cycle(Some(&usage(200, 80, 280)));
        assert_eq!(meter.metrics().projected_context_size(), Some(280));
    }

    #[test]
    fn counts_cache_reads_on_disjoint_providers_where_they_add_to_input_tokens() {
        let mut meter = Meter::default();
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_usage(usage)));
        assert_eq!(meter.metrics().projected_context_size(), Some(5862));
    }

    #[test]
    fn does_not_double_count_cache_reads_on_subset_providers() {
        let mut meter = Meter::default();
        let usage = Usage::new(12936, 10, 12946).with_cache_read_input_tokens(6457);
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_usage(usage)));
        assert_eq!(meter.metrics().projected_context_size(), Some(12946));
    }
}

mod update_cycle {
    use super::*;

    #[test]
    fn accumulates_usage_and_latency_from_metadata() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&usage(5, 3, 8).with_metrics(Metrics::new(100))));
        meter.update_cycle(Some(&usage(10, 7, 17).with_metrics(Metrics::new(200))));
        assert_eq!(meter.metrics().accumulated_usage(), &Usage::new(15, 10, 25));
        assert_eq!(meter.metrics().accumulated_metrics(), &Metrics::new(300));
    }

    #[test]
    fn accumulates_cache_tokens_across_calls() {
        let mut meter = Meter::default();
        let first = Usage::new(10, 5, 15)
            .with_cache_read_input_tokens(3)
            .with_cache_write_input_tokens(2);
        let second = Usage::new(5, 2, 7).with_cache_read_input_tokens(4);
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_usage(first)));
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_usage(second)));
        assert_eq!(
            serde_json::to_value(meter.metrics().accumulated_usage()).unwrap(),
            json!({
                "inputTokens": 15,
                "outputTokens": 7,
                "totalTokens": 22,
                "cacheReadInputTokens": 7,
                "cacheWriteInputTokens": 2,
            })
        );
    }

    #[test]
    fn handles_usage_only_metadata() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&usage(10, 5, 15)));
        assert_eq!(meter.metrics().accumulated_usage(), &Usage::new(10, 5, 15));
        assert_eq!(meter.metrics().accumulated_metrics(), &Metrics::new(0));
    }

    #[test]
    fn handles_metrics_only_metadata() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&latency(250)));
        assert_eq!(meter.metrics().accumulated_usage(), &Usage::new(0, 0, 0));
        assert_eq!(meter.metrics().accumulated_metrics(), &Metrics::new(250));
    }

    #[test]
    fn propagates_usage_to_invocation_and_current_cycle() {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        meter.start_cycle();
        meter.update_cycle(Some(&usage(10, 5, 15)));
        assert_eq!(
            meter.metrics().latest_agent_invocation().unwrap(),
            &InvocationMetricsData {
                usage: Usage::new(10, 5, 15),
                cycles: vec![cycle("cycle-1", 0, Usage::new(10, 5, 15))],
            }
        );
    }

    #[test]
    fn is_a_no_op_when_metadata_is_undefined() {
        let mut meter = Meter::default();
        meter.update_cycle(None);
        assert_eq!(meter.metrics(), &AgentMetrics::default());
    }

    /// TS calls `updateCycle()` with no argument; Rust has the one `None` form.
    #[test]
    fn is_a_no_op_when_called_with_no_arguments() {
        let mut meter = Meter::default();
        meter.update_cycle(None);
        assert_eq!(meter.metrics().accumulated_usage(), &Usage::new(0, 0, 0));
        assert_eq!(meter.metrics().accumulated_metrics(), &Metrics::new(0));
    }

    #[test]
    fn is_a_no_op_when_metadata_has_neither_usage_nor_metrics() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&ModelMetadataEvent::new()));
        assert_eq!(meter.metrics(), &AgentMetrics::default());
    }

    #[test]
    fn does_not_fail_when_no_invocation_exists() {
        let mut meter = Meter::default();
        meter.update_cycle(Some(&usage(10, 5, 15)));
        assert_eq!(meter.metrics().accumulated_usage(), &Usage::new(10, 5, 15));
    }
}

mod agent_metrics {
    use super::*;

    fn realistic() -> AgentMetrics {
        AgentMetrics {
            cycle_count: 2,
            total_duration: Duration::from_millis(8000),
            tool_metrics: [(
                "search".to_owned(),
                tool_metrics(2, 1, 1, Duration::from_millis(2)),
            )]
            .into(),
            accumulated_usage: Usage::new(30, 15, 45),
            accumulated_metrics: Metrics::new(350),
            agent_invocations: vec![InvocationMetricsData {
                usage: Usage::new(30, 15, 45),
                cycles: vec![
                    cycle("cycle-1", 3000, Usage::new(10, 5, 15)),
                    cycle("cycle-2", 5000, Usage::new(20, 10, 30)),
                ],
            }],
            ..AgentMetrics::default()
        }
    }

    #[test]
    fn returns_complete_zeroed_data_for_default_instance() {
        assert_eq!(
            serde_json::to_value(AgentMetrics::default()).unwrap(),
            json!({
                "cycleCount": 0,
                "accumulatedUsage": { "inputTokens": 0, "outputTokens": 0, "totalTokens": 0 },
                "accumulatedMetrics": { "latencyMs": 0 },
                "agentInvocations": [],
                "toolMetrics": {},
                "totalDuration": 0,
            })
        );
    }

    #[test]
    fn returns_data_from_provided_metrics() {
        assert_eq!(
            serde_json::to_value(realistic()).unwrap(),
            json!({
                "cycleCount": 2,
                "accumulatedUsage": { "inputTokens": 30, "outputTokens": 15, "totalTokens": 45 },
                "accumulatedMetrics": { "latencyMs": 350 },
                "agentInvocations": [{
                    "usage": { "inputTokens": 30, "outputTokens": 15, "totalTokens": 45 },
                    "cycles": [
                        { "cycleId": "cycle-1", "duration": 3000, "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } },
                        { "cycleId": "cycle-2", "duration": 5000, "usage": { "inputTokens": 20, "outputTokens": 10, "totalTokens": 30 } },
                    ],
                }],
                "toolMetrics": {
                    "search": { "callCount": 2, "successCount": 1, "errorCount": 1, "totalTime": 2 },
                },
                "totalDuration": 8000,
            })
        );
    }

    #[test]
    fn includes_latest_context_size_when_set() {
        let metrics = AgentMetrics {
            latest_context_size: Some(42),
            ..AgentMetrics::default()
        };
        assert_eq!(
            serde_json::to_value(metrics).unwrap()["latestContextSize"],
            json!(42)
        );
    }

    #[test]
    fn omits_latest_context_size_when_undefined() {
        let json = serde_json::to_value(AgentMetrics::default()).unwrap();
        assert!(json.get("latestContextSize").is_none());
    }

    #[test]
    fn latest_agent_invocation_returns_the_last_invocation() {
        let invocation = |usage| InvocationMetricsData {
            cycles: vec![],
            usage,
        };
        let metrics = AgentMetrics {
            agent_invocations: vec![
                invocation(Usage::new(10, 5, 15)),
                invocation(Usage::new(20, 10, 30)),
            ],
            ..AgentMetrics::default()
        };
        assert!(std::ptr::eq(
            metrics.latest_agent_invocation().unwrap(),
            &metrics.agent_invocations()[1]
        ));
    }

    #[test]
    fn latest_agent_invocation_returns_undefined_when_empty() {
        assert_eq!(AgentMetrics::default().latest_agent_invocation(), None);
    }

    /// TS `accumulatedData` pairs the two; Rust has the two accessors.
    #[test]
    fn accumulated_data_returns_usage_and_metrics_together() {
        let metrics = realistic();
        assert_eq!(metrics.accumulated_usage(), &Usage::new(30, 15, 45));
        assert_eq!(metrics.accumulated_metrics(), &Metrics::new(350));
    }

    #[test]
    fn average_cycle_time_computes_average() {
        assert_eq!(realistic().average_cycle_time(), Duration::from_millis(4000));
    }

    #[test]
    fn average_cycle_time_returns_0_when_no_cycles() {
        assert_eq!(AgentMetrics::default().average_cycle_time(), Duration::ZERO);
    }

    #[test]
    fn tool_usage_adds_computed_average_time_and_success_rate() {
        let metrics = realistic();
        let search = metrics.tool_metric("search").unwrap();
        assert_eq!(search.average_time(), Duration::from_millis(1));
        assert!((search.success_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn tool_usage_returns_0_for_average_time_and_success_rate_when_call_count_is_0() {
        let broken = tool_metrics(0, 0, 0, Duration::ZERO);
        assert_eq!(broken.average_time(), Duration::ZERO);
        assert!(broken.success_rate().abs() < f64::EPSILON);
    }
}

#[cfg(feature = "otel")]
mod otel_instrument_emission {
    use std::sync::{Arc, Mutex, PoisonError};

    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
    use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
    use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
    use serde_json::{Map, Value as JsonValue};

    use super::*;

    /// Keeps the latest export as `{name: {meter, kind, description, unit, dataPoints}}`; a data
    /// point is `{value, attributes}` with the sum of the values recorded under the attributes.
    #[derive(Debug, Clone, Default)]
    struct Recorder(Arc<Mutex<Map<String, JsonValue>>>);

    fn attributes<'a>(attributes: impl Iterator<Item = &'a opentelemetry::KeyValue>) -> JsonValue {
        attributes
            .map(|pair| (pair.key.to_string(), json!(pair.value.as_str())))
            .collect::<Map<_, _>>()
            .into()
    }

    fn data_points<T: Copy + serde::Serialize>(data: &MetricData<T>) -> (&'static str, Vec<JsonValue>) {
        match data {
            MetricData::Sum(sum) => (
                "Counter",
                sum.data_points()
                    .map(|point| json!({ "value": point.value(), "attributes": attributes(point.attributes()) }))
                    .collect(),
            ),
            MetricData::Histogram(histogram) => (
                "Histogram",
                histogram
                    .data_points()
                    .map(|point| json!({ "value": point.sum(), "attributes": attributes(point.attributes()) }))
                    .collect(),
            ),
            _ => ("other", vec![]),
        }
    }

    impl PushMetricExporter for Recorder {
        async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
            let mut recorded = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            for scope in metrics.scope_metrics() {
                for metric in scope.metrics() {
                    let (kind, points) = match metric.data() {
                        AggregatedMetrics::F64(data) => data_points(data),
                        AggregatedMetrics::U64(data) => data_points(data),
                        AggregatedMetrics::I64(data) => data_points(data),
                    };
                    recorded.insert(
                        metric.name().to_owned(),
                        json!({
                            "meter": scope.scope().name(),
                            "kind": kind,
                            "description": metric.description(),
                            "unit": metric.unit(),
                            "dataPoints": points,
                        }),
                    );
                }
            }
            Ok(())
        }

        fn force_flush(&self) -> OTelSdkResult {
            Ok(())
        }

        fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
            Ok(())
        }

        fn temporality(&self) -> Temporality {
            Temporality::Cumulative
        }
    }

    struct Harness {
        recorder: Recorder,
        provider: SdkMeterProvider,
    }

    impl Harness {
        fn new() -> (Self, Meter) {
            let recorder = Recorder::default();
            let provider = SdkMeterProvider::builder()
                .with_reader(PeriodicReader::builder(recorder.clone()).build())
                .build();
            let meter = Meter::with_otel_meter(&provider.meter("strands-agents"));
            (Self { recorder, provider }, meter)
        }

        fn instrument(&self, name: &str) -> Option<JsonValue> {
            self.provider.force_flush().expect("flush");
            let recorded = self.recorder.0.lock().unwrap_or_else(PoisonError::into_inner);
            recorded.get(name).cloned()
        }

        /// The instrument's data points; none when nothing was recorded.
        fn data_points(&self, name: &str) -> JsonValue {
            self.instrument(name)
                .map_or_else(|| json!([]), |instrument| instrument["dataPoints"].clone())
        }

        fn sum(&self, name: &str) -> JsonValue {
            self.data_points(name)[0]["value"].clone()
        }
    }

    fn search() -> JsonValue {
        json!({ "gen_ai.tool.name": "search" })
    }

    #[test]
    fn emits_invocation_counter_on_start_new_invocation() {
        let (otel, mut meter) = Harness::new();
        meter.start_new_invocation();
        assert_eq!(otel.sum("gen_ai.agent.invocation.count"), json!(1));
    }

    #[test]
    fn emits_cycle_counter_on_start_cycle() {
        let (otel, mut meter) = Harness::new();
        meter.start_cycle();
        assert_eq!(otel.sum("gen_ai.agent.cycle.count"), json!(1));
    }

    #[tokio::test(start_paused = true)]
    async fn emits_cycle_duration_histogram_on_end_cycle() {
        let (otel, mut meter) = Harness::new();
        let start = Instant::now();
        tokio::time::advance(Duration::from_millis(2000)).await;
        meter.end_cycle(start);
        assert_eq!(otel.sum("gen_ai.agent.cycle.duration"), json!(2000));
    }

    #[test]
    fn emits_tool_call_counter_and_duration_on_successful_end_tool_call() {
        let (otel, mut meter) = Harness::new();
        meter.end_tool_call("search", Duration::from_millis(150), true);
        assert_eq!(
            otel.data_points("gen_ai.agent.tool.call.count"),
            json!([{ "value": 1, "attributes": search() }])
        );
        assert_eq!(
            otel.data_points("gen_ai.agent.tool.duration"),
            json!([{ "value": 150, "attributes": search() }])
        );
        assert_eq!(otel.data_points("gen_ai.agent.tool.error.count"), json!([]));
    }

    #[test]
    fn emits_tool_call_counter_error_counter_and_duration_on_failed_end_tool_call() {
        let (otel, mut meter) = Harness::new();
        meter.end_tool_call("search", Duration::from_millis(50), false);
        assert_eq!(
            otel.data_points("gen_ai.agent.tool.call.count"),
            json!([{ "value": 1, "attributes": search() }])
        );
        assert_eq!(
            otel.data_points("gen_ai.agent.tool.error.count"),
            json!([{ "value": 1, "attributes": search() }])
        );
        assert_eq!(
            otel.data_points("gen_ai.agent.tool.duration"),
            json!([{ "value": 50, "attributes": search() }])
        );
    }

    #[test]
    fn emits_input_token_counter_output_token_counter_and_model_latency_on_update_cycle() {
        let (otel, mut meter) = Harness::new();
        meter.update_cycle(Some(&usage(100, 50, 150).with_metrics(Metrics::new(200))));
        assert_eq!(otel.sum("gen_ai.agent.tokens.input"), json!(100));
        assert_eq!(otel.sum("gen_ai.agent.tokens.output"), json!(50));
        assert_eq!(otel.sum("gen_ai.agent.model.latency"), json!(200));
    }

    fn assert_no_token_or_latency_points(otel: &Harness) {
        assert_eq!(otel.data_points("gen_ai.agent.tokens.input"), json!([]));
        assert_eq!(otel.data_points("gen_ai.agent.tokens.output"), json!([]));
        assert_eq!(otel.data_points("gen_ai.agent.model.latency"), json!([]));
    }

    #[test]
    fn does_not_emit_token_counters_or_latency_when_update_cycle_has_no_usage_or_metrics() {
        let (otel, mut meter) = Harness::new();
        meter.update_cycle(Some(&ModelMetadataEvent::new()));
        assert_no_token_or_latency_points(&otel);
    }

    #[test]
    fn does_not_emit_any_otel_instruments_when_update_cycle_is_called_with_undefined() {
        let (otel, mut meter) = Harness::new();
        meter.update_cycle(None);
        assert_no_token_or_latency_points(&otel);
    }

    #[test]
    fn emits_time_to_first_token_histogram_in_seconds_when_time_to_first_byte_ms_is_provided() {
        let (otel, mut meter) = Harness::new();
        let metrics = Metrics::new(500).with_time_to_first_byte_ms(150);
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_metrics(metrics)));
        let seconds = otel.sum("gen_ai.server.time_to_first_token").as_f64().unwrap();
        assert!((seconds - 0.15).abs() < 1e-9, "{seconds}");
    }

    #[test]
    fn does_not_emit_time_to_first_token_histogram_when_time_to_first_byte_ms_is_undefined() {
        let (otel, mut meter) = Harness::new();
        meter.update_cycle(Some(&latency(500)));
        assert_eq!(otel.data_points("gen_ai.server.time_to_first_token"), json!([]));
    }

    #[test]
    fn does_not_emit_time_to_first_token_histogram_when_time_to_first_byte_ms_is_zero() {
        let (otel, mut meter) = Harness::new();
        let metrics = Metrics::new(500).with_time_to_first_byte_ms(0);
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_metrics(metrics)));
        assert_eq!(otel.data_points("gen_ai.server.time_to_first_token"), json!([]));
    }

    /// Names, kinds, descriptions, units and the meter name against the TS-produced fixture.
    #[test]
    fn instruments_match_the_conformance_fixture() {
        let (otel, mut meter) = Harness::new();
        meter.start_new_invocation();
        let (_, start) = meter.start_cycle();
        let metrics = Metrics::new(500).with_time_to_first_byte_ms(150);
        meter.update_cycle(Some(&usage(1, 1, 2).with_metrics(metrics)));
        meter.end_tool_call("search", Duration::from_millis(1), false);
        meter.end_cycle(start);

        let path = format!(
            "{}/../conformance/fixtures/telemetry/metric_instruments.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(path).expect("the fixture file");
        let fixture: Vec<JsonValue> = serde_json::from_str(&text).expect("fixture JSON");
        assert_eq!(fixture.len(), 10);
        for expected in fixture {
            let name = expected["name"].as_str().unwrap();
            let actual = otel
                .instrument(name)
                .unwrap_or_else(|| panic!("{name} was not exported"));
            assert_eq!(actual["meter"], expected["meter"], "{name}");
            assert_eq!(actual["kind"], expected["kind"], "{name}");
            assert_eq!(
                actual["description"], expected["options"]["description"],
                "{name}"
            );
            let unit = expected["options"]
                .get("unit")
                .cloned()
                .unwrap_or_else(|| json!(""));
            assert_eq!(actual["unit"], unit, "{name}");
        }
    }
}
