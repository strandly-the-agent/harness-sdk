use std::sync::Arc;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands_harness::{HarnessAgentOptions, MemoryOption, SessionOption, SkillsOption, create_harness, shutdown_telemetry};

fn quiet() -> HarnessAgentOptions {
    HarnessAgentOptions {
        model: Some((Arc::new(MockMessageModel::new()) as Arc<dyn Model>).into()),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        builtin_plugins: Some(Vec::new()),
        builtin_tools: Some(Vec::<strands_harness::BuiltinToolName>::new().into()),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_then_create_then_shutdown() {
    // console exporter: no network
    unsafe { std::env::set_var("OTEL_TRACES_EXPORTER", "console") };
    println!("shutdown before setup: {:?}", shutdown_telemetry().await);
    let _a = create_harness(quiet()).await.expect("first harness");
    println!("first shutdown: {:?}", shutdown_telemetry().await);
    println!("shutdown again, no harness between: {:?}", shutdown_telemetry().await);
    let _b = create_harness(quiet()).await.expect("second harness");
    let second = shutdown_telemetry().await;
    println!("shutdown after a second create_harness: {second:?}");
    assert!(second.is_ok(), "second shutdown_telemetry failed: {second:?}");
}
