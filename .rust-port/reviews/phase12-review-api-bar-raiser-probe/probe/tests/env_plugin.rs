use std::sync::Arc;
use strands::models::Model;
use strands::plugins::Plugin;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, TextBlock};
use strands_harness::*;

async fn environment_blocks(plugins: Vec<Arc<dyn Plugin>>) -> usize {
    let mock = Arc::new(MockMessageModel::new());
    mock.add_turn(vec![ContentBlock::Text(TextBlock::new("done"))]);
    let model: Arc<dyn Model> = mock.clone();
    let options = HarnessAgentOptions {
        model: Some(model.into()),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        builtin_tools: Some(Vec::<BuiltinToolName>::new().into()),
        plugins,
        ..Default::default()
    };
    let mut agent = create_harness(options).await.unwrap();
    agent.invoke("hi").await.unwrap();
    let requests = format!("{:?}", mock.requests());
    requests.matches("<environment>").count()
}

#[tokio::test]
async fn a_renamed_environment_context_replaces_the_builtin() {
    let baseline = environment_blocks(vec![]).await;
    let default_named = environment_blocks(vec![Arc::new(EnvironmentContext::default())]).await;
    let renamed = environment_blocks(vec![Arc::new(EnvironmentContext::new(EnvironmentContextConfig {
        name: Some("my:environment".into()),
    }))])
    .await;
    println!("baseline={baseline} default_named={default_named} renamed={renamed}");
    assert_eq!(renamed, baseline);
}
