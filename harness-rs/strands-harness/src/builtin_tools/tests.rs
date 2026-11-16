//! Ports `harness-ts/test/builtin-tools.test.ts`, plus cases for `web_search_mode`, the built
//! tool set and the default subagent's child options, which TS covers through `agent.test.ts`.

use std::sync::Arc;

use strands::agent::ModelSource;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::tools::ToolSource;
use tracing::Level;

use super::*;
use crate::config::builtin_tool_config_keys;
use crate::test_support::WarnRecorder;
use crate::types::WebFetchTransport;

fn names(resolved: &ResolvedBuiltinTools) -> Vec<&'static str> {
    enabled_builtin_tools(resolved)
        .into_iter()
        .map(BuiltinToolName::as_str)
        .collect()
}

fn mapping(config: BuiltinToolsConfig) -> ResolvedBuiltinTools {
    resolve_builtin_tools(Some(&BuiltinToolsOption::Config(config)))
}

fn list(names: &[BuiltinToolName]) -> BuiltinToolsOption {
    BuiltinToolsOption::Names(names.to_vec())
}

fn no_env() -> EnvLookup {
    Arc::new(|_| None)
}

fn mock_model() -> ModelSource {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    ModelSource::Model(model)
}

fn never_build() -> BuildAgent {
    Arc::new(|_| Box::pin(async { panic!("no test here runs a delegate") }))
}

#[test]
fn selects_the_defaults_when_undefined() {
    assert_eq!(names(&resolve_builtin_tools(None)), DEFAULT_BUILTIN_TOOLS);
}

#[test]
fn pins_a_list_exactly() {
    let resolved = resolve_builtin_tools(Some(&list(&[BuiltinToolName::Read, BuiltinToolName::Shell])));
    assert_eq!(names(&resolved), ["shell", "read"]);
    assert!(!resolved.web_fetch.is_enabled());
    assert!(!resolved.subagent.is_enabled());
}

#[test]
fn resolves_every_built_in_name_in_the_record() {
    assert!(names(&resolve_builtin_tools(Some(&list(&[])))).is_empty());
    let all = resolve_builtin_tools(Some(&list(&BUILTIN_TOOL_NAMES)));
    assert_eq!(enabled_builtin_tools(&all), BUILTIN_TOOL_NAMES);
}

#[test]
fn edits_the_defaults_with_a_mapping() {
    let resolved = mapping(BuiltinToolsConfig {
        subagent: Some(Setting::Off),
        web_search: Some(WebSearchSetting::On),
        ..Default::default()
    });
    assert!(!resolved.subagent.is_enabled());
    assert_eq!(resolved.web_search, WebSearchSetting::On);
    assert_eq!(resolved.shell, Setting::On);
    assert_eq!(resolved.read, Setting::On);
}

#[test]
fn starts_from_nothing_when_star_is_false() {
    let resolved = mapping(BuiltinToolsConfig {
        all: Some(false),
        read: Some(Setting::On),
        ..Default::default()
    });
    assert_eq!(names(&resolved), ["read"]);
}

#[test]
fn treats_star_true_as_the_defaults() {
    let resolved = mapping(BuiltinToolsConfig {
        all: Some(true),
        ..Default::default()
    });
    assert_eq!(
        format!("{resolved:?}"),
        format!("{:?}", resolve_builtin_tools(None))
    );
}

#[test]
fn keeps_a_per_tool_web_fetch_config() {
    let resolved = mapping(BuiltinToolsConfig {
        web_fetch: Some(Setting::Config(WebFetchConfig {
            model: Some(ModelSource::Id("openai/gpt-5-mini".to_owned())),
            transport: Some(WebFetchTransport::Direct),
        })),
        ..Default::default()
    });
    let config = builtin_tool_config(&resolved.web_fetch).unwrap();
    assert!(matches!(config.model, Some(ModelSource::Id(spec)) if spec == "openai/gpt-5-mini"));
    assert_eq!(config.transport, Some(WebFetchTransport::Direct));
}

#[test]
fn reports_an_empty_web_fetch_config_when_enabled_without_one_and_none_when_off() {
    let config = builtin_tool_config(&resolve_builtin_tools(None).web_fetch).unwrap();
    assert!(config.model.is_none() && config.transport.is_none());
    let off = mapping(BuiltinToolsConfig {
        web_fetch: Some(Setting::Off),
        ..Default::default()
    });
    assert!(builtin_tool_config(&off.web_fetch).is_none());
}

#[test]
fn keeps_a_per_tool_shell_config() {
    let config = ShellConfig {
        description: Some("Run a command in the sandbox.".to_owned()),
    };
    let set = |shell| {
        mapping(BuiltinToolsConfig {
            shell: Some(shell),
            ..Default::default()
        })
        .shell
    };
    assert_eq!(
        builtin_tool_config(&set(Setting::Config(config.clone()))),
        Some(config)
    );
    assert_eq!(
        builtin_tool_config(&set(Setting::On)),
        Some(ShellConfig::default())
    );
    assert_eq!(builtin_tool_config(&set(Setting::Off)), None);
}

#[test]
fn keeps_a_per_tool_programmatic_tool_caller_config() {
    let config = ProgrammaticToolCallerConfig {
        allowed_tools: Some(Some(vec!["read".to_owned()])),
        timeout: Some(Some(60.0)),
    };
    let set = |programmatic_tool_caller| {
        mapping(BuiltinToolsConfig {
            programmatic_tool_caller: Some(programmatic_tool_caller),
            ..Default::default()
        })
        .programmatic_tool_caller
    };
    assert_eq!(
        builtin_tool_config(&set(Setting::Config(config.clone()))),
        Some(config)
    );
    assert_eq!(
        builtin_tool_config(&set(Setting::On)),
        Some(ProgrammaticToolCallerConfig::default())
    );
    assert_eq!(builtin_tool_config(&set(Setting::Off)), None);
}

#[test]
fn keeps_a_per_tool_subagent_config() {
    let config = SubagentConfig { max_depth: Some(1) };
    let set = |subagent| {
        mapping(BuiltinToolsConfig {
            subagent: Some(subagent),
            ..Default::default()
        })
        .subagent
    };
    assert_eq!(
        builtin_tool_config(&set(Setting::Config(config.clone()))),
        Some(config)
    );
    assert_eq!(
        builtin_tool_config(&set(Setting::On)),
        Some(SubagentConfig::default())
    );
    assert_eq!(builtin_tool_config(&set(Setting::Off)), None);
}

#[test]
fn pins_the_configurable_tools_and_their_keys() {
    let keys = |name| builtin_tool_config_keys(name).map(<[&str]>::to_vec);
    assert_eq!(keys(BuiltinToolName::Read), Some(vec!["media"]));
    assert_eq!(keys(BuiltinToolName::Shell), Some(vec!["description"]));
    assert_eq!(keys(BuiltinToolName::WebFetch), Some(vec!["model", "transport"]));
    assert_eq!(
        keys(BuiltinToolName::ProgrammaticToolCaller),
        Some(vec!["allowedTools", "timeout"])
    );
    assert_eq!(keys(BuiltinToolName::Subagent), Some(vec!["maxDepth"]));
    for name in [
        BuiltinToolName::Write,
        BuiltinToolName::Edit,
        BuiltinToolName::WebSearch,
    ] {
        assert_eq!(keys(name), None);
    }
}

#[test]
fn accepts_only_a_boolean_or_exa_for_web_search() {
    let resolved = mapping(BuiltinToolsConfig {
        web_search: Some(WebSearchSetting::Exa),
        ..Default::default()
    });
    assert_eq!(resolved.web_search, WebSearchSetting::Exa);
}

#[test]
fn web_search_explicit_is_false_for_the_defaults() {
    assert!(!web_search_explicit(None));
}

#[test]
fn web_search_explicit_is_true_when_named_in_a_list_or_as_a_mapping_key() {
    assert!(web_search_explicit(Some(&list(&[
        BuiltinToolName::Read,
        BuiltinToolName::WebSearch
    ]))));
    for setting in [WebSearchSetting::On, WebSearchSetting::Off, WebSearchSetting::Exa] {
        let config = BuiltinToolsConfig {
            web_search: Some(setting),
            ..Default::default()
        };
        assert!(web_search_explicit(Some(&config.into())));
    }
}

#[test]
fn web_search_explicit_is_false_when_a_mapping_leaves_it_alone() {
    let config = BuiltinToolsConfig {
        read: Some(Setting::On),
        ..Default::default()
    };
    assert!(!web_search_explicit(Some(&config.into())));
}

#[test]
fn web_search_mode_is_off_native_or_exa() {
    let native = ModelSource::Id("anthropic/claude-opus-4-5".to_owned());
    assert_eq!(
        web_search_mode(WebSearchSetting::Off, true, Some(&native)).unwrap(),
        None
    );
    assert_eq!(
        web_search_mode(WebSearchSetting::On, false, Some(&native)).unwrap(),
        supports_web_search(Some(&native)).then_some(WebSearchMode::Native)
    );
    #[cfg(feature = "mcp")]
    assert_eq!(
        web_search_mode(WebSearchSetting::Exa, false, None).unwrap(),
        Some(WebSearchMode::Exa)
    );
}

#[test]
fn web_search_mode_disables_a_default_web_search_on_a_model_without_it_and_says_so() {
    let (mode, logged) =
        WarnRecorder::capture_level(Level::INFO, || web_search_mode(WebSearchSetting::On, false, None));
    assert_eq!(mode.unwrap(), None);
    assert_eq!(
        logged,
        [format!(
            "Model {DEFAULT_MODEL} has no built-in web search, so the web_search tool is disabled. To enable it through Exa (a third party), pass builtinTools: {{ web_search: 'exa' }}. See https://strandsagents.com/docs/user-guide/harness/tools/web-access/#web_search"
        )]
    );
}

#[test]
fn web_search_mode_rejects_an_explicit_web_search_on_a_model_without_it() {
    let error = web_search_mode(WebSearchSetting::On, true, Some(&mock_model())).unwrap_err();
    assert_eq!(
        error.to_string(),
        "A pre-built Model instance has no built-in web search. Pass builtinTools: { web_search: 'exa' } to search through Exa (a third party), or remove 'web_search' from builtinTools. See https://strandsagents.com/docs/user-guide/harness/tools/web-access/#web_search"
    );
    let error = web_search_mode(
        WebSearchSetting::On,
        true,
        Some(&ModelSource::Id("bedrock/some-model".to_owned())),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Model bedrock/some-model has no built-in web search.")
    );
}

#[test]
fn build_builtin_tools_builds_each_tool_under_its_own_name_and_select_keeps_the_order_asked_for() {
    let parent = HarnessAgentOptions {
        model: Some(mock_model()),
        ..Default::default()
    };
    let built = build_builtin_tools(never_build(), &parent, &resolve_builtin_tools(None), &no_env()).unwrap();
    for (name, tool) in &built {
        assert_eq!(tool.name(), name.as_str());
    }
    let expected: Vec<_> = BUILTIN_TOOL_NAMES
        .into_iter()
        .filter(|name| cfg!(feature = "mcp") || *name != BuiltinToolName::WebSearch)
        .collect();
    assert_eq!(built.keys().copied().collect::<Vec<_>>(), expected);

    let selected = select_builtin_tools(&[BuiltinToolName::Edit, BuiltinToolName::Shell], &built);
    let selected: Vec<_> = selected
        .iter()
        .map(|source| match source {
            ToolSource::Tool(tool) => tool.name().to_owned(),
            _ => panic!("built-ins are plain tools"),
        })
        .collect();
    assert_eq!(selected, ["edit", "shell"]);
}

#[test]
fn child_options_split_a_tool_selection_into_built_ins_keeping_their_config_and_consumer_tools() {
    let consumer: Arc<dyn Tool> = Arc::new(crate::tools::read());
    let parent = HarnessAgentOptions {
        model: Some(mock_model()),
        tools: vec![ToolSource::Tool(Arc::clone(&consumer))],
        builtin_tools: Some(
            BuiltinToolsConfig {
                shell: Some(Setting::Config(ShellConfig {
                    description: Some("run".to_owned()),
                })),
                edit: Some(false),
                ..Default::default()
            }
            .into(),
        ),
        memory: MemoryOption::Off,
        ..Default::default()
    };
    let builtins = resolve_builtin_tools(parent.builtin_tools.as_ref());
    let consumers = IndexMap::from([("read".to_owned(), consumer)]);
    let mut spec = AgentSpec::new("task", None);
    spec.instructions = Some("be brief".to_owned());
    spec.tools = Some(vec!["shell".to_owned(), "edit".to_owned(), "read".to_owned()]);

    let child = child_options(
        &parent,
        &builtins,
        None,
        &consumers,
        &McpClients::new(),
        spec,
        &no_env(),
    )
    .unwrap();

    assert_eq!(child.instructions.as_deref(), Some("be brief"));
    assert_eq!(child.agent_config.printer, Some(false));
    assert!(matches!(child.memory, MemoryOption::Off));
    assert!(child.agent_config.memory_manager.is_none());
    let resolved = resolve_builtin_tools(child.builtin_tools.as_ref());
    // `read` names the consumer tool, `edit` is off in the parent, and the default web_search
    // is left for the child to resolve against its own model.
    assert_eq!(names(&resolved), ["shell", "web_search"]);
    assert_eq!(
        builtin_tool_config(&resolved.shell)
            .unwrap()
            .description
            .as_deref(),
        Some("run")
    );
    assert!(!web_search_explicit(child.builtin_tools.as_ref()));
    assert!(matches!(child.tools.as_slice(), [ToolSource::Tool(tool)] if tool.name() == "read"));
}

#[test]
fn child_options_keep_the_parents_tools_and_turn_memory_off_when_the_spec_narrows_nothing() {
    let parent = HarnessAgentOptions {
        model: Some(mock_model()),
        builtin_tools: Some(list(&[BuiltinToolName::Read, BuiltinToolName::WebSearch])),
        ..Default::default()
    };
    let builtins = resolve_builtin_tools(parent.builtin_tools.as_ref());
    let spec = AgentSpec::new("task", None);

    let child = child_options(
        &parent,
        &builtins,
        None,
        &IndexMap::new(),
        &McpClients::new(),
        spec,
        &no_env(),
    )
    .unwrap();

    assert!(matches!(child.memory, MemoryOption::Off));
    assert!(matches!(&child.builtin_tools, Some(BuiltinToolsOption::Names(names)) if names.len() == 2));
    assert!(child.tools.is_empty());
}

/// `agent.test.ts` runs this through the `subagent` tool; the default subagent's schema offers no
/// `tools` here, so the rule is checked on `child_options`.
#[test]
fn forwards_a_per_tool_web_fetch_config_to_the_delegate_as_a_pinned_mapping() {
    let parent = HarnessAgentOptions {
        model: Some(mock_model()),
        builtin_tools: Some(
            BuiltinToolsConfig {
                web_fetch: Some(Setting::Config(WebFetchConfig {
                    model: Some(ModelSource::Id("openai/gpt-5-mini".to_owned())),
                    transport: None,
                })),
                shell: Some(Setting::Config(ShellConfig {
                    description: Some("Run it.".to_owned()),
                })),
                ..Default::default()
            }
            .into(),
        ),
        memory: MemoryOption::Off,
        ..Default::default()
    };
    let builtins = resolve_builtin_tools(parent.builtin_tools.as_ref());
    let mut spec = AgentSpec::new("go", None);
    spec.tools = Some(vec![
        "web_fetch".to_owned(),
        "read".to_owned(),
        "shell".to_owned(),
    ]);

    let child = child_options(
        &parent,
        &builtins,
        None,
        &IndexMap::new(),
        &McpClients::new(),
        spec,
        &no_env(),
    )
    .unwrap();

    let Some(BuiltinToolsOption::Config(pinned)) = child.builtin_tools else {
        panic!("the child's built-ins are a pinned mapping");
    };
    assert!(
        matches!(&pinned.shell, Some(Setting::Config(shell)) if shell.description.as_deref() == Some("Run it."))
    );
    assert!(matches!(pinned.read, Some(Setting::On)));
    assert_eq!(pinned.write, Some(false));
    assert_eq!(pinned.edit, Some(false));
    assert!(matches!(
        &pinned.web_fetch,
        Some(Setting::Config(WebFetchConfig { model: Some(ModelSource::Id(model)), .. })) if model == "openai/gpt-5-mini"
    ));
    assert!(matches!(pinned.programmatic_tool_caller, Some(Setting::Off)));
    assert!(matches!(pinned.subagent, Some(Setting::Off)));
    // Every name is pinned except web_search, which the parent left to the defaults.
    assert!(pinned.web_search.is_none());
    assert!(pinned.all.is_none());
}

/// `agent.test.ts` builds the child; here the child's options are resolved instead.
#[test]
fn narrows_the_built_child_to_the_requested_subset_dropping_the_rest() {
    let parent = HarnessAgentOptions {
        model: Some(mock_model()),
        builtin_tools: Some(list(&[
            BuiltinToolName::Read,
            BuiltinToolName::Shell,
            BuiltinToolName::Subagent,
        ])),
        memory: MemoryOption::Off,
        ..Default::default()
    };
    let builtins = resolve_builtin_tools(parent.builtin_tools.as_ref());
    let mut spec = AgentSpec::new("x", None);
    spec.tools = Some(vec!["read".to_owned()]);

    let child = child_options(
        &parent,
        &builtins,
        None,
        &IndexMap::new(),
        &McpClients::new(),
        spec,
        &no_env(),
    )
    .unwrap();

    let resolved = resolve_builtin_tools(child.builtin_tools.as_ref());
    assert!(names(&resolved).contains(&"read"));
    assert!(!names(&resolved).contains(&"shell"));
}

/// `agent.test.ts` builds the child and finds the Exa tool on it; here the child's options carry
/// the parent's explicit setting, which the child resolves as the parent did.
#[test]
fn carries_the_exa_fallback_onto_a_narrowed_delegate() {
    let parent = HarnessAgentOptions {
        model: Some(mock_model()),
        builtin_tools: Some(
            BuiltinToolsConfig {
                web_search: Some(WebSearchSetting::Exa),
                ..Default::default()
            }
            .into(),
        ),
        memory: MemoryOption::Off,
        ..Default::default()
    };
    let builtins = resolve_builtin_tools(parent.builtin_tools.as_ref());
    let mut spec = AgentSpec::new("x", None);
    spec.tools = Some(vec!["read".to_owned()]);

    let child = child_options(
        &parent,
        &builtins,
        None,
        &IndexMap::new(),
        &McpClients::new(),
        spec,
        &no_env(),
    )
    .unwrap();

    assert!(web_search_explicit(child.builtin_tools.as_ref()));
    let resolved = resolve_builtin_tools(child.builtin_tools.as_ref());
    assert!(matches!(resolved.web_search, WebSearchSetting::Exa));
    assert_eq!(names(&resolved), ["read", "web_search"]);
}
