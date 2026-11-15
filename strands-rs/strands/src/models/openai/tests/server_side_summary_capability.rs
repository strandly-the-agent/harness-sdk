//! Ported from strands-ts/src/models/__tests__/server-side-summary-capability.test.ts (the
//! `openai chat` and `openai responses` rows).

use serde_json::json;

use super::*;
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::models::openai::OpenAIApi;

fn model(api: OpenAIApi, model_info: Option<ModelInfo>) -> OpenAIModel {
    injected(
        &MockClient::new(json!([])),
        OpenAIModelOptions {
            api,
            config: OpenAIModelConfig {
                model_id: Some("claude-sonnet-5-5".into()),
                model_info,
                ..Default::default()
            },
            ..Default::default()
        },
    )
}

fn server_side_summary(model: &OpenAIModel) -> Option<bool> {
    model
        .model_info()
        .capabilities
        .and_then(|capabilities| capabilities.server_side_summary)
}

#[test]
fn turns_it_off_for_listed_models_unless_the_model_info_override_sets_it() {
    for api in [OpenAIApi::Chat, OpenAIApi::Responses] {
        assert_eq!(server_side_summary(&model(api, None)), Some(false), "{api:?}");
        let overridden = ModelInfo {
            capabilities: Some(ModelCapabilities {
                server_side_summary: Some(true),
                ..Default::default()
            }),
            limits: None,
        };
        assert_eq!(
            server_side_summary(&model(api, Some(overridden))),
            Some(true),
            "{api:?}"
        );
    }
}
