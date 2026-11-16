//! Ported from harness-ts/test/tools/shell.test.ts

use std::sync::Arc;

use serde_json::json;
use strands::JsonValue;
use strands::sandbox::NotASandboxLocalEnvironment;
use strands::testing::tool_context_with_sandbox;
use strands::tools::Tool;
use strands::types::tools::ToolUse;
use strands::vended_tools::SANDBOX_SHELL_DESCRIPTION;

use strands::tools::FunctionTool;

fn make_shell(description: Option<String>) -> FunctionTool {
    super::make_shell(description).unwrap()
}

async fn invoke(input: JsonValue) -> JsonValue {
    let sandbox = Arc::new(NotASandboxLocalEnvironment::new());
    let context = tool_context_with_sandbox(ToolUse::new("shell", "tool-1", input.clone()), sandbox);
    make_shell(None).invoke(input, context).await.unwrap()
}

#[test]
fn exposes_only_command_and_timeout_with_no_mode_discriminator() {
    let spec = serde_json::to_value(make_shell(None).tool_spec()).unwrap();
    let schema = &spec["inputSchema"];
    let mut properties: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
    properties.sort();
    assert_eq!(properties, ["command", "timeout"]);
    assert_eq!(schema["required"], json!(["command"]));
}

#[tokio::test]
async fn runs_a_command_through_the_agent_sandbox() {
    let result = invoke(json!({ "command": "echo hello" })).await;
    assert!(result["output"].as_str().unwrap().contains("hello"), "{result}");
}

#[tokio::test]
async fn is_stateless_state_does_not_persist_across_calls() {
    invoke(json!({ "command": "export STRANDS_CLI_TEST=42" })).await;
    let result = invoke(json!({ "command": "echo \"[$STRANDS_CLI_TEST]\"" })).await;
    assert!(result["output"].as_str().unwrap().contains("[]"), "{result}");
}

// Rust-only: the configuration surface.

#[test]
fn the_description_is_the_configured_one_or_the_sdk_default() {
    assert_eq!(make_shell(None).description(), SANDBOX_SHELL_DESCRIPTION);
    assert_eq!(make_shell(Some("Run it.".to_owned())).description(), "Run it.");
    assert_eq!(make_shell(None).name(), "shell");
}
