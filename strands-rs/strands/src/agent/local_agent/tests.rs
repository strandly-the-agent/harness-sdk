//! `LocalAgent` accessors and the unconfigured default model.

use futures::StreamExt;

use super::*;
use crate::hooks::events::BeforeToolCallEvent;
use crate::hooks::fixtures::mock_local_agent;
use crate::testing::MockModel;
use crate::types::messages::Role;

#[test]
fn aux_model_falls_back_to_the_model() {
    let mut agent = mock_local_agent();
    assert!(Arc::ptr_eq(agent.aux_model(), agent.model()));
    let aux: Arc<dyn Model> = Arc::new(MockModel::new());
    agent.set_aux_model(Some(Arc::clone(&aux)));
    assert!(Arc::ptr_eq(agent.aux_model(), &aux));
    let model: Arc<dyn Model> = Arc::new(MockModel::new());
    agent.set_model(Arc::clone(&model));
    assert!(Arc::ptr_eq(agent.model(), &model));
    agent.set_aux_model(None);
    assert!(Arc::ptr_eq(agent.aux_model(), &model));
}

#[test]
fn accessors_change_the_agent_state() {
    let mut agent = mock_local_agent();
    assert_eq!(
        (agent.id(), agent.name(), agent.description()),
        ("agent", "Strands Agent", None)
    );
    agent
        .messages_mut()
        .push(Message::new(Role::User, vec!["Hi".into()]));
    assert_eq!(agent.messages().len(), 1);
    agent.set_messages(Vec::new());
    assert!(agent.messages().is_empty());
    agent.set_system_prompt(Some(SystemPrompt::Text("Be brief.".to_owned())));
    assert_eq!(
        agent.system_prompt(),
        Some(&SystemPrompt::Text("Be brief.".to_owned()))
    );
    assert!(agent.tool_registry_mut().list().is_empty());
    agent.app_state().set("k", 1).unwrap();
    assert_eq!(agent.app_state().get::<i32>("k").unwrap(), Some(1));
    assert!(agent.model_state().keys().is_empty());
}

#[test]
fn add_hook_registers_on_the_shared_registry() {
    let agent = mock_local_agent();
    agent.add_hook(|_: &mut BeforeToolCallEvent| {});
    assert!(format!("{:?}", agent.hooks()).contains("before_tool_call: 1"));
}

#[tokio::test]
async fn an_agent_without_a_model_fails_its_first_model_call() {
    let model = default_model();
    let mut stream = model.stream(ModelRequest::default());
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.name(), "DefaultNotConfiguredError");
    assert_eq!(error.to_string(), DEFAULT_MODEL_NOT_CONFIGURED);
    model.update_config(BaseModelConfig {
        model_id: Some("m".to_owned()),
        ..BaseModelConfig::default()
    });
    assert_eq!(model.model_id().as_deref(), Some("m"));
}
