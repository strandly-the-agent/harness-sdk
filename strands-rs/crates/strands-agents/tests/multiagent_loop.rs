//! Cross-module multi-agent behaviour through the public API: `Graph` and `Swarm` driving real
//! `Agent` nodes, nested orchestrators, snapshot round-trips across instances, plugins,
//! cancellation and streams (`strands-ts/src/multiagent/__tests__/{graph,swarm,interrupts}.test.ts`).
//!
//! The per-orchestrator unit tests in `src/multiagent/{graph,swarm}.rs` cover each piece in
//! isolation; these exercise the combinations — a node that is itself an orchestrator, state that
//! travels through a serialized snapshot, an invocation state that has to reach a tool.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use strands_agents::multiagent::{
    edge_handler, take_snapshot, AfterMultiAgentInvocationEvent, BeforeNodeCallEvent, Graph, MultiAgent,
    MultiAgentInput, MultiAgentInvokeOptions, MultiAgentPlugin, MultiAgentResult, MultiAgentState,
    MultiAgentStream, MultiAgentStreamEvent, NodeInnerEvent, OrchestratorRef, ResultStatus, Swarm,
    TakeMultiAgentSnapshotOptions,
};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::FunctionTool;
use strands_agents::types::{
    ContentBlock, InterruptParams, InterruptResponseContent, Message, Role, StopReason, ToolUseBlock,
};
use strands_agents::InvocationState;
use strands_agents::{Agent, Result, Snapshot};
use tokio_util::sync::CancellationToken;

// ---- fixtures ----

/// Agent whose single turn is `text`.
fn saying(id: &str, text: &str) -> Agent {
    Agent::builder()
        .id(id)
        .model(MockMessageModel::shared(vec![MockResponse::message(
            Message::assistant(text),
        )]))
        .build()
        .expect("agent builds")
}

/// One `strands_structured_output` call carrying a swarm `HandoffResult`.
fn handoff_turn(handoff: serde_json::Value) -> MockResponse {
    MockResponse::with_stop_reason(
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "strands_structured_output",
                "so-1",
                handoff,
            ))],
        ),
        StopReason::ToolUse,
    )
}

/// Swarm agent that answers with one handoff decision.
fn deciding(id: &str, handoff: serde_json::Value) -> Agent {
    Agent::builder()
        .id(id)
        .description(format!("Agent {id}"))
        .model(MockMessageModel::shared(vec![handoff_turn(handoff)]))
        .build()
        .expect("agent builds")
}

/// Assistant turn calling `name` with no input.
fn calling(name: &str) -> MockResponse {
    MockResponse::with_stop_reason(
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                name,
                "call-1",
                json!({}),
            ))],
        ),
        StopReason::ToolUse,
    )
}

/// Agent whose tool records how many nodes are in flight while it sleeps, then reports `text`.
fn tracking(id: &str, in_flight: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> Agent {
    let tool = Arc::new(
        FunctionTool::builder("work")
            .description("Works for a while")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, _ctx| {
                let (in_flight, peak) = (Arc::clone(&in_flight), Arc::clone(&peak));
                async move {
                    let running = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(running, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(json!("worked"))
                }
            }),
    );
    Agent::builder()
        .id(id)
        .model(MockMessageModel::shared(vec![
            calling("work"),
            MockResponse::message(Message::assistant(format!("{id} done"))),
        ]))
        .tool(tool)
        .build()
        .expect("agent builds")
}

/// Agent whose tool interrupts for approval, then answers with `final_turn`.
fn approving_with(id: &str, final_turn: MockResponse) -> Agent {
    let tool = Arc::new(
        FunctionTool::builder("confirm")
            .description("Interrupts for approval")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(|_input, ctx| async move {
                let answer: String = ctx.interrupt(InterruptParams::new("approve").with_reason("ok?"))?;
                Ok(json!(format!("approved: {answer}")))
            }),
    );
    Agent::builder()
        .id(id)
        .model(MockMessageModel::shared(vec![calling("confirm"), final_turn]))
        .tool(tool)
        .build()
        .expect("agent builds")
}

/// `approving_with` for a graph node: the final turn is plain text.
fn approving(id: &str) -> Agent {
    approving_with(id, MockResponse::message(Message::assistant("approved")))
}

/// `approving_with` for a swarm node: the final turn is a handoff decision ending the swarm.
fn approving_in_a_swarm(id: &str) -> Agent {
    approving_with(id, handoff_turn(json!({ "message": "approved" })))
}

fn node_ids(result: &MultiAgentResult) -> Vec<&str> {
    result.results.iter().map(|r| r.node_id.as_str()).collect()
}

fn text(result: &MultiAgentResult) -> Vec<String> {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Text a node produced, as an edge handler sees it in the shared state.
fn said(state: &MultiAgentState, node_id: &str) -> String {
    state
        .node(node_id)
        .map(|node| {
            node.content()
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

async fn collect(stream: MultiAgentStream) -> Vec<MultiAgentStreamEvent> {
    stream
        .map(|event| event.expect("stream event"))
        .collect::<Vec<_>>()
        .await
}

// ---- graph routing ----

#[tokio::test]
async fn conditional_edges_route_to_the_branch_the_router_chose() {
    let build = |answer: &str| {
        Graph::builder()
            .node(saying("router", answer))
            .node(saying("specialist", "escalated"))
            .node(saying("closer", "closed"))
            .edge_when(
                "router",
                "specialist",
                edge_handler(|state| said(state, "router") == "escalate"),
            )
            .edge_when(
                "router",
                "closer",
                edge_handler(|state| said(state, "router") != "escalate"),
            )
            .max_steps(10)
            .build()
            .expect("graph builds")
    };

    let escalated = build("escalate").invoke("ticket").await.expect("graph runs");
    assert_eq!(escalated.status, ResultStatus::Completed);
    assert_eq!(node_ids(&escalated), vec!["router", "specialist"]);
    assert_eq!(text(&escalated), vec!["escalated".to_owned()]);

    let closed = build("resolve").invoke("ticket").await.expect("graph runs");
    assert_eq!(closed.status, ResultStatus::Completed);
    assert_eq!(node_ids(&closed), vec!["router", "closer"]);
    assert_eq!(text(&closed), vec!["closed".to_owned()]);
}

#[tokio::test]
async fn max_concurrency_caps_the_nodes_actually_running_at_once() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let graph = Graph::builder()
        .node(saying("fan", "fan out"))
        .node(tracking("b", Arc::clone(&in_flight), Arc::clone(&peak)))
        .node(tracking("c", Arc::clone(&in_flight), Arc::clone(&peak)))
        .node(tracking("d", Arc::clone(&in_flight), Arc::clone(&peak)))
        .edge("fan", "b")
        .edge("fan", "c")
        .edge("fan", "d")
        .max_concurrency(2)
        .max_steps(10)
        .build()
        .expect("graph builds");

    let result = graph.invoke("go").await.expect("graph runs");

    assert_eq!(result.status, ResultStatus::Completed);
    let mut ids = node_ids(&result);
    ids[1..].sort_unstable();
    assert_eq!(ids, vec!["fan", "b", "c", "d"]);
    assert_eq!(
        peak.load(Ordering::SeqCst),
        2,
        "two of the three branches run concurrently, never three"
    );
    assert_eq!(in_flight.load(Ordering::SeqCst), 0, "every node finished");
}

#[tokio::test]
async fn parallel_branches_without_a_concurrency_cap_all_run_together() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let graph = Graph::builder()
        .node(saying("fan", "fan out"))
        .node(tracking("b", Arc::clone(&in_flight), Arc::clone(&peak)))
        .node(tracking("c", Arc::clone(&in_flight), Arc::clone(&peak)))
        .node(tracking("d", Arc::clone(&in_flight), Arc::clone(&peak)))
        .edge("fan", "b")
        .edge("fan", "c")
        .edge("fan", "d")
        .max_steps(10)
        .build()
        .expect("graph builds");

    graph.invoke("go").await.expect("graph runs");

    assert_eq!(
        peak.load(Ordering::SeqCst),
        3,
        "all three branches run concurrently"
    );
}

// ---- swarm handoffs ----

#[tokio::test]
async fn a_swarm_chains_handoffs_through_every_agent_to_a_final_answer() {
    let swarm = Swarm::builder()
        .node(deciding(
            "intake",
            json!({ "agentId": "research", "message": "look it up" }),
        ))
        .node(deciding(
            "research",
            json!({ "agentId": "writer", "message": "write it up", "context": { "sources": 2 } }),
        ))
        .node(deciding("writer", json!({ "message": "the final report" })))
        .max_steps(5)
        .build()
        .expect("swarm builds");

    let events = collect(swarm.stream("research rust")).await;
    let handoffs: Vec<(&str, &[String])> = events
        .iter()
        .filter_map(|event| match event {
            MultiAgentStreamEvent::MultiAgentHandoff(event) => {
                Some((event.source.as_str(), event.targets.as_slice()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        handoffs,
        vec![
            ("intake", ["research".to_owned()].as_slice()),
            ("research", ["writer".to_owned()].as_slice()),
        ]
    );

    let result = match events.last() {
        Some(MultiAgentStreamEvent::MultiAgentResult(event)) => event.result.clone(),
        other => panic!(
            "last event is the aggregate result, got {:?}",
            other.map(|e| e.type_name())
        ),
    };
    assert_eq!(result.status, ResultStatus::Completed);
    assert_eq!(node_ids(&result), vec!["intake", "research", "writer"]);
    assert_eq!(text(&result), vec!["the final report".to_owned()]);
    assert_eq!(
        result.results[1].structured_output,
        Some(json!({ "agentId": "writer", "message": "write it up", "context": { "sources": 2 } })),
        "each node's handoff decision is kept on its result"
    );
}

// ---- nested orchestrators ----

#[tokio::test]
async fn a_swarm_node_runs_inside_a_graph_and_keeps_its_inner_node_identities() {
    let crew: Arc<dyn MultiAgent> = Arc::new(
        Swarm::builder()
            .id("crew")
            .node(deciding(
                "drafter",
                json!({ "agentId": "editor", "message": "polish it" }),
            ))
            .node(deciding("editor", json!({ "message": "polished draft" })))
            .max_steps(4)
            .build()
            .expect("swarm builds"),
    );
    let graph = Graph::builder()
        .node(saying("planner", "plan ready"))
        .node(crew)
        .edge("planner", "crew")
        .max_steps(10)
        .build()
        .expect("graph builds");

    let events = collect(graph.stream("write a post")).await;
    let result = match events.last() {
        Some(MultiAgentStreamEvent::MultiAgentResult(event)) => event.result.clone(),
        other => panic!(
            "last event is the aggregate result, got {:?}",
            other.map(|e| e.type_name())
        ),
    };

    assert_eq!(result.status, ResultStatus::Completed);
    assert_eq!(node_ids(&result), vec!["planner", "crew"]);
    assert_eq!(text(&result), vec!["polished draft".to_owned()]);

    // The nested swarm's own node events pass through unchanged (`MultiAgentNode` only re-wraps
    // orchestrator-level events), so the inner agents stay identifiable from the outer stream.
    let inner_agents: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            MultiAgentStreamEvent::NodeStreamUpdate(event) => match &event.inner {
                NodeInnerEvent::Agent(_) => Some(event.node_id.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        inner_agents.contains(&"drafter") && inner_agents.contains(&"editor"),
        "inner agent events keep their own node ids: {inner_agents:?}"
    );
}

#[tokio::test]
async fn a_graph_node_inside_a_swarm_ends_the_swarm_with_its_own_content() {
    // A nested orchestrator is never handed the handoff schema (TS `nodes.ts`:
    // "`structuredOutputSchema` is not applicable here"), so it cannot decide a handoff and the
    // swarm ends with its content after one step.
    let pipeline: Arc<dyn MultiAgent> = Arc::new(
        Graph::builder()
            .id("pipeline")
            .node(saying("extract", "extracted"))
            .node(saying("summarize", "pipeline summary"))
            .edge("extract", "summarize")
            .max_steps(5)
            .build()
            .expect("graph builds"),
    );
    let swarm = Swarm::builder()
        .node(pipeline)
        .node(deciding("reviewer", json!({ "message": "never reached" })))
        .entry_point("pipeline")
        .max_steps(4)
        .build()
        .expect("swarm builds");

    let result = swarm.invoke("go").await.expect("swarm runs");

    assert_eq!(result.status, ResultStatus::Completed);
    assert_eq!(node_ids(&result), vec!["pipeline"]);
    assert_eq!(text(&result), vec!["pipeline summary".to_owned()]);
    assert_eq!(
        result.results[0].structured_output, None,
        "the nested graph produces no handoff decision"
    );
}

#[tokio::test]
async fn a_nested_orchestrator_interrupt_bubbles_up_and_resumes_on_the_same_instances() {
    let inner: Arc<dyn MultiAgent> = Arc::new(
        Swarm::builder()
            .id("inner")
            .node(approving_in_a_swarm("inner-agent"))
            .max_steps(4)
            .build()
            .expect("swarm builds"),
    );
    let outer = Graph::builder()
        .node(Arc::clone(&inner))
        .max_steps(5)
        .build()
        .expect("graph builds");

    let interrupted = outer.invoke("go").await.expect("graph runs");
    assert_eq!(interrupted.status, ResultStatus::Interrupted);
    assert_eq!(interrupted.interrupts.len(), 1);
    assert_eq!(node_ids(&interrupted), vec!["inner"]);

    let resumed = outer
        .invoke(MultiAgentInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("yes")),
        ]))
        .await
        .expect("graph resumes");

    assert_eq!(resumed.status, ResultStatus::Completed);
    assert_eq!(text(&resumed), vec!["approved".to_owned()]);
}

#[tokio::test]
async fn a_nested_interrupt_cannot_be_resumed_on_fresh_instances_from_the_outer_snapshot() {
    // `interrupts.test.ts`: "Nested orchestrator: interrupts bubble up on first run but do not
    // round-trip without a nested SessionManager". The outer snapshot holds no nested state, so
    // the rebuilt swarm has no node owning the interrupt and the nested node fails diagnosably.
    let build = || {
        let inner: Arc<dyn MultiAgent> = Arc::new(
            Swarm::builder()
                .id("inner")
                .node(approving_in_a_swarm("inner-agent"))
                .max_steps(4)
                .build()
                .expect("swarm builds"),
        );
        Graph::builder()
            .node(inner)
            .max_steps(5)
            .build()
            .expect("graph builds")
    };

    let first = build();
    let interrupted = first.invoke("go").await.expect("graph runs");
    assert_eq!(interrupted.status, ResultStatus::Interrupted);
    let snapshot = first
        .take_snapshot(None, TakeMultiAgentSnapshotOptions::default())
        .expect("snapshot");

    let second = build();
    second.load_snapshot(&snapshot, None).expect("snapshot loads");
    let resumed = second
        .invoke(MultiAgentInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("yes")),
        ]))
        .await
        .expect("graph runs");

    assert_eq!(resumed.status, ResultStatus::Failed);
    let inner_result = resumed
        .results
        .iter()
        .find(|result| result.node_id == "inner")
        .expect("the nested node reports");
    assert_eq!(inner_result.status, ResultStatus::Failed);
    assert!(
        inner_result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("no node found with matching interrupt")),
        "diagnosable error, got {:?}",
        inner_result.error
    );
}

// ---- snapshot round-trips across instances ----

/// Serializes `snapshot` and reads it back, as a session store would.
fn through_json(snapshot: &Snapshot) -> Snapshot {
    serde_json::from_str(&serde_json::to_string(snapshot).expect("snapshot serializes"))
        .expect("snapshot deserializes")
}

#[tokio::test]
async fn a_graph_resumes_its_remaining_nodes_on_a_new_instance_through_a_serialized_snapshot() {
    let build = |max_steps: u64| {
        Graph::builder()
            .node(saying("a", "from a"))
            .node(saying("b", "from b"))
            .edge("a", "b")
            .max_steps(max_steps)
            .build()
            .expect("graph builds")
    };

    let first = build(1);
    first
        .invoke("go")
        .await
        .expect_err("the step budget stops after a");
    let snapshot = through_json(
        &first
            .take_snapshot(None, TakeMultiAgentSnapshotOptions::default())
            .expect("snapshot"),
    );

    let second = build(5);
    second.load_snapshot(&snapshot, None).expect("snapshot loads");
    let resumed = second.invoke("go").await.expect("graph resumes");

    assert_eq!(resumed.status, ResultStatus::Completed);
    assert_eq!(node_ids(&resumed), vec!["a", "b"], "a's result carries over");
    assert_eq!(text(&resumed), vec!["from b".to_owned()]);
}

#[tokio::test]
async fn a_swarm_resumes_its_pending_handoff_target_on_a_new_instance() {
    let saved: Arc<Mutex<Option<Snapshot>>> = Arc::new(Mutex::new(None));
    let record = Arc::clone(&saved);
    let crashed = Swarm::builder()
        .id("crew")
        .node(deciding("a", json!({ "agentId": "b", "message": "b's turn" })))
        .node(deciding("b", json!({ "message": "done by b" })))
        .max_steps(1)
        .hook::<AfterMultiAgentInvocationEvent, _>(move |event| {
            // What a session manager does: snapshot the live state, error path included.
            let snapshot = take_snapshot(
                event.orchestrator.as_ref(),
                Some(&event.state),
                TakeMultiAgentSnapshotOptions::default(),
            )?;
            *record.lock().expect("not poisoned") = Some(snapshot);
            Ok(())
        })
        .build()
        .expect("swarm builds");

    let error = crashed.invoke("go").await.expect_err("the step budget runs out");
    assert_eq!(error.to_string(), "max_steps=<1> | swarm reached step limit");
    let snapshot = through_json(
        saved
            .lock()
            .expect("not poisoned")
            .as_ref()
            .expect("the after-invocation hook saved a snapshot"),
    );

    let resumed_model = MockMessageModel::shared(vec![handoff_turn(json!({ "message": "done by b" }))]);
    let second = Swarm::builder()
        .id("crew")
        .node(deciding("a", json!({ "agentId": "b", "message": "b's turn" })))
        .node(
            Agent::builder()
                .id("b")
                .description("Agent b")
                .model(Arc::clone(&resumed_model))
                .build()
                .expect("agent builds"),
        )
        .max_steps(4)
        .build()
        .expect("swarm builds");
    second.load_snapshot(&snapshot, None).expect("snapshot loads");

    let result = second.invoke("go").await.expect("swarm resumes");

    assert_eq!(result.status, ResultStatus::Completed);
    assert_eq!(node_ids(&result), vec!["a", "b"], "a's result carries over");
    assert_eq!(text(&result), vec!["done by b".to_owned()]);
    let prompt = resumed_model.calls()[0].0[0].content.clone();
    assert_eq!(
        prompt,
        vec![ContentBlock::Text("b's turn".into())],
        "the resumed node is handed the handoff message that was in flight"
    );
}

#[tokio::test]
async fn a_snapshot_taken_from_one_orchestrator_is_rejected_by_another() {
    let graph = Graph::builder()
        .id("left")
        .node(saying("a", "from a"))
        .max_steps(5)
        .build()
        .expect("graph builds");
    graph.invoke("go").await.expect("graph runs");
    let snapshot = graph
        .take_snapshot(None, TakeMultiAgentSnapshotOptions::default())
        .expect("snapshot");

    let other = Graph::builder()
        .id("right")
        .node(saying("a", "from a"))
        .max_steps(5)
        .build()
        .expect("graph builds");

    let error = other
        .load_snapshot(&through_json(&snapshot), None)
        .expect_err("ids must match");
    assert_eq!(
        error.to_string(),
        "Snapshot orchestrator ID mismatch: expected 'right', got 'left'"
    );
}

#[tokio::test]
#[ignore = "bug: Agent::take_snapshot never captures interrupt state (src/agent/snapshot.rs:124), \
            so a node restored from a snapshot restarts its agent instead of resuming the paused tool"]
async fn a_graph_interrupt_resumes_on_a_new_instance_through_a_serialized_snapshot() {
    let build = || {
        Graph::builder()
            .node(approving("gate"))
            .node(saying("finish", "shipped"))
            .edge("gate", "finish")
            .max_steps(10)
            .build()
            .expect("graph builds")
    };

    let first = build();
    let interrupted = first.invoke("go").await.expect("graph runs");
    assert_eq!(interrupted.status, ResultStatus::Interrupted);
    assert_eq!(node_ids(&interrupted), vec!["gate"]);
    let snapshot = through_json(
        &first
            .take_snapshot(None, TakeMultiAgentSnapshotOptions::default())
            .expect("snapshot"),
    );

    let second = build();
    second.load_snapshot(&snapshot, None).expect("snapshot loads");
    let resumed = second
        .invoke(MultiAgentInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("yes")),
        ]))
        .await
        .expect("graph resumes");

    // Observed: INTERRUPTED again, with the same interrupt, because the restored agent replays
    // the tool call from its first message instead of resuming it.
    assert_eq!(resumed.status, ResultStatus::Completed);
    assert_eq!(node_ids(&resumed), vec!["gate", "finish"]);
    assert_eq!(text(&resumed), vec!["shipped".to_owned()]);
    assert!(resumed.interrupts.is_empty());
}

#[tokio::test]
#[ignore = "bug: Swarm::take_snapshot(None, ..) drops the state (src/multiagent/swarm.rs:176-182), \
            where DESIGN.md and Graph (src/multiagent/graph.rs:539-556) snapshot the last run"]
async fn a_swarm_snapshot_without_a_state_handle_carries_the_last_runs_state() {
    let swarm = Swarm::builder()
        .id("crew")
        .node(deciding("a", json!({ "message": "done" })))
        .max_steps(2)
        .build()
        .expect("swarm builds");
    swarm.invoke("go").await.expect("swarm runs");

    let snapshot = swarm
        .take_snapshot(None, TakeMultiAgentSnapshotOptions::default())
        .expect("snapshot");

    // Observed: `data` holds `orchestratorId` only, so the snapshot restores nothing.
    assert!(
        snapshot.data.contains_key("state"),
        "the last invocation's state is captured: {:?}",
        snapshot.data.keys().collect::<Vec<_>>()
    );
}

// ---- plugins ----

/// Plugin that registers a node-call hook once and records every node it sees.
struct Recording {
    initialized: Arc<AtomicUsize>,
    nodes: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl MultiAgentPlugin for Recording {
    fn name(&self) -> &str {
        "recording"
    }

    async fn init_multi_agent(&self, orchestrator: &OrchestratorRef) -> Result<()> {
        self.initialized.fetch_add(1, Ordering::SeqCst);
        let nodes = Arc::clone(&self.nodes);
        orchestrator
            .hooks()
            .add_sync_callback(move |event: &mut BeforeNodeCallEvent| {
                nodes.lock().expect("not poisoned").push(event.node_id.clone());
                Ok(())
            });
        Ok(())
    }
}

#[tokio::test]
async fn a_plugin_initializes_once_and_its_hooks_see_every_node_of_every_run() {
    let initialized = Arc::new(AtomicUsize::new(0));
    let nodes = Arc::new(Mutex::new(Vec::new()));
    let graph = Graph::builder()
        .node(saying("a", "from a"))
        .node(saying("b", "from b"))
        .edge("a", "b")
        .max_steps(10)
        .plugin(Arc::new(Recording {
            initialized: Arc::clone(&initialized),
            nodes: Arc::clone(&nodes),
        }))
        .build()
        .expect("graph builds");

    let first = graph.invoke("first").await.expect("graph runs");
    let second = graph.invoke("second").await.expect("graph runs again");

    assert_eq!(first.status, ResultStatus::Completed);
    assert_eq!(node_ids(&first), vec!["a", "b"]);
    assert_eq!(second.status, ResultStatus::Completed);
    assert_eq!(initialized.load(Ordering::SeqCst), 1, "initialized once");
    assert_eq!(
        *nodes.lock().expect("not poisoned"),
        vec!["a", "b", "a", "b"],
        "the hook registered by the plugin fires for every node of both runs"
    );
}

// ---- cancellation ----

/// Agent whose tool waits for the cancel token and records which side won.
fn waiting(id: &str, outcome: Arc<Mutex<Option<&'static str>>>, final_turn: MockResponse) -> Agent {
    let tool = Arc::new(
        FunctionTool::builder("wait")
            .description("Waits, unless cancelled")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, ctx| {
                let outcome = Arc::clone(&outcome);
                async move {
                    let reached = tokio::select! {
                        () = tokio::time::sleep(Duration::from_millis(500)) => "slept",
                        () = ctx.cancel.cancelled() => "cancelled",
                    };
                    *outcome.lock().expect("not poisoned") = Some(reached);
                    Ok(json!(reached))
                }
            }),
    );
    Agent::builder()
        .id(id)
        .model(MockMessageModel::shared(vec![calling("wait"), final_turn]))
        .tool(tool)
        .build()
        .expect("agent builds")
}

#[tokio::test]
async fn cancelling_through_the_invoke_options_stops_the_in_flight_tool_and_skips_the_rest() {
    let outcome = Arc::new(Mutex::new(None));
    let downstream = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("from b"))]);
    let graph = Graph::builder()
        .node(waiting(
            "a",
            Arc::clone(&outcome),
            MockResponse::message(Message::assistant("from a")),
        ))
        .node(
            Agent::builder()
                .id("b")
                .model(Arc::clone(&downstream))
                .build()
                .expect("agent builds"),
        )
        .edge("a", "b")
        .max_steps(5)
        .build()
        .expect("graph builds");

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
    });

    let error = graph
        .invoke_with("go", MultiAgentInvokeOptions::default().with_cancel(cancel))
        .await
        .expect_err("the external signal trips the graph");

    assert_eq!(
        error.to_string(),
        "graph_id=<graph> | graph cancelled by external signal"
    );
    assert_eq!(
        *outcome.lock().expect("not poisoned"),
        Some("cancelled"),
        "the signal reaches the tool running inside the node"
    );
    assert_eq!(downstream.call_count(), 0, "the downstream node never starts");
}

#[tokio::test]
#[ignore = "bug: Swarm::stream_with spawns a detached task with no CancelOnDrop guard \
            (src/multiagent/swarm.rs:241-259), so dropping the stream leaves the run going; \
            Graph does cancel (src/multiagent/graph.rs:512-525, 1200-1211)"]
async fn dropping_a_swarm_stream_mid_run_cancels_the_node_in_flight() {
    let outcome = Arc::new(Mutex::new(None));
    let swarm = Swarm::builder()
        .node(waiting(
            "a",
            Arc::clone(&outcome),
            handoff_turn(json!({ "message": "done" })),
        ))
        .max_steps(2)
        .build()
        .expect("swarm builds");

    let mut stream = swarm.stream("go");
    while let Some(event) = stream.next().await {
        if matches!(
            event.expect("stream event"),
            MultiAgentStreamEvent::BeforeNodeCall(_)
        ) {
            break;
        }
    }
    drop(stream);

    // The tool's own sleep is 500 ms; dropping the stream has to cut it short.
    // Observed: nothing cancels, the tool sleeps the full 500 ms and the run finishes detached.
    tokio::time::timeout(Duration::from_millis(400), async {
        while outcome.lock().expect("not poisoned").is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the tool observes the cancel");
    assert_eq!(*outcome.lock().expect("not poisoned"), Some("cancelled"));
}

// ---- invocation state ----

#[tokio::test]
async fn the_invocation_state_reaches_a_tool_inside_a_nested_orchestrator() {
    let seen = Arc::new(Mutex::new(None));
    let recorder = Arc::clone(&seen);
    let tool = Arc::new(
        FunctionTool::builder("whoami")
            .description("Reports the caller's tenant")
            .input_schema(json!({ "type": "object", "properties": {} }))
            .handler(move |_input, ctx| {
                let recorder = Arc::clone(&recorder);
                async move {
                    let tenant = ctx.invocation_state.get::<String>("tenant");
                    let tenant = tenant.map(|value| value.to_string());
                    *recorder.lock().expect("not poisoned") = tenant.clone();
                    Ok(json!(tenant))
                }
            }),
    );
    let inner: Arc<dyn MultiAgent> = Arc::new(
        Swarm::builder()
            .id("crew")
            .node(
                Agent::builder()
                    .id("worker")
                    .model(MockMessageModel::shared(vec![
                        calling("whoami"),
                        handoff_turn(json!({ "message": "reported" })),
                    ]))
                    .tool(tool)
                    .build()
                    .expect("agent builds"),
            )
            .max_steps(2)
            .build()
            .expect("swarm builds"),
    );
    let graph = Graph::builder()
        .node(saying("plan", "planned"))
        .node(inner)
        .edge("plan", "crew")
        .max_steps(5)
        .build()
        .expect("graph builds");

    let invocation_state = InvocationState::new();
    invocation_state.insert("tenant", "acme".to_owned());
    let result = graph
        .invoke_with(
            "go",
            MultiAgentInvokeOptions::default().with_invocation_state(invocation_state),
        )
        .await
        .expect("graph runs");

    assert_eq!(result.status, ResultStatus::Completed);
    assert_eq!(node_ids(&result), vec!["plan", "crew"]);
    assert_eq!(
        *seen.lock().expect("not poisoned"),
        Some("acme".to_owned()),
        "the outer invocation state reaches a tool two orchestrators down"
    );
}

#[tokio::test]
async fn dropping_a_graph_stream_mid_run_cancels_the_tool_and_skips_the_rest() {
    let outcome = Arc::new(Mutex::new(None));
    let downstream = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("from b"))]);
    let graph = Graph::builder()
        .node(waiting(
            "a",
            Arc::clone(&outcome),
            MockResponse::message(Message::assistant("from a")),
        ))
        .node(
            Agent::builder()
                .id("b")
                .model(Arc::clone(&downstream))
                .build()
                .expect("agent builds"),
        )
        .edge("a", "b")
        .max_steps(5)
        .build()
        .expect("graph builds");

    let mut stream = graph.stream("go");
    while let Some(event) = stream.next().await {
        if matches!(
            event.expect("stream event"),
            MultiAgentStreamEvent::BeforeNodeCall(_)
        ) {
            break;
        }
    }
    drop(stream);

    tokio::time::timeout(Duration::from_millis(400), async {
        while outcome.lock().expect("not poisoned").is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the tool observes the cancel");
    assert_eq!(*outcome.lock().expect("not poisoned"), Some("cancelled"));
    assert_eq!(downstream.call_count(), 0, "the downstream node never starts");
}

#[tokio::test]
#[ignore = "bug: same as the graph case — Agent::take_snapshot drops interrupt state \
            (src/agent/snapshot.rs:124), so the restored node re-raises the same interrupt"]
async fn a_swarm_interrupt_resumes_on_a_new_instance_through_a_serialized_snapshot() {
    let saved: Arc<Mutex<Option<Snapshot>>> = Arc::new(Mutex::new(None));
    let record = Arc::clone(&saved);
    // The snapshot is taken from the live state handle, the one path that works today
    // (`Swarm::take_snapshot(None, ..)` drops the state — see the test above).
    let first = Swarm::builder()
        .id("approvals")
        .node(approving_in_a_swarm("gate"))
        .max_steps(4)
        .hook::<AfterMultiAgentInvocationEvent, _>(move |event| {
            let snapshot = take_snapshot(
                event.orchestrator.as_ref(),
                Some(&event.state),
                TakeMultiAgentSnapshotOptions::default(),
            )?;
            *record.lock().expect("not poisoned") = Some(snapshot);
            Ok(())
        })
        .build()
        .expect("swarm builds");

    let interrupted = first.invoke("go").await.expect("swarm runs");
    assert_eq!(interrupted.status, ResultStatus::Interrupted);
    let snapshot = through_json(
        saved
            .lock()
            .expect("not poisoned")
            .as_ref()
            .expect("the after-invocation hook saved a snapshot"),
    );

    let second = Swarm::builder()
        .id("approvals")
        .node(approving_in_a_swarm("gate"))
        .max_steps(4)
        .build()
        .expect("swarm builds");
    second.load_snapshot(&snapshot, None).expect("snapshot loads");
    let resumed = second
        .invoke(MultiAgentInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("yes")),
        ]))
        .await
        .expect("swarm resumes");

    assert_eq!(resumed.status, ResultStatus::Completed);
    assert_eq!(text(&resumed), vec!["approved".to_owned()]);
    assert!(resumed.interrupts.is_empty());
}
