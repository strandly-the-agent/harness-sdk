//! Agent loop tests, ported from strands-ts/src/agent/__tests__/{agent,agent.hook}.test.ts.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;

use crate::agent::{Agent, AgentStream};
use crate::error::{Error, ModelError};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolsEvent, MessageAddedEvent, ModelMessageEvent,
};
use crate::models::streaming::Usage;
use crate::testing::{HookRecorder, MockMessageModel, MockTurnOptions, test_agent};
use crate::tools::{FunctionTool, SequentialToolExecutor, ToolError, ToolSource};
use crate::types::agent::{AgentResult, AgentStreamEvent, InvocationState, InvokeOptions};
use crate::types::json::JsonValue;
use crate::types::messages::{
    ContentBlock, Message, MessageMetadata, ReasoningBlock, Role, StopReason, SystemPrompt, TextBlock,
    ToolResultBlock, ToolUseBlock,
};
use crate::types::tools::ToolResultStatus;

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn tool_use(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

fn mock(turns: Vec<Vec<ContentBlock>>) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    for turn in turns {
        model.add_turn(turn);
    }
    model
}

fn add_tool() -> FunctionTool {
    FunctionTool::new("add", "Adds two numbers.", |input: JsonValue, _ctx| async move {
        Ok::<_, ToolError>(json!(
            input["a"].as_i64().unwrap_or(0) + input["b"].as_i64().unwrap_or(0)
        ))
    })
}

fn tool_turns() -> Arc<MockMessageModel> {
    mock(vec![
        vec![tool_use("add", "tool-1", json!({"a": 1, "b": 2}))],
        vec![text("The sum is 3.")],
    ])
}

fn tool_agent(model: &Arc<MockMessageModel>) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .tools([add_tool()])
        .build()
        .unwrap()
}

fn type_of(event: &AgentStreamEvent) -> String {
    serde_json::to_value(event).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn collect(mut stream: AgentStream<'_>) -> Vec<AgentStreamEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    events
}

fn compact(types: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for kind in types {
        if out.last() != Some(&kind.as_str()) {
            out.push(kind);
        }
    }
    out
}

fn texts(message: &Message) -> Vec<&str> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect()
}

// Compile-time `Send` checks (section 9.4).
fn assert_send<T: Send>(_: &T) {}

#[test]
fn agent_handles_and_futures_are_send() {
    fn send<T: Send>() {}
    send::<Agent>();
    send::<AgentResult>();
    send::<AgentStream<'static>>();
    let mut agent = test_agent(mock(vec![vec![text("hi")]]));
    assert_send(&agent.invoke("hi"));
    assert_send(&agent.stream("hi"));
    assert_send(&agent.initialize());
}

mod stream {
    use super::*;

    #[tokio::test]
    async fn yields_agent_stream_events_in_ts_order() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        let events = collect(agent.stream("Hi")).await;
        let types: Vec<String> = events.iter().map(type_of).collect();
        assert_eq!(
            compact(&types),
            [
                "beforeInvocationEvent",
                "messageAddedEvent",
                "beforeModelCallEvent",
                "modelStreamUpdateEvent",
                "contentBlockEvent",
                "modelStreamUpdateEvent",
                "modelMessageEvent",
                "afterModelCallEvent",
                "messageAddedEvent",
                "afterInvocationEvent",
                "agentResultEvent",
            ]
        );
    }

    #[tokio::test]
    async fn ends_with_the_agent_result() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        let events = collect(agent.stream("Hi")).await;
        let Some(AgentStreamEvent::AgentResult(result)) = events.last() else {
            panic!("the last item is the result");
        };
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(texts(result.last_message()), ["Hello"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yields_tool_related_events() {
        let model = tool_turns();
        let mut agent = tool_agent(&model);
        let types: Vec<String> = collect(agent.stream("Add 1 and 2"))
            .await
            .iter()
            .map(type_of)
            .collect();
        let position = |kind: &str| types.iter().position(|t| t == kind).unwrap();
        assert!(position("beforeToolsEvent") < position("beforeToolCallEvent"));
        assert!(position("beforeToolCallEvent") < position("afterToolCallEvent"));
        assert!(position("afterToolCallEvent") < position("toolResultEvent"));
        assert!(position("toolResultEvent") < position("afterToolsEvent"));
    }

    #[tokio::test]
    async fn a_max_tokens_stop_is_an_error() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![text("partial")],
            MockTurnOptions::builder()
                .stop_reason(StopReason::MaxTokens)
                .build(),
        );
        let mut agent = test_agent(model);
        let error = agent.invoke("Hi").await.unwrap_err();
        assert!(matches!(error, Error::MaxTokens(_)), "{error}");
    }

    #[tokio::test]
    async fn a_dropped_stream_leaves_the_agent_reusable() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        {
            let mut stream = agent.stream("Hi");
            stream.next().await.unwrap().unwrap();
        }
        let result = agent.invoke("Again").await.unwrap();
        assert_eq!(texts(result.last_message()), ["Hello"]);
    }
}

mod invoke {
    use super::*;

    #[tokio::test]
    async fn returns_the_stop_reason_and_last_message() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().role(), Role::Assistant);
        assert_eq!(result.to_string(), "Hello");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn executes_tools_and_appends_the_tool_turn_after_the_tools_finish() {
        let model = tool_turns();
        let mut agent = tool_agent(&model);
        let result = agent.invoke("Add 1 and 2").await.unwrap();
        assert_eq!(result.to_string(), "The sum is 3.");
        let roles: Vec<Role> = agent.messages().iter().map(Message::role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant]);
        let ContentBlock::ToolResult(tool_result) = &agent.messages()[2].content()[0] else {
            panic!("a tool result");
        };
        assert_eq!(tool_result.tool_use_id(), "tool-1");
        assert_eq!(*tool_result.status(), ToolResultStatus::Success);
        // The second model call saw the tool turn.
        assert_eq!(model.requests()[1].messages.len(), 3);
    }

    #[tokio::test]
    async fn the_assistant_tool_use_message_is_appended_only_after_its_tools_finish() {
        let model = tool_turns();
        let agent = tool_agent(&model);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            let history = event
                .agent()
                .messages()
                .iter()
                .map(Message::role)
                .collect::<Vec<_>>();
            record.lock().unwrap().push(history);
        });
        let mut agent = agent;
        agent.invoke("Add").await.unwrap();
        assert_eq!(*seen.lock().unwrap(), [vec![Role::User]]);
    }

    #[tokio::test]
    async fn stores_the_cycle_id_and_tool_metadata_in_the_traces() {
        let mut agent = tool_agent(&tool_turns());
        let result = agent.invoke("Add").await.unwrap();
        let traces = result.traces();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].name(), "Cycle 1");
        assert_eq!(traces[0].metadata_value("cycleId"), Some("cycle-1"));
        assert_eq!(traces[1].metadata_value("cycleId"), Some("cycle-2"));
        let names: Vec<&str> = traces[0]
            .children()
            .iter()
            .map(crate::telemetry::AgentTrace::name)
            .collect();
        assert_eq!(names, ["stream_messages", "Tool: add"]);
        let tool = &traces[0].children()[1];
        assert_eq!(tool.metadata_value("toolUseId"), Some("tool-1"));
        assert_eq!(tool.metadata_value("toolName"), Some("add"));
        assert_eq!(tool.parent_id(), Some(traces[0].id()));
        assert!(tool.end_time().is_some());
        assert!(traces[0].children()[0].message().is_some());
    }

    #[tokio::test]
    async fn exposes_accumulated_metrics() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![text("Hello")],
            MockTurnOptions::builder().usage(Usage::new(10, 5, 15)).build(),
        );
        let mut agent = test_agent(model);
        let result = agent.invoke("Hi").await.unwrap();
        agent.invoke("Again").await.unwrap();
        assert_eq!(result.metrics().cycle_count(), 1);
        let metrics = agent.metrics();
        assert_eq!(metrics.cycle_count(), 2);
        assert_eq!(metrics.accumulated_usage().total_tokens(), 30);
        assert_eq!(metrics.agent_invocations().len(), 2);
        assert_eq!(
            metrics.latest_agent_invocation().unwrap().cycles()[0].cycle_id(),
            "cycle-2"
        );
        assert_eq!(result.context_size(), Some(10));
        assert_eq!(result.projected_context_size(), Some(15));
    }

    #[tokio::test]
    async fn records_tool_metrics() {
        let mut agent = tool_agent(&tool_turns());
        let result = agent.invoke("Add").await.unwrap();
        let add = result.metrics().tool_metric("add").unwrap();
        assert_eq!(
            (add.call_count(), add.success_count(), add.error_count()),
            (1, 1, 0)
        );
        assert_eq!(add.success_rate(), 1.0);
    }

    #[tokio::test]
    async fn tracks_the_cycle_count_when_a_max_tokens_error_occurs() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![text("partial")],
            MockTurnOptions::builder()
                .stop_reason(StopReason::MaxTokens)
                .build(),
        );
        let mut agent = test_agent(model);
        agent.invoke("Hi").await.unwrap_err();
        assert_eq!(agent.metrics().cycle_count(), 1);
    }

    #[tokio::test]
    async fn invoke_and_stream_produce_the_same_result() {
        let mut first = tool_agent(&tool_turns());
        let mut second = tool_agent(&tool_turns());
        let invoked = first.invoke("Add").await.unwrap();
        let Some(AgentStreamEvent::AgentResult(streamed)) = collect(second.stream("Add")).await.pop() else {
            panic!("a result");
        };
        assert_eq!(invoked.stop_reason(), streamed.stop_reason());
        assert_eq!(invoked.to_string(), streamed.to_string());
        assert_eq!(first.messages().len(), second.messages().len());
    }

    #[tokio::test]
    async fn shares_the_invocation_state_with_tools_hooks_and_the_result() {
        #[derive(Clone)]
        struct RequestId(&'static str);
        let model = tool_turns();
        let tool = FunctionTool::new(
            "add",
            "Adds.",
            |_input: JsonValue, ctx: crate::tools::ToolContext| async move {
                Ok::<_, ToolError>(json!(ctx.invocation_state().get::<RequestId>().map(|id| id.0)))
            },
        );
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([tool])
            .build()
            .unwrap();
        let state = InvocationState::new();
        state.insert(RequestId("req-1"));
        let options = InvokeOptions::builder().invocation_state(state.clone()).build();
        let result = agent.invoke_with("Add", options).await.unwrap();
        assert!(result.invocation_state().get::<RequestId>().is_some());
        let ContentBlock::ToolResult(tool_result) = &agent.messages()[2].content()[0] else {
            panic!("a tool result");
        };
        assert_eq!(
            serde_json::to_value(tool_result.content()).unwrap(),
            json!([{"text": "req-1"}])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn runs_tools_with_the_sequential_executor() {
        let model = mock(vec![
            vec![
                tool_use("add", "t1", json!({"a": 1, "b": 1})),
                tool_use("add", "t2", json!({"a": 2, "b": 2})),
            ],
            vec![text("done")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([add_tool()])
            .tool_executor(SequentialToolExecutor::new())
            .build()
            .unwrap();
        agent.invoke("Add twice").await.unwrap();
        let results: Vec<&str> = agent.messages()[2]
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult(result) => Some(result.tool_use_id()),
                _ => None,
            })
            .collect();
        assert_eq!(results, ["t1", "t2"]);
    }
}

mod config {
    use super::*;

    #[test]
    fn defaults_match_ts() {
        let agent = test_agent(mock(vec![]));
        assert_eq!(
            (agent.id(), agent.name(), agent.description()),
            ("agent", "Strands Agent", None)
        );
        assert!(agent.messages().is_empty());
        assert_eq!(agent.tools().len(), 0);
        assert!(Arc::ptr_eq(agent.aux_model(), agent.model()));
    }

    #[test]
    fn flattens_nested_tool_lists_and_tools_extends() {
        let nested = ToolSource::from(vec![ToolSource::from(vec![add_tool()])]);
        let other = FunctionTool::new("other", "Other.", |_input: JsonValue, _ctx| async {
            Ok::<_, ToolError>(json!(1))
        });
        let agent = Agent::builder()
            .model(MockMessageModel::new())
            .tools([nested])
            .tools([other])
            .build()
            .unwrap();
        let names: Vec<&str> = agent.tools().map(|tool| tool.name()).collect();
        assert_eq!(names, ["add", "other"]);
    }

    #[test]
    fn catches_duplicate_tool_names_across_nested_lists() {
        let error = Agent::builder()
            .model(MockMessageModel::new())
            .tools([
                ToolSource::from(vec![add_tool()]),
                ToolSource::from(vec![vec![add_tool()]]),
            ])
            .build()
            .unwrap_err();
        assert_eq!(error.to_string(), "Tool with name 'add' already registered");
    }

    #[test]
    fn stores_the_configured_keys() {
        let mut app_state = crate::types::json::JsonObject::new();
        app_state.insert("counter".into(), json!(3));
        let aux = Arc::new(MockMessageModel::new());
        let agent = Agent::builder()
            .model(MockMessageModel::new())
            .aux_model(Arc::clone(&aux))
            .id("a-1")
            .name("Helper")
            .description("Helps.")
            .system_prompt("Be brief.")
            .messages(vec![Message::new(Role::User, vec![text("seed")])])
            .app_state(app_state)
            .build()
            .unwrap();
        assert_eq!(
            (agent.id(), agent.name(), agent.description()),
            ("a-1", "Helper", Some("Helps."))
        );
        assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from("Be brief.")));
        assert_eq!(agent.messages().len(), 1);
        assert_eq!(agent.app_state().get::<u64>("counter").unwrap(), Some(3));
        assert!(!Arc::ptr_eq(agent.aux_model(), agent.model()));
    }

    #[cfg(not(feature = "bedrock"))]
    #[tokio::test]
    async fn without_a_model_the_first_call_fails_with_a_default_not_configured_error() {
        let mut agent = Agent::builder().build().unwrap();
        let error = agent.invoke("Hi").await.unwrap_err();
        assert_eq!(error.name(), "DefaultNotConfiguredError");
        assert!(error.to_string().starts_with("no model configured"), "{error}");
    }

    #[cfg(feature = "bedrock")]
    #[test]
    fn a_model_id_string_builds_a_bedrock_model_and_no_model_means_the_default_one() {
        let agent = Agent::builder()
            .model("global.anthropic.claude-sonnet-4-6")
            .build()
            .unwrap();
        assert_eq!(
            agent.model().model_id().as_deref(),
            Some("global.anthropic.claude-sonnet-4-6")
        );
        let agent = Agent::builder().build().unwrap();
        assert_eq!(
            agent.model().model_id(),
            crate::models::model::Model::model_id(&crate::models::bedrock::BedrockModel::default())
        );
    }

    #[cfg(not(feature = "bedrock"))]
    #[test]
    fn a_model_id_string_fails_the_build_without_the_bedrock_feature() {
        let error = Agent::builder()
            .model("global.anthropic.claude-sonnet-4-6")
            .build()
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("model_id=<global.anthropic.claude-sonnet-4-6>"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn the_system_prompt_can_change_between_turns() {
        let model = mock(vec![vec![text("one")], vec![text("two")]]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .system_prompt("first")
            .build()
            .unwrap();
        agent.invoke("Hi").await.unwrap();
        agent.set_system_prompt(Some("second".into()));
        agent.invoke("Again").await.unwrap();
        let prompts: Vec<_> = model
            .requests()
            .into_iter()
            .map(|request| request.system_prompt)
            .collect();
        assert_eq!(prompts, [Some("first".into()), Some("second".into())]);
    }

    #[tokio::test]
    async fn a_replaced_model_takes_over_from_the_next_invocation() {
        let first = mock(vec![vec![text("one")]]);
        let second = mock(vec![vec![text("two")]]);
        let mut agent = test_agent(first.clone());
        agent.invoke("Hi").await.unwrap();
        agent.set_model(second.clone());
        let result = agent.invoke("Again").await.unwrap();
        assert_eq!(result.to_string(), "two");
        assert_eq!((first.requests().len(), second.requests().len()), (1, 1));
    }
}

mod input {
    use super::*;

    #[tokio::test]
    async fn a_string_becomes_one_user_text_message() {
        let mut agent = test_agent(mock(vec![vec![text("ok")]]));
        agent.invoke("Hello").await.unwrap();
        assert_eq!(agent.messages()[0].role(), Role::User);
        assert_eq!(texts(&agent.messages()[0]), ["Hello"]);
    }

    #[tokio::test]
    async fn content_blocks_become_one_user_message() {
        let mut agent = test_agent(mock(vec![vec![text("ok")]]));
        agent.invoke(vec![text("a"), text("b")]).await.unwrap();
        assert_eq!(texts(&agent.messages()[0]), ["a", "b"]);
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn empty_content_or_messages_append_only_the_response() {
        let mut agent = test_agent(mock(vec![vec![text("Response")]]));
        agent.invoke(Vec::<ContentBlock>::new()).await.unwrap();
        assert_eq!(agent.messages().len(), 1);
        agent.invoke(Vec::<Message>::new()).await.unwrap();
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn messages_are_appended_in_order_and_keep_their_tracking_ids() {
        let mut agent = test_agent(mock(vec![vec![text("ok")]]));
        let first = Message::new(Role::User, vec![text("one")]).with_tracking_id("caller-id");
        let second = Message::new(Role::Assistant, vec![text("two")]);
        let third = Message::new(Role::User, vec![text("three")]);
        agent.invoke(vec![first, second, third]).await.unwrap();
        assert_eq!(agent.messages().len(), 4);
        assert_eq!(agent.messages()[0].tracking_id(), "caller-id");
        assert_eq!(texts(&agent.messages()[2]), ["three"]);
        let ids: std::collections::HashSet<&str> =
            agent.messages().iter().map(Message::tracking_id).collect();
        assert_eq!(ids.len(), 4, "every recorded message has its own tracking id");
    }

    #[tokio::test]
    async fn a_blank_prompt_is_rejected_with_the_ts_text() {
        let mut agent = test_agent(mock(vec![vec![text("ok")]]));
        let error = agent.invoke("   ").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Input prompt is blank. Provide non-blank text or at least one non-text content block."
        );
        assert!(agent.messages().is_empty());
    }

    #[tokio::test]
    async fn supports_all_content_block_types() {
        use crate::types::media::{
            AudioBlock, AudioFormat, DocumentBlock, DocumentSource, ImageBlock, ImageFormat, ImageSource,
            S3Location, VideoBlock, VideoFormat,
        };
        use crate::types::messages::{CachePointBlock, GuardContentBlock, GuardContentText, GuardQualifier};

        let content: Vec<ContentBlock> = vec![
            text("Text content"),
            tool_use("tool1", "id-1", json!({"key": "value"})),
            ToolResultBlock::new("id-1", ToolResultStatus::Success, vec!["Result".into()]).into(),
            ReasoningBlock::new().with_text("My reasoning").into(),
            CachePointBlock::new().into(),
            GuardContentBlock::from(GuardContentText::new(
                "Guard content",
                vec![GuardQualifier::GroundingSource],
            ))
            .into(),
            AudioBlock::new(AudioFormat::Mp3, vec![1, 2, 3]).into(),
            ImageBlock::new(
                ImageFormat::Png,
                ImageSource::Url("https://example.com/image.png".into()),
            )
            .into(),
            VideoBlock::new(VideoFormat::Mp4, S3Location::new("s3://bucket/video.mp4")).into(),
            DocumentBlock::new("doc.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3])).into(),
        ];
        let mut agent = test_agent(mock(vec![vec![text("Response")]]));

        agent.invoke(content.clone()).await.unwrap();

        assert_eq!(agent.messages().len(), 2);
        assert_eq!(agent.messages()[0].role(), Role::User);
        assert_eq!(
            serde_json::to_value(agent.messages()[0].content()).unwrap(),
            serde_json::to_value(&content).unwrap()
        );
    }

    #[tokio::test]
    async fn blank_means_empty_after_js_trim() {
        let mut agent = test_agent(mock(vec![vec![text("ok")]]));
        let error = agent.invoke("\u{feff}").await.unwrap_err();
        assert!(error.to_string().starts_with("Input prompt is blank."));
        let mut agent = test_agent(mock(vec![vec![text("ok")]]));
        let result = agent.invoke("\u{85}").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn invalid_tool_names_and_empty_assistant_messages_are_fixed_only_in_the_request() {
        let model = mock(vec![vec![text("ok")]]);
        let history = vec![
            Message::new(Role::User, vec![text("hi")]),
            Message::new(
                Role::Assistant,
                vec![
                    ToolUseBlock::new("bad name!", "t1", json!({}))
                        .with_reasoning_signature("sig")
                        .into(),
                ],
            ),
            Message::new(
                Role::User,
                vec![ToolResultBlock::new("t1", ToolResultStatus::Success, vec!["ok".into()]).into()],
            ),
            Message::new(Role::Assistant, vec![]),
        ];
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .messages(history.clone())
            .build()
            .unwrap();
        agent.invoke("next").await.unwrap();
        let sent = &model.requests()[0].messages;
        let ContentBlock::ToolUse(renamed) = &sent[1].content()[0] else {
            panic!("a tool use");
        };
        assert_eq!(
            (renamed.name(), renamed.reasoning_signature()),
            ("INVALID_TOOL_NAME", Some("sig"))
        );
        assert_eq!(texts(&sent[3]), ["[blank text]"]);
        assert_eq!(agent.messages()[..4], history[..]);
    }
}

mod redaction {
    use super::*;

    fn agent_with(messages: Vec<Message>) -> Agent {
        Agent::builder()
            .model(MockMessageModel::new())
            .messages(messages)
            .build()
            .unwrap()
    }

    #[test]
    fn redacts_a_text_user_message_and_keeps_its_tracking_id() {
        let mut agent = agent_with(vec![
            Message::new(Role::User, vec![text("secret"), text("more")]).with_tracking_id("id-1"),
        ]);
        agent.redact_last_message("[REDACTED]");
        assert_eq!(texts(&agent.messages()[0]), ["[REDACTED]"]);
        assert_eq!(agent.messages()[0].tracking_id(), "id-1");
    }

    #[test]
    fn redacts_only_the_content_of_tool_results() {
        let result = ToolResultBlock::new("t1", ToolResultStatus::Error, vec!["secret".into()]);
        let mut agent = agent_with(vec![Message::new(Role::User, vec![result.into(), text("drop")])]);
        agent.redact_last_message("[REDACTED]");
        let expected = ToolResultBlock::new("t1", ToolResultStatus::Error, vec!["[REDACTED]".into()]);
        assert_eq!(agent.messages()[0].content(), [expected.into()]);
    }

    #[test]
    fn skips_a_last_message_not_from_the_user_and_an_empty_history() {
        let assistant = Message::new(Role::Assistant, vec![text("keep")]);
        let mut agent = agent_with(vec![assistant.clone()]);
        agent.redact_last_message("[REDACTED]");
        assert_eq!(agent.messages(), [assistant]);
        let mut empty = agent_with(vec![]);
        empty.redact_last_message("[REDACTED]");
        assert!(empty.messages().is_empty());
    }
}

mod estimate_input_tokens {
    use super::*;

    async fn projected(agent: &mut Agent) -> Option<u64> {
        let seen = Arc::new(Mutex::new(None));
        let record = Arc::clone(&seen);
        let cleanup = agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
            *record.lock().unwrap() = Some(event.projected_input_tokens());
        });
        agent.invoke("What next?").await.unwrap();
        cleanup.remove();
        seen.lock().unwrap().unwrap()
    }

    #[tokio::test]
    async fn counts_everything_on_a_cold_start() {
        let mut agent = test_agent(mock(vec![vec![text("ok")]]));
        assert!(projected(&mut agent).await.unwrap() > 0);
    }

    #[tokio::test]
    async fn adds_new_messages_to_the_last_reported_usage() {
        let assistant = Message::new(Role::Assistant, vec![text("earlier")])
            .with_metadata(MessageMetadata::new().with_usage(Usage::new(100, 20, 120)));
        let history = vec![Message::new(Role::User, vec![text("hi")]), assistant];
        let model = mock(vec![vec![text("ok")]]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .messages(history)
            .build()
            .unwrap();
        let new_message = [Message::new(Role::User, vec![text("What next?")])];
        let delta =
            crate::models::model::Model::count_tokens(model.as_ref(), &new_message, Default::default())
                .await
                .unwrap();
        assert_eq!(projected(&mut agent).await, Some(120 + delta));
    }

    /// A mock whose `count_tokens` returns a fixed count, or fails (the TS `countTokens` spy).
    struct FixedCount {
        inner: Arc<MockMessageModel>,
        count: Option<u64>,
    }

    #[async_trait::async_trait]
    impl crate::models::model::Model for FixedCount {
        fn stream(
            &self,
            request: crate::models::model::ModelRequest,
        ) -> crate::models::model::BoxStream<
            'static,
            crate::error::Result<crate::models::streaming::ModelStreamEvent>,
        > {
            self.inner.stream(request)
        }

        fn get_config(&self) -> crate::models::model::BaseModelConfig {
            self.inner.get_config()
        }

        fn update_config(&self, config: crate::models::model::BaseModelConfig) {
            self.inner.update_config(config);
        }

        async fn count_tokens(
            &self,
            _messages: &[Message],
            _options: crate::models::model::CountTokensOptions,
        ) -> crate::error::Result<u64> {
            self.count.ok_or_else(|| Error::other("API unavailable"))
        }
    }

    fn fixed(model: &Arc<MockMessageModel>, count: Option<u64>) -> Arc<FixedCount> {
        Arc::new(FixedCount {
            inner: Arc::clone(model),
            count,
        })
    }

    async fn projections(
        agent: &mut Agent,
        args: impl Into<crate::types::agent::InvokeArgs>,
    ) -> Vec<Option<u64>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let cleanup = agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
            record.lock().unwrap().push(event.projected_input_tokens());
        });
        agent.invoke(args).await.unwrap();
        cleanup.remove();
        let projections = seen.lock().unwrap();
        projections.clone()
    }

    fn after_usage(usage: Usage) -> Vec<Message> {
        vec![
            Message::new(Role::User, vec![text("Hi")]),
            Message::new(Role::Assistant, vec![text("Hello")])
                .with_metadata(MessageMetadata::new().with_usage(usage)),
        ]
    }

    #[tokio::test]
    async fn uses_known_baseline_when_no_new_messages_after_last_assistant() {
        let mut agent = Agent::builder()
            .model(mock(vec![vec![text("Hello")]]))
            .messages(after_usage(Usage::new(100, 20, 120)))
            .printer(false)
            .build()
            .unwrap();
        assert_eq!(
            projections(&mut agent, Vec::<ContentBlock>::new()).await,
            [Some(120)]
        );
    }

    #[tokio::test]
    async fn counts_disjoint_provider_cache_reads_in_the_known_baseline() {
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        let mut agent = Agent::builder()
            .model(mock(vec![vec![text("Hello")]]))
            .messages(after_usage(usage))
            .printer(false)
            .build()
            .unwrap();
        assert_eq!(
            projections(&mut agent, Vec::<ContentBlock>::new()).await,
            [Some(5862)]
        );
    }

    #[tokio::test]
    async fn returns_undefined_projected_input_tokens_when_estimation_fails() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = test_agent(fixed(&model, None));
        assert_eq!(projections(&mut agent, "Hi").await, [None]);
    }

    #[tokio::test]
    async fn estimates_delta_for_new_messages_after_last_assistant() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![tool_use("add", "id-1", json!({}))],
            MockTurnOptions {
                usage: Some(Usage::new(100, 30, 130)),
                ..MockTurnOptions::default()
            },
        );
        model.add_turn(vec![text("Done")]);
        let mut agent = Agent::builder()
            .model(fixed(&model, Some(50)))
            .tools([add_tool()])
            .printer(false)
            .build()
            .unwrap();
        assert_eq!(
            projections(&mut agent, "Use the tool").await,
            [Some(50), Some(180)]
        );
    }

    #[tokio::test]
    async fn uses_baseline_from_prior_invocation_on_second_invoke() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![text("First response")],
            MockTurnOptions {
                usage: Some(Usage::new(200, 50, 250)),
                ..MockTurnOptions::default()
            },
        );
        model.add_turn(vec![text("Second response")]);
        let mut agent = test_agent(fixed(&model, Some(15)));
        agent.invoke("First question").await.unwrap();
        assert_eq!(projections(&mut agent, "Second question").await, [Some(265)]);
    }
}

mod cycle_duration {
    use super::*;
    use std::time::Duration;

    use crate::hooks::AfterToolsEvent;

    fn assert_one_cycle_of(result: &AgentResult, duration: Duration) {
        let metrics = result.metrics();
        assert_eq!(metrics.cycle_count(), 1);
        assert_eq!(metrics.total_duration(), duration);
        assert_eq!(metrics.average_cycle_time(), duration);
        let cycles = metrics.latest_agent_invocation().unwrap().cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].duration(), duration);
    }

    #[tokio::test(start_paused = true)]
    async fn includes_the_completed_cycle_duration_in_the_returned_result_metrics() {
        let slow = FunctionTool::new("slowTool", "Slow.", |_input: JsonValue, _ctx| async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            Ok::<_, ToolError>("Done")
        });
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![tool_use("slowTool", "tool-1", json!({}))],
            MockTurnOptions {
                usage: Some(Usage::new(10, 5, 15)),
                ..MockTurnOptions::default()
            },
        );
        let mut agent = Agent::builder()
            .model(model)
            .tools([slow])
            .printer(false)
            .build()
            .unwrap();
        agent.add_hook(|event: &mut AfterToolsEvent<'_>| {
            event.set_end_turn(crate::hooks::EndTurn::Requested);
        });

        let result = agent.invoke("Test").await.unwrap();

        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_one_cycle_of(&result, Duration::from_millis(60));
    }

    #[tokio::test(start_paused = true)]
    async fn includes_the_cycle_duration_when_the_model_ends_the_turn_without_tools() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![text("Hello")],
            MockTurnOptions {
                delay: Some(Duration::from_millis(60)),
                ..MockTurnOptions::default()
            },
        );
        let mut agent = test_agent(model);

        let result = agent.invoke("Test").await.unwrap();

        assert_one_cycle_of(&result, Duration::from_millis(60));
    }
}

mod hooks {
    use super::*;

    #[tokio::test]
    async fn allows_retry_after_cancel_on_before_tool_call_event() {
        let calls = Arc::new(Mutex::new(0));
        let tool_calls = Arc::clone(&calls);
        let retry_tool = FunctionTool::new("retryTool", "Retry.", move |_input: JsonValue, _ctx| {
            *tool_calls.lock().unwrap() += 1;
            async move { Ok::<_, ToolError>("Success") }
        });
        let model = mock(vec![
            vec![tool_use("retryTool", "tool-1", json!({}))],
            vec![text("Done")],
        ]);
        let mut agent = Agent::builder()
            .model(model)
            .tools([retry_tool])
            .printer(false)
            .build()
            .unwrap();
        let before = Arc::new(Mutex::new(0));
        let before_count = Arc::clone(&before);
        agent.add_hook(move |event: &mut crate::hooks::BeforeToolCallEvent| {
            let mut count = before_count.lock().unwrap();
            *count += 1;
            if *count == 1 {
                event.set_cancel("Not yet");
            }
        });
        let after_before = Arc::clone(&before);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            if event.result().status() == &ToolResultStatus::Error && *after_before.lock().unwrap() == 1 {
                event.set_retry(true);
            }
        });

        let result = agent.invoke("Test").await.unwrap();

        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*before.lock().unwrap(), 2);
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn cancel_wins_over_selected_tool() {
        let ran = Arc::new(Mutex::new(false));
        let replacement_ran = Arc::clone(&ran);
        let replacement: Arc<dyn crate::tools::Tool> = Arc::new(FunctionTool::new(
            "replacementTool",
            "Replacement.",
            move |_input: JsonValue, _ctx| {
                *replacement_ran.lock().unwrap() = true;
                async move { Ok::<_, ToolError>("replacement") }
            },
        ));
        let registry_tool = FunctionTool::new(
            "registryTool",
            "Registry.",
            |_input: JsonValue, _ctx| async move { Ok::<_, ToolError>("registry") },
        );
        let model = mock(vec![
            vec![tool_use("registryTool", "tool-1", json!({}))],
            vec![text("Done")],
        ]);
        let mut agent = Agent::builder()
            .model(model)
            .tools([registry_tool])
            .printer(false)
            .build()
            .unwrap();
        let selected = Arc::clone(&replacement);
        agent.add_hook(move |event: &mut crate::hooks::BeforeToolCallEvent| {
            event.set_selected_tool(Arc::clone(&selected));
            event.set_cancel("blocked");
        });
        let after_tools = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&after_tools);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            seen.lock().unwrap().push(event.tool().cloned());
        });

        agent.invoke("Test").await.unwrap();

        assert!(!*ran.lock().unwrap());
        let after_tools = after_tools.lock().unwrap();
        assert_eq!(after_tools.len(), 1);
        assert!(
            after_tools[0]
                .as_ref()
                .is_some_and(|tool| Arc::ptr_eq(tool, &replacement))
        );
    }

    #[tokio::test]
    async fn fire_in_ts_order_during_invoke_and_initialized_fires_once() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        let recorder = HookRecorder::new();
        recorder.register(agent.hooks());
        agent.invoke("Hi").await.unwrap();
        let first = recorder.events();
        assert_eq!(first[0], "initializedEvent");
        let streamed: Vec<String> = collect(agent.stream("Again")).await.iter().map(type_of).collect();
        let second = recorder.events()[first.len()..].to_vec();
        assert_eq!(second, streamed, "hooks fire in stream order");
        assert_eq!(first[1..], second[..]);
    }

    #[tokio::test]
    async fn before_invocation_can_rewrite_or_drop_the_input() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        let cleanup = agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
            event
                .messages_mut()
                .push(Message::new(Role::User, vec![TextBlock::new("added").into()]));
        });
        agent.invoke("original").await.unwrap();
        assert_eq!(agent.messages().len(), 3);
        assert_eq!(texts(&agent.messages()[1]), ["added"]);
        cleanup.remove();

        agent.messages_mut().clear();
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_messages(Vec::new()));
        agent.invoke("removed").await.unwrap();
        assert_eq!(agent.messages().len(), 1);
        assert_eq!(agent.messages()[0].role(), Role::Assistant);
    }

    #[tokio::test]
    async fn message_added_fires_for_the_input_and_the_response() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        let roles = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&roles);
        agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
            record.lock().unwrap().push(event.message().role());
        });
        agent.invoke("Hi").await.unwrap();
        assert_eq!(*roles.lock().unwrap(), [Role::User, Role::Assistant]);
    }

    #[tokio::test]
    async fn after_tool_call_sees_a_failing_tool_and_its_result_replacement_reaches_history() {
        let model = mock(vec![vec![tool_use("boom", "t1", json!({}))], vec![text("done")]]);
        let boom = FunctionTool::new("boom", "Fails.", |_input: JsonValue, _ctx| async {
            Err::<JsonValue, _>(ToolError::msg("exploded"))
        });
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([boom])
            .build()
            .unwrap();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&errors);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            record.lock().unwrap().push(event.result().status().clone());
            event.set_result(ToolResultBlock::new(
                "t1",
                ToolResultStatus::Success,
                vec!["replaced".into()],
            ));
        });
        agent.invoke("Go").await.unwrap();
        assert_eq!(*errors.lock().unwrap(), [ToolResultStatus::Error]);
        let ContentBlock::ToolResult(result) = &agent.messages()[2].content()[0] else {
            panic!("a tool result");
        };
        assert_eq!(
            serde_json::to_value(result.content()).unwrap(),
            json!([{"text": "replaced"}])
        );
    }

    #[tokio::test]
    async fn after_model_call_retry_re_calls_the_model_without_duplicating_input() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| ModelError::new("transient").into());
        model.add_turn(vec![text("Recovered")]);
        let mut agent = test_agent(model.clone());
        agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
            if event.error().is_some() {
                event.set_retry(true);
            }
        });
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(result.to_string(), "Recovered");
        assert_eq!(model.call_count(), 2);
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn after_model_call_retry_on_success_and_attempt_counts() {
        let model = mock(vec![vec![text("first")], vec![text("second")]]);
        let mut agent = test_agent(model.clone());
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&attempts);
        agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
            record.lock().unwrap().push(event.attempt_count().get());
            if event.attempt_count().get() == 1 {
                event.set_retry(true);
            }
        });
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(result.to_string(), "second");
        assert_eq!(*attempts.lock().unwrap(), [1, 2]);
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn a_model_error_without_retry_propagates_after_after_invocation_fires() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| ModelError::new("down").into());
        let mut agent = test_agent(model);
        let recorder = HookRecorder::new();
        recorder.register(agent.hooks());
        let error = agent.invoke("Hi").await.unwrap_err();
        assert_eq!(error.to_string(), "down");
        let events = recorder.events();
        assert_eq!(
            events[events.len() - 2..],
            ["afterModelCallEvent", "afterInvocationEvent"]
        );
    }

    #[tokio::test]
    async fn before_invocation_cancel_skips_the_model_and_the_input() {
        for (cancel, expected) in [(None, "invocation denied by hook"), (Some("custom"), "custom")] {
            let model = mock(vec![vec![text("never")]]);
            let mut agent = test_agent(model.clone());
            agent.add_hook(move |event: &mut BeforeInvocationEvent<'_>| match cancel {
                Some(message) => event.set_cancel(message),
                None => event.set_cancel(crate::hooks::Cancel::Requested),
            });
            let types: Vec<String> = collect(agent.stream("Hi")).await.iter().map(type_of).collect();
            assert_eq!(
                types,
                [
                    "beforeInvocationEvent",
                    "messageAddedEvent",
                    "afterInvocationEvent",
                    "agentResultEvent"
                ]
            );
            assert!(model.requests().is_empty());
            assert_eq!(agent.messages().len(), 1);
            assert_eq!(texts(&agent.messages()[0]), [expected]);
        }
    }

    #[tokio::test]
    async fn before_model_call_cancel_ends_the_turn_and_can_be_retried() {
        let model = mock(vec![vec![text("real")]]);
        let mut agent = test_agent(model.clone());
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.set_cancel("blocked"));
        let messages = Arc::new(Mutex::new(0));
        let record = Arc::clone(&messages);
        agent.add_hook(move |_: &mut ModelMessageEvent<'_>| *record.lock().unwrap() += 1);
        let retried = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&retried);
        let cleanup = agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
            let mut retried = flag.lock().unwrap();
            assert!(event.stop_data().is_some());
            if !*retried {
                *retried = true;
                event.set_retry(true);
            }
        });
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.to_string(), "blocked");
        assert!(model.requests().is_empty());
        assert_eq!(*messages.lock().unwrap(), 0);
        assert!(*retried.lock().unwrap());
        cleanup.remove();
    }

    #[tokio::test]
    async fn before_tools_cancel_turns_every_call_into_an_error_result() {
        for (cancel, expected) in [(None, "Tool cancelled by hook"), (Some("no tools"), "no tools")] {
            let model = mock(vec![
                vec![tool_use("add", "t1", json!({})), tool_use("add", "t2", json!({}))],
                vec![text("done")],
            ]);
            let mut agent = tool_agent(&model);
            agent.add_hook(move |event: &mut BeforeToolsEvent<'_>| match cancel {
                Some(message) => event.set_cancel(message),
                None => event.set_cancel(crate::hooks::Cancel::Requested),
            });
            let types: Vec<String> = collect(agent.stream("Go")).await.iter().map(type_of).collect();
            assert!(!types.iter().any(|t| t == "beforeToolCallEvent"));
            assert_eq!(types.iter().filter(|t| *t == "toolResultEvent").count(), 2);
            let results: Vec<(&str, Vec<&str>)> = agent.messages()[2]
                .content()
                .iter()
                .map(|block| match block {
                    ContentBlock::ToolResult(result) => (
                        result.tool_use_id(),
                        result
                            .content()
                            .iter()
                            .filter_map(|content| match content {
                                crate::types::messages::ToolResultContent::Text(text) => Some(text.text()),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => panic!("a tool result"),
                })
                .collect();
            assert_eq!(results, [("t1", vec![expected]), ("t2", vec![expected])]);
        }
    }

    #[tokio::test]
    async fn after_invocation_fires_on_every_invocation() {
        let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
        let count = Arc::new(Mutex::new(0));
        let record = Arc::clone(&count);
        agent.add_hook(move |_: &mut AfterInvocationEvent<'_>| *record.lock().unwrap() += 1);
        agent.invoke("one").await.unwrap();
        agent.invoke("two").await.unwrap();
        assert_eq!(*count.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn reasoning_shows_in_the_result_text() {
        let reasoning: ContentBlock = ReasoningBlock::new().with_text("think").into();
        let mut agent = test_agent(mock(vec![vec![reasoning, text("answer")]]));
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(result.to_string(), "💭 Reasoning:\n   think\nanswer");
    }
}

/// Ported from strands-ts/src/agent/__tests__/agent.concurrent.test.ts (agent-level cases).
mod concurrent {
    use super::*;
    use std::time::Duration;

    use crate::hooks::BeforeToolCallEvent;
    use crate::tools::ToolExecutor;

    type Log = Arc<Mutex<Vec<String>>>;

    /// A tool that logs its start and end, waiting on `barrier` (when set) in between.
    fn gated(name: &'static str, log: &Log, barrier: Option<Arc<tokio::sync::Barrier>>) -> FunctionTool {
        let log = Arc::clone(log);
        FunctionTool::new(name, "Gated.", move |_input: JsonValue, _ctx| {
            let log = Arc::clone(&log);
            let barrier = barrier.clone();
            async move {
                log.lock().unwrap().push(format!("start {name}"));
                if let Some(barrier) = barrier {
                    barrier.wait().await;
                }
                tokio::task::yield_now().await;
                log.lock().unwrap().push(format!("end {name}"));
                Ok::<_, ToolError>(name)
            }
        })
    }

    fn two_tool_turn() -> Arc<MockMessageModel> {
        mock(vec![
            vec![
                tool_use("toolA", "a1", json!({})),
                tool_use("toolB", "b1", json!({})),
            ],
            vec![text("Done")],
        ])
    }

    #[tokio::test(start_paused = true)]
    async fn runs_tools_concurrently_by_default() {
        let log = Log::default();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut agent = Agent::builder()
            .model(two_tool_turn())
            .tools([
                gated("toolA", &log, Some(Arc::clone(&barrier))),
                gated("toolB", &log, Some(barrier)),
            ])
            .printer(false)
            .build()
            .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(5), agent.invoke("Go")).await;

        assert!(result.is_ok(), "both tools must run at once to pass the barrier");
    }

    #[tokio::test]
    async fn uses_a_reassigned_tool_executor_for_subsequent_tool_calls() {
        let log = Log::default();
        let mut agent = Agent::builder()
            .model(two_tool_turn())
            .tools([gated("toolA", &log, None), gated("toolB", &log, None)])
            .printer(false)
            .build()
            .unwrap();
        agent.set_tool_executor(SequentialToolExecutor::new());
        assert!(matches!(agent.tool_executor(), ToolExecutor::Sequential(_)));

        agent.invoke("Go").await.unwrap();

        assert_eq!(
            *log.lock().unwrap(),
            ["start toolA", "end toolA", "start toolB", "end toolB"]
        );
    }

    #[tokio::test]
    async fn retries_one_tool_independently_from_the_other() {
        let log = Log::default();
        let mut agent = Agent::builder()
            .model(two_tool_turn())
            .tools([gated("toolA", &log, None), gated("toolB", &log, None)])
            .printer(false)
            .build()
            .unwrap();
        let before_calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&before_calls);
        agent.add_hook(move |event: &mut BeforeToolCallEvent| {
            seen.lock().unwrap().push(event.tool_use().name().to_owned());
        });
        let retried = Arc::new(Mutex::new(false));
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            let mut retried = retried.lock().unwrap();
            if event.tool_use().name() == "toolA" && !*retried {
                *retried = true;
                event.set_retry(true);
            }
        });

        agent.invoke("Go").await.unwrap();

        let before_calls = before_calls.lock().unwrap();
        assert_eq!(before_calls.iter().filter(|name| *name == "toolA").count(), 2);
        assert_eq!(before_calls.iter().filter(|name| *name == "toolB").count(), 1);
    }
}

mod cancel;
mod continuation;
mod copy_on_input;
mod drop_guard;
mod end_turn_resume;
mod interrupt;
mod invocation_state;
mod limits;
mod middleware;
mod middleware_interrupts;
mod model_retry;
mod stateful_model;
mod structured_output;
