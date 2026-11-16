use std::sync::Arc;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::{AgentConfig, Tool};
use strands_harness::*;

fn mock() -> Arc<dyn Model> {
    Arc::new(MockMessageModel::new())
}

fn quiet() -> HarnessAgentOptions {
    HarnessAgentOptions {
        model: Some(mock().into()),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn builds_on_a_current_thread_runtime() {
    let mut agent = create_harness(quiet()).await.unwrap();
    agent.initialize().await.unwrap();
    let mut names: Vec<String> = agent.tools().map(|tool| tool.name().to_owned()).collect();
    names.sort();
    println!("tools: {names:?}");
    agent.shutdown().await.unwrap();
}

#[tokio::test]
async fn passthrough_errors() {
    let options = HarnessAgentOptions {
        agent_config: AgentConfig { model: Some(mock().into()), ..Default::default() },
        ..quiet()
    };
    let error = create_harness(options).await.err().unwrap();
    println!("passthrough: {error} / {error:?}");
    assert!(matches!(error, HarnessError::PassthroughOption { key: "model" }));
}

#[tokio::test]
async fn empty_everything() {
    let options = HarnessAgentOptions {
        builtin_tools: Some(Vec::<BuiltinToolName>::new().into()),
        builtin_plugins: Some(vec![]),
        background_tasks: Some(false.into()),
        context_manager: Some(ContextManagerOption::Off),
        ..quiet()
    };
    let mut agent = create_harness(options).await.unwrap();
    agent.initialize().await.unwrap();
    let names: Vec<String> = agent.tools().map(|tool| tool.name().to_owned()).collect();
    println!("empty: {names:?}");
}

#[test]
fn outside_a_runtime() {
    // No tokio runtime: the future must report an error or work, not panic.
    let result = std::panic::catch_unwind(|| {
        futures_lite_block_on(create_harness(quiet())).map(|_| ())
    });
    println!("outside runtime: {:?}", result.as_ref().map(|r| r.as_ref().map_err(ToString::to_string)).map_err(|_| "panicked"));
}

fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop { fn wake(self: Arc<Self>) {} }
    let waker = Waker::from(Arc::new(Noop));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) { return output; }
        std::thread::yield_now();
    }
}
