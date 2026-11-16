//! Run: (sleep 2; echo y; sleep 6) | hitl_stale
//! t=0 prompt A (read_notes) is shown; the invocation is dropped at t=1; the human's "y" for A
//! arrives at t=2; at t=3 a second invocation asks about delete_everything.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolError};
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::vended_interventions::{Ask, HumanInTheLoop, HumanInTheLoopConfig};

#[tokio::main]
async fn main() {
    let start = Instant::now();
    let deleted = Arc::new(AtomicBool::new(false));
    let flag = deleted.clone();
    let delete = FunctionTool::new("delete_everything", "Deletes.", move |_: serde_json::Value, _| {
        let flag = flag.clone();
        async move {
            flag.store(true, Ordering::SeqCst);
            Ok::<_, ToolError>(json!("deleted"))
        }
    });
    let read = FunctionTool::new("read_notes", "Reads.", |_: serde_json::Value, _| async { Ok::<_, ToolError>(json!("notes")) });
    let model = MockMessageModel::new();
    model.add_turn(vec![ToolUseBlock::new("read_notes", "t1", json!({})).into()]);
    model.add_turn(vec![ToolUseBlock::new("delete_everything", "t2", json!({})).into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let hitl = HumanInTheLoop::new(HumanInTheLoopConfig { ask: Some(Ask::Stdio), ..Default::default() });
    let mut agent = Agent::builder().model(model).tools([read, delete]).intervention(hitl).printer(false).build().unwrap();

    let first = tokio::time::timeout(Duration::from_secs(1), agent.invoke("read my notes")).await;
    eprintln!("\n[{:.1}s] first invocation dropped while prompt A waits: {}", start.elapsed().as_secs_f32(), first.is_err());
    tokio::time::sleep(Duration::from_secs(2)).await;
    eprintln!("[{:.1}s] second invocation starts (the human typed `y` for prompt A at 2.0s)", start.elapsed().as_secs_f32());
    let asked = Instant::now();
    let second = agent.invoke("now clean up").await;
    eprintln!(
        "\n[{:.1}s] second invocation finished {:.2}s after it started: ok={} delete_everything ran={}",
        start.elapsed().as_secs_f32(), asked.elapsed().as_secs_f32(), second.is_ok(), deleted.load(Ordering::SeqCst)
    );
    std::process::exit(i32::from(deleted.load(Ordering::SeqCst)));
}
