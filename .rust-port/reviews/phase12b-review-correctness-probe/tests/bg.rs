use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use futures::StreamExt;
use serde_json::{Value as JsonValue, json};
use strands::Agent;
use strands::background_tasks::BackgroundTasksConfig;
use strands::cancel::CancelOptions;
use strands::hooks::AfterModelCallEvent;
use strands::interrupt::{InterruptParams, InterruptResponse};
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolContext, ToolError};
use strands::types::messages::{ContentBlock, Message, Role, StopReason, TextBlock, ToolUseBlock};

fn text(t: &str) -> ContentBlock { TextBlock::new(t).into() }
fn call(name: &str, id: &str, input: JsonValue) -> ContentBlock { ToolUseBlock::new(name, id, input).into() }
fn model(turns: Vec<Vec<ContentBlock>>) -> Arc<MockMessageModel> {
    let m = Arc::new(MockMessageModel::new());
    for t in turns { m.add_turn(t); }
    m
}
fn roles(m: &[Message]) -> Vec<Role> { m.iter().map(Message::role).collect() }

/// `asks` interrupts that many times once the parent's turn has ended, then `hang` decides whether it ever returns.
fn setup(asks: usize, hang: bool, wait: bool, turns: Vec<Vec<ContentBlock>>) -> (Agent, Arc<MockMessageModel>) {
    let (turn_ended, ended) = tokio::sync::watch::channel(false);
    let tool = FunctionTool::new("work", "Ask first.", move |_i: JsonValue, ctx: ToolContext| {
        let mut ended = ended.clone();
        async move {
            ended.wait_for(|e| *e).await.ok();
            for n in 0..asks {
                ctx.interrupt(InterruptParams::new(format!("approve{n}")))?;
            }
            if hang { ctx.cancel_signal().cancelled().await; }
            Ok::<_, ToolError>(json!("done"))
        }
    });
    let model = model(turns);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools(vec![tool])
        .background_tasks(BackgroundTasksConfig { agentic: Some(vec!["work".to_owned()]), wait_for_completion: wait, ..Default::default() })
        .printer(false)
        .build()
        .unwrap();
    let calls = AtomicUsize::new(0);
    agent.add_hook(move |_: &mut AfterModelCallEvent<'_>| {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 { turn_ended.send_replace(true); }
    });
    (agent, model)
}

fn answers(result: &strands::AgentResult) -> Vec<InterruptResponse> {
    result.interrupts().iter().map(|i| InterruptResponse::new(i.id(), json!("yes"))).collect()
}

fn first_turns() -> Vec<Vec<ContentBlock>> {
    vec![vec![call("work", "work-use", json!({"_background_execution": true}))], vec![text("Task admitted.")], vec![text("Delivered.")], vec![text("Extra.")]]
}

#[tokio::test]
async fn resumed_task_interrupts_again() {
    let (mut agent, model) = setup(2, false, true, first_turns());
    let first = agent.invoke("Run work.").await.unwrap();
    assert_eq!(first.stop_reason(), StopReason::Interrupt);
    let second = agent.invoke(answers(&first)).await.unwrap();
    println!("second stop={:?} interrupts={} requests={}", second.stop_reason(), second.interrupts().len(), model.requests().len());
    let third = agent.invoke(answers(&second)).await.unwrap();
    println!("third stop={:?} text={} requests={}", third.stop_reason(), third, model.requests().len());
    for (n, r) in model.requests().iter().enumerate() { println!("req{n} {:?}", roles(&r.messages)); }
}

#[tokio::test]
async fn cancel_while_waiting_for_a_resumed_task() {
    let (mut agent, model) = setup(1, true, true, first_turns());
    let first = agent.invoke("Run work.").await.unwrap();
    assert_eq!(first.stop_reason(), StopReason::Interrupt);
    let handle = agent.cancel_handle();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        handle.cancel(CancelOptions::default());
    });
    let second = tokio::time::timeout(std::time::Duration::from_secs(5), agent.invoke(answers(&first))).await;
    canceller.await.unwrap();
    match second {
        Err(_) => println!("cancel: invoke did not return within 5s"),
        Ok(r) => println!("cancel: result={:?}", r.as_ref().map(|r| r.stop_reason()).map_err(|e| e.to_string())),
    }
    println!("cancel: requests={}", model.requests().len());
    for (n, r) in model.requests().iter().enumerate() { println!("cancel req{n} {:?}", roles(&r.messages)); }
    println!("cancel: history={:?}", roles(agent.messages()));
}

#[tokio::test]
async fn stream_dropped_while_waiting_for_a_resumed_task() {
    let (mut agent, model) = setup(1, true, true, first_turns());
    let first = agent.invoke("Run work.").await.unwrap();
    assert_eq!(first.stop_reason(), StopReason::Interrupt);
    {
        let mut stream = agent.stream(answers(&first));
        let polled = tokio::time::timeout(std::time::Duration::from_millis(300), async { let mut n = 0; while stream.next().await.is_some() { n += 1; } n }).await;
        println!("drop: polled={polled:?}");
    }
    println!("drop: requests after drop={}", model.requests().len());
    let handle = agent.cancel_handle();
    let next = tokio::time::timeout(std::time::Duration::from_secs(3), agent.invoke("Hello again")).await;
    println!("drop: next invoke={:?}", next.as_ref().map(|r| r.as_ref().map(|r| (r.stop_reason(), r.to_string())).map_err(|e| e.to_string())).map_err(|_| "timed out after 3s"));
    handle.cancel(CancelOptions::default());
    println!("drop: requests={}", model.requests().len());
    for (n, r) in model.requests().iter().enumerate() { println!("drop req{n} {:?}", roles(&r.messages)); }
}

#[tokio::test]
async fn no_wait_resume_after_turn_ended() {
    let (mut agent, model) = setup(1, false, false, first_turns());
    let first = agent.invoke("Run work.").await.unwrap();
    println!("nowait first stop={:?} interrupts={}", first.stop_reason(), first.interrupts().len());
    if first.stop_reason() == StopReason::Interrupt {
        let second = agent.invoke(answers(&first)).await.unwrap();
        println!("nowait second stop={:?} text={}", second.stop_reason(), second);
    }
    for (n, r) in model.requests().iter().enumerate() { println!("nowait req{n} {:?}", roles(&r.messages)); }
}
