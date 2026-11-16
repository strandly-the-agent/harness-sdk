//! TS agent.tracer.test.node.ts:212 "ends the loop span once when the public stream iterator is closed mid-cycle"
//! and :415 "ends the model invoke span once when the consumer breaks mid-model-response".
use std::sync::Arc;

use futures::StreamExt;
use opentelemetry::trace::Status;
use pv_probe::{attrs, recording};
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, TextBlock};

fn text(t: &str) -> ContentBlock {
    TextBlock::new(t).into()
}

#[tokio::test(flavor = "current_thread")]
async fn ends_the_loop_span_once_when_the_public_stream_is_dropped_mid_cycle() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![text("Done")]);
    model.add_turn(vec![text("Again")]);
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .name("dropper")
        .build()
        .unwrap();
    {
        let mut stream = agent.stream("Hi");
        let mut seen = 0;
        while let Some(event) = stream.next().await {
            seen += 1;
            println!("event {seen}: {}", format!("{event:?}").chars().take(60).collect::<String>());
            if seen == 3 {
                break;
            }
        }
    }
    tokio::task::yield_now().await;
    let after_drop = recorder.spans();
    println!("after drop: {:?}", after_drop.iter().map(|s| (s.name.to_string(), s.status.clone())).collect::<Vec<_>>());
    let metrics = agent.metrics();
    println!(
        "cycle_count={} total_duration={:?} cycles={:?}",
        metrics.cycle_count(),
        metrics.total_duration(),
        metrics.latest_agent_invocation().map(|i| i.cycles().len())
    );
    agent.invoke("Again").await.unwrap();
    let all = recorder.spans();
    for s in &all {
        println!("after next invoke: {} status={:?} end_time={:?}", s.name, s.status, attrs(s, "gen_ai.event.end_time").len());
    }
    assert!(
        after_drop.iter().any(|s| s.name == "execute_agent_loop_cycle" && matches!(s.status, Status::Ok)),
        "TS ends the loop span when the stream is closed; Rust exported {:?} after the drop",
        after_drop.iter().map(|s| s.name.to_string()).collect::<Vec<_>>()
    );
    assert!(after_drop.iter().any(|s| s.name == "invoke_agent dropper" && matches!(s.status, Status::Ok)));
}
