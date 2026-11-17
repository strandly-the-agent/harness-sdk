//! The `builds %s from its factory ...` cases of `harness-ts/test/agent.test.ts`.
//!
//! TS spies on each factory's arguments. A Rust factory cannot be spied on, so each row checks
//! what the factory was given where it shows: the shell tool's description, the options the
//! `programmatic_tool_caller` config maps to, and the depth budget the `subagent` tool leaves its
//! delegate.

use std::time::Duration;

use serde_json::json;

use super::delegates::built_child;
use super::*;
use crate::builtin_tools::programmatic_tool_caller_options;
use crate::config::{ProgrammaticToolCallerConfig, ShellConfig, SubagentConfig};
use crate::tools::make_shell;

async fn with_builtins(config: BuiltinToolsConfig) -> Agent {
    harness(HarnessAgentOptions {
        builtin_tools: Some(config.into()),
        ..mock_options()
    })
    .await
    .unwrap()
}

fn description(agent: &Agent, name: &str) -> String {
    let tool = agent.tools().find(|tool| tool.name() == name).unwrap();
    tool.tool_spec().description().to_owned()
}

/// The depth budget a delegate of a parent with this `subagent` setting is left with.
async fn delegate_depth(subagent: Setting<SubagentConfig>) -> Option<JsonValue> {
    let options = HarnessAgentOptions {
        builtin_tools: Some(
            BuiltinToolsConfig {
                subagent: Some(subagent),
                ..Default::default()
            }
            .into(),
        ),
        ..mock_options()
    };
    built_child(options, json!({ "task": "x" })).await.depth
}

#[tokio::test]
async fn builds_from_its_factory_with_its_per_tool_config() {
    let agent = with_builtins(BuiltinToolsConfig {
        shell: Some(Setting::Config(ShellConfig {
            description: Some("Run a command.".to_owned()),
        })),
        ..Default::default()
    })
    .await;
    assert_eq!(description(&agent, "shell"), "Run a command.");

    let config = ProgrammaticToolCallerConfig {
        allowed_tools: Some(strings(&["read"])),
        timeout: Some(Some(Duration::from_secs(60))),
    };
    let options = programmatic_tool_caller_options(config.clone());
    assert_eq!(options.allowed_tools, Some(strings(&["read"])));
    assert_eq!(options.timeout, Some(Some(Duration::from_secs(60))));
    let agent = with_builtins(BuiltinToolsConfig {
        programmatic_tool_caller: Some(Setting::Config(config)),
        ..Default::default()
    })
    .await;
    assert!(has_tool(&agent, "programmatic_tool_caller"));

    // `timeout: null` turns the bound off.
    let options = programmatic_tool_caller_options(ProgrammaticToolCallerConfig {
        timeout: Some(None),
        ..Default::default()
    });
    assert_eq!(options.allowed_tools, None);
    assert_eq!(options.timeout, Some(None));

    let config = SubagentConfig { max_depth: Some(1) };
    assert_eq!(delegate_depth(Setting::Config(config)).await, Some(json!(0)));
}

#[tokio::test]
async fn builds_from_its_factory_with_no_config_when_enabled_with_true() {
    let agent = with_builtins(BuiltinToolsConfig {
        shell: Some(Setting::On),
        ..Default::default()
    })
    .await;
    let default_shell = make_shell(None).unwrap();
    assert_eq!(
        description(&agent, "shell"),
        default_shell.tool_spec().description()
    );

    let options = programmatic_tool_caller_options(ProgrammaticToolCallerConfig::default());
    assert_eq!(options.allowed_tools, None);
    assert_eq!(options.timeout, None);

    // No `max_depth` leaves the factory default of 2, so the delegate has one level left.
    assert_eq!(delegate_depth(Setting::On).await, Some(json!(1)));
}
