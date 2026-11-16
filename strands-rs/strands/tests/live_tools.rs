//! Live vended-tool and skills tests against Bedrock (TS checklist:
//! strands-ts/test/integ/tools/bash.test.node.ts, tools/http-request.test.ts,
//! tools/notebook.test.ts and skills/agent-skills.test.node.ts). The TS specs assert on the
//! final wording; these assert on the trace (tool name, parsed input, tool-result content) and
//! on the state the tools write.
//!
//! The `http_request` test asks for a URL served by a localhost server the test starts, where TS
//! calls Open-Meteo: the tool path is the same and the result content is exact.
//!
//! The bash tool runs model-chosen commands on the host with the process environment; the
//! prompts name the exact commands and never ask for the environment.
//!
//! They use the default AWS credential chain and skip with a message when no credentials
//! resolve. Run with `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(feature = "bedrock")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod live_support;

use live_support::{
    bedrock, bedrock_with, credentials_available, last_assistant_text, results_of, tool_attempted,
    tool_result_contains, tool_succeeded, tool_uses, trace,
};
use serde_json::{Value, json};
use strands::vended_plugins::skills::{AgentSkills, AgentSkillsConfig, Skill, SkillConfig};
use strands::vended_tools::{bash, http_request, notebook};
use strands::{Agent, StopReason};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// TS cases (bash): `captures stdout streams correctly`, `captures stderr streams correctly`,
/// `handles complex command patterns`, `handles command errors gracefully`.
#[cfg(unix)]
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_bash_captures_stdout_and_stderr_and_handles_command_errors() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(500))
        .tools([bash()])
        .printer(false)
        .build()
        .unwrap();

    let result = agent
        .invoke("Use bash to echo \"Hello from bash\"")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    let call = tool_uses(&messages)
        .into_iter()
        .find(|tool_use| tool_use["name"] == "bash")
        .expect("a bash call");
    assert!(
        call["input"]["command"]
            .as_str()
            .unwrap()
            .contains("Hello from bash")
    );
    assert!(tool_succeeded(&messages, "bash"));
    assert!(tool_result_contains(&messages, "bash", "Hello from bash"));

    agent
        .invoke("Use bash to run: echo \"stderr-marker\" >&2")
        .await
        .unwrap();
    assert!(tool_result_contains(&trace(&agent), "bash", "stderr-marker"));

    agent
        .invoke("Use bash to: create a variable TEST=hello-variable, then echo it")
        .await
        .unwrap();
    assert!(tool_result_contains(&trace(&agent), "bash", "hello-variable"));

    let before = results_of(&trace(&agent), "bash").len();
    let result = agent
        .invoke("Use bash to run: nonexistent_command_xyz")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    let results = results_of(&messages, "bash");
    assert!(results.len() > before);
    let failed = results[before..]
        .iter()
        .map(|result| result["content"].to_string())
        .collect::<String>();
    assert!(failed.contains("not found") || failed.contains("127"), "{failed}");
    agent.shutdown().await.unwrap();
}

/// Serves one fixed JSON document to every request on a localhost port.
async fn forecast_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/forecast", listener.local_addr().unwrap());
    let body = r#"{"city":"Testville","temperature_celsius":17.5,"conditions":"marker-light-drizzle"}"#;
    let server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut head = [0_u8; 4096];
            let _ = socket.read(&mut head).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (url, server)
}

/// TS case (http-request): `agent uses http_request tool to fetch weather from Open-Meteo`,
/// against a localhost server.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_agent_uses_http_request_tool_to_fetch_a_forecast() {
    if !credentials_available().await {
        return;
    }
    let (url, server) = forecast_server().await;
    let mut agent = Agent::builder()
        .model(bedrock(500))
        .tools([http_request()])
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke(format!(
            "Send a GET request to {url} and tell me the conditions it reports."
        ))
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(
        serde_json::to_value(result.last_message()).unwrap()["role"],
        "assistant"
    );
    let messages = trace(&agent);
    let call = tool_uses(&messages)
        .into_iter()
        .find(|tool_use| tool_use["name"] == "http_request")
        .expect("an http_request call");
    assert_eq!(
        call["input"]["method"].as_str().map(str::to_uppercase).as_deref(),
        Some("GET")
    );
    assert_eq!(call["input"]["url"], json!(url));
    assert!(tool_succeeded(&messages, "http_request"));
    assert!(tool_result_contains(
        &messages,
        "http_request",
        "marker-light-drizzle"
    ));
    assert!(last_assistant_text(&messages).contains("drizzle"));
    agent.shutdown().await.unwrap();
    server.abort();
}

fn notebooks(agent: &Agent) -> Value {
    agent.app_state().get_value("notebooks").unwrap_or(Value::Null)
}

fn notebook_agent(app_state: Option<Value>) -> Agent {
    let builder = Agent::builder()
        .model(bedrock(500))
        .system_prompt("Use the notebook tool for every notebook request.")
        .tools([notebook()])
        .printer(false);
    match app_state {
        Some(state) => builder
            .app_state(state.as_object().unwrap().clone())
            .build()
            .unwrap(),
        None => builder.build().unwrap(),
    }
}

/// TS cases (notebook): `should persist notebook state across tool invocations`, `should restore
/// state across agent instances`, `should handle errors gracefully`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_notebook_persists_state_across_invocations_and_agent_instances() {
    if !credentials_available().await {
        return;
    }
    let mut agent = notebook_agent(None);
    agent
        .invoke("Create a notebook called \"test\" with content \"# Test Notebook\"")
        .await
        .unwrap();
    assert!(tool_succeeded(&trace(&agent), "notebook"));
    assert!(
        notebooks(&agent)["test"]
            .as_str()
            .unwrap()
            .contains("# Test Notebook")
    );

    agent
        .invoke("Add \"- First item\" to the test notebook")
        .await
        .unwrap();
    let text = notebooks(&agent)["test"].as_str().unwrap().to_owned();
    assert!(
        text.contains("# Test Notebook") && text.contains("- First item"),
        "{text}"
    );

    let saved = Value::Object(agent.app_state().get_all());
    agent.shutdown().await.unwrap();

    // A second agent built from the saved state reads the same notebook.
    let mut restored = notebook_agent(Some(saved));
    assert!(
        notebooks(&restored)["test"]
            .as_str()
            .unwrap()
            .contains("- First item")
    );
    let result = restored.invoke("Read the test notebook").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&restored);
    assert!(tool_result_contains(&messages, "notebook", "- First item"));

    // Reading a notebook that does not exist is a tool error the agent recovers from.
    let before = results_of(&messages, "notebook").len();
    let result = restored
        .invoke("Read a notebook called \"nonexistent\"")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&restored);
    assert!(results_of(&messages, "notebook").len() > before);
    assert!(!last_assistant_text(&messages).is_empty());
    assert!(notebooks(&restored).get("nonexistent").is_none());
    restored.shutdown().await.unwrap();
}

const SECRET_CODEWORD: &str = "PINEAPPLE-7392";
const ALT_SECRET_CODEWORD: &str = "MANGO-5518";

fn skill(name: &str, description: &str, instructions: &str) -> Skill {
    Skill::new(
        SkillConfig::builder()
            .name(name)
            .description(description)
            .instructions(instructions)
            .build(),
    )
}

fn system_prompt(agent: &Agent) -> String {
    serde_json::to_string(&agent.system_prompt()).unwrap()
}

/// TS cases (agent-skills): `activates a skill via prompt and includes the secret codeword`,
/// `tracks activated skills in agent appState`, `replaces the skills block with updated content
/// between invocations`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_skills_activates_a_skill_tracks_it_in_app_state_and_replaces_the_skills_block() {
    if !credentials_available().await {
        return;
    }
    let summarization = skill(
        "summarization",
        "Summarize text into concise bullet points",
        &format!(
            "You are a summarization expert. When this skill is activated, summarize the provided text. \
             Always include the codeword {SECRET_CODEWORD} in your response."
        ),
    );
    let translation = skill(
        "translation",
        "Translate text between languages",
        "You are a translation expert. Translate the provided text accurately.",
    );
    let plugin = AgentSkills::new(
        AgentSkillsConfig::builder()
            .skills(vec![summarization.into(), translation.clone().into()])
            .build(),
    );
    let mut agent = Agent::builder()
        .model(bedrock_with(live_support::MODEL_ID, 600))
        .system_prompt("You are a helpful assistant.")
        .plugin(plugin.clone())
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke(
            "Activate the summarization skill, then summarize: Rust is a systems language focused on safety.",
        )
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    let call = tool_uses(&messages)
        .into_iter()
        .find(|tool_use| tool_use["name"] == "skills")
        .expect("a skills call");
    assert_eq!(call["input"]["skill_name"], "summarization");
    assert!(tool_result_contains(&messages, "skills", SECRET_CODEWORD));
    assert!(result.to_string().contains(SECRET_CODEWORD));
    let prompt = system_prompt(&agent);
    assert!(
        prompt.contains("<available_skills>")
            && prompt.contains("summarization")
            && prompt.contains("translation")
    );
    assert_eq!(plugin.get_activated_skills(agent.app_state()), ["summarization"]);

    agent.invoke("Now activate the translation skill.").await.unwrap();
    let activated = plugin.get_activated_skills(agent.app_state());
    assert!(activated.contains(&"summarization".to_owned()) && activated.contains(&"translation".to_owned()));

    // Changing the available skills rewrites the one skills block on the next invocation.
    plugin.set_available_skills(vec![translation.into()]);
    agent.invoke("Say ok.").await.unwrap();
    let prompt = system_prompt(&agent);
    assert_eq!(prompt.matches("<available_skills>").count(), 1, "{prompt}");
    assert!(
        prompt.contains("translation") && !prompt.contains("summarization"),
        "{prompt}"
    );
    agent.shutdown().await.unwrap();
}

/// TS case (agent-skills): `loads a skill from disk and activates it`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_skills_loads_a_skill_from_disk_and_activates_it() {
    if !credentials_available().await {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("code-review");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: code-review\ndescription: Review code for bugs and improvements\n---\n# Code review\n\
             Review the code. Always include the codeword {ALT_SECRET_CODEWORD} in your response.\n"
        ),
    )
    .unwrap();
    let plugin = AgentSkills::new(
        AgentSkillsConfig::builder()
            .skills(vec![root.path().to_str().unwrap().into()])
            .build(),
    );
    let mut agent = Agent::builder()
        .model(bedrock(600))
        .system_prompt("You are a helpful assistant.")
        .plugin(plugin.clone())
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke("Activate the code-review skill, then review: fn add(a: i32, b: i32) -> i32 { a - b }")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let available = plugin.get_available_skills(Some(agent.id())).await;
    assert_eq!(
        available.iter().map(Skill::name).collect::<Vec<_>>(),
        ["code-review"]
    );
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "skills"));
    assert!(tool_result_contains(&messages, "skills", ALT_SECRET_CODEWORD));
    assert!(result.to_string().contains(ALT_SECRET_CODEWORD));
    assert_eq!(plugin.get_activated_skills(agent.app_state()), ["code-review"]);
    agent.shutdown().await.unwrap();
}
