//! The `subagent delegate (built child)` cases of `harness-ts/test/agent.test.ts` that read the
//! built child itself, and the memory-forwarding case of `agent-generalist-memory.test.ts`.
//!
//! TS calls the tool's private `builder`; here the real `subagent` tool runs, with a `BuildAgent`
//! that builds the real child, keeps it, and hands the tool a mock-model stand-in to run.

use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use futures::StreamExt;
use serde_json::json;
use strands::agent::MemoryManagerSource;
use strands::memory::{MemoryEntry, MemorySearchOptions, MemoryStore, SearchOptions};
use strands::testing::tool_context;
use strands::types::messages::TextBlock;
use strands::types::tools::ToolUse;

use super::provider_models::{env, model, openai_config, params};
use super::*;

/// The delegate the `subagent` tool built, and what its model was resolved with.
struct Child {
    agent: Agent,
    resolved: ResolveModelOptions,
}

/// Runs the `subagent` tool of a harness built from `options` once with `input`.
async fn built_child(options: HarnessAgentOptions, input: JsonValue) -> Child {
    let dir = tempfile::tempdir().unwrap();
    let resolved = Arc::new(Mutex::new(None));
    let parent_seams = Seams {
        base_dir: Some(dir.path().to_owned()),
        env: env(),
        on_resolve_model: None,
    };
    let child_seams = Seams {
        on_resolve_model: Some(Arc::new({
            let resolved = Arc::clone(&resolved);
            move |options| *resolved.lock().unwrap() = Some(options)
        })),
        ..parent_seams.clone()
    };
    let children = Arc::new(Mutex::new(Vec::new()));
    let build_agent: BuildAgent = {
        let (children, stand_in_seams) = (Arc::clone(&children), parent_seams.clone());
        Arc::new(move |child: HarnessAgentOptions| {
            let (children, child_seams, stand_in_seams) =
                (Arc::clone(&children), child_seams.clone(), stand_in_seams.clone());
            Box::pin(async move {
                let agent = create_harness_with(child, child_seams).await?;
                children.lock().unwrap().push(agent);
                // The child may hold a provider model, so a mock-model agent runs in its place.
                let stand_in = Arc::new(MockMessageModel::new());
                stand_in.add_turn(vec![TextBlock::new("done").into()]);
                let stand_in = HarnessAgentOptions {
                    model: Some(ModelSource::Model(stand_in)),
                    session: SessionOption::Off,
                    memory: MemoryOption::Off,
                    skills: SkillsOption::Off,
                    builtin_tools: names(&[]),
                    ..Default::default()
                };
                create_harness_with(stand_in, stand_in_seams).await
            })
        })
    };
    let parent = create_harness_from(options, parent_seams, build_agent)
        .await
        .unwrap();
    let subagent = Arc::clone(parent.tools().find(|tool| tool.name() == "subagent").unwrap());
    let mut stream = subagent.stream(tool_context(ToolUse::new("subagent", "t1", input)));
    while stream.next().await.is_some() {}
    let mut children = children.lock().unwrap();
    assert_eq!(children.len(), 1, "one delegate was built");
    let resolved = resolved
        .lock()
        .unwrap()
        .take()
        .expect("the child's model was resolved");
    Child {
        agent: children.remove(0),
        resolved,
    }
}

#[tokio::test]
async fn registers_the_inherited_interventions_gate_on_the_built_child() {
    let options = HarnessAgentOptions {
        interventions: Some("ask".into()),
        ..mock_options()
    };
    let child = built_child(options, json!({ "task": "x" })).await;
    let names: Vec<&str> = child
        .agent
        .interventions()
        .map(|handler| handler.name())
        .collect();
    assert_eq!(names, [HumanInTheLoop::NAME]);
}

// The model, not the registry, serves native search, so narrowing to `read` leaves it out of the
// selection; the parent's setting still reaches the delegate, which resolves it for its model.
#[tokio::test]
async fn carries_explicit_web_search_onto_a_narrowed_delegate() {
    let spec = "openai/gpt-5.6-sol";
    let options = HarnessAgentOptions {
        builtin_tools: names(&[
            BuiltinToolName::Read,
            BuiltinToolName::WebSearch,
            BuiltinToolName::Subagent,
        ]),
        ..model(spec)
    };
    let child = built_child(options, json!({ "task": "x", "tools": ["read"] })).await;
    assert_eq!(tool_names(&child.agent), ["read"]);
    let config = openai_config(&child.agent, spec, child.resolved);
    assert_eq!(params(&config)["tools"], json!([{ "type": "web_search" }]));
}

#[tokio::test]
async fn keeps_native_search_on_a_narrowed_delegate_of_a_default_configured_parent() {
    let spec = "openai/gpt-5.6-sol";
    let child = built_child(model(spec), json!({ "task": "x", "tools": ["read"] })).await;
    let config = openai_config(&child.agent, spec, child.resolved);
    assert_eq!(params(&config)["tools"], json!([{ "type": "web_search" }]));
}

/// A consumer store that counts its searches.
struct CustomStore(AtomicUsize);

#[strands::async_trait]
impl MemoryStore for CustomStore {
    fn name(&self) -> &str {
        "custom"
    }

    fn writable(&self) -> bool {
        true
    }

    async fn search(&self, _: &str, _: SearchOptions) -> strands::Result<Vec<MemoryEntry>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn supports_add(&self) -> bool {
        true
    }

    async fn add(&self, _: &str, _: Option<&strands::types::json::JsonObject>) -> strands::Result<()> {
        Ok(())
    }
}

/// `agent-generalist-memory.test.ts`. TS reads the stores off the parent record; Rust hands the
/// child a recall-only manager over them, so the case searches it and counts on the parent's store.
#[tokio::test]
async fn forwards_the_parent_memory_stores_to_the_delegate_so_it_shares_the_same_backend() {
    let store = Arc::new(CustomStore(AtomicUsize::new(0)));
    let (model, mut options) = shared_mock();
    options.memory = MemoryOption::Config(MemoryConfig {
        stores: Some(vec![Arc::clone(&store) as Arc<dyn MemoryStore>]),
        ..Default::default()
    });
    let child = child_options_of(options, &model).await;
    let Some(MemoryManagerSource::Manager(manager)) = child.agent_config.memory_manager else {
        panic!("the delegate has no memory manager");
    };
    // The delegate's own run may already have recalled from the store.
    let before = store.0.load(Ordering::SeqCst);
    manager
        .search("anything", MemorySearchOptions::default())
        .await
        .unwrap();
    assert_eq!(store.0.load(Ordering::SeqCst), before + 1);
}
