use serde_json::json;

use super::*;

fn names(resolved: &ResolvedBuiltinTools) -> Vec<&'static str> {
    enabled_builtin_tools(resolved)
        .into_iter()
        .map(BuiltinToolName::as_str)
        .collect()
}

fn err(value: &BuiltinToolsConfig) -> String {
    resolve_builtin_tools(Some(value))
        .expect_err("rejected")
        .to_string()
}

#[test]
fn selects_the_defaults_when_undefined() {
    let resolved = resolve_builtin_tools(None).expect("resolves");
    assert_eq!(names(&resolved), DEFAULT_BUILTIN_TOOLS.to_vec());
}

#[test]
fn pins_a_list_exactly() {
    let resolved =
        resolve_builtin_tools(Some(&BuiltinToolsConfig::list(["read", "shell"]))).expect("resolves");
    assert_eq!(names(&resolved), vec!["shell", "read"]);
    let empty =
        resolve_builtin_tools(Some(&BuiltinToolsConfig::list(Vec::<String>::new()))).expect("resolves");
    assert!(names(&empty).is_empty());
}

#[test]
fn resolves_every_builtin_name_in_the_record() {
    let resolved = resolve_builtin_tools(Some(&BuiltinToolsConfig::list(["read"]))).expect("resolves");
    for name in BUILTIN_TOOL_NAMES {
        let setting = resolved.get(*name);
        assert_eq!(
            setting.is_off(),
            *name != BuiltinToolName::Read,
            "{name} resolved to {setting:?}"
        );
    }
}

#[test]
fn edits_the_defaults_with_a_mapping() {
    let resolved = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([
        ("shell", json!(false)),
        ("web_search", json!("exa")),
    ])))
    .expect("resolves");
    assert_eq!(
        names(&resolved),
        vec![
            "read",
            "write",
            "edit",
            "web_fetch",
            "web_search",
            "programmatic_tool_caller",
            "subagent"
        ]
    );
    assert_eq!(resolved.get(BuiltinToolName::WebSearch), &BuiltinToolSetting::Exa);
}

#[test]
fn starts_from_nothing_when_star_is_false() {
    let resolved = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([
        ("*", json!(false)),
        ("read", json!(true)),
    ])))
    .expect("resolves");
    assert_eq!(names(&resolved), vec!["read"]);
}

#[test]
fn treats_star_true_as_the_defaults() {
    let resolved =
        resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([("*", json!(true))]))).expect("resolves");
    assert_eq!(names(&resolved), DEFAULT_BUILTIN_TOOLS.to_vec());
}

#[test]
fn keeps_a_per_tool_web_fetch_config() {
    let config = json!({ "model": "openai/gpt-5-mini", "transport": "direct" });
    let resolved = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([(
        "web_fetch",
        config.clone(),
    )])))
    .expect("resolves");
    assert_eq!(
        builtin_tool_config(&resolved, BuiltinToolName::WebFetch),
        Some(config.as_object().unwrap().clone())
    );
}

#[test]
fn reports_an_empty_web_fetch_config_when_enabled_without_one_and_none_when_off() {
    let on = resolve_builtin_tools(None).expect("resolves");
    assert_eq!(
        builtin_tool_config(&on, BuiltinToolName::WebFetch),
        Some(Map::new())
    );
    let off = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([("web_fetch", json!(false))])))
        .expect("resolves");
    assert_eq!(builtin_tool_config(&off, BuiltinToolName::WebFetch), None);
}

#[test]
fn rejects_an_unknown_name_in_a_list() {
    assert!(err(&BuiltinToolsConfig::list(["grep"])).contains(r#"Unknown built-in tool "grep""#));
}

#[test]
fn rejects_an_unknown_name_in_a_mapping() {
    assert_eq!(
        err(&BuiltinToolsConfig::mapping([("grep", json!(true))])),
        "Unknown built-in tool \"grep\". Available: shell, read, write, edit, web_fetch, web_search, programmatic_tool_caller, subagent."
    );
}

#[test]
fn keeps_a_per_tool_config_for_each_configurable_tool() {
    for (name, tool, config) in [
        (
            "shell",
            BuiltinToolName::Shell,
            json!({ "description": "Run a command in the sandbox." }),
        ),
        (
            "programmatic_tool_caller",
            BuiltinToolName::ProgrammaticToolCaller,
            json!({ "allowedTools": ["read"], "timeout": 60 }),
        ),
        ("subagent", BuiltinToolName::Subagent, json!({ "maxDepth": 1 })),
    ] {
        let resolved = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([(name, config.clone())])))
            .expect("resolves");
        assert_eq!(
            builtin_tool_config(&resolved, tool),
            Some(config.as_object().unwrap().clone())
        );
        let on = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([(name, json!(true))])))
            .expect("resolves");
        assert_eq!(builtin_tool_config(&on, tool), Some(Map::new()));
        let off = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([(name, json!(false))])))
            .expect("resolves");
        assert_eq!(builtin_tool_config(&off, tool), None);
    }
}

#[test]
fn pins_the_configurable_tools_and_their_keys() {
    assert_eq!(BuiltinToolName::Read.config_keys(), Some(&["media"][..]));
    assert_eq!(BuiltinToolName::Shell.config_keys(), Some(&["description"][..]));
    assert_eq!(
        BuiltinToolName::WebFetch.config_keys(),
        Some(&["model", "transport"][..])
    );
    assert_eq!(
        BuiltinToolName::ProgrammaticToolCaller.config_keys(),
        Some(&["allowedTools", "timeout"][..])
    );
    assert_eq!(BuiltinToolName::Subagent.config_keys(), Some(&["maxDepth"][..]));
    assert_eq!(BuiltinToolName::Write.config_keys(), None);
    assert_eq!(BuiltinToolName::Edit.config_keys(), None);
    assert_eq!(BuiltinToolName::WebSearch.config_keys(), None);
}

#[test]
fn rejects_unknown_config_keys_and_config_on_a_bool_only_tool() {
    assert_eq!(
        err(&BuiltinToolsConfig::mapping([(
            "read",
            json!({ "nope": true, "alsoNope": 1 })
        )])),
        "Unknown read config keys: alsoNope, nope. Allowed: media."
    );
    assert_eq!(
        err(&BuiltinToolsConfig::mapping([("write", json!({ "x": 1 }))])),
        "Built-in tool \"write\" takes no config; pass true or false."
    );
}

#[test]
fn accepts_only_a_boolean_or_exa_for_web_search() {
    assert_eq!(
        err(&BuiltinToolsConfig::mapping([("web_search", json!("native"))])),
        "builtinTools.web_search must be a boolean or 'exa', got \"native\"."
    );
    let exa = resolve_builtin_tools(Some(&BuiltinToolsConfig::mapping([("web_search", json!("exa"))])))
        .expect("ok");
    assert_eq!(exa.get(BuiltinToolName::WebSearch), &BuiltinToolSetting::Exa);
}

#[test]
fn rejects_a_non_boolean_value() {
    assert_eq!(
        err(&BuiltinToolsConfig::mapping([("shell", json!("yes"))])),
        "builtinTools.shell must be a boolean or a config object, got \"yes\"."
    );
    assert_eq!(
        err(&BuiltinToolsConfig::mapping([("shell", json!([1]))])),
        "builtinTools.shell must be a boolean or a config object, got [1]."
    );
}

#[test]
fn rejects_a_non_boolean_star() {
    assert_eq!(
        err(&BuiltinToolsConfig::mapping([("*", json!("all"))])),
        "builtinTools['*'] must be a boolean, got \"all\"."
    );
}

#[test]
fn web_search_explicit_is_false_for_the_defaults() {
    assert!(!web_search_explicit(None));
}

#[test]
fn web_search_explicit_is_true_when_named_in_a_list_or_as_a_mapping_key() {
    assert!(web_search_explicit(Some(&BuiltinToolsConfig::list([
        "read",
        "web_search"
    ]))));
    for value in [json!(true), json!(false), json!("exa")] {
        assert!(web_search_explicit(Some(&BuiltinToolsConfig::mapping([(
            "web_search",
            value
        )]))));
    }
}

#[test]
fn web_search_explicit_is_false_when_a_mapping_leaves_it_alone() {
    assert!(!web_search_explicit(Some(&BuiltinToolsConfig::mapping([(
        "read",
        json!(true)
    )]))));
}

#[test]
fn web_search_mode_resolves_native_exa_and_off() {
    let off = BuiltinToolSetting::Enabled(false);
    assert_eq!(
        web_search_mode(&off, false, &WebSearchModel::Default, true).expect("ok"),
        None
    );
    assert_eq!(
        web_search_mode(&BuiltinToolSetting::Exa, false, &WebSearchModel::Default, false).expect("ok"),
        Some(WebSearchMode::Exa)
    );
    assert_eq!(
        web_search_mode(
            &BuiltinToolSetting::Enabled(true),
            false,
            &WebSearchModel::Default,
            true
        )
        .expect("ok"),
        Some(WebSearchMode::Native)
    );
    assert_eq!(
        web_search_mode(
            &BuiltinToolSetting::Enabled(true),
            false,
            &WebSearchModel::Id("openai/gpt-5-mini"),
            false
        )
        .expect("ok"),
        None
    );
}

#[test]
fn web_search_mode_throws_when_explicitly_asked_for_on_a_model_without_it() {
    let error = web_search_mode(
        &BuiltinToolSetting::Enabled(true),
        true,
        &WebSearchModel::Id("openai/gpt-5-mini"),
        false,
    )
    .expect_err("rejected")
    .to_string();
    assert_eq!(
        error,
        "Model openai/gpt-5-mini has no built-in web search. Pass builtinTools: { web_search: 'exa' } to search through Exa (a third party), or remove 'web_search' from builtinTools. See https://strandsagents.com/docs/user-guide/harness/tools/web-access/#web_search"
    );
    let instance = web_search_mode(
        &BuiltinToolSetting::Enabled(true),
        true,
        &WebSearchModel::Instance,
        false,
    )
    .expect_err("rejected")
    .to_string();
    assert!(
        instance.starts_with("A pre-built Model instance has no built-in web search."),
        "{instance}"
    );
}

#[test]
fn select_builtin_tools_keeps_order_and_drops_the_programmatic_tool_caller() {
    let read = strands_agents::tools::function_tool("read").handler(|_input, _ctx| async { Ok("ok") });
    let tools: BTreeMap<BuiltinToolName, ToolRef> =
        BTreeMap::from([(BuiltinToolName::Read, Arc::new(read) as ToolRef)]);
    let selected = select_builtin_tools(
        &[
            BuiltinToolName::ProgrammaticToolCaller,
            BuiltinToolName::Read,
            BuiltinToolName::Shell,
        ],
        &tools,
    );
    assert_eq!(
        selected.iter().map(|t| t.name().to_owned()).collect::<Vec<_>>(),
        vec!["read"]
    );
}

#[test]
fn select_builtin_plugins_builds_the_defaults_and_skips_existing() {
    let selected = select_builtin_plugins(BUILTIN_PLUGIN_NAMES, &[]).expect("resolves");
    assert_eq!(
        selected.iter().map(|p| p.name().to_owned()).collect::<Vec<_>>(),
        vec!["strands:todos", "strands:environment"]
    );
    let existing: Vec<Arc<dyn Plugin>> = vec![Arc::new(Todos::default())];
    let second = select_builtin_plugins(BUILTIN_PLUGIN_NAMES, &existing).expect("resolves");
    assert_eq!(
        second.iter().map(|p| p.name().to_owned()).collect::<Vec<_>>(),
        vec!["strands:environment"]
    );
    assert!(select_builtin_plugins(&[], &[]).expect("resolves").is_empty());
}

#[test]
fn select_builtin_plugins_throws_on_an_unknown_builtin_plugin_name() {
    let error = match select_builtin_plugins(&["nope"], &[]) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("expected an error"),
    };
    assert_eq!(
        error,
        "Unknown built-in plugin \"nope\". Available: environment, todos."
    );
}
