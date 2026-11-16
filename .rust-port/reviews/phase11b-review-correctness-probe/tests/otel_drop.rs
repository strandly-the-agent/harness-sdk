use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use strands::agent::AgentAsToolOptions;
use strands::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use strands::models::streaming::{ModelMessageStartEvent, ModelStreamEvent};
use strands::telemetry::opentelemetry::trace::{SpanId, Status};
use strands::telemetry::opentelemetry_sdk::error::OTelSdkResult;
use strands::telemetry::opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use strands::telemetry::tracing_layer;
use strands::telemetry::tracing_subscriber::layer::SubscriberExt;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolError};
use strands::types::messages::{Role, TextBlock, ToolUseBlock};
use strands::{Agent, CancelOptions};
use futures::StreamExt;

#[derive(Debug, Clone, Default)]
struct Capture(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Capture {
    fn export(&self, batch: Vec<SpanData>) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        self.0.lock().unwrap().extend(batch);
        std::future::ready(Ok(()))
    }
}

struct StalledModel;
impl Model for StalledModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, strands::Result<ModelStreamEvent>> {
        let start = futures::stream::iter([Ok(ModelMessageStartEvent::new(Role::Assistant).into())]);
        Box::pin(start.chain(futures::stream::pending()))
    }
    fn get_config(&self) -> BaseModelConfig { BaseModelConfig::default() }
    fn update_config(&self, _config: BaseModelConfig) {}
}

fn dump(label: &str, capture: &Capture) -> Vec<String> {
    let spans = capture.0.lock().unwrap();
    let name_of = |id: SpanId| spans.iter().find(|s| s.span_context.span_id() == id).map_or_else(|| if id == SpanId::INVALID { "-".to_owned() } else { "<missing>".to_owned() }, |s| s.name.to_string());
    let mut lines = Vec::new();
    for span in spans.iter() {
        let end = span.attributes.iter().any(|kv| kv.key.as_str() == "gen_ai.event.end_time");
        let status = match &span.status { Status::Unset => "unset".to_owned(), Status::Ok => "ok".to_owned(), Status::Error { description } => format!("error({description})") };
        lines.push(format!("{} <- {} status={} end_time_attr={}", span.name, name_of(span.parent_span_id), status, end));
    }
    eprintln!("== {label}");
    for line in &lines { eprintln!("   {line}"); }
    lines
}

fn with_capture() -> (Capture, tracing::subscriber::DefaultGuard, SdkTracerProvider) {
    let capture = Capture::default();
    let provider = SdkTracerProvider::builder().with_simple_exporter(capture.clone()).build();
    let subscriber = strands::telemetry::tracing_subscriber::registry().with(tracing_layer(&provider));
    let guard = tracing::subscriber::set_default(subscriber);
    (capture, guard, provider)
}

fn waiting_tool(name: &str) -> FunctionTool {
    FunctionTool::new(name, "Waits.", |_: serde_json::Value, _| async {
        std::future::pending::<()>().await;
        Ok::<_, ToolError>(json!("never"))
    })
}

#[tokio::test]
async fn completed_run() {
    let (capture, _guard, _provider) = with_capture();
    let model = MockMessageModel::new();
    model.add_turn(vec![ToolUseBlock::new("echo", "t1", json!({})).into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let echo = FunctionTool::new("echo", "Echo.", |_: serde_json::Value, _| async { Ok::<_, ToolError>(json!("hi")) });
    let mut agent = Agent::builder().model(model).tools([echo]).printer(false).build().unwrap();
    agent.invoke("go").await.unwrap();
    dump("completed", &capture);
}

#[tokio::test]
async fn dropped_mid_tool() {
    let (capture, _guard, _provider) = with_capture();
    let model = MockMessageModel::new();
    model.add_turn(vec![ToolUseBlock::new("wait", "t1", json!({})).into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder().model(model).tools([waiting_tool("wait")]).printer(false).build().unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(300), agent.invoke("go")).await.is_err());
    let before = dump("dropped mid tool, before shutdown", &capture).len();
    agent.shutdown().await.unwrap();
    drop(agent);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after = dump("dropped mid tool, after shutdown + drop", &capture);
    eprintln!("before={before} after={}", after.len());
}

#[tokio::test]
async fn dropped_mid_model() {
    let (capture, _guard, _provider) = with_capture();
    let mut agent = Agent::builder().model(StalledModel).printer(false).build().unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(300), agent.invoke("go")).await.is_err());
    dump("dropped mid model", &capture);
    agent.shutdown().await.unwrap();
    dump("dropped mid model after shutdown", &capture);
}

#[tokio::test]
async fn cancelled_mid_tool() {
    let (capture, _guard, _provider) = with_capture();
    let model = MockMessageModel::new();
    model.add_turn(vec![ToolUseBlock::new("wait", "t1", json!({})).into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let coop = FunctionTool::new("wait", "Waits.", |_: serde_json::Value, ctx: strands::ToolContext| async move {
        ctx.cancel_signal().cancelled().await;
        Ok::<_, ToolError>(json!("stopped"))
    });
    let mut agent = Agent::builder().model(model).tools([coop]).printer(false).build().unwrap();
    let handle = agent.cancel_handle();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(200)).await; handle.cancel(CancelOptions::default()); });
    let result = agent.invoke("go").await;
    eprintln!("cancel result: {:?}", result.as_ref().map(|r| r.stop_reason().clone()).map_err(ToString::to_string));
    dump("cancelled mid tool", &capture);
}

#[tokio::test]
async fn model_error() {
    let (capture, _guard, _provider) = with_capture();
    let model = MockMessageModel::new();
    model.add_error(|| strands::Error::other("boom"));
    let mut agent = Agent::builder().model(model).no_retry_strategy().printer(false).build().unwrap();
    let result = agent.invoke("go").await;
    eprintln!("error result: {:?}", result.map(|_| ()).map_err(|e| e.to_string()));
    dump("model error", &capture);
}

#[tokio::test]
async fn sub_agent_dropped() {
    let (capture, _guard, _provider) = with_capture();
    let child_model = MockMessageModel::new();
    child_model.add_turn(vec![ToolUseBlock::new("wait", "c1", json!({})).into()]);
    let child = Agent::builder().model(child_model).name("child").tools([waiting_tool("wait")]).printer(false).build().unwrap();
    let model = MockMessageModel::new();
    model.add_turn(vec![ToolUseBlock::new("child", "t1", json!({"input": "x"})).into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder().model(model).name("parent").tools([child.as_tool(AgentAsToolOptions::default()).unwrap()]).printer(false).build().unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(400), agent.invoke("go")).await.is_err());
    agent.shutdown().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    dump("sub agent dropped", &capture);
}

struct SlowStore;

#[strands::async_trait]
impl strands::memory::MemoryStore for SlowStore {
    fn name(&self) -> &str { "slow" }
    fn writable(&self) -> bool { false }
    async fn search(&self, _query: &str, _options: strands::memory::SearchOptions) -> strands::Result<Vec<strands::memory::MemoryEntry>> {
        std::future::pending::<()>().await;
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn dropped_mid_memory_search() {
    let (capture, _guard, _provider) = with_capture();
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("done").into()]);
    let memory = strands::memory::MemoryManager::new(strands::memory::MemoryManagerConfig {
        stores: vec![Arc::new(SlowStore)],
        injection: Some(true.into()),
        ..Default::default()
    })
    .unwrap();
    let mut agent = Agent::builder().model(model).memory_manager(memory).printer(false).build().unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(300), agent.invoke("what do you remember")).await.is_err());
    agent.shutdown().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let lines = dump("dropped mid memory search", &capture);
    let unfinished: Vec<&String> = lines.iter().filter(|line| line.starts_with("memory.") && line.contains("end_time_attr=false")).collect();
    assert!(unfinished.is_empty(), "memory spans closed without their end bookkeeping: {unfinished:?}");
}
