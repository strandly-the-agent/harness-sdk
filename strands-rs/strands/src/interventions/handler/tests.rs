//! Ported from strands-ts/src/interventions/__tests__/handler.test.ts.
//!
//! `override detection works via prototype comparison` has no Rust form: a trait can't tell
//! whether an impl overrides a default method, so the registry always dispatches to every
//! handler and the defaults proceed.

use std::num::NonZeroU32;
use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::testing::MockMessageModel;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::tools::ToolUse;

struct NoOpHandler;

impl InterventionHandler for NoOpHandler {
    fn name(&self) -> &str {
        "no-op"
    }
}

#[tokio::test]
async fn default_methods_return_proceed() {
    let handler = NoOpHandler;
    let model = Arc::new(MockMessageModel::new());
    let mut agent = LocalAgent::new(model.clone());
    let mut tool_event = BeforeToolCallEvent::new(
        agent.tool_agent(),
        ToolUse::new("test", "id", json!({})),
        None,
        InvocationState::new(),
    );
    let action = handler.before_tool_call(&mut tool_event).await.unwrap();
    assert!(matches!(action, BeforeToolCallAction::Proceed(proceed) if proceed.reason.is_none()));

    let mut model_event =
        AfterModelCallEvent::new(&mut agent, model, InvocationState::new(), NonZeroU32::MIN);
    let action = handler.after_model_call(&mut model_event).await.unwrap();
    assert!(matches!(action, AfterModelCallAction::Proceed(proceed) if proceed.reason.is_none()));
    assert_eq!(handler.on_error(), OnError::Throw);
}
