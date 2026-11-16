//! The conformance generator's `telemetry()` run, replayed by the Rust agent and compared with the
//! TS-produced fixtures `telemetry/agent_metrics.json`, `telemetry/agent_traces.json` and
//! `telemetry/metric_points.json`. The clock is tokio's paused clock, so the cycle durations are
//! the scripted model delays.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::StreamExt as _;
use futures::stream::BoxStream;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use serde_json::{Map, Value as JsonValue, json};

use super::{agent_of, semconv, tool_use, usage_turn, weather_tool};
use crate::models::model::{BaseModelConfig, Model, ModelRequest};
use crate::models::streaming::{Metrics, ModelStreamEvent};
use crate::telemetry::meter::Meter;
use crate::test_support::mock_tool;
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::types::agent::AgentResult;
use crate::types::messages::{TextBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;

/// A scripted model whose metadata events carry the latencies the generator's model reports.
struct TimedModel {
    script: MockMessageModel,
    metrics: Mutex<VecDeque<Metrics>>,
}

impl Model for TimedModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        let metrics = self
            .metrics
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        self.script
            .stream(request)
            .map(move |event| match (event, metrics.clone()) {
                (Ok(ModelStreamEvent::Metadata(metadata)), Some(metrics)) => {
                    Ok(ModelStreamEvent::Metadata(metadata.with_metrics(metrics)))
                }
                (event, _) => event,
            })
            .boxed()
    }

    fn get_config(&self) -> BaseModelConfig {
        self.script.get_config()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.script.update_config(config);
    }
}

/// Keeps the latest export in the fixture's shape: `{scope, name, unit, description, dataPoints}`.
#[derive(Debug, Clone, Default)]
struct PointRecorder(Arc<Mutex<Vec<JsonValue>>>);

fn point_attributes<'a>(attributes: impl Iterator<Item = &'a opentelemetry::KeyValue>) -> JsonValue {
    attributes
        .map(|pair| (pair.key.to_string(), json!(pair.value.as_str())))
        .collect::<Map<_, _>>()
        .into()
}

fn data_points<T: Copy + serde::Serialize>(data: &MetricData<T>) -> Vec<JsonValue> {
    match data {
        MetricData::Sum(sum) => sum
            .data_points()
            .map(|point| json!({"attributes": point_attributes(point.attributes()), "value": point.value()}))
            .collect(),
        MetricData::Histogram(histogram) => histogram
            .data_points()
            .map(|point| {
                json!({
                    "attributes": point_attributes(point.attributes()),
                    "value": {
                        "min": point.min(),
                        "max": point.max(),
                        "sum": point.sum(),
                        "buckets": {
                            "boundaries": point.bounds().collect::<Vec<_>>(),
                            "counts": point.bucket_counts().collect::<Vec<_>>(),
                        },
                        "count": point.count(),
                    },
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

impl PushMetricExporter for PointRecorder {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let mut recorded = Vec::new();
        for scope in metrics.scope_metrics() {
            for metric in scope.metrics() {
                let points = match metric.data() {
                    AggregatedMetrics::F64(data) => data_points(data),
                    AggregatedMetrics::U64(data) => data_points(data),
                    AggregatedMetrics::I64(data) => data_points(data),
                };
                recorded.push(json!({
                    "scope": scope.scope().name(),
                    "name": metric.name(),
                    "unit": metric.unit(),
                    "description": metric.description(),
                    "dataPoints": points,
                }));
            }
        }
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = recorded;
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

struct Run {
    result: AgentResult,
    /// The metric points the run recorded, in the fixture's shape.
    points: Vec<JsonValue>,
}

/// The generator's first telemetry agent: two tools, one failing, cycles of 63 ms and 42 ms and
/// model latencies of 40 ms (9 ms to the first byte) and 21 ms.
async fn telemetry_run() -> Run {
    let script = MockMessageModel::new();
    let turn = |input_tokens, output_tokens, delay| MockTurnOptions {
        delay: Some(Duration::from_millis(delay)),
        ..usage_turn(input_tokens, output_tokens)
    };
    script.add_turn_with(
        vec![
            tool_use("get_weather", "tooluse_tel_1", json!({"city": "Lisbon"})),
            tool_use("flaky_tool", "tooluse_tel_2", json!({})),
        ],
        turn(30, 14, 63),
    );
    script.add_turn_with(vec![TextBlock::new("Lisbon is sunny.").into()], turn(12, 5, 42));
    let model = TimedModel {
        script,
        metrics: Mutex::new(VecDeque::from([
            Metrics::new(40).with_time_to_first_byte_ms(9),
            Metrics::new(21),
        ])),
    };
    let flaky = mock_tool("flaky_tool", |context| {
        ToolResultBlock::new(
            context.tool_use().tool_use_id(),
            ToolResultStatus::Error,
            vec![ToolResultContent::Text(TextBlock::new(
                "Error: flaky_tool exploded",
            ))],
        )
        .with_error("flaky_tool exploded")
    });
    let mut agent = agent_of(
        MockMessageModel::new(),
        vec![weather_tool(), Arc::new(flaky)],
        semconv(""),
    );
    agent.set_model(Arc::new(model) as Arc<dyn Model>);

    let recorder = PointRecorder::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(recorder.clone()).build())
        .build();
    agent.local.telemetry().lock().meter = Meter::with_otel_meter(&provider.meter("strands-agents"));

    let result = agent
        .invoke("Weather in Lisbon?")
        .await
        .expect("the run succeeds");
    provider.force_flush().expect("flush");
    let points = recorder.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
    Run { result, points }
}

fn fixture(name: &str) -> JsonValue {
    let path = format!(
        "{}/../conformance/fixtures/telemetry/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(path).expect("the fixture file");
    serde_json::from_str(&text).expect("fixture JSON")
}

/// `value` with every number as a float (`42` and `42.0` are one value in JS) and the per-run
/// values replaced: ids by their order of first appearance, so parent links still compare, and
/// wall-clock times and tracking ids by their JSON type.
fn normalized(value: &JsonValue, key: &str, ids: &mut HashMap<String, usize>) -> JsonValue {
    match value {
        JsonValue::Object(map) => map
            .iter()
            .map(|(key, value)| (key.clone(), normalized(value, key, ids)))
            .collect::<Map<_, _>>()
            .into(),
        JsonValue::Array(items) => items.iter().map(|item| normalized(item, key, ids)).collect(),
        JsonValue::String(id) if matches!(key, "id" | "parentId") => {
            let next = ids.len();
            json!(format!("id-{}", ids.entry(id.clone()).or_insert(next)))
        }
        JsonValue::String(_) if key == "trackingId" => json!("string"),
        JsonValue::Number(_) if matches!(key, "startTime" | "endTime") => json!("number"),
        JsonValue::Number(number) => json!(number.as_f64()),
        other => other.clone(),
    }
}

fn normal(value: &JsonValue) -> JsonValue {
    normalized(value, "", &mut HashMap::new())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn agent_metrics_match_the_conformance_fixture() {
    let run = telemetry_run().await;
    let metrics = serde_json::to_value(run.result.metrics()).expect("metrics JSON");
    assert_eq!(normal(&metrics), normal(&fixture("agent_metrics")));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn agent_traces_match_the_conformance_fixture() {
    let run = telemetry_run().await;
    let traces = serde_json::to_value(run.result.traces()).expect("traces JSON");
    assert_eq!(normal(&traces), normal(&fixture("agent_traces")));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn metric_points_match_the_conformance_fixture() {
    /// Instruments by name, each with its data points ordered by their attributes.
    fn by_name(metrics: &JsonValue) -> Vec<JsonValue> {
        let mut metrics = normal(metrics).as_array().expect("a metric list").clone();
        metrics.sort_by_key(|metric| metric["name"].as_str().map(str::to_owned));
        for metric in &mut metrics {
            let points = metric["dataPoints"].as_array_mut().expect("data points");
            points.sort_by_key(|point| point["attributes"].to_string());
        }
        metrics
    }

    let run = telemetry_run().await;
    assert_eq!(
        by_name(&JsonValue::Array(run.points)),
        by_name(&fixture("metric_points"))
    );
}
