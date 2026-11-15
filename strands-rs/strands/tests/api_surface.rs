//! Public API shapes a downstream crate relies on.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use strands::agent::{AgentBuilder, agent_builder};
use strands::hooks::{AfterInvocationEvent, AfterModelCallEvent, BeforeInvocationEvent, Hook};
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolSource};
use strands::types::agent::InvokeLimits;
use strands::types::json::JsonValue;
use strands::types::messages::TextBlock;
use strands::{Agent, ToolError};

fn ping() -> FunctionTool {
    FunctionTool::new(
        "ping",
        "Replies pong.",
        |_input: JsonValue, _context| async move { Ok::<_, ToolError>("pong") },
    )
}

fn model(replies: &[&str]) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    for reply in replies {
        model.add_turn(vec![TextBlock::new(*reply).into()]);
    }
    model
}

fn with_ping<S: agent_builder::State>(builder: AgentBuilder<S>) -> AgentBuilder<S> {
    builder.tools([Arc::new(ping())])
}

#[tokio::test]
async fn a_helper_can_take_and_return_an_agent_builder() {
    let agent = with_ping(Agent::builder().model(model(&[])).printer(false))
        .build()
        .unwrap();
    assert!(agent.tool_registry().get("ping").is_some());
    let _ = ToolSource::from(Arc::new(ping()));
}

#[tokio::test]
async fn set_model_takes_an_arc_of_a_concrete_model() {
    let mut agent = Agent::builder().model(model(&[])).printer(false).build().unwrap();
    agent.set_model(model(&["second"]));
    let prompt = String::from("hi");
    let result = agent.invoke(&prompt).await.unwrap();
    assert_eq!(result.to_string(), "second");
    let _ = Agent::builder().model(&prompt);
}

#[tokio::test]
async fn sequential_hooks_read_metrics_and_traces_use_system_time() {
    let cycles = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&cycles);
    let mut agent = Agent::builder()
        .model(model(&["ok"]))
        .printer(false)
        .build()
        .unwrap();
    agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
        seen.lock().unwrap().push(event.agent().metrics().cycle_count());
    });
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen_attempts = Arc::clone(&attempts);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        let attempt: NonZeroU32 = event.attempt_count();
        seen_attempts.lock().unwrap().push(attempt.get());
    });

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(*cycles.lock().unwrap(), [1]);
    assert_eq!(*attempts.lock().unwrap(), [1]);
    let trace = &result.traces()[0];
    assert!(trace.start_time().duration_since(UNIX_EPOCH).is_ok());
    assert!(trace.end_time().unwrap() >= trace.start_time());
}

#[test]
fn invoke_limits_have_a_builder() {
    let limits = InvokeLimits::builder().maybe_turns(NonZeroU32::new(3)).build();
    assert_eq!(limits.turns, NonZeroU32::new(3));
}

struct Audit;

#[strands::async_trait]
impl<'x> Hook<BeforeInvocationEvent<'x>> for Audit {
    async fn handle(&self, event: &mut BeforeInvocationEvent<'x>) {
        let _ = event.agent().id();
    }
}

#[test]
fn hook_impls_need_only_the_strands_async_trait_re_export() {
    let agent = Agent::builder().model(model(&[])).printer(false).build().unwrap();
    agent.hooks().add_hook_impl::<BeforeInvocationEvent<'_>>(Audit);
}
