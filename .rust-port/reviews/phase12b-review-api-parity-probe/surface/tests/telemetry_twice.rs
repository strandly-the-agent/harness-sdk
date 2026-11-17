use std::sync::Arc;

use strands::models::Model;
use strands::testing::MockMessageModel;
use strands_harness::*;

fn options() -> HarnessAgentOptions {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    HarnessAgentOptions {
        model: Some(model.into()),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        builtin_plugins: Some(Vec::new()),
        builtin_tools: Some(Vec::<BuiltinToolName>::new().into()),
        ..Default::default()
    }
}

// Run with OTEL_TRACES_EXPORTER=console.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_then_build_then_shutdown() {
    assert_eq!(std::env::var("OTEL_TRACES_EXPORTER").as_deref(), Ok("console"));
    let _first = create_harness(options()).await.unwrap();
    println!("FIRST SHUTDOWN: {:?}", shutdown_telemetry().await.map_err(|e| e.to_string()));
    println!("REPEAT SHUTDOWN: {:?}", shutdown_telemetry().await.map_err(|e| e.to_string()));
    let _second = create_harness(options()).await.unwrap();
    println!("SECOND SHUTDOWN: {:?}", shutdown_telemetry().await.map_err(|e| e.to_string()));
}
