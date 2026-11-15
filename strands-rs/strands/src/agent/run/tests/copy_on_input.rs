//! Copy-on-input isolation, ported from
//! strands-ts/src/middleware/__tests__/copy-on-input.test.ts. The TS reference-identity checks
//! become checks that changing the context's copy leaves the agent unchanged.

use schemars::JsonSchema;
use serde::Deserialize;

use super::*;
use crate::middleware::{AgentStreamStage, ExecuteToolStage, InvokeModelStage, MiddlewareStage};
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::ModelStreamEvent;
use crate::state_store::StateStore;
use crate::test_support::{mock_tool, text_result};
use crate::tools::StructuredOutputSchema;
use crate::types::agent::InvokeArgs;
use crate::types::json::JsonObject;
use crate::types::tools::{ToolChoice, ToolSpec};

/// A mock model that runs `act` on the request's model state and records it.
struct StateModel {
    inner: Arc<MockMessageModel>,
    act: Box<dyn Fn(&StateStore) + Send + Sync>,
    seen: Arc<Mutex<Option<JsonObject>>>,
}

impl StateModel {
    fn new(act: impl Fn(&StateStore) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            inner: mock(vec![vec![text("Hi")]]),
            act: Box::new(act),
            seen: Arc::default(),
        })
    }
}

impl Model for StateModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        *self.seen.lock().unwrap() = Some(request.model_state.get_all());
        (self.act)(&request.model_state);
        self.inner.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        BaseModelConfig::default()
    }

    fn update_config(&self, _config: BaseModelConfig) {}
}

fn agent_on(model: Arc<dyn Model>, model_state: JsonObject) -> Agent {
    Agent::builder()
        .model(model)
        .model_state(model_state)
        .printer(false)
        .build()
        .unwrap()
}

fn object(value: &JsonValue) -> JsonObject {
    value.as_object().cloned().unwrap()
}

fn hi_agent() -> Agent {
    Agent::builder()
        .model(mock(vec![vec![text("Hi")]]))
        .printer(false)
        .build()
        .unwrap()
}

#[tokio::test]
async fn middleware_receives_a_deep_copy_of_messages_not_a_reference_to_the_agent_array() {
    let mut agent = hi_agent();
    agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
        context.messages.clear();
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(texts(&agent.messages()[0]), ["Hello"]);
}

#[tokio::test]
async fn mutating_the_context_messages_array_does_not_affect_agent_messages() {
    let mut agent = hi_agent();
    agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
        context
            .messages
            .push(Message::new(Role::User, vec![text("injected")]));
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    let users: Vec<_> = agent
        .messages()
        .iter()
        .filter(|message| message.role() == Role::User)
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(texts(users[0]), ["Hello"]);
}

#[tokio::test]
async fn message_content_blocks_are_deep_copied_not_shared_references() {
    let mut agent = hi_agent();
    agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
        if let Some(first) = context.messages.first_mut() {
            *first = Message::new(Role::User, vec![text("rewritten")]);
        }
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(texts(&agent.messages()[0]), ["Hello"]);
}

#[tokio::test]
async fn message_metadata_is_deep_copied_not_shared_reference() {
    let mut agent = hi_agent();
    let mut custom = JsonObject::new();
    custom.insert("original".to_owned(), json!("data"));
    let tagged = Message::new(Role::User, vec![text("Hello")])
        .with_metadata(MessageMetadata::new().with_custom(custom.clone()));
    agent.messages_mut().push(tagged);
    agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
        if let Some(first) = context.messages.first_mut() {
            let mut injected = JsonObject::new();
            injected.insert("injected".to_owned(), json!("value"));
            *first = first
                .clone()
                .with_metadata(MessageMetadata::new().with_custom(injected));
        }
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    let metadata = agent.messages()[0].metadata().unwrap();
    assert_eq!(metadata.custom(), Some(&custom));
}

#[tokio::test]
async fn string_systemprompt_is_passed_through_immutable_primitive() {
    let mut agent = Agent::builder()
        .model(mock(vec![vec![text("Hi")]]))
        .system_prompt("Be helpful")
        .printer(false)
        .build()
        .unwrap();
    let seen: Arc<Mutex<Option<SystemPrompt>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        *slot.lock().unwrap() = context.system_prompt.clone();
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(
        seen.lock().unwrap().take(),
        Some(SystemPrompt::from("Be helpful"))
    );
}

#[tokio::test]
async fn array_systemprompt_is_deep_copied() {
    let blocks = SystemPrompt::from(vec![TextBlock::new("Be helpful").into()]);
    let mut agent = Agent::builder()
        .model(mock(vec![vec![text("Hi")]]))
        .system_prompt(blocks.clone())
        .printer(false)
        .build()
        .unwrap();
    agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
        context.system_prompt = Some(SystemPrompt::from("replaced"));
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(agent.system_prompt(), Some(&blocks));
}

#[tokio::test]
async fn toolspecs_are_deep_copied_across_invocations() {
    let mut agent = Agent::builder()
        .model(mock(vec![vec![text("Hi")], vec![text("Hi again")]]))
        .tools([mock_tool("testTool", |ctx| {
            text_result(ctx.tool_use().tool_use_id(), "ok")
        })])
        .printer(false)
        .build()
        .unwrap();
    let seen: Arc<Mutex<Vec<Vec<ToolSpec>>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(InvokeModelStage::wrap(move |mut context, next| {
        slot.lock().unwrap().push(context.tool_specs.clone());
        context.tool_specs.clear();
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    agent.invoke("Hello again").await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0], seen[1]);
    assert!(seen[0].iter().any(|spec| spec.name() == "testTool"));
}

#[tokio::test]
async fn model_state_changes_are_written_back_to_agent_modelstate_after_streaming() {
    let model = StateModel::new(|state| state.set("responseId", "resp_123").unwrap());
    let mut agent = agent_on(model, JsonObject::new());
    agent.invoke("Hello").await.unwrap();
    assert_eq!(
        agent.model_state().get_all(),
        object(&json!({"responseId": "resp_123"}))
    );
}

#[tokio::test]
async fn model_state_deletions_are_synced_back_to_agent_modelstate() {
    let model = StateModel::new(|state| {
        state.delete("oldKey");
        state.set("newKey", "fresh").unwrap();
    });
    let mut agent = agent_on(model, object(&json!({"oldKey": "stale"})));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(agent.model_state().get_value("oldKey"), None);
    assert_eq!(agent.model_state().get_value("newKey"), Some(json!("fresh")));
}

#[tokio::test]
async fn middleware_mutations_to_agent_modelstate_before_next_do_not_affect_the_model_call() {
    let model = StateModel::new(|_| {});
    let seen = Arc::clone(&model.seen);
    let mut agent = agent_on(model, object(&json!({"key": "snapshotted"})));
    let state = agent.model_state().clone();
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        state.set("key", "mutated-before-next").unwrap();
        state.set("extra", "injected").unwrap();
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(
        seen.lock().unwrap().take(),
        Some(object(&json!({"key": "snapshotted"})))
    );
}

#[tokio::test]
async fn middleware_mutations_to_agent_modelstate_after_next_do_not_persist() {
    let model = StateModel::new(|state| state.set("fromModel", "model-wrote-this").unwrap());
    let mut agent = agent_on(model, object(&json!({"key": "original"})));
    let state = agent.model_state().clone();
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        let state = state.clone();
        let mut inner = next.run(context);
        Box::pin(async_stream::stream! {
            while let Some(item) = inner.next().await {
                yield item;
            }
            state.set("sneaky", "should-be-gone").unwrap();
            state.set("fromModel", "overwritten").unwrap();
        })
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(
        agent.model_state().get_value("fromModel"),
        Some(json!("model-wrote-this"))
    );
    assert_eq!(agent.model_state().get_value("sneaky"), None);
    assert_eq!(agent.model_state().get_value("key"), Some(json!("original")));
}

#[derive(Clone)]
struct Touched(bool);

#[derive(Clone)]
struct MyKey(&'static str);

#[tokio::test]
async fn invocationstate_is_shared_by_reference_mutations_are_visible_on_the_result() {
    let mut agent = hi_agent();
    agent.add_middleware(InvokeModelStage::wrap(|context, next| {
        context.invocation_state.insert(Touched(true));
        next.run(context)
    }));
    let state = InvocationState::new();
    state.insert(MyKey("myValue"));
    let options = InvokeOptions {
        invocation_state: state,
        ..InvokeOptions::default()
    };
    let result = agent.invoke_with("Hello", options).await.unwrap();
    assert_eq!(
        result.invocation_state().get::<MyKey>().map(|key| key.0),
        Some("myValue")
    );
    assert!(
        result
            .invocation_state()
            .get::<Touched>()
            .is_some_and(|touched| touched.0)
    );
}

#[tokio::test]
async fn toolchoice_is_undefined_when_not_explicitly_configured() {
    let mut agent = hi_agent();
    let seen: Arc<Mutex<Vec<Option<ToolChoice>>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        slot.lock().unwrap().push(context.tool_choice.clone());
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(*seen.lock().unwrap(), [None]);
}

#[tokio::test]
async fn toolchoice_is_deep_copied_when_present() {
    #[derive(Deserialize, JsonSchema)]
    #[expect(dead_code, reason = "only the schema is used")]
    struct Person {
        name: String,
    }
    let model = mock(vec![
        vec![text("plain response")],
        vec![tool_use(
            "strands_structured_output",
            "so-1",
            json!({"name": "Alice"}),
        )],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .structured_output_schema(StructuredOutputSchema::of::<Person>())
        .printer(false)
        .build()
        .unwrap();
    let seen: Arc<Mutex<Vec<Option<ToolChoice>>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        slot.lock().unwrap().push(context.tool_choice.clone());
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    let defined = seen.lock().unwrap().iter().flatten().next().cloned();
    assert_eq!(
        serde_json::to_value(defined).unwrap(),
        json!({"tool": {"name": "strands_structured_output"}})
    );
}

#[tokio::test]
async fn middleware_can_pass_a_new_context_with_modified_messages_to_next() {
    let mut agent = hi_agent();
    agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
        context
            .messages
            .push(Message::new(Role::User, vec![text("extra context")]));
        next.run(context)
    }));
    let seen: Arc<Mutex<Vec<Message>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        *slot.lock().unwrap() = context.messages.clone();
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(texts(&seen[1]), ["extra context"]);
}

#[tokio::test]
async fn middleware_can_pass_a_new_context_with_modified_toolspecs_to_next() {
    let mut agent = Agent::builder()
        .model(mock(vec![vec![text("Hi")]]))
        .tools([mock_tool("myTool", |ctx| {
            text_result(ctx.tool_use().tool_use_id(), "ok")
        })])
        .printer(false)
        .build()
        .unwrap();
    agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
        context.tool_specs.clear();
        next.run(context)
    }));
    let seen: Arc<Mutex<Option<Vec<ToolSpec>>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        *slot.lock().unwrap() = Some(context.tool_specs.clone());
        next.run(context)
    }));
    agent.invoke("Hello").await.unwrap();
    assert_eq!(seen.lock().unwrap().take(), Some(Vec::new()));
}

fn tool_calling_agent() -> Agent {
    let model = mock(vec![
        vec![tool_use(
            "testTool",
            "tool-1",
            json!({"key": "value", "nested": {"arr": [1, 2, 3]}}),
        )],
        vec![text("Done")],
    ]);
    Agent::builder()
        .model(model)
        .tools([mock_tool("testTool", |ctx| {
            text_result(ctx.tool_use().tool_use_id(), "ok")
        })])
        .printer(false)
        .build()
        .unwrap()
}

#[tokio::test]
async fn tooluse_input_is_a_deep_copy_of_the_model_output() {
    let mut agent = tool_calling_agent();
    let seen: Arc<Mutex<Option<JsonValue>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(ExecuteToolStage::wrap(move |context, next| {
        *slot.lock().unwrap() = Some(context.tool_use.input().clone());
        next.run(context)
    }));
    agent.invoke("Call the tool").await.unwrap();
    assert_eq!(
        seen.lock().unwrap().take(),
        Some(json!({"key": "value", "nested": {"arr": [1, 2, 3]}}))
    );
}

#[tokio::test]
async fn mutating_tooluse_input_in_middleware_does_not_affect_the_original_message() {
    let mut agent = tool_calling_agent();
    agent.add_middleware(ExecuteToolStage::wrap(|mut context, next| {
        let mut input = context.tool_use.input().clone();
        input["injected"] = json!(true);
        context.tool_use =
            crate::types::tools::ToolUse::new(context.tool_use.name(), context.tool_use.tool_use_id(), input);
        next.run(context)
    }));
    agent.invoke("Call the tool").await.unwrap();
    let ContentBlock::ToolUse(block) = &agent.messages()[1].content()[0] else {
        panic!("expected a tool use");
    };
    assert!(block.input().get("injected").is_none());
}

#[tokio::test]
async fn invocationstate_is_shared_by_reference_intentionally_mutable() {
    #[derive(Clone)]
    struct Original;
    let mut agent = tool_calling_agent();
    agent.add_middleware(ExecuteToolStage::wrap(|context, next| {
        context.invocation_state.insert(Touched(true));
        next.run(context)
    }));
    let state = InvocationState::new();
    state.insert(Original);
    let options = InvokeOptions {
        invocation_state: state,
        ..InvokeOptions::default()
    };
    let result = agent.invoke_with("Call the tool", options).await.unwrap();
    assert!(result.invocation_state().get::<Original>().is_some());
    assert!(result.invocation_state().get::<Touched>().is_some());
}

#[tokio::test]
async fn args_is_shared_by_reference_with_the_caller() {
    let mut agent = hi_agent();
    let input = vec![Message::new(Role::User, vec![text("Hello")])];
    let seen: Arc<Mutex<Option<InvokeArgs>>> = Arc::default();
    let slot = Arc::clone(&seen);
    agent.add_middleware(AgentStreamStage::wrap(move |context, next| {
        *slot.lock().unwrap() = Some(context.args.clone());
        next.run(context)
    }));
    agent.invoke(input.clone()).await.unwrap();
    assert_eq!(seen.lock().unwrap().take(), Some(InvokeArgs::from(input)));
}
