//! Live end-to-end tests for the harness defaults (port of `harness-ts/test/integration/e2e.test.ts`).
//! `#[ignore]`d; run with AWS credentials:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features -p strands-harness --test integ_harness -- --ignored --test-threads=1
//! ```
//!
//! Each test runs in its own temp directory passed explicitly as session/memory/environment root
//! (the Rust tools resolve paths against the process cwd, which tests must not change).

#![cfg(feature = "bedrock")]

use std::sync::Arc;

use strands_agents::memory::MemoryManager;
use strands_harness::{
    create_harness, resolve_memory, resolve_model, HarnessAgentOptions, MemoryOption, ResolveMemoryOptions,
    ResolveModelOptions, SessionConfig, SessionOption, SkillsOption,
};

async fn memory_manager(dir: &std::path::Path) -> Arc<MemoryManager> {
    let model = resolve_model(ResolveModelOptions::default()).await.unwrap();
    resolve_memory(ResolveMemoryOptions {
        dir: Some(dir.to_path_buf()),
        model: Some(model),
        ..Default::default()
    })
    .await
    .unwrap()
}

const TESTING: &str = "You are an automated integration test. Do exactly what is asked and nothing else.";

fn base(dir: &std::path::Path) -> HarnessAgentOptions {
    HarnessAgentOptions {
        instructions: Some(TESTING.to_owned()),
        session: SessionOption::Config(SessionConfig {
            id: None,
            dir: Some(dir.join("sessions")),
        }),
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        ..Default::default()
    }
}

fn used_tool(agent: &strands_agents::Agent, name: &str) -> bool {
    agent
        .messages()
        .iter()
        .any(|m| m.tool_uses().any(|t| t.name == name))
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn uses_the_shell_tool() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("marker.txt"), "hello").unwrap();
    let agent = create_harness(base(dir.path())).await.unwrap();
    let result = agent
        .invoke(format!(
            "{TESTING} Use the shell tool to run `ls {}` and report the file names you see.",
            dir.path().display()
        ))
        .await
        .unwrap();
    assert!(
        used_tool(&agent, "shell"),
        "shell tool was not used: {:?}",
        agent.messages()
    );
    assert!(result.text().contains("marker.txt"), "{}", result.text());
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn uses_the_todo_write_tool_from_the_todos_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let agent = create_harness(base(dir.path())).await.unwrap();
    agent
        .invoke(format!(
            "{TESTING} Call the todo_write tool once with two items: 'first' (in_progress) and 'second' (pending). Then reply 'done'."
        ))
        .await
        .unwrap();
    assert!(used_tool(&agent, "todo_write"), "{:?}", agent.messages());
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn uses_the_file_tools_in_one_turn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    let agent = create_harness(base(dir.path())).await.unwrap();
    agent
        .invoke(format!(
            "{TESTING} Step 1: use the write tool to create {p} with the content 'alpha'. Step 2: use the edit tool to replace 'alpha' with 'beta' in {p}. Step 3: use the read tool to read {p}. Then reply with its content.",
            p = path.display()
        ))
        .await
        .unwrap();
    for tool in ["write", "edit", "read"] {
        assert!(used_tool(&agent, tool), "{tool} not used: {:?}", agent.messages());
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "beta");
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn persists_sessions_across_agents() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path().join("sessions");
    let session = SessionOption::Config(SessionConfig {
        id: Some("e2e-session".into()),
        dir: Some(session_dir.clone()),
    });

    let first = create_harness(HarnessAgentOptions {
        session: session.clone(),
        ..base(dir.path())
    })
    .await
    .unwrap();
    first
        .invoke(format!("{TESTING} Remember the code word PINEAPPLE. Reply 'ok'."))
        .await
        .unwrap();
    assert!(std::path::Path::new(&session_dir).join("e2e-session").is_dir());

    let second = create_harness(HarnessAgentOptions {
        session,
        ..base(dir.path())
    })
    .await
    .unwrap();
    assert!(second.messages().len() >= 2, "history restored from the session");
    let result = second
        .invoke(format!(
            "{TESTING} What is the code word? Reply with just the word."
        ))
        .await
        .unwrap();
    assert!(result.text().contains("PINEAPPLE"), "{}", result.text());
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn persists_a_session_with_a_minted_id() {
    let dir = tempfile::tempdir().unwrap();
    let agent = create_harness(base(dir.path())).await.unwrap();
    agent.invoke(format!("{TESTING} Reply 'ok'.")).await.unwrap();
    let minted: Vec<_> = std::fs::read_dir(dir.path().join("sessions"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() == 8 && n.chars().all(|c| c.is_ascii_hexdigit()))
        .collect();
    assert_eq!(minted.len(), 1, "{minted:?}");
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn persists_memory_across_agents() {
    let dir = tempfile::tempdir().unwrap();
    let memory_dir = dir.path().join("memory");
    let manager = memory_manager(&memory_dir).await;
    let first = create_harness(HarnessAgentOptions {
        memory: MemoryOption::Instance(Arc::clone(&manager)),
        session: SessionOption::Off,
        ..base(dir.path())
    })
    .await
    .unwrap();
    first
        .invoke(format!(
            "{TESTING} Remember this durable fact about me: my favorite fruit is TANGERINE."
        ))
        .await
        .unwrap();
    manager.flush().await.unwrap();
    let distilled: String = std::fs::read_dir(&memory_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(
        distilled.to_uppercase().contains("TANGERINE"),
        "memory file should contain the fact: {distilled:?}"
    );

    let second_manager = memory_manager(&memory_dir).await;
    let second = create_harness(HarnessAgentOptions {
        memory: MemoryOption::Instance(second_manager),
        session: SessionOption::Off,
        ..base(dir.path())
    })
    .await
    .unwrap();
    let result = second
        .invoke(format!("{TESTING} What is my favorite fruit?"))
        .await
        .unwrap();
    assert!(
        result.text().to_uppercase().contains("TANGERINE"),
        "{}",
        result.text()
    );
}

/// A scripted `ask` that denies every approval request: the model must report the denial instead of
/// running the shell tool.
#[tokio::test]
#[ignore = "live Bedrock"]
async fn interventions_deny_the_shell_tool() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use strands_agents::interventions::vended::hitl::AskFn;
    use strands_agents::types::JsonValue;
    use strands_harness::InterventionsOption;

    let dir = tempfile::tempdir().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&asked);
    let ask: AskFn = Arc::new(move |_prompt: String| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(JsonValue::String("n".to_string()))
        })
    });

    let agent = create_harness(HarnessAgentOptions {
        interventions: InterventionsOption::presets(["ask"]),
        interventions_ask: Some(ask),
        ..base(dir.path())
    })
    .await
    .unwrap();

    let result = agent
        .invoke(format!(
            "{TESTING} Run the shell command `echo hello` with the shell tool, then tell me what happened."
        ))
        .await
        .unwrap();

    assert!(
        asked.load(Ordering::SeqCst) > 0,
        "the ask callback should have run"
    );
    // Deterministic evidence: the tool call was blocked (an error tool result, no shell output),
    // not the model's wording about it.
    let history = agent.messages();
    let shell_result = history
        .iter()
        .flat_map(|m| m.tool_results())
        .next()
        .expect("the blocked shell call leaves a tool result in history");
    assert_eq!(
        shell_result.status,
        strands_agents::types::ToolResultStatus::Error,
        "{shell_result:?}"
    );
    assert!(
        !shell_result.text().contains("hello\n"),
        "shell must not have run: {shell_result:?}"
    );
    assert!(!result.text().is_empty());
}
