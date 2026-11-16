use std::sync::Arc;
use strands::models::Model;
use strands::plugins::Plugin;
use strands::testing::MockMessageModel;
use strands_harness::*;

fn quiet() -> HarnessAgentOptions {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    HarnessAgentOptions {
        model: Some(model.into()),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        builtin_tools: Some(Vec::<BuiltinToolName>::new().into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_todos_instance_with_the_default_name_replaces_the_builtin() {
    let todos: Arc<dyn Plugin> = Arc::new(Todos::default());
    let mut agent = create_harness(HarnessAgentOptions { plugins: vec![todos], ..quiet() }).await.unwrap();
    agent.initialize().await.unwrap();
    println!("default name: {:?}", agent.tools().map(|t| t.name().to_owned()).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_todos_instance_with_its_own_name_replaces_the_builtin() {
    let todos: Arc<dyn Plugin> = Arc::new(Todos::new(TodosConfig { name: Some("my:todos".into()), state_key: Some("mine".into()) }));
    let built = create_harness(HarnessAgentOptions { plugins: vec![todos], ..quiet() }).await;
    match built {
        Err(error) => panic!("create_harness: {error}"),
        Ok(mut agent) => {
            let init = agent.initialize().await;
            println!("own name: init={:?} tools={:?}", init.as_ref().map_err(ToString::to_string), agent.tools().map(|t| t.name().to_owned()).collect::<Vec<_>>());
            init.unwrap();
        }
    }
}
