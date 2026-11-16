//! Ported from strands-ts/src/vended-plugins/context-injector/__tests__/plugin.test.ts.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::*;
use crate::Agent;
use crate::agent::continuation::ContinuationInput;
use crate::error::Error;
use crate::experimental::context_manager::{
    ContextManagerConfig, Offload, OffloadConditions, OffloadTarget, SummarizeConfig,
};
use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent, MessageAddedEvent};
use crate::middleware::InvokeModelStage;
use crate::models::model::{BaseModelConfig, Model};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::test_support::{WarnRecorder, mock_tool};
use crate::testing::MockMessageModel;
use crate::tools::Tool;
use crate::types::messages::{TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

fn injected(text: &str) -> ContentBlock {
    InjectedBlock::new(text, "test").into()
}

fn injected_by(text: &str, source: &str) -> ContentBlock {
    InjectedBlock::new(text, source).into()
}

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn constant(text: &'static str) -> ContextInjectorConfig {
    ContextInjectorConfig::new(move |_context| async move { Ok(Some(text.to_owned())) })
}

fn model_with(turns: &[&str]) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    for turn in turns {
        model.add_turn(vec![text(turn)]);
    }
    model
}

fn content(message: &Message) -> Value {
    serde_json::to_value(message.content()).unwrap()
}

/// The messages each model call received.
fn requests(model: &MockMessageModel) -> Vec<Vec<Message>> {
    model
        .requests()
        .into_iter()
        .map(|request| request.messages)
        .collect()
}

mod plugin_interface {
    use super::*;

    #[test]
    fn defaults_to_the_strands_context_injector_name() {
        assert_eq!(
            ContextInjector::new(constant("x")).name(),
            "strands:context-injector"
        );
    }

    #[test]
    fn honors_a_custom_name_so_multiple_injectors_can_be_told_apart() {
        assert_eq!(ContextInjector::new(constant("x").with_name("now")).name(), "now");
    }

    #[tokio::test]
    async fn registers_an_invoke_model_stage_input_middleware_on_init_agent() {
        let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
        assert!(!agent.middleware().has::<InvokeModelStage>());
        Arc::new(ContextInjector::new(constant("x")))
            .init_agent(&mut agent)
            .await
            .unwrap();

        assert!(agent.middleware().has::<InvokeModelStage>());
    }
}

mod with_an_agent {
    use super::*;

    #[tokio::test]
    async fn sends_the_injected_text_last_and_keeps_the_cache_point_ahead_of_it() {
        let model = model_with(&["Hello"]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .printer(false)
            .plugin(ContextInjector::new(constant("<ctx>INJECTED</ctx>")))
            .build()
            .unwrap();

        agent.invoke("what is the weather?").await.unwrap();

        let seen: Vec<_> = model
            .requests()
            .into_iter()
            .map(|request| {
                let last_block = request
                    .messages
                    .last()
                    .and_then(|last| last.content().last().cloned());
                (last_block, request.dynamic_trailing_blocks.unwrap_or(0))
            })
            .collect();
        assert_eq!(seen, [(Some(text("\n\n<ctx>INJECTED</ctx>")), 1)]);
    }
}

mod writing_into_the_conversation {
    use super::*;

    /// An agent with one injection, named `test`.
    fn setup_with(
        model: &Arc<MockMessageModel>,
        config: ContextInjectorConfig,
        tools: Vec<Arc<dyn Tool>>,
        context_manager: Option<ContextManagerConfig>,
    ) -> Agent {
        Agent::builder()
            .model(Arc::clone(model))
            .printer(false)
            .maybe_context_manager(context_manager)
            .tools(tools)
            .plugin(ContextInjector::new(config.with_name("test")))
            .build()
            .unwrap()
    }

    fn setup(model: &Arc<MockMessageModel>, config: ContextInjectorConfig) -> Agent {
        setup_with(model, config, Vec::new(), None)
    }

    /// Every request starts with the previous request, byte for byte.
    fn assert_each_request_extends_the_previous(requests: &[Vec<Message>]) {
        for pair in requests.windows(2) {
            assert_eq!(pair[1][..pair[0].len()], pair[0][..]);
        }
    }

    /// A tool that moves the todo list to `v2`, and a renderer of the list.
    fn todo_setup() -> (Arc<dyn Tool>, ContextInjectorConfig, Arc<MockMessageModel>) {
        let todo = Arc::new(Mutex::new("v1"));
        let written = Arc::clone(&todo);
        let tool = mock_tool("todo_write", move |_context| {
            *written.lock().unwrap() = "v2";
            ToolResultBlock::new(
                "tool-1",
                ToolResultStatus::Success,
                vec![ToolResultContent::Text(TextBlock::new("saved"))],
            )
        });
        let config = ContextInjectorConfig::new(move |_context| {
            let todo = *todo.lock().unwrap();
            async move { Ok(Some(format!("<todos>{todo}</todos>"))) }
        })
        .with_trigger(InjectionTrigger::EveryTurn);
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![ToolUseBlock::new("todo_write", "tool-1", json!({})).into()]);
        model.add_turn(vec![text("done")]);
        (Arc::new(tool), config, model)
    }

    fn retry_on_error(agent: &Agent) {
        agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
            if event.error().is_some() {
                event.set_retry(true);
            }
        });
    }

    #[tokio::test]
    async fn writes_the_text_into_the_latest_user_message_as_an_injected_block_under_its_name_sent_as_text() {
        let model = model_with(&["ok"]);
        let mut agent = setup(&model, constant("<memory>dark mode</memory>"));

        agent.invoke("what theme?").await.unwrap();

        assert_eq!(
            agent.messages()[0].content(),
            [text("what theme?"), injected("<memory>dark mode</memory>")]
        );
        assert_eq!(
            content(&requests(&model)[0][0]),
            json!([{ "text": "what theme?" }, { "text": "\n\n<memory>dark mode</memory>" }])
        );
    }

    #[tokio::test]
    async fn fires_no_event_for_the_write_so_listeners_keep_the_message_as_it_was_added() {
        let model = model_with(&["ok"]);
        let mut agent = setup(&model, constant("reminder"));
        let added = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&added);
        agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
            record.lock().unwrap().push(event.message().clone());
        });

        agent.invoke("ask").await.unwrap();

        let added = added.lock().unwrap();
        let contents: Vec<_> = added.iter().map(|message| message.content().to_vec()).collect();
        assert_eq!(contents, [vec![text("ask")], vec![text("ok")]]);
        assert_eq!(agent.messages()[0].tracking_id(), added[0].tracking_id());
    }

    #[tokio::test]
    async fn keeps_each_injector_apart_when_two_write_into_the_same_message() {
        let model = model_with(&["ok"]);
        let mut agent = Agent::builder()
            .model(model)
            .printer(false)
            .plugin(ContextInjector::new(constant("likes tea").with_name("memory")))
            .plugin(ContextInjector::new(constant("cwd: /repo").with_name("env")))
            .build()
            .unwrap();

        agent.invoke("ask").await.unwrap();

        assert_eq!(
            agent.messages()[0].content(),
            [
                text("ask"),
                injected_by("likes tea", "memory"),
                injected_by("cwd: /repo", "env"),
            ]
        );
    }

    #[tokio::test]
    async fn writes_on_every_tool_result_turn_with_every_turn_and_sends_only_the_latest_text() {
        let (tool, config, model) = todo_setup();
        let mut agent = setup_with(&model, config, vec![tool], None);

        agent.invoke("plan the work").await.unwrap();

        assert_eq!(
            agent.messages()[0].content(),
            [text("plan the work"), injected("<todos>v1</todos>")]
        );
        // The tool result stays first in its message; the new list follows it.
        let turn = agent.messages()[2].content();
        assert!(matches!(turn[0], ContentBlock::ToolResult(_)));
        assert_eq!(turn[1..], [injected("<todos>v2</todos>")]);
        let requests = requests(&model);
        assert_eq!(requests.len(), 2);
        assert_eq!(content(&requests[1][0]), json!([{ "text": "plan the work" }]));
        let sent = content(&requests[1][2]);
        assert!(sent[0].get("toolResult").is_some());
        assert_eq!(sent[1], json!({ "text": "\n\n<todos>v2</todos>" }));
        assert_eq!(sent.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn keeps_every_request_a_prefix_of_the_next_for_a_model_that_takes_temporary_messages() {
        let (tool, config, model) = todo_setup();
        model.add_turn(vec![text("again")]);
        model.update_config(BaseModelConfig {
            model_info: Some(ModelInfo {
                capabilities: Some(ModelCapabilities {
                    temporary_messages: Some(true),
                    ..Default::default()
                }),
                limits: None,
            }),
            ..BaseModelConfig::default()
        });
        let mut agent = setup_with(&model, config, vec![tool], None);

        agent.invoke("plan the work").await.unwrap();
        agent.invoke("next").await.unwrap();

        let requests = requests(&model);
        assert_eq!(requests.len(), 3);
        assert_each_request_extends_the_previous(&requests);
        assert_eq!(
            content(requests[2].last().unwrap()),
            json!([{ "text": "next" }, { "injected": { "text": "<todos>v2</todos>", "source": "test" } }])
        );
    }

    #[tokio::test]
    async fn writes_on_every_user_turn_and_sends_only_the_latest_text() {
        let model = model_with(&["one", "two"]);
        let mut agent = setup(&model, constant("cwd: /repo"));

        agent.invoke("first").await.unwrap();
        agent.invoke("second").await.unwrap();

        assert_eq!(
            agent.messages()[0].content(),
            [text("first"), injected("cwd: /repo")]
        );
        assert_eq!(
            agent.messages()[2].content(),
            [text("second"), injected("cwd: /repo")]
        );
        let sent: Vec<_> = requests(&model)[1].iter().map(content).collect();
        assert_eq!(
            sent,
            [
                json!([{ "text": "first" }]),
                json!([{ "text": "one" }]),
                json!([{ "text": "second" }, { "text": "\n\ncwd: /repo" }]),
            ]
        );
    }

    #[tokio::test]
    async fn keeps_injected_blocks_out_of_context_offloading() {
        let rendered = format!("<agents_md>{}</agents_md>", "Keep functions small. ".repeat(40));
        let model = model_with(&["one", "two", "three"]);
        let strategy = Offload::summarize(OffloadTarget::All, SummarizeConfig::default())
            .unwrap()
            .when(OffloadConditions {
                threshold: Some(20),
                utilization: None,
                preserve_recent: None,
            })
            .unwrap();
        let context_manager = ContextManagerConfig {
            strategies: Some(vec![strategy.into()]),
            ..ContextManagerConfig::default()
        };
        let text_to_render = rendered.clone();
        let config = ContextInjectorConfig::new(move |_context| {
            let text = text_to_render.clone();
            async move { Ok(Some(text)) }
        });
        let mut agent = setup_with(&model, config, Vec::new(), Some(context_manager));

        agent.invoke("first").await.unwrap();
        agent.invoke("second").await.unwrap();
        agent.invoke("third").await.unwrap();

        // Three requests means the summarizer never ran on the injected block.
        assert_eq!(model.requests().len(), 3);
        assert_eq!(
            agent.messages()[0].content(),
            [text("first"), injected(&rendered)]
        );
    }

    #[tokio::test]
    async fn writes_into_a_message_once_so_a_retry_resends_the_same_request_even_when_the_text_changes() {
        let renders = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&renders);
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| Error::other("throttled"));
        model.add_turn(vec![text("ok")]);
        let config = ContextInjectorConfig::new(move |_context| {
            let render = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move { Ok(Some(format!("<now>{render}</now>"))) }
        });
        let mut agent = setup(&model, config);
        retry_on_error(&agent);

        agent.invoke("ask").await.unwrap();

        assert_eq!(
            agent.messages()[0].content(),
            [text("ask"), injected("<now>1</now>")]
        );
        let requests = requests(&model);
        assert_eq!(requests[1], requests[0]);
        assert_eq!(renders.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn writes_nothing_when_another_hook_cancels_the_model_call() {
        let renders = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&renders);
        let config = ContextInjectorConfig::new(move |_context| {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(Some("reminder".to_owned())) }
        });
        let mut agent = setup(&Arc::new(MockMessageModel::new()), config);
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.set_cancel("denied"));

        agent.invoke("ask").await.unwrap();

        assert_eq!(agent.messages()[0].content(), [text("ask")]);
        assert_eq!(renders.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn writes_into_a_user_message_another_before_model_call_event_hook_adds() {
        let model = model_with(&["ok"]);
        let mut agent = setup(&model, constant("reminder"));
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
            let guide = Message::new(Role::User, vec![text("[guide] be brief")]);
            event.agent_mut().messages_mut().push(guide);
        });

        agent.invoke("ask").await.unwrap();

        let contents: Vec<_> = agent.messages()[..2]
            .iter()
            .map(|message| message.content().to_vec())
            .collect();
        assert_eq!(
            contents,
            [
                vec![text("ask")],
                vec![text("[guide] be brief"), injected("reminder")]
            ]
        );
    }

    #[tokio::test]
    async fn writes_into_the_result_a_background_task_delivers_before_the_model_call() {
        let model = model_with(&["one", "two"]);
        let mut agent = setup(
            &model,
            constant("reminder").with_trigger(InjectionTrigger::EveryTurn),
        );
        agent.invoke("Run work.").await.unwrap();
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
            let result = ToolResultBlock::new(
                "task-1",
                ToolResultStatus::Success,
                vec![ToolResultContent::Text(TextBlock::new("done"))],
            );
            event.add_continuation(ContinuationInput::new(vec![
                Message::new(
                    Role::Assistant,
                    vec![ToolUseBlock::new("task", "task-1", json!({})).into()],
                ),
                Message::new(Role::User, vec![result.into()]),
            ]));
        });

        agent.invoke("Continue.").await.unwrap();

        let sent = content(requests(&model)[1].last().unwrap());
        assert!(sent[0].get("toolResult").is_some());
        assert_eq!(sent[1], json!({ "text": "\n\nreminder" }));
        assert_eq!(sent.as_array().unwrap().len(), 2);
    }

    struct PerCall;

    #[async_trait]
    impl Plugin for PerCall {
        fn name(&self) -> &str {
            "per-call"
        }

        async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
            let _cleanup = agent.add_middleware(InvokeModelStage::input(|mut context| async move {
                if let Some(latest) = context.messages.last_mut() {
                    let mut content = latest.content().to_vec();
                    content.push(text("\n\nper-call"));
                    *latest = Message::new(Role::User, content).with_tracking_id(latest.tracking_id());
                }
                Ok(context)
            }));
            Ok(())
        }
    }

    #[tokio::test]
    async fn keeps_text_other_middleware_adds_for_one_call_after_the_injected_block() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| Error::other("throttled"));
        model.add_turn(vec![text("ok")]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .printer(false)
            .plugin(PerCall)
            .plugin(ContextInjector::new(constant("reminder").with_name("test")))
            .build()
            .unwrap();
        retry_on_error(&agent);

        agent.invoke("ask").await.unwrap();

        let requests = requests(&model);
        assert_eq!(
            content(&requests[0][0]),
            json!([{ "text": "ask" }, { "text": "\n\nreminder" }, { "text": "\n\nper-call" }])
        );
        assert_eq!(requests[1], requests[0]);
    }

    #[tokio::test]
    async fn does_not_render_on_a_tool_result_turn_with_the_default_user_turn_trigger() {
        let tool = mock_tool("lookup", |_context| {
            ToolResultBlock::new(
                "tool-1",
                ToolResultStatus::Success,
                vec![ToolResultContent::Text(TextBlock::new("found"))],
            )
        });
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![ToolUseBlock::new("lookup", "tool-1", json!({})).into()]);
        model.add_turn(vec![text("done")]);
        let renders = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&renders);
        let config = ContextInjectorConfig::new(move |_context| {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(Some("reminder".to_owned())) }
        });
        let mut agent = setup_with(&model, config, vec![Arc::new(tool)], None);

        agent.invoke("ask").await.unwrap();

        assert_eq!(renders.load(Ordering::SeqCst), 1);
        let requests = requests(&model);
        assert_eq!(
            content(&requests[0][0]),
            json!([{ "text": "ask" }, { "text": "\n\nreminder" }])
        );
        assert_eq!(content(&requests[1][0]), json!([{ "text": "ask" }]));
    }

    #[tokio::test]
    async fn passes_the_conversation_as_data_app_state_and_the_agent_to_render_content() {
        let model = model_with(&["ok"]);
        let received = Arc::new(Mutex::new(None));
        let record = Arc::clone(&received);
        let config = ContextInjectorConfig::new(move |context| {
            let roles: Vec<_> = context.messages.iter().map(Message::role).collect();
            *record.lock().unwrap() = Some((
                roles,
                context.app_state.get_value("key"),
                context.agent.app_state().get_value("key"),
                context.agent.id().to_owned(),
            ));
            async { Ok(None) }
        });
        let mut agent = setup(&model, config);
        agent.app_state().set("key", json!("stashed")).unwrap();

        agent.invoke("ask").await.unwrap();

        assert_eq!(
            received.lock().unwrap().take(),
            Some((
                vec![Role::User],
                Some(json!("stashed")),
                Some(json!("stashed")),
                agent.id().to_owned()
            ))
        );
    }

    /// TS `it.each`: `returns empty text` and `throws`.
    #[test]
    fn leaves_the_conversation_unchanged_when_render_content() {
        let blank = ContextInjectorConfig::new(|_context| async { Ok(Some("   ".to_owned())) });
        let throws = ContextInjectorConfig::new(|_context| async { Err(Error::other("boom")) });
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        for (config, expected) in [
            (blank, Vec::new()),
            (
                throws,
                vec!["reason=<boom> | injection renderContent threw; skipping injection".to_owned()],
            ),
        ] {
            let model = model_with(&["ok"]);
            let mut agent = setup(&model, config);

            let (result, warnings) = WarnRecorder::capture(|| runtime.block_on(agent.invoke("ask")));
            result.unwrap();

            assert_eq!(agent.messages()[0].content(), [text("ask")]);
            assert_eq!(warnings, expected);
        }
    }
}
