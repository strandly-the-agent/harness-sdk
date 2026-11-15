//! Drives the TS conformance agent runs through scripted models and checks that the Rust stream
//! records, model requests and hook order equal the TS fixtures (`conformance/fixtures/agent/` and
//! `conformance/fixtures/hooks/*.order.json`, written by `conformance/src/events.ts`).
//!
//! Each fixture's model calls are replayed from its own `modelStreamUpdateEvent` records. Tracking
//! ids are minted per message, so both sides replace them with a placeholder before comparing.

#![expect(clippy::unwrap_used, clippy::panic, reason = "test code")]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use serde_json::{Value, json};
use strands::error::DataError;
use strands::models::model::ModelRequest;
use strands::models::streaming::ModelStreamEvent;
use strands::testing::{HookRecorder, TestModelProvider, test_agent};
use strands::tools::executors::ToolExecutorStrategy;
use strands::tools::structured_output::StructuredOutputSchema;
use strands::tools::{FunctionTool, Tool, ToolStream, ToolStreamEvent, ToolStreamItem};
use strands::types::messages::ToolResultBlock;
use strands::types::tools::{ToolResultStatus, ToolSpec};
use strands::{Agent, ToolContext, ToolError, ToolOutcome};

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../conformance/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The model calls of a stream fixture: the stream events of each call, or the error it failed
/// with.
fn model_calls(records: &[Value]) -> Vec<Result<Vec<Value>, String>> {
    let mut calls = Vec::new();
    let mut current = None;
    for record in records {
        match record["type"].as_str().unwrap() {
            "beforeModelCallEvent" => current = Some(Vec::new()),
            "modelStreamUpdateEvent" => current.as_mut().unwrap().push(record["event"].clone()),
            "afterModelCallEvent" => {
                let events = current.take().unwrap();
                calls.push(match record["error"]["message"].as_str() {
                    Some(message) => Err(message.to_owned()),
                    None => Ok(events),
                });
            }
            _ => {}
        }
    }
    calls
}

fn scripted_model(records: &[Value]) -> Arc<TestModelProvider> {
    let calls = model_calls(records);
    let next = AtomicUsize::new(0);
    Arc::new(TestModelProvider::new(move |_: &ModelRequest| {
        match calls.get(next.fetch_add(1, Ordering::SeqCst)) {
            Some(Ok(events)) => events
                .iter()
                .map(|event| Ok(serde_json::from_value::<ModelStreamEvent>(event.clone()).unwrap()))
                .collect(),
            Some(Err(message)) => vec![Err(DataError::new(message.clone()).into())],
            None => vec![Err(DataError::new("ScriptedModel ran out of turns").into())],
        }
    }))
}

fn without_tracking_ids(mut value: Value) -> Value {
    match &mut value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                *item = if key == "trackingId" {
                    json!("<tracking-id>")
                } else {
                    without_tracking_ids(item.take())
                };
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                *item = without_tracking_ids(item.take());
            }
        }
        _ => {}
    }
    value
}

/// Streams `prompt` and returns the records as JSON plus the error the stream ended with.
async fn run(agent: &mut Agent, prompt: &str) -> (Vec<Value>, Option<String>) {
    let mut records = Vec::new();
    let mut error = None;
    let mut stream = agent.stream(prompt);
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => records.push(without_tracking_ids(serde_json::to_value(event).unwrap())),
            Err(failure) => error = Some(failure.to_string()),
        }
    }
    (records, error)
}

fn expected_records(name: &str) -> Vec<Value> {
    let Value::Array(records) = without_tracking_ids(fixture(name)) else {
        panic!("{name} is not an array");
    };
    records
}

fn assert_records(actual: &[Value], expected: &[Value]) {
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(actual, expected, "record {index} differs");
    }
    assert_eq!(actual.len(), expected.len());
}

/// What `ScriptedModel.calls` records for a request.
fn model_call(request: &ModelRequest) -> Value {
    let mut call = serde_json::Map::new();
    if let Some(choice) = &request.tool_choice {
        call.insert("toolChoice".into(), serde_json::to_value(choice).unwrap());
    }
    call.insert(
        "toolSpecs".into(),
        serde_json::to_value(&request.tool_specs).unwrap(),
    );
    if let Some(prompt) = &request.system_prompt {
        call.insert("systemPrompt".into(), serde_json::to_value(prompt).unwrap());
    }
    call.insert("messageCount".into(), json!(request.messages.len()));
    Value::Object(call)
}

struct ProgressTool {
    spec: ToolSpec,
}

impl Tool for ProgressTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn tool_spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let tool_use = context.tool_use().clone();
        let steps = tool_use.input["steps"].as_i64().unwrap_or(0);
        ToolStream::new(futures::stream::iter(
            (1..=steps)
                .map(|step| ToolStreamItem::Event(ToolStreamEvent::new().with_data(json!({ "step": step }))))
                .chain([ToolStreamItem::Outcome(ToolOutcome::Result(
                    ToolResultBlock::new(
                        tool_use.tool_use_id,
                        ToolResultStatus::Success,
                        vec![format!("done after {steps} steps").into()],
                    ),
                ))]),
        ))
    }
}

#[tokio::test]
async fn text_turn_matches_the_fixture() {
    let expected = expected_records("agent/text_turn.stream.json");
    let model = scripted_model(&expected);
    let mut agent = test_agent(Arc::clone(&model))
        .system_prompt("You are a conformance fixture.")
        .retry_strategy(Vec::new())
        .build()
        .unwrap();
    let recorder = HookRecorder::new();
    recorder.attach(agent.hooks());

    let (records, error) = run(&mut agent, "Say hello").await;

    assert_eq!(error, None);
    assert_records(&records, &expected);
    assert_eq!(json!(recorder.events()), fixture("hooks/text_turn.order.json"));
}

#[tokio::test]
async fn tool_turn_matches_the_fixture() {
    let expected = expected_records("agent/tool_turn.stream.json");
    let calls = fixture("agent/tool_turn.model_calls.json");
    let specs: Vec<ToolSpec> = serde_json::from_value(calls[0]["toolSpecs"].clone()).unwrap();
    let progress = ProgressTool {
        spec: specs[0].clone(),
    };
    let failing = FunctionTool::new("failing_tool", specs[1].description.clone(), |_, _| async move {
        Err::<Value, _>(ToolError::msg("tool exploded"))
    })
    .with_input_schema(specs[1].input_schema.clone().unwrap());
    let model = scripted_model(&expected);
    let mut agent = test_agent(Arc::clone(&model))
        .tools(strands::tool_list![progress, failing])
        .tool_executor(ToolExecutorStrategy::Sequential)
        .retry_strategy(Vec::new())
        .build()
        .unwrap();
    let recorder = HookRecorder::new();
    recorder.attach(agent.hooks());

    let (records, error) = run(&mut agent, "Run the tools").await;

    assert_eq!(error, None);
    assert_records(&records, &expected);
    assert_eq!(json!(recorder.events()), fixture("hooks/tool_turn.order.json"));
    let requests: Vec<Value> = model.requests().iter().map(model_call).collect();
    assert_eq!(json!(requests), calls);
}

#[tokio::test]
async fn structured_output_matches_the_fixture() {
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[expect(dead_code, reason = "only the schema is used")]
    struct Weather {
        /// City name
        city: String,
        temperature: f64,
    }
    let expected = expected_records("agent/structured_output.stream.json");
    let model = scripted_model(&expected);
    let mut agent = test_agent(Arc::clone(&model))
        .structured_output_schema(StructuredOutputSchema::of::<Weather>())
        .retry_strategy(Vec::new())
        .build()
        .unwrap();

    let (records, error) = run(&mut agent, "Weather in Paris as JSON").await;

    assert_eq!(error, None);
    assert_records(&records, &expected);
    let requests: Vec<Value> = model.requests().iter().map(model_call).collect();
    assert_eq!(
        json!(requests),
        fixture("agent/structured_output.model_calls.json")
    );
}

#[tokio::test]
async fn redaction_matches_the_fixture() {
    let expected = expected_records("agent/redaction.stream.json");
    let model = scripted_model(&expected);
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(Vec::new())
        .build()
        .unwrap();

    let (records, error) = run(&mut agent, "Tell me the secret").await;

    assert_eq!(error, None);
    assert_records(&records, &expected);
}

#[tokio::test]
async fn model_error_matches_the_fixture() {
    let expected = expected_records("agent/model_error.stream.json");
    let model = scripted_model(&expected);
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(Vec::new())
        .build()
        .unwrap();

    let (records, error) = run(&mut agent, "Hello?").await;

    assert_records(&records, &expected);
    assert_eq!(
        json!({ "message": error.unwrap() }),
        fixture("agent/model_error.error.json")
    );
}
