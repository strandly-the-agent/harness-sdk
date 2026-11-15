//! Ported from strands-ts/src/vended-tools/subagent/__tests__/subagent.test.ts
//!
//! The TS `FakeChild` stand-in becomes a real child agent on a `MockMessageModel`: what the
//! child was prompted with is read from the mock's first request.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::agent::{InvocationContext, ModelSource, TakeSnapshotOptions};
use crate::interrupt::{Interrupt, InterruptParams};
use crate::models::streaming::Usage;
use crate::state_store::StateStore;
use crate::test_support::WarnRecorder;
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::tools::{FunctionTool, ToolError};
use crate::types::agent::{InterruptState, InvocationState, InvokeLimits, LocalAgent};
use crate::types::messages::{
    InjectedBlock, ReasoningBlock, ToolResultContent, ToolUseBlock, limit_stop_message,
};
use crate::types::tools::ToolUse;

const DEPTH_KEY: &str = "strands.subagent_depth";

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn tool_use(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

fn text_message(role: Role, body: &str) -> Message {
    Message::new(role, vec![text(body)])
}

fn tool_use_message(id: &str, name: &str) -> Message {
    Message::new(Role::Assistant, vec![tool_use(name, id, json!({}))])
}

fn tool_result() -> ContentBlock {
    ToolResultBlock::new(
        "r1",
        ToolResultStatus::Success,
        vec![ToolResultContent::from("A")],
    )
    .into()
}

fn framed(task: &str) -> ContentBlock {
    text(&format!("{FORK_PREAMBLE}\n\n{task}"))
}

/// Role and serialized content, ignoring message metadata.
fn shape(message: &Message) -> JsonValue {
    json!({ "role": message.role().as_str(), "content": serde_json::to_value(message.content()).unwrap() })
}

/// A builder recording each spec and building children on one scripted model.
struct Capture {
    builder: AgentBuilder,
    specs: Arc<Mutex<Vec<AgentSpec>>>,
    model: Arc<MockMessageModel>,
    states: Arc<Mutex<Vec<StateStore>>>,
}

fn capturing() -> Capture {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![text("done")]);
    capturing_on(model)
}

fn capturing_on(model: Arc<MockMessageModel>) -> Capture {
    let specs = Arc::new(Mutex::new(Vec::new()));
    let states = Arc::new(Mutex::new(Vec::new()));
    let (seen, kept, child_model) = (Arc::clone(&specs), Arc::clone(&states), Arc::clone(&model));
    let builder: AgentBuilder = Arc::new(move |spec: &AgentSpec| {
        seen.lock().unwrap().push(spec.clone());
        let child = Agent::builder()
            .model(Arc::clone(&child_model))
            .printer(false)
            .build()?;
        kept.lock().unwrap().push(child.app_state().clone());
        Ok(child)
    });
    Capture {
        builder,
        specs,
        model,
        states,
    }
}

impl Capture {
    fn tool(&self) -> SubagentTool {
        self.tool_with(MakeSubagentOptions::default())
    }

    fn tool_with(&self, options: MakeSubagentOptions) -> SubagentTool {
        make_subagent(MakeSubagentOptions {
            builder: Some(Arc::clone(&self.builder)),
            ..options
        })
        .unwrap()
    }

    fn specs(&self) -> Vec<AgentSpec> {
        self.specs.lock().unwrap().clone()
    }

    /// The messages the child was first prompted with.
    fn prompt(&self) -> Vec<Message> {
        self.model.requests()[0].messages.clone()
    }

    fn prompt_text(&self) -> String {
        let prompt = self.prompt();
        assert_eq!(prompt.len(), 1);
        match prompt[0].content() {
            [ContentBlock::Text(text)] => text.text().to_owned(),
            other => panic!("expected one text block, got {other:?}"),
        }
    }
}

fn parent_agent(messages: Vec<Message>) -> LocalAgent {
    let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    agent.set_messages(messages);
    agent
}

fn context_for(agent: &LocalAgent, input: JsonValue) -> ToolContext {
    ToolContext::new(
        ToolUse::new("subagent", "t1", input),
        agent.tool_agent(),
        InvocationState::new(),
    )
}

/// Streams `tool` and returns the event data it yielded and its outcome.
async fn run_in(tool: &SubagentTool, context: ToolContext) -> (Vec<JsonValue>, ToolOutcome) {
    let mut items: Vec<ToolStreamItem> = tool.stream(context).collect().await;
    let Some(ToolStreamItem::Outcome(outcome)) = items.pop() else {
        panic!("expected an outcome last");
    };
    let events = items
        .into_iter()
        .map(|item| match item {
            ToolStreamItem::Event(event) => event.data().cloned().unwrap_or_default(),
            other => panic!("expected an event, got {other:?}"),
        })
        .collect();
    (events, outcome)
}

async fn run(tool: &SubagentTool, input: JsonValue) -> ToolResultBlock {
    run_on(tool, input, &parent_agent(Vec::new())).await
}

async fn run_on(tool: &SubagentTool, input: JsonValue, parent: &LocalAgent) -> ToolResultBlock {
    result_of(run_in(tool, context_for(parent, input)).await.1)
}

fn result_of(outcome: ToolOutcome) -> ToolResultBlock {
    match outcome {
        ToolOutcome::Result(result) => result,
        other => panic!("expected a result, got {other:?}"),
    }
}

fn result_text(result: &ToolResultBlock) -> &str {
    match result.content() {
        [ToolResultContent::Text(text)] => text.text(),
        other => panic!("expected one text block, got {other:?}"),
    }
}

fn properties(tool: &SubagentTool) -> JsonObject {
    tool.tool_spec().input_schema().unwrap()["properties"]
        .as_object()
        .unwrap()
        .clone()
}

fn keys(tool: &SubagentTool) -> Vec<String> {
    properties(tool).keys().cloned().collect()
}

fn no_presets() -> Option<IndexMap<String, Preset>> {
    Some(IndexMap::new())
}

fn stored_turns(state: &InterruptState) -> Vec<String> {
    state
        .lock()
        .context
        .get("subAgentInterruptedTurns")
        .and_then(JsonValue::as_object)
        .map(|turns| turns.keys().cloned().collect())
        .unwrap_or_default()
}

mod tool_metadata {
    use super::*;

    #[test]
    fn exposes_the_default_name_description_and_schema() {
        let tool = subagent();
        assert_eq!(tool.name(), "subagent");
        assert_eq!(tool.description(), DEFAULT_SUBAGENT_DESCRIPTION);
        assert_eq!(
            tool.tool_spec().input_schema(),
            Some(&json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": TASK_DESCRIPTION },
                    "agent_type": {
                        "type": "string",
                        "enum": ["generalist"],
                        "description": format!("Role to assign this agent. Omit to use the default ('generalist').\nOptions:\n- generalist: {GENERALIST_DESCRIPTION} (sets instructions)"),
                    },
                    "instructions": { "type": "string", "description": "A system prompt defining this agent's role." },
                },
                "required": ["task"],
            }))
        );
        assert_eq!(
            make_subagent(MakeSubagentOptions::default()).unwrap().tool_spec(),
            tool.tool_spec()
        );
    }

    #[test]
    fn uses_a_custom_name() {
        let tool = make_subagent(MakeSubagentOptions::builder().name("delegate").build()).unwrap();
        assert_eq!(tool.tool_spec().name(), "delegate");
    }

    #[test]
    fn uses_a_custom_description() {
        let tool = make_subagent(
            MakeSubagentOptions::builder()
                .description("Hand off research.")
                .build(),
        )
        .unwrap();
        assert_eq!(tool.tool_spec().description(), "Hand off research.");
    }

    #[test]
    fn derives_parameters_from_choice_axes() {
        let tool = make_subagent(MakeSubagentOptions {
            presets: no_presets(),
            instructions: InstructionsAxis::Choice(Choice::new(["concise", "verbose"])),
            model: ModelAxis::Choice(Choice::new(["fast", "deep"])),
            context: ContextAxis::Choice(Choice::new([
                ChoiceOption::new("none").with_description("fresh"),
                ChoiceOption::new("all"),
            ])),
            tools: ToolsAxis::Choice(Choice::multiple(["read", "shell"])),
            ..MakeSubagentOptions::default()
        })
        .unwrap();
        let properties = properties(&tool);
        assert_eq!(
            properties.keys().collect::<Vec<_>>(),
            [
                "task",
                "instructions",
                "tools",
                "model",
                "context",
                "last_messages"
            ]
        );
        assert_eq!(properties["instructions"]["enum"], json!(["concise", "verbose"]));
        assert_eq!(properties["tools"]["type"], "array");
        assert_eq!(
            properties["tools"]["items"],
            json!({ "type": "string", "enum": ["read", "shell"] })
        );
        assert_eq!(properties["model"]["enum"], json!(["fast", "deep"]));
        assert_eq!(properties["context"]["enum"], json!(["none", "all"]));
        assert!(
            properties["context"]["description"]
                .as_str()
                .unwrap()
                .ends_with("more tokens.\nOptions:\n- none: fresh")
        );
        assert_eq!(
            properties["last_messages"],
            json!({ "type": "integer", "description": LAST_MESSAGES_DESCRIPTION })
        );
    }

    #[test]
    fn omits_last_messages_when_the_only_context_option_is_none() {
        let tool = make_subagent(MakeSubagentOptions {
            presets: no_presets(),
            context: ContextAxis::Choice(Choice::new(["none"])),
            ..MakeSubagentOptions::default()
        })
        .unwrap();
        assert_eq!(keys(&tool), ["task", "instructions", "context"]);
    }

    #[test]
    fn hides_fixed_and_inherit_axes_from_the_model() {
        let tool = make_subagent(MakeSubagentOptions {
            presets: no_presets(),
            instructions: InstructionsAxis::Fixed(Some("x".into())),
            tools: ToolsAxis::Inherit,
            model: ModelAxis::Inherit,
            context: ContextAxis::Fixed(Some("none".into())),
            ..MakeSubagentOptions::default()
        })
        .unwrap();
        assert_eq!(keys(&tool), ["task"]);
    }

    #[test]
    fn adds_an_agent_type_parameter_naming_the_presets_and_default() {
        let tool = make_subagent(MakeSubagentOptions {
            presets: Some(IndexMap::from([
                ("generalist".to_owned(), generalist()),
                (
                    "reviewer".to_owned(),
                    Preset::builder()
                        .instructions("review")
                        .description("reviews diffs")
                        .build(),
                ),
            ])),
            default_preset: Some("generalist".into()),
            instructions: InstructionsAxis::Fixed(None),
            ..MakeSubagentOptions::default()
        })
        .unwrap();
        let properties = properties(&tool);
        assert_eq!(properties.keys().collect::<Vec<_>>(), ["task", "agent_type"]);
        assert_eq!(
            properties["agent_type"]["enum"],
            json!(["generalist", "reviewer"])
        );
        let description = properties["agent_type"]["description"].as_str().unwrap();
        assert!(description.contains("'generalist'"));
        assert!(description.contains("- reviewer: reviews diffs (sets instructions)"));
    }

    #[test]
    fn describes_the_fields_a_preset_sets() {
        let tool = make_subagent(MakeSubagentOptions {
            presets: Some(IndexMap::from([
                (
                    "a".to_owned(),
                    Preset::builder().tools(vec![]).model("some-model-id").build(),
                ),
                (
                    "b".to_owned(),
                    Preset::builder()
                        .tools(vec!["read".into(), "shell".into()])
                        .build(),
                ),
            ])),
            ..MakeSubagentOptions::default()
        })
        .unwrap();
        let description = properties(&tool)["agent_type"]["description"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            description
                .ends_with("- a: sets tools: none; model: some-model-id\n- b: sets tools: read, shell")
        );
    }
}

mod factory_validation {
    use super::*;

    #[test]
    fn rejects_invalid_options() {
        let cases = [
            (
                MakeSubagentOptions::builder().name("").build(),
                "name must be a non-empty string.",
            ),
            (
                MakeSubagentOptions::builder().description("").build(),
                "description must be a non-empty string.",
            ),
            (
                MakeSubagentOptions::builder().max_depth(0).build(),
                "maxDepth must be a positive integer (>= 1).",
            ),
            (
                MakeSubagentOptions::builder().default_preset("reseacher").build(),
                "defaultPreset 'reseacher' is not one of the presets: generalist.",
            ),
            (
                MakeSubagentOptions::builder()
                    .context(ContextAxis::Fixed(Some("al".into())))
                    .build(),
                "context mode \"al\" must be one of: none, all, no_tools.",
            ),
            (
                MakeSubagentOptions::builder()
                    .context(ContextAxis::Choice(Choice::new([
                        ChoiceOption::new("none"),
                        ChoiceOption::new("shared").with_value("everything"),
                    ])))
                    .build(),
                "context mode \"everything\" must be one of: none, all, no_tools.",
            ),
            (
                MakeSubagentOptions::builder()
                    .tools(ToolsAxis::Choice(Choice::new(["read"])))
                    .build(),
                "tools: new Choice(...) must set multiple to true; a child agent selects a subset of tools.",
            ),
            (
                MakeSubagentOptions::builder()
                    .model(ModelAxis::Choice(Choice::new(Vec::<&str>::new())))
                    .build(),
                "model: new Choice([]) offers no options; use new Fixed(...), or omit model to use its default.",
            ),
        ];
        for (options, message) in cases {
            assert_eq!(make_subagent(options).unwrap_err().to_string(), message);
        }
    }
}

mod resolution {
    use super::*;

    #[tokio::test]
    async fn builds_a_child_from_the_default_preset_and_returns_its_output() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![text("the answer")]);
        let capture = capturing_on(model);
        let parent = parent_agent(Vec::new());

        let (events, outcome) =
            run_in(&capture.tool(), context_for(&parent, json!({ "task": "do it" }))).await;

        assert_eq!(
            result_of(outcome),
            ToolResultBlock::new("t1", ToolResultStatus::Success, vec!["the answer".into()])
        );
        let specs = capture.specs();
        assert_eq!(specs[0].name.as_deref(), Some("subagent"));
        assert_eq!(specs[0].agent_type.as_deref(), Some("generalist"));
        assert_eq!(specs[0].instructions.as_deref(), Some(GENERALIST_INSTRUCTIONS));
        assert_eq!(events.last().unwrap()["type"], "agentResultEvent");
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "modelStreamUpdateEvent")
        );
    }

    #[tokio::test]
    async fn passes_a_copy_of_the_invocation_state_and_the_tool_cancel_signal_to_the_child() {
        #[derive(Clone, Debug, PartialEq)]
        struct Scratch(u32);
        let seen = Arc::new(Mutex::new(None));
        let probe = {
            let seen = Arc::clone(&seen);
            FunctionTool::new("probe", "probes", move |_input: JsonValue, ctx: ToolContext| {
                *seen.lock().unwrap() = ctx.invocation_state().get::<Scratch>();
                ctx.invocation_state().insert(Scratch(2));
                async { Ok::<_, ToolError>("ok") }
            })
        };
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use("probe", "p1", json!({}))]);
        model.add_turn(vec![text("done")]);
        let builder: AgentBuilder = {
            let (model, probe) = (Arc::clone(&model), Arc::new(probe));
            Arc::new(move |_spec: &AgentSpec| {
                Agent::builder()
                    .model(Arc::clone(&model))
                    .tools([Arc::clone(&probe) as Arc<dyn Tool>])
                    .printer(false)
                    .build()
            })
        };
        let tool = make_subagent(MakeSubagentOptions::builder().builder(builder).build()).unwrap();
        let (signal, handle) = crate::CancelSignal::new();
        let mut parent = parent_agent(Vec::new());
        parent.begin_cancel(Some(&signal));
        let state = InvocationState::new();
        state.insert(Scratch(1));
        let context = ToolContext::new(
            ToolUse::new("subagent", "t1", json!({ "task": "x" })),
            parent.tool_agent(),
            state.clone(),
        );

        let (_, outcome) = run_in(&tool, context).await;

        assert_eq!(*result_of(outcome).status(), ToolResultStatus::Success);
        assert_eq!(*seen.lock().unwrap(), Some(Scratch(1)));
        assert_eq!(state.get::<Scratch>(), Some(Scratch(1)));

        let cancelled = Capture::tool(&capturing());
        handle.cancel(crate::CancelOptions::default());
        let context = ToolContext::new(
            ToolUse::new("subagent", "t2", json!({ "task": "x" })),
            parent.tool_agent(),
            InvocationState::new(),
        );
        let result = result_of(run_in(&cancelled, context).await.1);
        assert_eq!(result_text(&result), "Subagent cancelled");
    }

    #[tokio::test]
    async fn runs_the_child_in_the_request_of_the_tool_call() {
        let capture = capturing();
        let request = InvocationContext::new(None, None, false);
        let mut parent = parent_agent(Vec::new());
        parent.set_invocation_context(Some(request.clone()));

        run_on(&capture.tool(), json!({ "task": "x" }), &parent).await;

        assert_eq!(request.usage().turns_taken(), 1);
    }

    #[tokio::test]
    async fn adds_the_childs_model_usage_to_the_requests_usage() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![text("child done")],
            MockTurnOptions {
                usage: Some(Usage::new(3, 4, 7)),
                ..MockTurnOptions::default()
            },
        );
        let capture = capturing_on(model);
        let request = InvocationContext::new(
            Some(InvokeLimits {
                turns: NonZeroU32::new(5),
                ..InvokeLimits::default()
            }),
            None,
            false,
        );
        let mut parent = parent_agent(Vec::new());
        parent.set_invocation_context(Some(request.clone()));

        let result = run_on(&capture.tool(), json!({ "task": "x" }), &parent).await;

        let usage = request.usage();
        assert_eq!(
            (
                result_text(&result),
                usage.turns_taken(),
                usage.tokens().input_tokens(),
                usage.tokens().output_tokens(),
                usage.tokens().total_tokens()
            ),
            ("child done", 1, 3, 4, 7)
        );
    }

    #[tokio::test]
    async fn maps_tools_choice_option_names_to_values() {
        let capture = capturing();
        let tool = capture.tool_with(MakeSubagentOptions {
            tools: ToolsAxis::Choice(Choice::multiple([
                ChoiceOption::new("readonly").with_value("read"),
                ChoiceOption::new("sh").with_value("shell"),
            ])),
            ..MakeSubagentOptions::default()
        });
        run(&tool, json!({ "task": "x", "tools": ["readonly"] })).await;
        assert_eq!(capture.specs()[0].tools, Some(vec!["read".to_owned()]));
    }

    #[tokio::test]
    async fn clamps_off_enum_tools() {
        let capture = capturing();
        let tool = capture.tool_with(MakeSubagentOptions {
            tools: ToolsAxis::Choice(Choice::multiple([
                ChoiceOption::new("readonly").with_value("read"),
                ChoiceOption::new("sh").with_value("shell"),
            ])),
            ..MakeSubagentOptions::default()
        });
        run(&tool, json!({ "task": "x", "tools": ["readonly", "write"] })).await;
        assert_eq!(capture.specs()[0].tools, Some(vec!["read".to_owned()]));
    }

    #[tokio::test]
    async fn ignores_model_values_for_fixed_axes() {
        let capture = capturing();
        let tool = capture.tool_with(MakeSubagentOptions {
            presets: no_presets(),
            instructions: InstructionsAxis::Fixed(Some("pinned".into())),
            context: ContextAxis::Fixed(Some("none".into())),
            ..MakeSubagentOptions::default()
        });
        let parent = parent_agent(vec![text_message(Role::User, "hi")]);

        run_on(
            &tool,
            json!({ "task": "x", "instructions": "override", "context": "all" }),
            &parent,
        )
        .await;

        assert_eq!(capture.specs()[0].instructions.as_deref(), Some("pinned"));
        assert_eq!(capture.prompt_text(), "x");
    }

    #[tokio::test]
    async fn rejects_invalid_input_as_an_error_result_without_building_a_child() {
        let cases = [
            (
                json!({ "task": "x", "agent_type": "nope" }),
                "Subagent error: Unknown agent_type \"nope\"; valid values: generalist.",
            ),
            (
                json!({}),
                "Missing required parameter 'task': describe the task to delegate.",
            ),
            (
                json!({ "task": "" }),
                "Missing required parameter 'task': describe the task to delegate.",
            ),
            (
                json!({ "task": "   " }),
                "Missing required parameter 'task': describe the task to delegate.",
            ),
            (
                json!({ "task": 42 }),
                "Missing required parameter 'task': describe the task to delegate.",
            ),
            (
                json!({ "task": "x", "instructions": ["not", "a", "string"] }),
                "Parameter 'instructions' must be a string.",
            ),
        ];
        for (input, message) in cases {
            let capture = capturing();
            let result = run(&capture.tool(), input.clone()).await;
            assert_eq!(*result.status(), ToolResultStatus::Error, "{input}");
            assert_eq!(result_text(&result), message);
            assert!(capture.specs().is_empty());
        }
    }

    #[tokio::test]
    async fn resolves_a_model_choice_and_a_preset_model() {
        let capture = capturing();
        let tool = capture.tool_with(MakeSubagentOptions {
            presets: Some(IndexMap::from([(
                "fast".to_owned(),
                Preset::builder().model("preset-model").build(),
            )])),
            model: ModelAxis::Choice(Choice::new(["model-a"])),
            ..MakeSubagentOptions::default()
        });
        run(&tool, json!({ "task": "x", "model": "model-a" })).await;
        run(&tool, json!({ "task": "x", "model": "off-enum" })).await;
        let models: Vec<String> = capture
            .specs()
            .into_iter()
            .map(|spec| match spec.model {
                Some(ModelSource::Id(id)) => id,
                other => panic!("expected a model id, got {other:?}"),
            })
            .collect();
        assert_eq!(models, ["model-a", "preset-model"]);
    }
}

mod context_modes {
    use super::*;

    fn all_context() -> MakeSubagentOptions {
        MakeSubagentOptions {
            context: ContextAxis::Choice(Choice::new(["none", "all"])),
            ..MakeSubagentOptions::default()
        }
    }

    #[tokio::test]
    async fn forks_messages_for_all_dropping_reasoning_injected_text_and_in_flight_tool_calls() {
        let capture = capturing();
        let tool = capture.tool_with(all_context());
        let parent = parent_agent(vec![
            Message::new(
                Role::User,
                vec![
                    text("go"),
                    InjectedBlock::new("<todos>v1</todos>", "todos").into(),
                ],
            ),
            Message::new(
                Role::Assistant,
                vec![
                    ReasoningBlock::new()
                        .with_text("think")
                        .with_signature("sig")
                        .into(),
                    tool_use("read", "r1", json!({})),
                ],
            ),
            Message::new(Role::User, vec![tool_result()]),
            tool_use_message("t1", "subagent"),
        ]);

        run_on(&tool, json!({ "task": "do X", "context": "all" }), &parent).await;

        let prompt: Vec<JsonValue> = capture.prompt().iter().map(shape).collect();
        assert_eq!(
            prompt,
            [
                shape(&text_message(Role::User, "go")),
                json!({ "role": "assistant", "content": [{ "toolUse": { "toolUseId": "r1", "name": "read", "input": {} } }] }),
                shape(&Message::new(Role::User, vec![tool_result(), framed("do X")])),
            ]
        );
        assert_eq!(parent.messages().len(), 4);
    }

    #[tokio::test]
    async fn appends_a_user_turn_when_the_last_forked_message_is_from_the_assistant_for_all() {
        let messages = vec![
            text_message(Role::User, "hi"),
            text_message(Role::Assistant, "hello"),
        ];
        let expected = vec![
            text_message(Role::User, "hi"),
            text_message(Role::Assistant, "hello"),
            Message::new(Role::User, vec![framed("x")]),
        ];
        assert_forked(messages, json!({}), &expected).await;
    }

    #[tokio::test]
    async fn merges_consecutive_same_role_messages_left_by_dropped_tool_calls_for_all() {
        let messages = vec![
            text_message(Role::User, "one"),
            tool_use_message("x1", "sub"),
            text_message(Role::User, "two"),
        ];
        let expected = vec![Message::new(
            Role::User,
            vec![text("one"), text("two"), framed("x")],
        )];
        assert_forked(messages, json!({}), &expected).await;
    }

    #[tokio::test]
    async fn widens_last_messages_so_a_tool_pair_is_never_split_for_all() {
        let messages = vec![
            text_message(Role::User, "old"),
            text_message(Role::Assistant, "ok"),
            text_message(Role::User, "recent"),
            tool_use_message("r1", "read"),
            Message::new(Role::User, vec![tool_result()]),
            tool_use_message("t1", "subagent"),
        ];
        let expected = vec![
            text_message(Role::User, "recent"),
            tool_use_message("r1", "read"),
            Message::new(Role::User, vec![tool_result(), framed("x")]),
        ];
        assert_forked(messages, json!({ "last_messages": 2 }), &expected).await;
    }

    async fn assert_forked(messages: Vec<Message>, input: JsonValue, expected: &[Message]) {
        let capture = capturing();
        let tool = capture.tool_with(all_context());
        let mut input = input;
        input["task"] = json!("x");
        input["context"] = json!("all");

        run_on(&tool, input, &parent_agent(messages)).await;

        let prompt: Vec<JsonValue> = capture.prompt().iter().map(shape).collect();
        assert_eq!(prompt, expected.iter().map(shape).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn handles_last_messages_edge_case() {
        for (last_messages, expected) in [
            (json!("bogus"), 3),
            (json!(0), 3),
            (json!(-5), 3),
            (json!("1"), 1),
        ] {
            let capture = capturing();
            let tool = capture.tool_with(all_context());
            let parent = parent_agent(vec![
                text_message(Role::User, "a"),
                text_message(Role::Assistant, "b"),
                text_message(Role::User, "c"),
            ]);
            run_on(
                &tool,
                json!({ "task": "x", "context": "all", "last_messages": last_messages }),
                &parent,
            )
            .await;
            assert_eq!(capture.prompt().len(), expected, "{last_messages}");
        }
    }

    #[tokio::test]
    async fn sends_the_plain_task_for() {
        for (messages, context) in [
            (Vec::new(), "all"),
            (vec![text_message(Role::User, "a")], "no_tools"),
        ] {
            let capture = capturing();
            let tool = capture.tool_with(all_context());
            run_on(
                &tool,
                json!({ "task": "x", "context": context }),
                &parent_agent(messages),
            )
            .await;
            assert_eq!(capture.prompt_text(), "x", "{context}");
        }
    }

    async fn assert_rendered(messages: Vec<Message>, input: JsonValue, transcript: &str) {
        let capture = capturing();
        let tool = capture.tool_with(MakeSubagentOptions {
            context: ContextAxis::Choice(Choice::new(["none", "no_tools"])),
            ..MakeSubagentOptions::default()
        });
        let mut input = input;
        input["task"] = json!("do X");
        input["context"] = json!("no_tools");

        run_on(&tool, input, &parent_agent(messages)).await;

        assert_eq!(
            capture.prompt_text(),
            format!("<parent_context>\n{transcript}\n</parent_context>\n\n{CONTEXT_PREAMBLE}\n\ndo X")
        );
    }

    #[tokio::test]
    async fn renders_text_turns_as_a_framed_block_for_no_tools() {
        assert_rendered(
            vec![
                text_message(Role::User, "hello\nworld"),
                tool_use_message("r1", "read"),
                text_message(Role::Assistant, "see </parent_context> here"),
            ],
            json!({}),
            "user: hello\n  world\nassistant: see <\\/parent_context> here",
        )
        .await;
    }

    #[tokio::test]
    async fn limits_to_the_last_n_messages_for_no_tools() {
        assert_rendered(
            vec![
                text_message(Role::User, "first"),
                text_message(Role::Assistant, "second"),
            ],
            json!({ "last_messages": 1 }),
            "assistant: second",
        )
        .await;
    }

    #[tokio::test]
    async fn strips_a_nested_subagents_framing_for_no_tools() {
        assert_rendered(
            vec![text_message(
                Role::User,
                &format!("<parent_context>\nold\n</parent_context>\n\n{CONTEXT_PREAMBLE}\n\nreal task"),
            )],
            json!({}),
            "user: real task",
        )
        .await;
    }
}

mod errors_and_cancellation {
    use super::*;

    #[test]
    fn turns_a_builder_exception_into_an_error_result_and_logs_it() {
        let builder: AgentBuilder = Arc::new(|_spec: &AgentSpec| Err(Error::other("boom")));
        let tool = make_subagent(MakeSubagentOptions::builder().builder(builder).build()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let (result, warnings) =
            WarnRecorder::capture(|| runtime.block_on(run(&tool, json!({ "task": "x" }))));

        assert_eq!(
            result,
            ToolResultBlock::new("t1", ToolResultStatus::Error, vec!["Subagent error: boom".into()])
        );
        assert_eq!(warnings, ["subagent failed"]);
    }

    #[tokio::test]
    async fn turns_into_an_error_result() {
        let capture = capturing();
        let request = InvocationContext::new(
            Some(InvokeLimits {
                turns: NonZeroU32::new(1),
                ..InvokeLimits::default()
            }),
            None,
            false,
        );
        request.set_turns_taken(1);
        let mut parent = parent_agent(Vec::new());
        parent.set_invocation_context(Some(request));

        let result = run_on(&capture.tool(), json!({ "task": "x" }), &parent).await;

        assert_eq!(
            result,
            ToolResultBlock::new(
                "t1",
                ToolResultStatus::Error,
                vec![limit_stop_message("subagent", &StopReason::LimitTurns).into()]
            )
        );
    }

    #[tokio::test]
    async fn turns_a_failed_child_into_an_error_result() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| Error::other("model down"));
        let capture = capturing_on(model);

        let result = run(&capture.tool(), json!({ "task": "x" })).await;

        assert_eq!(*result.status(), ToolResultStatus::Error);
        assert!(
            result_text(&result).starts_with("Subagent error: "),
            "{}",
            result_text(&result)
        );
    }

    #[tokio::test]
    async fn turns_an_immediately_cancelled_child_into_an_error_result() {
        let capture = capturing();
        let (signal, handle) = crate::CancelSignal::new();
        let mut parent = parent_agent(Vec::new());
        parent.begin_cancel(Some(&signal));
        handle.cancel(crate::CancelOptions::default());

        let result = run_on(&capture.tool(), json!({ "task": "x" }), &parent).await;

        assert_eq!(
            result,
            ToolResultBlock::new("t1", ToolResultStatus::Error, vec!["Subagent cancelled".into()])
        );
    }

    #[tokio::test]
    async fn treats_a_child_that_ended_via_a_deferred_cancel_stop_tool_as_a_success() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use("stop", "s1", json!({ "message": "summary" }))]);
        let builder: AgentBuilder = {
            let model = Arc::clone(&model);
            Arc::new(move |_spec: &AgentSpec| {
                Agent::builder()
                    .model(Arc::clone(&model))
                    .tools([crate::vended_tools::stop()])
                    .printer(false)
                    .build()
            })
        };
        let tool = make_subagent(MakeSubagentOptions::builder().builder(builder).build()).unwrap();

        let result = run(&tool, json!({ "task": "x" })).await;

        assert_eq!(
            result,
            ToolResultBlock::new("t1", ToolResultStatus::Success, vec!["summary".into()])
        );
    }
}

mod depth_tracking {
    use super::*;

    #[tokio::test]
    async fn refuses_to_delegate_once_the_depth_is_exhausted() {
        let capture = capturing();
        let tool = capture.tool_with(MakeSubagentOptions {
            max_depth: Some(3),
            name: Some("delegate".into()),
            ..MakeSubagentOptions::default()
        });
        let parent = parent_agent(Vec::new());
        parent.app_state().set(DEPTH_KEY, 0).unwrap();

        let result = run_on(&tool, json!({ "task": "x" }), &parent).await;

        assert_eq!(*result.status(), ToolResultStatus::Error);
        assert_eq!(
            result_text(&result),
            "Delegation depth limit reached (3 levels); you cannot delegate further. Complete this task yourself instead of calling delegate again."
        );
        assert!(capture.specs().is_empty());
    }

    #[tokio::test]
    async fn sets_the_child_depth() {
        for (name, max_depth, parent_depth, expected) in [
            ("starts at maxDepth and decrements", Some(3), None, 2),
            ("starts at the default maxDepth of 2", None, None, 1),
            ("decrements from the depth stored on the parent", None, Some(1), 0),
        ] {
            let capture = capturing();
            let tool = capture.tool_with(MakeSubagentOptions {
                max_depth,
                ..MakeSubagentOptions::default()
            });
            let parent = parent_agent(Vec::new());
            if let Some(depth) = parent_depth {
                parent.app_state().set(DEPTH_KEY, depth).unwrap();
            }
            run_on(&tool, json!({ "task": "x" }), &parent).await;
            let child_state = capture.states.lock().unwrap()[0].clone();
            assert_eq!(child_state.get_value(DEPTH_KEY), Some(json!(expected)), "{name}");
        }
    }
}

mod interrupts {
    use super::*;

    const CHILD_ID: &str = "subagent:t1:tool:c1:confirm";

    struct Confirming {
        parent: Agent,
        executions: Arc<Mutex<Vec<String>>>,
        fail_next_build: Arc<Mutex<bool>>,
    }

    /// A parent delegating a confirm-guarded action to a subagent; a `rebuilt` parent's models
    /// start at the turns that follow the interrupted tool call.
    fn confirming_parent(rebuilt: bool) -> Confirming {
        let executions = Arc::new(Mutex::new(Vec::new()));
        let dangerous = {
            let executions = Arc::clone(&executions);
            FunctionTool::new(
                "dangerousAction",
                "acts",
                move |input: JsonValue, ctx: ToolContext| {
                    let executions = Arc::clone(&executions);
                    async move {
                        let answer = ctx.interrupt(InterruptParams {
                            name: "confirm".into(),
                            reason: Some(json!("confirm?")),
                            ..Default::default()
                        })?;
                        if answer != json!("APPROVE") {
                            return Ok::<_, ToolError>("not approved".to_owned());
                        }
                        let target = input["target"].as_str().unwrap_or_default().to_owned();
                        executions.lock().unwrap().push(target.clone());
                        Ok(format!("done: {target}"))
                    }
                },
            )
        };
        let child_model = Arc::new(MockMessageModel::new());
        if !rebuilt {
            child_model.add_turn(vec![tool_use(
                "dangerousAction",
                "c1",
                json!({ "target": "prod" }),
            )]);
        }
        child_model.add_turn(vec![text("child done")]);
        let fail_next_build = Arc::new(Mutex::new(false));
        let builder: AgentBuilder = {
            let (dangerous, fail) = (Arc::new(dangerous), Arc::clone(&fail_next_build));
            Arc::new(move |_spec: &AgentSpec| {
                if std::mem::take(&mut *fail.lock().unwrap()) {
                    return Err(Error::other("mcp down"));
                }
                Agent::builder()
                    .model(Arc::clone(&child_model))
                    .tools([Arc::clone(&dangerous) as Arc<dyn Tool>])
                    .printer(false)
                    .build()
            })
        };
        let parent_model = Arc::new(MockMessageModel::new());
        if !rebuilt {
            parent_model.add_turn(vec![tool_use("subagent", "t1", json!({ "task": "do it" }))]);
        }
        parent_model.add_turn(vec![text("parent done")]);
        let parent = Agent::builder()
            .model(parent_model)
            .tools([make_subagent(MakeSubagentOptions::builder().builder(builder).build()).unwrap()])
            .printer(false)
            .build()
            .unwrap();
        Confirming {
            parent,
            executions,
            fail_next_build,
        }
    }

    fn approve() -> Vec<InterruptResponse> {
        vec![InterruptResponse::new(CHILD_ID, json!("APPROVE"))]
    }

    #[tokio::test]
    async fn resumes_the_stored_child_turn_after_the_parent_is_rebuilt() {
        let mut first = confirming_parent(false);
        let interrupted = first.parent.invoke("go").await.unwrap();
        assert_eq!(interrupted.interrupts()[0].id(), CHILD_ID);
        assert_eq!(stored_turns(first.parent.local.interrupt_state()), ["t1"]);
        let snapshot = serde_json::to_string(
            &first
                .parent
                .take_snapshot(&TakeSnapshotOptions::session())
                .unwrap(),
        )
        .unwrap();

        // Process boundary: every agent is rebuilt and only the snapshot survives.
        let mut second = confirming_parent(true);
        second
            .parent
            .load_snapshot(&serde_json::from_str(&snapshot).unwrap())
            .unwrap();
        let result = second.parent.invoke(approve()).await.unwrap();

        assert_eq!(result.to_string(), "parent done");
        assert_eq!(*second.executions.lock().unwrap(), ["prod"]);
        let messages = second.parent.messages();
        assert_eq!(
            messages[messages.len() - 2].content(),
            [ContentBlock::from(ToolResultBlock::new(
                "t1",
                ToolResultStatus::Success,
                vec!["child done".into()]
            ))]
        );
    }

    #[tokio::test]
    async fn raises_the_interrupt_again_and_keeps_the_turn_when_the_child_cannot_be_rebuilt_on_resume() {
        let mut confirming = confirming_parent(false);
        confirming.parent.invoke("go").await.unwrap();
        *confirming.fail_next_build.lock().unwrap() = true;

        let failed = confirming.parent.invoke(approve()).await.unwrap();

        let raised: Vec<(&str, Option<&JsonValue>)> = failed
            .interrupts()
            .iter()
            .map(|interrupt| (interrupt.id(), interrupt.response()))
            .collect();
        assert_eq!(raised, [(CHILD_ID, None)]);
        assert_eq!(stored_turns(confirming.parent.local.interrupt_state()), ["t1"]);

        let result = confirming.parent.invoke(approve()).await.unwrap();

        assert_eq!(result.to_string(), "parent done");
        assert_eq!(*confirming.executions.lock().unwrap(), ["prod"]);
    }

    #[tokio::test]
    async fn raises_the_interrupts_again_when_none_were_answered() {
        let parent = parent_agent(Vec::new());
        {
            let mut state = parent.interrupt_state().lock();
            state.interrupts.insert(
                "subagent:t1:i1".into(),
                Interrupt::new("subagent:t1:i1", "confirm"),
            );
            state.activated = true;
        }

        let (_, outcome) = run_in(&capturing().tool(), context_for(&parent, json!({ "task": "x" }))).await;

        match outcome {
            ToolOutcome::Interrupt(interrupt) => assert_eq!(interrupt.id(), "subagent:t1:i1"),
            other => panic!("expected an interrupt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn returns_an_error_when_the_interrupted_child_turn_is_not_available() {
        let parent = parent_agent(Vec::new());
        {
            let mut state = parent.interrupt_state().lock();
            state.interrupts.insert(
                "subagent:t1:i1".into(),
                Interrupt::new("subagent:t1:i1", "confirm"),
            );
            state.activated = true;
        }
        parent
            .interrupt_state()
            .lock()
            .resume(&[InterruptResponse::new("subagent:t1:i1", json!("yes"))])
            .unwrap();

        let result = run_on(&capturing().tool(), json!({ "task": "x" }), &parent).await;

        assert_eq!(*result.status(), ToolResultStatus::Error);
        assert_eq!(result_text(&result), UNAVAILABLE_MESSAGE);
    }
}

mod with_real_agents {
    use super::*;

    fn confirm_tool(seen: &Arc<Mutex<Vec<JsonValue>>>) -> FunctionTool {
        let seen = Arc::clone(seen);
        FunctionTool::new(
            "confirmTool",
            "confirms",
            move |_input: JsonValue, ctx: ToolContext| {
                let seen = Arc::clone(&seen);
                async move {
                    let response = ctx.interrupt(InterruptParams {
                        name: "confirm".into(),
                        reason: Some(json!("Please confirm")),
                        ..Default::default()
                    })?;
                    seen.lock().unwrap().push(response);
                    Ok::<_, ToolError>("ok")
                }
            },
        )
    }

    fn agent_with(model: &Arc<MockMessageModel>, seen: &Arc<Mutex<Vec<JsonValue>>>) -> Agent {
        Agent::builder()
            .model(Arc::clone(model))
            .tools(crate::tool_list![subagent(), confirm_tool(seen)])
            .printer(false)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn delegates_to_a_child_built_from_the_parent_propagating_a_child_tool_interrupt_and_resuming_it() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        // The child inherits the parent's model, so turns are consumed in order across both agents.
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use("subagent", "t1", json!({ "task": "confirm it" }))]);
        model.add_turn(vec![tool_use("confirmTool", "inner-1", json!({}))]);
        model.add_turn(vec![text("child report")]);
        model.add_turn(vec![text("parent done")]);
        let mut parent = agent_with(&model, &seen);

        let interrupted = parent.invoke("go").await.unwrap();

        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        let interrupt = &interrupted.interrupts()[0];
        let id = "subagent:t1:tool:inner-1:confirm";
        assert_eq!(
            (interrupt.id(), interrupt.name(), interrupt.reason()),
            (id, "confirm", Some(&json!("Please confirm")))
        );

        let result = parent
            .invoke(vec![InterruptResponse::new(id, json!("yes"))])
            .await
            .unwrap();

        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.to_string(), "parent done");
        assert_eq!(*seen.lock().unwrap(), [json!("yes")]);
        assert_eq!(
            parent.messages()[2].content(),
            [ContentBlock::from(ToolResultBlock::new(
                "t1",
                ToolResultStatus::Success,
                vec!["child report".into()]
            ))]
        );
    }

    #[tokio::test]
    async fn resumes_the_same_child_across_consecutive_interrupts() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use(
            "subagent",
            "t1",
            json!({ "task": "confirm twice" }),
        )]);
        model.add_turn(vec![tool_use("confirmTool", "inner-1", json!({}))]);
        model.add_turn(vec![tool_use("confirmTool", "inner-2", json!({}))]);
        model.add_turn(vec![text("child report")]);
        model.add_turn(vec![text("parent done")]);
        let mut parent = agent_with(&model, &seen);

        let first = parent.invoke("go").await.unwrap();
        assert_eq!(first.interrupts()[0].id(), "subagent:t1:tool:inner-1:confirm");

        let second = parent
            .invoke(vec![InterruptResponse::new(
                "subagent:t1:tool:inner-1:confirm",
                json!("A"),
            )])
            .await
            .unwrap();
        assert_eq!(second.interrupts()[0].id(), "subagent:t1:tool:inner-2:confirm");

        let result = parent
            .invoke(vec![InterruptResponse::new(
                "subagent:t1:tool:inner-2:confirm",
                json!("B"),
            )])
            .await
            .unwrap();

        assert_eq!(result.to_string(), "parent done");
        assert_eq!(*seen.lock().unwrap(), [json!("A"), json!("B")]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delegates_two_levels_deep() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use(
            "subagent",
            "t1",
            json!({ "task": "delegate again" }),
        )]);
        model.add_turn(vec![tool_use("subagent", "t2", json!({ "task": "do it" }))]);
        model.add_turn(vec![text("grandchild report")]);
        model.add_turn(vec![text("child report")]);
        model.add_turn(vec![text("parent done")]);
        let mut parent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([subagent()])
            .printer(false)
            .build()
            .unwrap();

        let result = parent.invoke("go").await.unwrap();

        assert_eq!(result.to_string(), "parent done");
        assert_eq!(
            parent.messages()[2].content(),
            [ContentBlock::from(ToolResultBlock::new(
                "t1",
                ToolResultStatus::Success,
                vec!["child report".into()]
            ))]
        );
    }

    #[tokio::test]
    async fn stops_delegating_at_the_default_depth() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![tool_use("subagent", "t1", json!({ "task": "a" }))]);
        model.add_turn(vec![tool_use("subagent", "t2", json!({ "task": "b" }))]);
        model.add_turn(vec![tool_use("subagent", "t3", json!({ "task": "c" }))]);
        model.add_turn(vec![text("grandchild report")]);
        model.add_turn(vec![text("child report")]);
        model.add_turn(vec![text("parent done")]);
        let mut parent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([subagent()])
            .printer(false)
            .build()
            .unwrap();

        parent.invoke("go").await.unwrap();

        let grandchild_request = &model.requests()[3];
        let refused = grandchild_request
            .messages
            .iter()
            .flat_map(Message::content)
            .find_map(|block| match block {
                ContentBlock::ToolResult(result) if result.tool_use_id() == "t3" => Some(result.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(*refused.status(), ToolResultStatus::Error);
        assert!(result_text(&refused).starts_with("Delegation depth limit reached (2 levels)"));
    }
}
