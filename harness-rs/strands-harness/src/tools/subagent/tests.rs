//! Tests of the `subagent` tool, ported from harness-ts/test/tools/subagent.test.ts.
//!
//! The TS suite drives the tool with fake children and a fake parent. Here the children are real
//! agents over [`MockMessageModel`]; a parent's messages reach the tool through the same two
//! steps the tool runs (`Axes::resolve`, then `context::child_prompt`).

use std::sync::{Arc, Mutex};

use futures::{FutureExt, StreamExt};
use indexmap::IndexMap;
use serde_json::json;
use strands::agent::ModelSource;
use strands::error::ModelError;
use strands::interrupt::InterruptParams;
use strands::testing::{MockMessageModel, MockTurnOptions, test_agent, tool_context};
use strands::tools::{FunctionTool, ToolStreamEvent, ToolStreamItem};
use strands::types::media::{ImageBlock, ImageFormat, ImageSource};
use strands::types::messages::{
    JsonBlock, ReasoningBlock, Role, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};
use strands::types::tools::{ToolResultStatus, ToolUse};
use strands::{
    Agent, ContentBlock, InterruptResponse, InvokeArgs, JsonValue, Message, StateStore, StopReason, Tool,
    ToolContext, ToolError, ToolOutcome,
};

use super::context::{CONTEXT_PREAMBLE, FORK_PREAMBLE, child_prompt};
use super::*;

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn tool_use(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

fn tool_result(id: &str, content: Vec<ToolResultContent>) -> ContentBlock {
    ToolResultBlock::new(id, ToolResultStatus::Success, content).into()
}

fn user(content: Vec<ContentBlock>) -> Message {
    Message::new(Role::User, content)
}

fn assistant(content: Vec<ContentBlock>) -> Message {
    Message::new(Role::Assistant, content)
}

/// What the builder saw and built: the specs, and each child's app state.
#[derive(Default)]
struct Built {
    specs: Mutex<Vec<AgentSpec>>,
    states: Mutex<Vec<StateStore>>,
}

impl Built {
    fn spec(&self) -> AgentSpec {
        self.specs
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a child was built")
    }
}

/// A builder whose children run over `model`.
fn builder_over(model: &Arc<MockMessageModel>) -> (AgentBuilder, Arc<Built>) {
    let built = Arc::new(Built::default());
    let (record, model) = (Arc::clone(&built), Arc::clone(model));
    let builder: AgentBuilder = Arc::new(move |spec| {
        record.specs.lock().unwrap().push(spec);
        let child = test_agent(model.clone());
        record.states.lock().unwrap().push(child.app_state().clone());
        async move { Ok(child) }.boxed()
    });
    (builder, built)
}

/// A model that answers once with `reply` and `stop_reason`.
fn replying(reply: &str, stop_reason: StopReason) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    let content = if reply.is_empty() {
        Vec::new()
    } else {
        vec![text(reply)]
    };
    model.add_turn_with(
        content,
        MockTurnOptions {
            stop_reason: Some(stop_reason),
            ..MockTurnOptions::default()
        },
    );
    model
}

/// A builder whose child answers `done`.
fn builder() -> (AgentBuilder, Arc<Built>) {
    builder_over(&replying("done", StopReason::EndTurn))
}

/// `builder` as the closure the options builder takes.
fn calling(builder: AgentBuilder) -> impl Fn(AgentSpec) -> BoxFuture<'static, Result<Agent>> + Send + Sync {
    move |spec| builder(spec)
}

fn options() -> (MakeSubagentOptions, Arc<Built>) {
    let (builder, built) = builder();
    (
        MakeSubagentOptions::builder().builder(calling(builder)).build(),
        built,
    )
}

// No TS case: these are the TS call shapes (`new Fixed('..')`, `builder: (spec) => agent`).
#[tokio::test]
async fn the_ts_value_forms_of_fixed_and_an_async_closure_builder_need_no_wrapping() {
    let tasks = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&tasks);
    let options = MakeSubagentOptions::builder()
        .builder(move |spec| {
            seen.lock().unwrap().push(spec.task);
            async move { Ok(test_agent(replying("done", StopReason::EndTurn))) }
        })
        .instructions(Fixed("You review code."))
        .tools(Fixed(["read"]))
        .mcp_servers(Fixed(vec!["docs".to_owned()]))
        .model(Fixed("anthropic/claude-haiku-4-5"))
        .build();

    assert!(
        matches!(&options.instructions, Some(InstructionsMode::Fixed(Some(text))) if text == "You review code.")
    );
    assert!(matches!(&options.tools, Some(ToolsMode::Fixed(Some(tools))) if *tools == ["read"]));
    assert!(matches!(&options.mcp_servers, Some(ToolsMode::Fixed(Some(servers))) if *servers == ["docs"]));
    assert!(matches!(
        &options.model,
        Some(ModelMode::Fixed(Some(ModelSource::Id(id)))) if id == "anthropic/claude-haiku-4-5"
    ));
    let owned = MakeSubagentOptions::builder()
        .builder(|_spec| async { Ok(test_agent(Arc::new(MockMessageModel::new()))) })
        .instructions(Fixed("x".to_owned()))
        .model(Fixed("anthropic/claude-haiku-4-5".to_owned()))
        .build();
    assert!(matches!(
        owned.instructions,
        Some(InstructionsMode::Fixed(Some(_)))
    ));
    assert!(matches!(
        owned.model,
        Some(ModelMode::Fixed(Some(ModelSource::Id(_))))
    ));

    let result = drive(options, json!({"task": "review the diff"})).await;

    assert_eq!(result.status(), &ToolResultStatus::Success);
    assert_eq!(*tasks.lock().unwrap(), ["review the diff"]);
}

fn generalist() -> IndexMap<String, Preset> {
    IndexMap::from([("generalist".to_owned(), GENERALIST.clone())])
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn context(input: JsonValue) -> ToolContext {
    tool_context(ToolUse::new("subagent", "t1", input))
}

async fn drive_in(tool: &dyn Tool, context: ToolContext) -> (Vec<ToolStreamEvent>, ToolOutcome) {
    let mut events = Vec::new();
    let mut outcome = None;
    let mut stream = tool.stream(context);
    while let Some(item) = stream.next().await {
        match item {
            ToolStreamItem::Event(event) => events.push(event),
            ToolStreamItem::Outcome(last) => outcome = Some(last),
            _ => {}
        }
    }
    (events, outcome.expect("the tool ends with an outcome"))
}

/// Drives the tool to completion on a fresh parent; the final tool result.
async fn drive(options: MakeSubagentOptions, input: JsonValue) -> ToolResultBlock {
    let tool = make_subagent(options).unwrap();
    match drive_in(tool.as_ref(), context(input)).await.1 {
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

fn schema(tool: &dyn Tool) -> JsonValue {
    tool.tool_spec().input_schema().cloned().expect("an input schema")
}

fn props(options: MakeSubagentOptions) -> JsonValue {
    schema(make_subagent(options).unwrap().as_ref())["properties"].clone()
}

/// A builder that hands out `child` once.
fn builder_of(child: Agent) -> AgentBuilder {
    let slot = Mutex::new(Some(child));
    Arc::new(move |_spec| {
        let child = slot.lock().unwrap().take().expect("one child is built");
        async move { Ok(child) }.boxed()
    })
}

fn keys(props: &JsonValue) -> Vec<&str> {
    props.as_object().unwrap().keys().map(String::as_str).collect()
}

fn error_text(options: MakeSubagentOptions) -> String {
    make_subagent(options)
        .expect_err("construction fails")
        .to_string()
}

/// The child's first prompt for `input` on a parent holding `parent`.
fn prompt(context_axis: ContextAxis, input: JsonValue, parent: &[Message]) -> InvokeArgs {
    let (mut options, _) = options();
    options.context = Some(context_axis);
    let tool = SubagentTool::new(options).unwrap();
    let spec = tool
        .shared
        .axes
        .resolve(context(input).tool_use().input())
        .unwrap();
    child_prompt(&spec, parent)
}

fn none_or(mode: &str) -> ContextAxis {
    ContextAxis::Choice(Choice::new(["none", mode]))
}

/// Role and content of each message: forked messages are new messages over the parent's blocks.
fn shape_of(messages: &[Message]) -> JsonValue {
    messages
        .iter()
        .map(|message| json!({"role": message.role().as_str(), "content": message.content()}))
        .collect()
}

fn shape(prompt: &InvokeArgs) -> JsonValue {
    match prompt {
        InvokeArgs::Messages(messages) => shape_of(messages),
        other => panic!("expected messages, got {other:?}"),
    }
}

fn as_text(prompt: &InvokeArgs) -> &str {
    match prompt {
        InvokeArgs::Text(text) => text,
        other => panic!("expected text, got {other:?}"),
    }
}

fn framed(task: &str) -> ContentBlock {
    text(&format!("{FORK_PREAMBLE}\n\n{task}"))
}

mod resolution;
mod running;
mod schema;
mod sharing;
