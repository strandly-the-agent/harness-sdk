//! TS agent.ts:2044-2046 `catch (error) { closeCycle(error as Error); throw error }`: a CancelledError raised in
//! the cycle (agent.ts:2409, :2724, :2743) ends the loop span with ERROR status and its message.
use std::sync::Arc;

use futures::StreamExt;
use opentelemetry::trace::Status;
use pv_probe::recording;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, TextBlock};
use strands::{Agent, AgentStreamEvent, CancelOptions};

#[tokio::test(flavor = "current_thread")]
async fn a_cancel_during_model_streaming_ends_the_loop_span_with_the_cancel_error() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![ContentBlock::from(TextBlock::new("Done"))]);
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .name("cancelled")
        .build()
        .unwrap();
    let handle = agent.cancel_handle();
    {
        let mut stream = agent.stream("Hi");
        let mut cancelled = false;
        while let Some(event) = stream.next().await {
            if !cancelled && matches!(event, Ok(AgentStreamEvent::ModelStreamUpdate(_))) {
                handle.cancel(CancelOptions::default());
                cancelled = true;
            }
        }
        assert!(cancelled);
    }
    let spans = recorder.spans();
    for s in &spans {
        println!("{} status={:?}", s.name, s.status);
    }
    let chat = spans.iter().find(|s| s.name == "chat").expect("chat span");
    assert!(matches!(&chat.status, Status::Error { description } if description == "Agent invocation cancelled"), "chat: {:?}", chat.status);
    let cycle = spans.iter().find(|s| s.name == "execute_agent_loop_cycle").expect("cycle span");
    assert!(
        matches!(&cycle.status, Status::Error { description } if description == "Agent invocation cancelled"),
        "TS ends the loop span with the CancelledError; Rust status is {:?}",
        cycle.status
    );
}
