use std::time::Duration;

use futures::StreamExt;
use strands::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use strands::models::streaming::{ModelMessageStartEvent, ModelStreamEvent};
use strands::types::messages::Role;
use strands::{Agent, CancelOptions};

/// A model that, like `AnthropicModel` (P-187), doesn't read `ModelRequest::cancel_signal`:
/// it streams a message start and then stays silent (time to first token, extended thinking).
struct SilentModel;

impl Model for SilentModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, strands::Result<ModelStreamEvent>> {
        let start = futures::stream::iter([Ok(ModelMessageStartEvent::new(Role::Assistant).into())]);
        Box::pin(start.chain(futures::stream::pending()))
    }
    fn get_config(&self) -> BaseModelConfig {
        BaseModelConfig::default()
    }
    fn update_config(&self, _config: BaseModelConfig) {}
}

#[tokio::test]
async fn a_signalled_cancel_stops_an_invocation_whose_model_stream_is_silent() {
    let mut agent = Agent::builder().model(SilentModel).printer(false).build().unwrap();
    let handle = agent.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.cancel(CancelOptions::default());
    });
    let result = tokio::time::timeout(Duration::from_secs(5), agent.invoke("go")).await;
    assert!(result.is_ok(), "cancel() fired 4.8 s ago and the invocation is still waiting on the model stream");
}
