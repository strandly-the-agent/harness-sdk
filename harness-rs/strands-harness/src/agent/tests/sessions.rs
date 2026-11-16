//! The session cases of `harness-ts/test/agent.test.ts` that run an invocation.

use std::sync::Mutex;

use serde_json::json;
use strands::hooks::BeforeModelCallEvent;
use strands::types::messages::{ContentBlock, TextBlock, ToolResultContent, ToolUseBlock};

use super::*;

fn session_in(id: &str, dir: &Path) -> SessionOption {
    SessionOption::Config(SessionConfig {
        id: Some(id.to_owned()),
        dir: Some(dir.to_string_lossy().into_owned()),
    })
}

// TS stops the invocation by throwing from the hook, before its Bedrock model is called; a Rust
// hook cannot fail, and the mock model just answers.
#[tokio::test]
async fn persists_messages_before_the_invocation_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let (model, mut options) = shared_mock();
    model.add_turn(vec![TextBlock::new("noted").into()]);
    options.session = session_in("checkpoint", dir.path());
    options.builtin_tools = names(&[]);
    options.builtin_plugins = Some(Vec::new());
    options.context_manager = Some(ContextManagerStrategy::Off);
    options.memory = MemoryOption::Off;

    let mut agent = create_harness_with(options.clone(), seams(&dir)).await.unwrap();
    let restored_messages = Arc::new(Mutex::new(None));
    agent.add_async_hook::<BeforeModelCallEvent>({
        let (seams, restored_messages) = (seams(&dir), Arc::clone(&restored_messages));
        move |_event| {
            let (options, seams) = (options.clone(), seams.clone());
            let restored_messages = Arc::clone(&restored_messages);
            Box::pin(async move {
                let mut restored = create_harness_with(options, seams).await.unwrap();
                restored.initialize().await.unwrap();
                let messages = serde_json::to_string(restored.messages()).unwrap();
                *restored_messages.lock().unwrap() = Some(messages);
            })
        }
    });
    agent.invoke("Remember the checkpoint").await.unwrap();

    let restored = restored_messages.lock().unwrap().take().expect("the hook ran");
    assert!(restored.contains("Remember the checkpoint"), "{restored}");
}

fn large_result() -> String {
    let lines: Vec<String> = (0..400)
        .map(|line| format!("Line {line}: quarterly budget notes."))
        .collect();
    lines.join("\n")
}

/// The text of the tool result that ends `request`.
fn tool_result_text(request: &strands::models::model::ModelRequest) -> String {
    let message = request.messages.last().expect("a message");
    let result = message
        .content()
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .expect("a tool result");
    let texts: Vec<String> = result
        .content()
        .iter()
        .map(|item| match item {
            ToolResultContent::Text(text) => text.text().to_owned(),
            other => serde_json::to_string(other).unwrap(),
        })
        .collect();
    texts.join("\n")
}

// https://github.com/strands-agents/harness-sdk/issues/4685
// TS drives this with a model that reads the stash reference out of the first tool result; the
// reference is `<tool use id>_<index>`, so the scripted model names it up front and the case
// checks that the truncated result carried it.
#[tokio::test]
async fn returns_an_offloaded_tool_result_to_the_model_in_full_on_retrieval() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let (model, mut options) = shared_mock();
    model.update_config(strands::models::model::BaseModelConfig {
        context_window_limit: Some(1_000_000),
        ..Default::default()
    });
    model.add_turn(vec![
        ToolUseBlock::new("read_large", "tooluse-0", json!({})).into(),
    ]);
    let retrieve = json!({ "reference": "tooluse-0_0" });
    model.add_turn(vec![
        ToolUseBlock::new("retrieve_context", "tooluse-1", retrieve).into(),
    ]);
    model.add_turn(vec![TextBlock::new("done").into()]);

    let read_large = FunctionTool::new(
        "read_large",
        "Read a large document.",
        |_input: JsonValue, _context| async move { Ok::<_, ToolError>(large_result()) },
    );
    options.tools = vec![ToolSource::Tool(Arc::new(read_large))];
    options.session = session_in("retrieval", &sessions);
    options.builtin_tools = names(&[]);
    options.builtin_plugins = Some(Vec::new());
    options.memory = MemoryOption::Off;
    options.skills = SkillsOption::Off;
    options.agent_config.printer = Some(false);
    let mut agent = create_harness_with(options, seams(&dir)).await.unwrap();
    agent.invoke("Read the document.").await.unwrap();

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    let truncated = tool_result_text(&requests[1]);
    assert!(truncated.starts_with("[Truncated:"), "{truncated}");
    assert!(truncated.contains("[ref: tooluse-0_0]"), "{truncated}");
    assert_eq!(tool_result_text(&requests[2]), large_result());

    assert!(
        agent
            .context_manager()
            .is_some_and(ContextManager::stash_is_durable)
    );
    let scope = Path::new("scopes").join("agent").join(agent.id());
    let snapshot = sessions
        .join("retrieval")
        .join(&scope)
        .join("snapshots")
        .join("snapshot_latest.json");
    let snapshot: JsonValue = serde_json::from_str(&std::fs::read_to_string(snapshot).unwrap()).unwrap();
    assert_eq!(
        snapshot["data"]["stash"],
        json!({ "location": "external", "storageType": "LocalFileStorage" })
    );
    let stashed = sessions
        .join("retrieval")
        .join("context")
        .join("retrieval")
        .join(&scope)
        .join("tooluse-0_0");
    assert!(stashed.exists(), "{stashed:?} missing");
    // Every folder under the session dir is read as a session, so the stash stays inside this one.
    let folders: Vec<_> = std::fs::read_dir(&sessions)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(folders, ["retrieval"]);
}
