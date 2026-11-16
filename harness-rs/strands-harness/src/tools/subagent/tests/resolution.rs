//! `describe('subagent spec resolution and execution')`.

use super::*;

#[tokio::test]
async fn builds_the_child_from_the_spec_and_returns_its_output() {
    let (builder, built) = builder_over(&replying("the answer", StopReason::EndTurn));
    let options = MakeSubagentOptions::builder()
        .builder(builder)
        .presets(generalist())
        .build();
    let result = drive(options, json!({"task": "do the thing"})).await;
    assert_eq!(result.status(), &ToolResultStatus::Success);
    assert_eq!(result_text(&result), "the answer");
    // A bare task resolves to the default (generalist) preset.
    let spec = built.spec();
    assert_eq!(spec.task, "do the thing");
    assert_eq!(spec.agent_type.as_deref(), Some("generalist"));
    assert_eq!(spec.instructions.as_deref(), Some(GENERALIST_INSTRUCTIONS));
}

#[tokio::test]
async fn lets_ad_hoc_instructions_suppress_the_default_preset() {
    let (mut options, built) = options();
    options.presets = generalist();
    options.instructions = Some(Open.into());
    drive(options, json!({"task": "x", "instructions": "You are a poet."})).await;
    assert_eq!(built.spec().agent_type, None);
    assert_eq!(built.spec().instructions.as_deref(), Some("You are a poet."));
}

#[tokio::test]
async fn rejects_an_off_schema_agent_type_unknown_empty_non_string_or_a_prototype_key() {
    // agent_type is a closed enum: a provided value is an exact preset name or an error.
    for (bad, shown) in [
        (json!("nope"), "\"nope\""),
        (json!(""), "\"\""),
        (json!("constructor"), "\"constructor\""),
        (json!(5), "5"),
    ] {
        let (mut options, built) = options();
        options.presets = generalist();
        let result = drive(options, json!({"task": "x", "agent_type": bad})).await;
        assert_eq!(result.status(), &ToolResultStatus::Error);
        assert_eq!(
            result_text(&result),
            format!("Subagent error: Unknown agent_type {shown}; valid values: generalist.")
        );
        assert!(built.specs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn lets_model_supplied_arguments_win_over_the_preset() {
    let (mut options, built) = options();
    options.presets = generalist();
    options.instructions = Some(Open.into());
    options.inherited_tools = names(&["read", "shell", "write"]);
    let input =
        json!({"task": "audit", "instructions": "You are a security auditor.", "tools": ["read", "shell"]});
    drive(options, input).await;
    assert_eq!(
        built.spec().instructions.as_deref(),
        Some("You are a security auditor.")
    );
    assert_eq!(built.spec().tools, Some(names(&["read", "shell"])));
}

#[tokio::test]
async fn clamps_the_tools_choice_to_the_allowed_set_at_runtime() {
    // A tool the parent lacks is dropped, never granted, even if asked.
    let (mut options, built) = options();
    options.inherited_tools = names(&["read", "shell"]);
    drive(
        options,
        json!({"task": "x", "tools": ["read", "shell", "write", "web_search"]}),
    )
    .await;
    assert_eq!(built.spec().tools, Some(names(&["read", "shell"])));
}

#[tokio::test]
async fn coerces_a_scalar_tools_argument_to_a_one_element_list_and_clamps_it() {
    let (mut options, built) = options();
    options.inherited_tools = names(&["read", "shell"]);
    drive(options, json!({"task": "x", "tools": "read"})).await;
    assert_eq!(built.spec().tools, Some(names(&["read"])));
}

#[tokio::test]
async fn yields_no_tools_for_a_malformed_tools_argument_never_widening_to_the_whole_set() {
    let (mut options, built) = options();
    options.inherited_tools = names(&["read", "shell"]);
    drive(options, json!({"task": "x", "tools": 123})).await;
    assert_eq!(built.spec().tools, Some(Vec::new()));
}

#[derive(Clone)]
struct Scratch(u32);

#[tokio::test]
async fn forwards_the_parent_invocation_state_to_the_child() {
    let seen = Arc::new(Mutex::new(None));
    let record = Arc::clone(&seen);
    let probe = FunctionTool::new(
        "probe",
        "reads the scratch value",
        move |_input: JsonValue, ctx: ToolContext| {
            let record = Arc::clone(&record);
            async move {
                *record.lock().unwrap() = ctx.invocation_state().get::<Scratch>().map(|scratch| scratch.0);
                Ok::<_, ToolError>("ok")
            }
        },
    );
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![tool_use("probe", "p1", json!({}))]);
    model.add_turn(vec![text("done")]);
    let child = Agent::builder().model(model).tools([probe]).build().unwrap();
    let options = MakeSubagentOptions::builder()
        .builder(builder_of(child))
        .presets(generalist())
        .build();
    let tool = make_subagent(options).unwrap();
    let context = context(json!({"task": "x"}));
    context.invocation_state().insert(Scratch(1));
    drive_in(tool.as_ref(), context).await;
    assert_eq!(*seen.lock().unwrap(), Some(1));
}

#[tokio::test]
async fn ignores_a_model_value_on_a_fixed_axis_and_an_off_enum_choice_value() {
    // A Fixed pin can't be overridden, and a Fixed('none') context can't be flipped to
    // full-history sharing by the model emitting the key.
    let (mut options, built) = options();
    options.instructions = Some(Fixed(Some("pinned".to_owned())).into());
    options.context = Some(Fixed(ContextMode::None).into());
    drive(
        options,
        json!({"task": "x", "instructions": "override me", "context": "all"}),
    )
    .await;
    assert_eq!(built.spec().instructions.as_deref(), Some("pinned"));
    assert_eq!(built.spec().context, ContextMode::None);
}

#[tokio::test]
async fn falls_back_to_the_default_when_a_choice_value_was_never_offered() {
    let (mut options, built) = options();
    options.presets = generalist();
    options.context = Some(none_or("all"));
    drive(options, json!({"task": "x", "context": "no_tools"})).await;
    assert_eq!(built.spec().context, ContextMode::None);
}

#[tokio::test]
async fn grants_the_whole_allowed_set_when_a_multiple_tools_choice_is_omitted() {
    let (mut options, built) = options();
    options.inherited_tools = names(&["read", "shell"]);
    drive(options, json!({"task": "x"})).await;
    assert_eq!(built.spec().tools, Some(names(&["read", "shell"])));
}

#[tokio::test]
async fn resolves_a_model_choice_name_back_to_the_original_instance() {
    let sentinel: Arc<dyn strands::models::Model> = Arc::new(MockMessageModel::new());
    let (mut options, built) = options();
    options.model = Some(ModelMode::Choice(Choice::new([ChoiceOption::new("smart")
        .with_value(Arc::clone(&sentinel))
        .with_description("the good one")])));
    drive(options, json!({"task": "x", "model": "smart"})).await;
    match built.spec().model {
        Some(ModelSource::Model(model)) => assert!(Arc::ptr_eq(&model, &sentinel)),
        other => panic!("expected the model instance, got {other:?}"),
    }
}

#[test]
fn maps_a_label_name_to_its_value() {
    // The model picks the 'full' label, which resolves to the 'all' context mode.
    let axis = ContextAxis::Choice(Choice::new([
        ChoiceOption::new("fresh").with_value(ContextMode::None),
        ChoiceOption::new("full").with_value(ContextMode::All),
    ]));
    let parent = [user(vec![text("earlier")])];
    let prompt = prompt(axis, json!({"task": "x", "context": "full"}), &parent);
    assert!(matches!(prompt, InvokeArgs::Messages(_)));
}
