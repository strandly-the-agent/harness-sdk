//! Live multi-agent integration tests: a `Graph` and a `Swarm` of real Bedrock-backed agents,
//! plus a graph with a swarm node. `#[ignore]`d; run with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features -p strands-agents --test integ_multiagent -- --ignored --nocapture
//! ```
//!
//! Usable AWS credentials are required: `BedrockModel::new` only builds a client and never
//! resolves them, so a run without credentials fails at the call rather than skipping (same as
//! `integ_bedrock.rs`). Throttling is skipped, not failed — at the invocation level and at the
//! node level, since an orchestrator reports a node's model error as a `FAILED` node result
//! rather than an `Err`.
//!
//! The system prompts cap the reply length on purpose: a swarm agent answers through structured
//! output, and a reply that runs past `max_tokens` fails the node with "Model reached maximum
//! token limit" instead of producing a handoff.

#![cfg(feature = "bedrock")]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use strands_agents::context_manager::ContextManagerOption;
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::multiagent::{
    edge_handler, Graph, MultiAgent, MultiAgentResult, MultiAgentState, MultiAgentStream,
    MultiAgentStreamEvent, NodeInnerEvent, ResultStatus, Swarm,
};
use strands_agents::types::ContentBlock;
use strands_agents::{Agent, Error};

const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";

/// One shared Bedrock client for every agent in a test.
async fn model() -> Arc<BedrockModel> {
    Arc::new(
        BedrockModel::new(BedrockModelConfig {
            model_id: Some(MODEL_ID.to_owned()),
            max_tokens: Some(2048),
            ..Default::default()
        })
        .await
        .expect("bedrock client"),
    )
}

fn agent(id: &str, description: &str, system_prompt: &str, model: &Arc<BedrockModel>) -> Agent {
    Agent::builder()
        .id(id)
        .description(description)
        .system_prompt(system_prompt)
        // The default `Auto` manager adds its retrieval tool and instructions to every node, which
        // derails short prompts like these into asking for a document to retrieve. These tests are
        // about orchestration, so the nodes run with context management off.
        .context_manager(ContextManagerOption::Off)
        .model(Arc::clone(model))
        .build()
        .expect("agent builds")
}

fn node_ids(result: &MultiAgentResult) -> Vec<&str> {
    result.results.iter().map(|r| r.node_id.as_str()).collect()
}

fn text(result: &MultiAgentResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the node content in `state` has any text, read from an edge handler.
fn produced_text(state: &MultiAgentState, node_id: &str) -> bool {
    state.node(node_id).is_some_and(|node| {
        node.content()
            .iter()
            .any(|block| matches!(block, ContentBlock::Text(text) if !text.trim().is_empty()))
    })
}

/// Drains `stream`, printing every orchestrator result it carries — the nested one included.
///
/// A nested orchestrator keeps its node errors on its own result, which `MultiAgentNode` does not
/// forward, so a throttled nested node is re-raised here as `ModelThrottled` to stay skippable.
async fn run_and_report(stream: MultiAgentStream) -> Result<MultiAgentResult, Error> {
    let mut stream = stream;
    let mut result = None;
    let mut nested_throttle = None;
    while let Some(event) = stream.next().await {
        match event? {
            MultiAgentStreamEvent::MultiAgentResult(event) => {
                eprintln!("result: {:#?}", event.result);
                result = Some(event.result);
            }
            MultiAgentStreamEvent::NodeStreamUpdate(event) => {
                if let NodeInnerEvent::MultiAgent(inner) = &event.inner {
                    if let MultiAgentStreamEvent::MultiAgentResult(inner) = inner.as_ref() {
                        eprintln!("nested result from node {}: {:#?}", event.node_id, inner.result);
                        nested_throttle = nested_throttle.take().or_else(|| throttled(&inner.result));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(message) = nested_throttle {
        return Err(Error::throttled(message));
    }
    result.ok_or_else(|| Error::other("the stream ended without a result"))
}

/// The first node error in `result` that reads like provider throttling.
fn throttled(result: &MultiAgentResult) -> Option<String> {
    result
        .results
        .iter()
        .filter_map(|node| node.error.as_deref())
        .find(|error| {
            let error = error.to_lowercase();
            error.contains("throttl") || error.contains("too many requests")
        })
        .map(str::to_owned)
}

/// `None` when Bedrock throttled the run, which is a skip rather than a failure.
fn unwrap_or_skip(outcome: Result<MultiAgentResult, Error>) -> Option<MultiAgentResult> {
    let result = match outcome {
        Ok(result) => result,
        Err(error @ Error::ModelThrottled { .. }) => {
            eprintln!("skipping: Bedrock throttled the request ({error})");
            return None;
        }
        Err(error) => panic!("live orchestration failed: {error}"),
    };

    // A node's model error arrives as a string on its result, so throttling is matched by text.
    if let Some(error) = throttled(&result) {
        eprintln!("skipping: Bedrock throttled a node ({error})");
        return None;
    }
    Some(result)
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn a_three_node_graph_routes_through_a_conditional_edge() {
    let model = model().await;
    let graph = Graph::builder()
        .node(agent(
            "researcher",
            "Gathers facts",
            "List exactly two short factual bullet points about the topic. No preamble.",
            &model,
        ))
        .node(agent(
            "writer",
            "Writes the summary",
            "Write one sentence summarising the research you are given. No preamble.",
            &model,
        ))
        .node(agent(
            "reviewer",
            "Reviews the summary",
            "Reply with APPROVED followed by at most one short sentence of feedback.",
            &model,
        ))
        .node(agent(
            "fallback",
            "Handles an empty draft",
            "Say that there was nothing to review.",
            &model,
        ))
        .edge("researcher", "writer")
        // Only one of the two branches is taken, decided from what the writer actually produced.
        .edge_when(
            "writer",
            "reviewer",
            edge_handler(|state| produced_text(state, "writer")),
        )
        .edge_when(
            "writer",
            "fallback",
            edge_handler(|state| !produced_text(state, "writer")),
        )
        .max_steps(6)
        .timeout(Duration::from_secs(180))
        .build()
        .expect("graph builds");

    let Some(result) = unwrap_or_skip(graph.invoke("The Rust ownership model").await) else {
        return;
    };
    eprintln!("graph result: {result:#?}");

    assert_eq!(result.status, ResultStatus::Completed, "{:?}", result.results);
    assert_eq!(
        node_ids(&result),
        vec!["researcher", "writer", "reviewer"],
        "the false branch is never scheduled"
    );
    assert!(
        result
            .results
            .iter()
            .all(|node| node.status == ResultStatus::Completed),
        "{:?}",
        result.results
    );
    assert!(
        text(&result).to_uppercase().contains("APPROVED"),
        "the reviewer's reply is the graph's content: {:?}",
        text(&result)
    );
    assert!(result.usage.total_tokens > 0, "Bedrock reports token usage");
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn a_two_agent_swarm_hands_off_to_the_specialist() {
    let model = model().await;
    let swarm = Swarm::builder()
        .id("support")
        .node(agent(
            "triage",
            "Routes incoming questions; cannot answer them",
            "You are triage and you never answer questions yourself. Always hand off to the \
             `specialist` agent: set agentId to \"specialist\" and put the question in message.",
            &model,
        ))
        .node(agent(
            "specialist",
            "Answers technical questions",
            "Answer the question in one short sentence. Never hand off; leave agentId unset.",
            &model,
        ))
        .entry_point("triage")
        .max_steps(4)
        .timeout(Duration::from_secs(180))
        .build()
        .expect("swarm builds");

    let Some(result) = unwrap_or_skip(swarm.invoke("Why does Rust move a value when I assign it?").await)
    else {
        return;
    };
    eprintln!("swarm result: {result:#?}");

    assert_eq!(result.status, ResultStatus::Completed, "{:?}", result.results);
    assert_eq!(
        node_ids(&result),
        vec!["triage", "specialist"],
        "triage hands off exactly once"
    );
    assert_eq!(
        result.results[0]
            .structured_output
            .as_ref()
            .and_then(|output| output.get("agentId"))
            .and_then(serde_json::Value::as_str),
        Some("specialist"),
        "the handoff decision is on triage's result: {:?}",
        result.results[0].structured_output
    );
    assert!(!text(&result).trim().is_empty(), "the specialist answers");
    assert!(result.usage.total_tokens > 0, "Bedrock reports token usage");
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn a_graph_runs_a_swarm_node() {
    let model = model().await;
    let crew: Arc<dyn MultiAgent> = Arc::new(
        Swarm::builder()
            .id("crew")
            .node(agent(
                "drafter",
                "Writes the first draft",
                "Write a one-sentence draft (at most 25 words) from the plan you are given, then hand \
                 off to the `editor` agent: set agentId to \"editor\" and put the draft in message.",
                &model,
            ))
            .node(agent(
                "editor",
                "Polishes the draft",
                "Reply with the polished draft only (at most 30 words), prefixed by FINAL:. Never \
                 hand off; leave agentId unset.",
                &model,
            ))
            .entry_point("drafter")
            .max_steps(4)
            .timeout(Duration::from_secs(180))
            .build()
            .expect("swarm builds"),
    );
    let graph = Graph::builder()
        .node(agent(
            "planner",
            "Plans the post",
            "Reply with a two-line plan for a short post about the topic. No preamble.",
            &model,
        ))
        .node(crew)
        .edge("planner", "crew")
        .max_steps(4)
        .timeout(Duration::from_secs(240))
        .build()
        .expect("graph builds");

    // Streamed, not invoked: a nested orchestrator's own result is only visible on the stream
    // (`MultiAgentNode` forwards the content and status but not the inner node results), so a
    // failure inside the swarm would otherwise arrive as a bare `FAILED` with no error.
    let Some(result) = unwrap_or_skip(run_and_report(graph.stream("Why borrow checking is worth it")).await)
    else {
        return;
    };

    assert_eq!(result.status, ResultStatus::Completed, "{:?}", result.results);
    assert_eq!(node_ids(&result), vec!["planner", "crew"]);
    assert!(
        text(&result).to_uppercase().contains("FINAL"),
        "the nested swarm's last agent produces the graph's content: {:?}",
        text(&result)
    );
    assert!(
        result.usage.total_tokens > 0,
        "usage aggregates through the nested orchestrator"
    );
}
