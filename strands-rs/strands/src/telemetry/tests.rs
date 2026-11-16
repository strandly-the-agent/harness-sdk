//! Spans of a mock-model agent run, recorded by a `tracing-opentelemetry` layer over an
//! `opentelemetry_sdk` provider. Ports the span and attribute cases of
//! `telemetry/__tests__/tracer.test.node.ts` and checks the names, attributes and events against
//! the conformance fixtures `telemetry/spans_stable.json` and `telemetry/spans_latest_experimental.json`.
//!
//! A scoped `set_default` subscriber is thread-local, so every test runs on a `current_thread`
//! runtime.
#![cfg(feature = "otel")]

use std::sync::{Arc, Mutex, PoisonError};

use opentelemetry::trace::{SpanId, Status, TracerProvider as _};
use opentelemetry::{Array, Value};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use serde_json::{Value as JsonValue, json};
use tracing_subscriber::layer::SubscriberExt;

use super::config::Semconv;
use crate::models::streaming::Usage;
use crate::test_support::mock_tool;
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::types::messages::{
    ContentBlock, JsonBlock, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};
use crate::types::tools::ToolResultStatus;
use crate::{Agent, AgentConfig};

#[derive(Debug, Clone, Default)]
struct Recorder(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Recorder {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(batch);
        Ok(())
    }
}

impl Recorder {
    fn spans(&self) -> Vec<SpanData> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A thread-local subscriber exporting every span to the returned recorder.
fn recording() -> (Recorder, tracing::subscriber::DefaultGuard) {
    let recorder = Recorder::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(recorder.clone())
        .build();
    let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("strands-agents"));
    let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    (recorder, guard)
}

fn attribute_json(value: &Value) -> JsonValue {
    match value {
        Value::Bool(value) => json!(value),
        Value::I64(value) => json!(value),
        Value::F64(value) => json!(value),
        Value::String(value) => json!(value.as_str()),
        Value::Array(Array::String(values)) => values.iter().map(|value| json!(value.as_str())).collect(),
        other => json!(other.to_string()),
    }
}

/// Attributes the layer adds on its own (code location, thread, timings), and the wall-clock ones.
fn skipped(key: &str) -> bool {
    key.starts_with("code.")
        || key.starts_with("thread.")
        || matches!(
            key,
            "busy_ns" | "idle_ns" | "target" | "level" | "gen_ai.event.start_time" | "gen_ai.event.end_time"
        )
}

fn attributes_json(attributes: &[opentelemetry::KeyValue]) -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = attributes
        .iter()
        .filter(|attribute| !skipped(attribute.key.as_str()))
        .map(|attribute| {
            (
                attribute.key.as_str().to_owned(),
                attribute_json(&attribute.value),
            )
        })
        .collect();
    JsonValue::Object(map)
}

/// The spans in the fixture's shape: `{name, parent, attributes, events}` in end order.
fn recorded(spans: &[SpanData]) -> Vec<JsonValue> {
    let name_of = |id: SpanId| {
        spans
            .iter()
            .find(|span| span.span_context.span_id() == id)
            .map(|span| span.name.to_string())
    };
    spans
        .iter()
        .map(|span| {
            let events: Vec<JsonValue> = span
                .events
                .iter()
                .map(|event| json!({"name": event.name, "attributes": attributes_json(&event.attributes)}))
                .collect();
            json!({
                "name": span.name,
                "parent": name_of(span.parent_span_id),
                "attributes": attributes_json(&span.attributes),
                "events": events,
            })
        })
        .collect()
}

/// The fixture's spans without `kind`, `status` and the attributes [`skipped`] drops.
fn fixture(name: &str) -> Vec<JsonValue> {
    let path = format!(
        "{}/../conformance/fixtures/telemetry/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(path).expect("the fixture file");
    let spans: Vec<JsonValue> = serde_json::from_str(&text).expect("fixture JSON");
    spans
        .into_iter()
        .map(|mut span| {
            let object = span.as_object_mut().expect("a span object");
            object.remove("kind");
            object.remove("status");
            // A Rust error has no JS class name or stack.
            for event in object["events"].as_array_mut().expect("events") {
                if let Some(attributes) = event["attributes"].as_object_mut() {
                    attributes.remove("exception.type");
                    attributes.remove("exception.stacktrace");
                }
            }
            if let Some(attributes) = object.get_mut("attributes").and_then(JsonValue::as_object_mut) {
                attributes.retain(|key, _| !skipped(key));
            }
            span
        })
        .collect()
}

fn tool_use(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

fn weather_tool() -> Arc<dyn crate::tools::Tool> {
    Arc::new(mock_tool("get_weather", |context| {
        let city = context.tool_use().input()["city"].clone();
        let content = JsonBlock::new(json!({"city": city, "forecast": "sunny", "high": 21}));
        ToolResultBlock::new(
            context.tool_use().tool_use_id(),
            ToolResultStatus::Success,
            vec![ToolResultContent::Json(content)],
        )
    }))
}

fn usage_turn(input_tokens: u64, output_tokens: u64) -> MockTurnOptions {
    let usage = Usage::new(
        input_tokens,
        output_tokens,
        input_tokens.saturating_add(output_tokens),
    );
    MockTurnOptions::builder().usage(usage).build()
}

fn agent_of(model: MockMessageModel, tools: Vec<Arc<dyn crate::tools::Tool>>, semconv: Semconv) -> Agent {
    let agent = AgentConfig::builder()
        .model(Arc::new(model) as Arc<dyn crate::models::model::Model>)
        .tools(tools)
        .name("telemetry_agent")
        .id("telemetry_agent")
        .system_prompt("You report the weather.")
        .build()
        .expect("a valid agent");
    agent.local.telemetry().lock().tracer.set_semconv(semconv);
    agent
}

/// The first agent of the conformance generator's `telemetry()` run: two tools, one failing, and
/// trace attributes of every scalar type.
fn telemetry_agent(semconv: Semconv) -> Agent {
    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![
            tool_use("get_weather", "tooluse_tel_1", json!({"city": "Lisbon"})),
            tool_use("flaky_tool", "tooluse_tel_2", json!({})),
        ],
        usage_turn(30, 14),
    );
    model.add_turn_with(vec![TextBlock::new("Lisbon is sunny.").into()], usage_turn(12, 5));
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
    let agent = agent_of(model, vec![weather_tool(), Arc::new(flaky)], semconv);
    agent.local.telemetry().lock().tracer.set_trace_attributes(vec![
        ("conformance.case".to_owned(), "telemetry".into()),
        ("conformance.number".to_owned(), 7.into()),
        ("conformance.flag".to_owned(), true.into()),
    ]);
    agent
}

/// The generator's second agent: one tool, no trace attributes.
fn one_tool_agent(semconv: Semconv) -> Agent {
    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![tool_use("get_weather", "tooluse_tel_3", json!({"city": "Porto"}))],
        usage_turn(30, 14),
    );
    model.add_turn_with(vec![TextBlock::new("Porto is sunny.").into()], usage_turn(12, 5));
    agent_of(model, vec![weather_tool()], semconv)
}

fn semconv(opt_in: &'static str) -> Semconv {
    Semconv::from_env(&move |name| (name == "OTEL_SEMCONV_STABILITY_OPT_IN").then(|| opt_in.to_owned()))
}

async fn run(agent: Agent) -> Vec<SpanData> {
    run_with(agent, "Weather in Lisbon?").await
}

async fn run_with(mut agent: Agent, prompt: &str) -> Vec<SpanData> {
    let (recorder, _guard) = recording();
    agent.invoke(prompt).await.expect("the run succeeds");
    recorder.spans()
}

fn by_name<'a>(spans: &'a [JsonValue], name: &str) -> Vec<&'a JsonValue> {
    spans.iter().filter(|span| span["name"] == name).collect()
}

/// Compares every span with the fixture's span of the same name and position. The scripted model
/// of the fixture run has another model id and reports a latency, which the mock model doesn't.
fn assert_matches_fixture(spans: &[SpanData], fixture_name: &str) {
    let actual = recorded(spans);
    let expected = fixture(fixture_name);
    let names = |spans: &[JsonValue]| -> Vec<String> {
        let mut names: Vec<String> = spans.iter().map(|span| span["name"].to_string()).collect();
        names.sort();
        names
    };
    assert_eq!(names(&actual), names(&expected));
    for name in names(&expected) {
        let name = name.trim_matches('"');
        for (actual, expected) in by_name(&actual, name).into_iter().zip(by_name(&expected, name)) {
            let mut expected = expected.clone();
            let attributes = expected["attributes"].as_object_mut().expect("attributes");
            attributes.remove("gen_ai.server.request.duration");
            if attributes.contains_key("gen_ai.request.model") {
                attributes.insert("gen_ai.request.model".to_owned(), json!("test-model"));
            }
            assert_eq!(actual, &expected, "span {name}");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn spans_match_the_stable_conformance_fixture() {
    let spans = run(telemetry_agent(semconv(""))).await;
    assert_matches_fixture(&spans, "spans_stable");
}

#[tokio::test(flavor = "current_thread")]
async fn spans_match_the_latest_experimental_conformance_fixture() {
    let agent = one_tool_agent(semconv("gen_ai_latest_experimental"));
    let spans = run_with(agent, "Weather in Porto?").await;
    assert_matches_fixture(&spans, "spans_latest_experimental");
}

#[tokio::test(flavor = "current_thread")]
async fn span_names_match_the_conformance_fixture() {
    let spans = run(telemetry_agent(semconv(""))).await;
    let mut names: Vec<String> = spans.iter().map(|span| span.name.to_string()).collect();
    let path = format!(
        "{}/../conformance/fixtures/telemetry/span_names.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut expected: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the fixture file"))
            .expect("fixture JSON");
    names.sort();
    expected.sort();
    assert_eq!(names, expected);
}

/// `execute_tool` spans come from spawned tool tasks and still parent under the cycle span, which
/// parents under the agent span.
#[tokio::test(flavor = "current_thread")]
async fn execute_tool_spans_from_spawned_tools_parent_under_the_cycle_span() {
    let spans = run(telemetry_agent(semconv(""))).await;
    let agent_span = spans
        .iter()
        .find(|span| span.name == "invoke_agent telemetry_agent")
        .expect("the agent span");
    assert_eq!(agent_span.parent_span_id, SpanId::INVALID);
    let cycles: Vec<&SpanData> = spans
        .iter()
        .filter(|span| span.name == "execute_agent_loop_cycle")
        .collect();
    assert_eq!(cycles.len(), 2);
    for cycle in &cycles {
        assert_eq!(cycle.parent_span_id, agent_span.span_context.span_id());
        assert_eq!(cycle.span_context.trace_id(), agent_span.span_context.trace_id());
    }
    let first_cycle = cycles
        .iter()
        .min_by_key(|span| span.start_time)
        .expect("a cycle span")
        .span_context
        .span_id();
    let tools: Vec<&SpanData> = spans
        .iter()
        .filter(|span| span.name.starts_with("execute_tool "))
        .collect();
    assert_eq!(tools.len(), 2);
    for tool in tools {
        assert_eq!(tool.parent_span_id, first_cycle, "{}", tool.name);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn sets_ok_status_on_success_and_error_status_with_the_message_on_failure() {
    let spans = run_with(one_tool_agent(semconv("")), "Weather in Porto?").await;
    assert!(spans.iter().all(|span| span.status == Status::Ok), "{spans:#?}");

    let model = MockMessageModel::new();
    model.add_error(|| crate::Error::other("model exploded"));
    let mut agent = crate::testing::test_agent(Arc::new(model));
    let (recorder, _guard) = recording();
    agent.invoke("hi").await.expect_err("the run fails");
    let spans = recorder.spans();
    let chat = spans
        .iter()
        .find(|span| span.name == "chat")
        .expect("the chat span");
    assert_eq!(chat.status, Status::error("model exploded"));
    let exception = chat
        .events
        .iter()
        .find(|event| event.name == "exception")
        .expect("the exception event");
    assert_eq!(
        attributes_json(&exception.attributes),
        json!({"exception.message": "model exploded"})
    );
    let agent_span = spans
        .iter()
        .find(|span| span.name.starts_with("invoke_agent "))
        .expect("the agent span");
    assert_eq!(agent_span.status, Status::error("model exploded"));
}

#[tokio::test(flavor = "current_thread")]
async fn records_latest_convention_content_as_span_attributes_for_langfuse_without_opt_in() {
    let langfuse = Semconv::from_env(&|name| {
        (name == "OTEL_SEMCONV_STABILITY_OPT_IN")
            .then(|| "gen_ai_latest_experimental".to_owned())
            .or_else(|| (name == "LANGFUSE_BASE_URL").then(|| "https://langfuse.example".to_owned()))
    });
    let spans = run(telemetry_agent(langfuse)).await;
    let agent_span = spans
        .iter()
        .find(|span| span.name == "invoke_agent telemetry_agent")
        .expect("the agent span");
    let attributes = attributes_json(&agent_span.attributes);
    assert_eq!(attributes["langfuse.observation.type"], "span");
    assert_eq!(
        attributes["gen_ai.input.messages"],
        r#"[{"role":"user","parts":[{"type":"text","content":"Weather in Lisbon?"}]}]"#
    );
    assert!(attributes["gen_ai.output.messages"].is_string());
    assert!(agent_span.events.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn dual_emits_cache_usage_under_both_the_semconv_names_and_the_deprecated_aliases() {
    for (opt_in, legacy) in [("", true), ("gen_ai_latest_experimental", false)] {
        let model = MockMessageModel::new();
        let usage = Usage::new(10, 5, 20)
            .with_cache_read_input_tokens(3)
            .with_cache_write_input_tokens(2);
        model.add_turn_with(
            vec![TextBlock::new("done").into()],
            MockTurnOptions::builder().usage(usage).build(),
        );
        let agent = crate::testing::test_agent(Arc::new(model));
        agent.local.telemetry().lock().tracer.set_semconv(semconv(opt_in));
        let spans = run(agent).await;
        let chat = spans
            .iter()
            .find(|span| span.name == "chat")
            .expect("the chat span");
        let attributes = attributes_json(&chat.attributes);
        assert_eq!(attributes["gen_ai.usage.cache_read.input_tokens"], 3);
        assert_eq!(attributes["gen_ai.usage.cache_creation.input_tokens"], 2);
        assert_eq!(attributes["gen_ai.usage.input_tokens"], 15);
        assert_eq!(attributes["gen_ai.usage.prompt_tokens"], 15);
        assert_eq!(attributes["gen_ai.usage.output_tokens"], 5);
        assert_eq!(attributes["gen_ai.usage.completion_tokens"], 5);
        assert_eq!(attributes["gen_ai.usage.total_tokens"], 20);
        let legacy_keys = [
            "gen_ai.usage.cache_read_input_tokens",
            "gen_ai.usage.cache_write_input_tokens",
        ];
        for key in legacy_keys {
            assert_eq!(attributes.get(key).is_some(), legacy, "{key} with {opt_in:?}");
        }
    }
}

/// TS `agent.ts` `_withBaggage`: the agent's `baggage_attributes` join the active baggage for the
/// whole invocation, so a baggage span processor stamps them on every span, spawned tools included.
#[tokio::test(flavor = "current_thread")]
async fn baggage_attributes_are_merged_into_the_active_baggage_for_every_span() {
    use opentelemetry::baggage::BaggageExt as _;

    use super::{BaggageKeys, BaggageSpanProcessor};

    let recorder = Recorder::default();
    let provider = SdkTracerProvider::builder()
        .with_span_processor(BaggageSpanProcessor::new(BaggageKeys::All))
        .with_simple_exporter(recorder.clone())
        .build();
    let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("strands-agents"));
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    let _ambient =
        opentelemetry::Context::current_with_baggage([opentelemetry::KeyValue::new("ambient", "kept")])
            .attach();

    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![tool_use("get_weather", "tooluse_tel_3", json!({"city": "Porto"}))],
        usage_turn(30, 14),
    );
    model.add_turn_with(vec![TextBlock::new("Porto is sunny.").into()], usage_turn(12, 5));
    let mut agent = AgentConfig::builder()
        .model(Arc::new(model) as Arc<dyn crate::models::model::Model>)
        .tools(vec![weather_tool()])
        .baggage_attributes([("tenant.id".to_owned(), "acme".to_owned())].into())
        .build()
        .expect("a valid agent");
    agent.invoke("Weather in Porto?").await.expect("the run succeeds");

    let spans = recorder.spans();
    let names: Vec<&str> = spans.iter().map(|span| span.name.as_ref()).collect();
    assert_eq!(names.len(), 6, "{names:?}");
    for span in &spans {
        let attributes = attributes_json(&span.attributes);
        assert_eq!(attributes["tenant.id"], "acme", "{}", span.name);
        assert_eq!(attributes["ambient"], "kept", "{}", span.name);
    }
}
