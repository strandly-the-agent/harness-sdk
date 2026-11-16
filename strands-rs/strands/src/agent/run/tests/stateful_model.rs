//! Ported from strands-ts/src/agent/__tests__/agent.stateful-model.test.ts. The three
//! `SessionManager` cases join with the session manager (Phase 6).

use super::*;
use crate::conversation_manager::{
    ConversationManager, ConversationManagerReduceOptions, SlidingWindowConversationManager,
    SlidingWindowConversationManagerConfig,
};
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::models::streaming::ModelStreamEvent;
use crate::types::json::JsonObject;

/// TS `StatefulMockModel`: advertises itself as stateful and stores the next response id in the
/// request's model state, as a provider capturing it from the wire would.
struct StatefulMockModel {
    inner: Arc<MockMessageModel>,
    response_ids: Vec<&'static str>,
    received: Mutex<Vec<JsonObject>>,
}

impl StatefulMockModel {
    fn new(response_ids: &[&'static str], replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            inner: mock(replies.iter().map(|reply| vec![text(reply)]).collect()),
            response_ids: response_ids.to_vec(),
            received: Mutex::default(),
        })
    }
}

impl Model for StatefulMockModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        let mut received = self.received.lock().unwrap();
        received.push(request.model_state.get_all());
        if let Some(next) = self.response_ids.get(received.len() - 1) {
            request.model_state.set("responseId", next).unwrap();
        }
        self.inner.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        BaseModelConfig {
            model_info: Some(ModelInfo {
                capabilities: Some(ModelCapabilities {
                    stateful: Some(true),
                    ..ModelCapabilities::default()
                }),
                ..ModelInfo::default()
            }),
            ..BaseModelConfig::default()
        }
    }

    fn update_config(&self, _config: BaseModelConfig) {}
}

fn stateful_agent(model: &Arc<StatefulMockModel>) -> Agent {
    Agent::builder()
        .model(Arc::clone(model) as Arc<dyn Model>)
        .printer(false)
        .build()
        .unwrap()
}

fn object(value: JsonValue) -> JsonObject {
    serde_json::from_value(value).unwrap()
}

mod constructor {
    use super::*;

    #[test]
    fn throws_when_a_conversation_manager_is_supplied_alongside_a_stateful_model() {
        let model = StatefulMockModel::new(&[], &[]);
        let manager = SlidingWindowConversationManager::new(
            SlidingWindowConversationManagerConfig::builder()
                .window_size(5)
                .build(),
        )
        .unwrap();
        let error = Agent::builder()
            .model(model as Arc<dyn Model>)
            .conversation_manager(manager)
            .build()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "contextManager and conversationManager cannot be used with a stateful model. The model manages conversation state server-side."
        );
    }

    #[test]
    fn assigns_null_conversation_manager_when_the_model_is_stateful() {
        let agent = stateful_agent(&StatefulMockModel::new(&[], &[]));
        assert_eq!(
            agent.conversation_manager().name(),
            "strands:null-conversation-manager"
        );
    }

    #[test]
    fn initializes_model_state_as_an_empty_store() {
        let agent = stateful_agent(&StatefulMockModel::new(&[], &[]));
        assert!(agent.model_state().get_all().is_empty());
    }

    #[test]
    fn hydrates_model_state_from_agent_config_model_state() {
        let agent = Agent::builder()
            .model(StatefulMockModel::new(&[], &[]) as Arc<dyn Model>)
            .model_state(object(json!({"responseId": "resp_restored"})))
            .printer(false)
            .build()
            .unwrap();
        assert_eq!(
            agent.model_state().get_all(),
            object(json!({"responseId": "resp_restored"}))
        );
    }
}

mod invocation {
    use super::*;

    #[tokio::test]
    async fn passes_model_state_snapshot_to_the_model_and_writes_back_changes() {
        let model = StatefulMockModel::new(&["resp_first"], &["Hi"]);
        let mut agent = stateful_agent(&model);
        agent.invoke("Hello").await.unwrap();
        assert!(model.received.lock().unwrap()[0].is_empty());
        assert_eq!(
            agent.model_state().get_all(),
            object(json!({"responseId": "resp_first"}))
        );
    }

    #[tokio::test]
    async fn clears_messages_after_invocation_since_the_server_holds_history() {
        let model = StatefulMockModel::new(&["resp_1"], &["Hi there"]);
        let mut agent = stateful_agent(&model);
        agent.invoke("First turn").await.unwrap();
        assert!(agent.messages().is_empty());
    }

    #[tokio::test]
    async fn preserves_model_state_across_invocations_so_previous_response_id_chains() {
        let model = StatefulMockModel::new(&["resp_1", "resp_2"], &["one", "two"]);
        let mut agent = stateful_agent(&model);
        agent.invoke("turn 1").await.unwrap();
        assert_eq!(
            agent.model_state().get_all(),
            object(json!({"responseId": "resp_1"}))
        );
        agent.invoke("turn 2").await.unwrap();
        assert_eq!(
            agent.model_state().get_all(),
            object(json!({"responseId": "resp_2"}))
        );
        let received = model.received.lock().unwrap().clone();
        assert_eq!(
            received,
            [JsonObject::new(), object(json!({"responseId": "resp_1"}))]
        );
    }
}

mod stateless_model_default {
    use super::*;

    #[tokio::test]
    async fn does_not_clear_messages_after_invocation() {
        let mut agent = Agent::builder()
            .model(mock(vec![vec![text("Hello")]]))
            .printer(false)
            .build()
            .unwrap();
        agent.invoke("Hi").await.unwrap();
        assert_eq!(agent.messages().len(), 2);
    }

    struct CallerManager;

    #[async_trait::async_trait]
    impl ConversationManager for CallerManager {
        fn name(&self) -> &str {
            "test:caller-manager"
        }

        async fn reduce(&self, _options: ConversationManagerReduceOptions<'_>) -> crate::Result<bool> {
            Ok(false)
        }
    }

    #[test]
    fn uses_the_caller_provided_conversation_manager() {
        let agent = Agent::builder()
            .model(mock(Vec::new()))
            .conversation_manager(CallerManager)
            .build()
            .unwrap();
        assert_eq!(agent.conversation_manager().name(), "test:caller-manager");
    }
}
