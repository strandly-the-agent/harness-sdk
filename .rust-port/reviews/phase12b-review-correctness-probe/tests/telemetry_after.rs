use std::sync::Arc;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, TextBlock};
use strands_harness::{HarnessAgentOptions, MemoryOption, SessionOption, SkillsOption, create_harness, shutdown_telemetry};

fn quiet(model: &Arc<MockMessageModel>) -> HarnessAgentOptions {
    HarnessAgentOptions {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        builtin_plugins: Some(Vec::new()),
        builtin_tools: Some(Vec::<strands_harness::BuiltinToolName>::new().into()),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_used_after_shutdown() {
    unsafe { std::env::set_var("OTEL_TRACES_EXPORTER", "console") };
    let model = Arc::new(MockMessageModel::new());
    for _ in 0..4 { model.add_turn(vec![ContentBlock::from(TextBlock::new("hi"))]); }
    let mut a = create_harness(quiet(&model)).await.expect("first harness");
    let r = a.invoke("one").await;
    eprintln!("PROBE before shutdown: {:?}", r.as_ref().map(|r| r.to_string()).map_err(|e| e.to_string()));
    eprintln!("PROBE shutdown: {:?}", shutdown_telemetry().await);
    let r = a.invoke("two").await;
    eprintln!("PROBE old agent after shutdown: {:?}", r.as_ref().map(|r| r.to_string()).map_err(|e| e.to_string()));
    let mut b = create_harness(quiet(&model)).await.expect("second harness");
    let r = b.invoke("three").await;
    eprintln!("PROBE new agent after shutdown: {:?}", r.as_ref().map(|r| r.to_string()).map_err(|e| e.to_string()));
    eprintln!("PROBE second shutdown: {:?}", shutdown_telemetry().await);
}
