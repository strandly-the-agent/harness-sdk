//! `describe('subagent schema derivation')`.

use super::*;

#[test]
fn derives_the_expected_parameters_from_the_axis_modes() {
    let (mut options, _) = options();
    options.instructions = Some(Open.into());
    options.model = Some(ModelMode::Choice(Choice::new(["fast", "deep"])));
    options.context = Some(none_or("all"));
    options.inherited_tools = names(&["read", "shell"]);
    let tool = make_subagent(options).unwrap();
    let schema = schema(tool.as_ref());
    let p = &schema["properties"];
    assert_eq!(
        keys(p),
        [
            "task",
            "instructions",
            "tools",
            "model",
            "context",
            "last_messages"
        ]
    );
    // The tools axis defaults to a multiple Choice over the inherited tools: an array of enum.
    assert_eq!(p["tools"]["items"]["enum"], json!(["read", "shell"]));
    assert_eq!(p["model"]["enum"], json!(["fast", "deep"]));
    assert_eq!(p["context"]["enum"], json!(["none", "all"]));
    assert_eq!(p["last_messages"]["type"], "integer");
    assert_eq!(schema["required"], json!(["task"]));
}

#[test]
fn drops_parameters_for_fixed_and_inherit_axes() {
    let (mut options, _) = options();
    options.instructions = Some(Fixed(Some("you are fixed".to_owned())).into());
    options.tools = Some(Inherit.into());
    options.model = Some(Inherit.into());
    options.context = Some(Fixed(ContextMode::None).into());
    options.inherited_tools = names(&["read"]);
    assert_eq!(keys(&props(options)), ["task"]);
}

#[test]
fn adds_an_agent_type_enum_and_lists_the_roles_from_presets() {
    let (mut options, _) = options();
    options.presets = generalist();
    options.presets.insert(
        "reviewer".to_owned(),
        Preset::builder()
            .instructions("review")
            .description("reviews diffs")
            .build(),
    );
    options.instructions = Some(InstructionsMode::Fixed(None));
    options.tools = Some(ToolsMode::Fixed(None));
    let tool = make_subagent(options).unwrap();
    let schema = schema(tool.as_ref());
    assert_eq!(
        schema["properties"]["agent_type"]["enum"],
        json!(["generalist", "reviewer"])
    );
    assert!(tool.description().contains("reviews diffs"));
    assert!(tool.description().ends_with(
        "\n\nAvailable subagents (agent_type):\n- generalist: a general-purpose agent for a focused subtask that runs in its own context\n- reviewer: reviews diffs"
    ));
}

#[test]
fn rejects_an_empty_choice_at_construction() {
    let (mut options, _) = options();
    options.tools = Some(ToolsMode::Choice(Choice::new(Vec::<String>::new())));
    assert_eq!(
        error_text(options),
        "tools=Choice([]) offers no options; use Fixed([]) for a toolless child, or Inherit()."
    );
}

#[tokio::test]
async fn maps_a_tools_choice_option_name_to_its_value() {
    // The model picks the enum name, the child gets the value.
    let choice = || {
        Choice::multiple([
            ChoiceOption::new("readonly").with_value("read"),
            ChoiceOption::new("sh").with_value("shell"),
        ])
    };
    let (mut shown, _) = options();
    shown.tools = Some(ToolsMode::Choice(choice()));
    assert_eq!(props(shown)["tools"]["items"]["enum"], json!(["readonly", "sh"]));
    let (mut options, built) = options();
    options.tools = Some(ToolsMode::Choice(choice()));
    drive(options, json!({"task": "x", "tools": ["readonly"]})).await;
    assert_eq!(built.spec().tools, Some(names(&["read"])));
}

#[test]
fn rejects_a_single_value_non_multiple_tools_choice_at_construction() {
    let (mut options, _) = options();
    options.tools = Some(ToolsMode::Choice(Choice::new(["read", "shell"])));
    assert_eq!(
        error_text(options),
        "tools=Choice(...) must be multiple=true; a subagent selects a subset of tools."
    );
}

#[test]
fn exposes_an_mcp_servers_array_of_enum_over_the_inherited_servers() {
    let (mut options, _) = options();
    options.inherited_mcp_servers = names(&["fs", "api"]);
    assert_eq!(
        props(options)["mcp_servers"]["items"]["enum"],
        json!(["fs", "api"])
    );
}

#[test]
fn omits_mcp_servers_when_the_parent_has_no_servers() {
    let (mut options, _) = options();
    options.inherited_tools = names(&["read"]);
    assert!(props(options).get("mcp_servers").is_none());
}

#[test]
fn rejects_an_empty_mcp_servers_choice_at_construction() {
    let (mut options, _) = options();
    options.mcp_servers = Some(McpServersMode::Choice(Choice::new(Vec::<String>::new())));
    assert_eq!(
        error_text(options),
        "mcp_servers=Choice([]) offers no options; use Fixed([]) for no servers, or Inherit()."
    );
}

#[test]
fn rejects_a_single_value_non_multiple_mcp_servers_choice_at_construction() {
    let (mut options, _) = options();
    options.mcp_servers = Some(McpServersMode::Choice(Choice::new(["fs"])));
    assert_eq!(
        error_text(options),
        "mcp_servers=Choice(...) must be multiple=true; a subagent selects a subset of servers."
    );
}

#[tokio::test]
async fn grants_all_inherited_mcp_servers_when_the_model_omits_mcp_servers() {
    let (mut options, built) = options();
    options.inherited_mcp_servers = names(&["fs", "api"]);
    drive(options, json!({"task": "x"})).await;
    assert_eq!(built.spec().mcp_servers, Some(names(&["fs", "api"])));
}

#[tokio::test]
async fn clamps_the_mcp_servers_choice_to_the_inherited_set_at_runtime() {
    let (mut options, built) = options();
    options.inherited_mcp_servers = names(&["fs", "api"]);
    drive(options, json!({"task": "x", "mcp_servers": ["fs", "secrets"]})).await;
    assert_eq!(built.spec().mcp_servers, Some(names(&["fs"])));
}

#[test]
fn still_offers_last_messages_when_the_only_history_mode_is_no_tools() {
    let (mut options, _) = options();
    options.context = Some(none_or("no_tools"));
    let p = props(options);
    assert_eq!(p["context"]["enum"], json!(["none", "no_tools"]));
    assert!(p.get("last_messages").is_some());
}

#[test]
fn omits_last_messages_when_the_only_context_mode_is_none() {
    let (mut options, _) = options();
    options.context = Some(ContextAxis::Choice(Choice::new(["none"])));
    assert!(props(options).get("last_messages").is_none());
}

#[test]
fn renders_per_option_descriptions_into_the_parameter_description() {
    let (mut options, _) = options();
    options.model = Some(ModelMode::Choice(Choice::new([
        ChoiceOption::new("haiku").with_description("cheap/fast"),
        ChoiceOption::new("opus").with_description("best quality"),
        ChoiceOption::new("sonnet"),
    ])));
    options.tools = Some(ToolsMode::Fixed(None));
    let model = props(options)["model"].clone();
    assert_eq!(
        model,
        json!({
            "type": "string",
            "enum": ["haiku", "opus", "sonnet"],
            "description": "Which model the subagent runs on.\nOptions:\n- haiku: cheap/fast\n- opus: best quality",
        })
    );
}

#[test]
fn renders_a_multiple_choice_as_an_array_of_enum() {
    let (mut options, _) = options();
    options.tools = Some(ToolsMode::Choice(Choice::multiple(["read", "write", "shell"])));
    let tools = props(options)["tools"].clone();
    assert_eq!(tools["type"], "array");
    assert_eq!(tools["items"]["enum"], json!(["read", "write", "shell"]));
}

#[test]
fn keeps_the_base_description_only_when_a_choice_has_no_per_option_descriptions() {
    let (mut options, _) = options();
    options.context = Some(none_or("all"));
    options.tools = Some(ToolsMode::Fixed(None));
    let context = props(options)["context"].clone();
    assert_eq!(context["enum"], json!(["none", "all"]));
    assert!(!context["description"].as_str().unwrap().contains("Options:"));
}

/// The TS source with its `'a ' + 'b'` string concatenations joined.
fn ts_source() -> Option<String> {
    let source = std::fs::read_to_string("/tmp/ts-spec/harness-ts/src/tools/subagent.ts").ok()?;
    let mut joined = String::new();
    for (index, piece) in source.replace("' + '", "").split(" +\n").enumerate() {
        if index > 0 {
            // Drop the quote that closes the piece before and the one that opens this piece.
            joined.pop();
            joined.push_str(piece.trim_start().get(1..).unwrap_or_default());
        } else {
            joined.push_str(piece);
        }
    }
    Some(joined)
}

#[test]
fn model_facing_text_matches_the_ts_source() {
    let Some(source) = ts_source() else {
        return;
    };
    let (mut options, _) = options();
    options.model = Some(ModelMode::Choice(Choice::new(["fast"])));
    options.context = Some(none_or("all"));
    options.inherited_tools = names(&["read"]);
    options.inherited_mcp_servers = names(&["fs"]);
    let tool = make_subagent(options).unwrap();
    let props = schema(tool.as_ref())["properties"].clone();
    let mut texts: Vec<String> = keys(&props)
        .iter()
        .map(|key| props[key]["description"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(texts.len(), 7);
    texts.extend([
        tool.description().to_owned(),
        "Which subagent role to use. Omit to use the default.".to_owned(),
        GENERALIST_INSTRUCTIONS.to_owned(),
        GENERALIST.description.clone(),
        FORK_PREAMBLE.to_owned(),
        CONTEXT_PREAMBLE.to_owned(),
    ]);
    for text in texts {
        assert!(source.contains(&text), "not in the TS source: {text}");
    }
}
