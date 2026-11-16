//! A portable `HarnessAgentConfig` end to end: JSON through `normalize_harness_agent_config` and
//! `options_from_config` into `create_harness_with`, then one turn against a localhost Responses
//! server, so the assertions are on the agent and on the request it sends. No TS spec file: TS
//! tests the two halves apart (`config.test.ts`, `agent.test.ts`).

use std::path::Path;

use serde_json::json;

use super::*;
use crate::config::{normalize_harness_agent_config, options_from_config};

/// The pipeline a host runs for a config file found under `base_dir`.
async fn build(config: &JsonValue, base_dir: &Path, env: EnvLookup) -> Result<Agent> {
    let config = normalize_harness_agent_config(config)?;
    let options = options_from_config(&config, base_dir, &env)?;
    let seams = Seams {
        base_dir: Some(base_dir.to_owned()),
        env,
    };
    create_harness_with(options, seams).await
}

async fn build_error(config: &JsonValue, base_dir: &Path, env: EnvLookup) -> String {
    match build(config, base_dir, env).await {
        Ok(_) => panic!("{config} built an agent"),
        Err(error) => error.to_string(),
    }
}

#[tokio::test]
async fn a_module_reference_anywhere_fails_with_the_unsupported_error() {
    let reference = |kind: &str| json!({ "kind": kind, "module": "./values.ts", "export": "value", "language": "typescript" });
    let configs = [
        json!({ "tools": [reference("tool")] }),
        json!({ "subagents": [reference("subagent")] }),
        json!({ "plugins": [reference("plugin")] }),
        json!({ "sandbox": reference("sandbox") }),
        json!({ "memoryStores": [reference("memory-store")] }),
        json!({ "modelModule": reference("model") }),
        json!({ "builtinTools": { "web_fetch": { "model": reference("model") } } }),
        json!({ "interventionModules": [reference("intervention")] }),
        json!({ "agentConfigModules": { "conversationManager": reference("agent-config") } }),
    ];
    let dir = tempfile::tempdir().unwrap();
    for config in configs {
        assert_eq!(
            build_error(&config, dir.path(), Arc::new(|_| None)).await,
            "module references are not supported in Rust yet (language \"typescript\")",
            "{config}"
        );
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(feature = "openai")]
mod wire {
    use super::super::openai_cache::{Reply, ResponsesServer};
    use super::*;

    fn tool_names_of(request: &JsonValue) -> Vec<&str> {
        request["tools"]
            .as_array()
            .map(|tools| tools.iter().filter_map(|tool| tool["name"].as_str()).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_portable_config_builds_the_agent_it_describes() {
        let server = ResponsesServer::start(vec![Reply::Text("Looks fine.")]).await;
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("team-skills/review");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: review\ndescription: Reviews one change at a time.\n---\nRead the diff first.\n",
        )
        .unwrap();
        let config = json!({
            "name": "Reviewer",
            "description": "Reviews changes",
            "instructions": "Be strict.",
            "model": "openai/gpt-6-sol",
            "effort": "high",
            "builtinTools": ["read", "shell"],
            "builtinPlugins": ["todos"],
            "contextManager": "off",
            "session": { "id": "Review 1", "dir": "state/sessions" },
            "skills": "team-skills",
            "memory": { "dir": "state/memory" },
            "interventions": "off",
            "agentConfig": { "printer": false, "backgroundTasks": false },
        });

        let mut agent = build(&config, dir.path(), server.env(&[])).await.unwrap();
        assert_eq!(agent.name(), "Reviewer");
        assert_eq!(agent.description(), Some("Reviews changes"));
        assert_eq!(agent.model().model_id().as_deref(), Some("gpt-6-sol"));
        assert_eq!(agent.session_manager().unwrap().session_id(), "review-1");
        agent.invoke("Review the change").await.unwrap();

        let requests = server.requests();
        let request = &requests[0];
        assert_eq!(request["model"], json!("gpt-6-sol"));
        assert_eq!(request["reasoning"]["effort"], json!("high"));
        assert_eq!(request["prompt_cache_key"], json!("strands-review-1"));
        let instructions = request["instructions"].as_str().unwrap();
        assert!(
            instructions.starts_with(&format!("{HARNESS_CONTRACT}\n\nBe strict.")),
            "{instructions}"
        );
        let location = skill.join("SKILL.md");
        assert!(
            instructions.contains(&format!("<location>{}</location>", location.display())),
            "{instructions}"
        );
        assert_eq!(
            tool_names_of(request),
            ["shell", "read", "skills", "todo_write", "search_memory"]
        );

        assert!(dir.path().join("state/sessions/review-1").is_dir());
        agent
            .memory_manager()
            .unwrap()
            .add(
                "# Note\nremember this",
                strands::memory::MemoryAddOptions::default(),
            )
            .await
            .unwrap();
        assert!(dir.path().join("state/memory/note.md").exists());
    }

    /// The options a config sets to their non-default side, and nothing else switched on.
    fn bare(extra: &JsonValue) -> JsonValue {
        let mut config = json!({
            "model": "openai/gpt-6-sol",
            "builtinTools": [],
            "builtinPlugins": [],
            "contextManager": "off",
            "session": false,
            "skills": false,
            "memory": false,
            "agentConfig": { "printer": false, "backgroundTasks": false },
        });
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        config
    }

    #[tokio::test]
    async fn a_builtin_tools_mapping_and_the_switched_off_features_reach_the_agent() {
        let server = ResponsesServer::start(vec![Reply::Text("Done")]).await;
        let dir = tempfile::tempdir().unwrap();
        let config = bare(&json!({
            "caching": false,
            "builtinTools": { "*": false, "read": true, "shell": { "description": "Runs one command." } },
        }));

        let mut agent = build(&config, dir.path(), server.env(&[])).await.unwrap();
        assert!(agent.session_manager().is_none());
        assert!(agent.memory_manager().is_none());
        agent.invoke("Hello").await.unwrap();

        let request = &server.requests()[0];
        assert_eq!(tool_names_of(request), ["shell", "read"]);
        assert_eq!(request["tools"][0]["description"], json!("Runs one command."));
        assert_eq!(request["instructions"], json!(HARNESS_CONTRACT));
        let request = request.as_object().unwrap();
        assert!(!request.contains_key("prompt_cache_key"));
        assert!(!request.contains_key("prompt_cache_options"));
        assert!(!request["input"].to_string().contains("prompt_cache_breakpoint"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn the_default_state_directories_land_under_the_base_dir() {
        let server = ResponsesServer::start(vec![Reply::Text("Done")]).await;
        let dir = tempfile::tempdir().unwrap();
        let config = json!({
            "model": "openai/gpt-6-sol",
            "builtinTools": [],
            "builtinPlugins": [],
            "contextManager": "off",
            "session": { "id": "defaults" },
            "agentConfig": { "printer": false, "backgroundTasks": false },
        });

        let mut agent = build(&config, dir.path(), server.env(&[])).await.unwrap();
        agent.invoke("Hello").await.unwrap();
        assert!(dir.path().join(".agent/sessions/defaults").is_dir());
        agent
            .memory_manager()
            .unwrap()
            .add(
                "# Note\nremember this",
                strands::memory::MemoryAddOptions::default(),
            )
            .await
            .unwrap();
        assert!(dir.path().join(".agent/memory/note.md").exists());
    }

    /// A relative `.cedar` path resolves against the base directory, and the policy it names
    /// decides the tool call: with nothing permitted, `read` never runs.
    #[tokio::test]
    async fn a_relative_cedar_policy_file_guards_the_agents_tool_calls() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("policies")).unwrap();
        std::fs::write(
            dir.path().join("policies/deny.cedar"),
            "forbid(principal, action, resource);\n",
        )
        .unwrap();
        let notes = dir.path().join("notes.txt");
        std::fs::write(&notes, "the notes file was read").unwrap();
        let read_notes = Reply::ToolCall {
            name: "read",
            arguments: json!({ "path": notes.to_string_lossy() }),
        };
        let config =
            |interventions: &str| bare(&json!({ "builtinTools": ["read"], "interventions": interventions }));

        let server = ResponsesServer::start(vec![read_notes.clone(), Reply::Text("Done")]).await;
        let mut agent = build(&config("off"), dir.path(), server.env(&[])).await.unwrap();
        agent.invoke("Read the notes").await.unwrap();
        assert!(
            server.requests()[1]["input"]
                .to_string()
                .contains("the notes file was read")
        );

        let server = ResponsesServer::start(vec![read_notes, Reply::Text("Done")]).await;
        let mut agent = build(&config("policies/deny.cedar"), dir.path(), server.env(&[]))
            .await
            .unwrap();
        agent.invoke("Read the notes").await.unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        let input = requests[1]["input"].to_string();
        assert!(input.contains("Access denied by Cedar policy"), "{input}");
        assert!(!input.contains("the notes file was read"), "{input}");
    }

    /// An inline stdio server: `${VAR}` expands from the environment, and with no `cwd` the
    /// server starts in the base directory, where the relative script path resolves. The server
    /// is a shell loop that answers `initialize` and `tools/list` and refuses every other request.
    #[cfg(all(unix, feature = "mcp"))]
    #[tokio::test]
    async fn an_inline_mcp_server_starts_in_the_base_dir_with_its_placeholders_expanded() {
        const SERVER: &str = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  version=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"notes","version":"1"}}}\n' "$id" "$version" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"Pings.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
    *'"id":'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"Method not found"}}\n' "$id" ;;
  esac
done
"#;
        let server = ResponsesServer::start(vec![Reply::Text("Done")]).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.sh"), SERVER).unwrap();
        let config = bare(&json!({
            "mcpServers": { "notes": { "command": "sh", "args": ["${SERVER_SCRIPT}"] } },
        }));

        let mut agent = build(&config, dir.path(), server.env(&[("SERVER_SCRIPT", "server.sh")]))
            .await
            .unwrap();
        agent.invoke("Hello").await.unwrap();
        assert_eq!(tool_names_of(&server.requests()[0]), ["notes_ping"]);

        let error = build_error(&config, dir.path(), server.env(&[])).await;
        assert!(error.contains("SERVER_SCRIPT"), "{error}");
    }
}
