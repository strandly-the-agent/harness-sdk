//! The steering handler base
//! (`strands-ts/src/vended-interventions/steering/handlers/handler.ts`).
//!
//! TS has an abstract `SteeringHandler` class; Rust has no inheritance, so the base splits into
//! the two reusable pieces: [`SteeringProviders`] (the provider plumbing a handler owns) and
//! [`SteeringAction`] (the narrowed decision, convertible into a [`ToolCallAction`]). A handler
//! implements [`InterventionHandler`](crate::interventions::InterventionHandler) itself and is
//! registered with `AgentBuilder::intervention`, exactly like any other intervention.

use std::sync::Arc;

use crate::agent::Agent;
use crate::error::Result;
use crate::interventions::{Confirm, ToolCallAction};

use super::context::{SteeringContextData, SteeringContextProvider};

/// What a steering handler may decide at the tool-call gate: the wider intervention vocabulary
/// (deny, transform) is deliberately out of the steering contract.
#[non_exhaustive]
pub enum SteeringAction {
    /// Let the tool run.
    Proceed,
    /// Let the model know it should try something else; the tool does not run.
    Guide(String),
    /// Ask a human first.
    Confirm(Confirm),
}

impl From<SteeringAction> for ToolCallAction {
    fn from(action: SteeringAction) -> Self {
        match action {
            SteeringAction::Proceed => ToolCallAction::Proceed,
            SteeringAction::Guide(feedback) => ToolCallAction::Guide(feedback),
            SteeringAction::Confirm(confirm) => ToolCallAction::Confirm(confirm),
        }
    }
}

/// The context providers a steering handler evaluates against.
///
/// Hold one in your handler, hand it the agent from
/// [`InterventionHandler::observe_agent`](crate::interventions::InterventionHandler::observe_agent)
/// so the providers can register their hooks, and read
/// [`steering_context`](Self::steering_context) when deciding.
///
/// ```
/// use std::sync::Arc;
/// use async_trait::async_trait;
/// use strands_agents::agent::Agent;
/// use strands_agents::error::Result;
/// use strands_agents::hooks::BeforeToolCallEvent;
/// use strands_agents::interventions::vended::steering::{
///     SteeringAction, SteeringProviders, ToolLedgerProvider,
/// };
/// use strands_agents::interventions::{InterventionHandler, ToolCallAction};
///
/// struct NoWritesHandler {
///     providers: SteeringProviders,
/// }
///
/// #[async_trait]
/// impl InterventionHandler for NoWritesHandler {
///     fn name(&self) -> &str {
///         "no-writes"
///     }
///
///     async fn observe_agent(&self, agent: &Agent) -> Result<()> {
///         self.providers.observe_agent(agent).await
///     }
///
///     async fn before_tool_call(&self, event: &BeforeToolCallEvent) -> Result<ToolCallAction> {
///         let action = if event.tool_use.name.starts_with("write") {
///             SteeringAction::Guide("Read before you write.".to_owned())
///         } else {
///             SteeringAction::Proceed
///         };
///         Ok(action.into())
///     }
/// }
///
/// let handler = NoWritesHandler {
///     providers: SteeringProviders::new([Arc::new(ToolLedgerProvider::new()) as _]),
/// };
/// ```
#[derive(Clone, Default)]
pub struct SteeringProviders {
    providers: Vec<Arc<dyn SteeringContextProvider>>,
}

impl SteeringProviders {
    /// Plumbing over `providers`, in evaluation order.
    pub fn new(providers: impl IntoIterator<Item = Arc<dyn SteeringContextProvider>>) -> Self {
        Self {
            providers: providers.into_iter().collect(),
        }
    }

    /// The providers, in evaluation order.
    pub fn providers(&self) -> &[Arc<dyn SteeringContextProvider>] {
        &self.providers
    }

    /// Hand `agent` to every provider so they can register their hooks.
    pub async fn observe_agent(&self, agent: &Agent) -> Result<()> {
        for provider in &self.providers {
            provider.observe_agent(agent).await?;
        }
        Ok(())
    }

    /// Snapshot of every provider's context (TS `getSteeringContext`).
    pub fn steering_context(&self) -> Vec<SteeringContextData> {
        self.providers.iter().map(|p| p.context()).collect()
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::hooks::events::{BeforeToolCallEvent, Cancel};
    use crate::interventions::{InterventionHandler, CONFIRMATION_FAILED_PREFIX};
    use crate::testing::{MockMessageModel, MockResponse};
    use crate::types::{Message, ToolUse};

    struct FakeProvider;

    #[async_trait]
    impl SteeringContextProvider for FakeProvider {
        fn name(&self) -> &str {
            "fake"
        }

        async fn observe_agent(&self, _agent: &Agent) -> Result<()> {
            Ok(())
        }

        fn context(&self) -> SteeringContextData {
            SteeringContextData::new("fake").with("tokens", 42)
        }
    }

    /// Records what the handler saw and returns a fixed action.
    struct Spy {
        providers: SteeringProviders,
        action: Box<dyn Fn() -> SteeringAction + Send + Sync>,
        seen: std::sync::Mutex<Vec<(String, Vec<SteeringContextData>)>>,
    }

    impl Spy {
        fn new(action: impl Fn() -> SteeringAction + Send + Sync + 'static) -> Self {
            Self {
                providers: SteeringProviders::default(),
                action: Box::new(action),
                seen: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn with_providers(mut self, providers: SteeringProviders) -> Self {
            self.providers = providers;
            self
        }
    }

    #[async_trait]
    impl InterventionHandler for Spy {
        fn name(&self) -> &str {
            "spy"
        }

        async fn observe_agent(&self, agent: &Agent) -> Result<()> {
            self.providers.observe_agent(agent).await
        }

        async fn before_tool_call(&self, event: &BeforeToolCallEvent) -> Result<ToolCallAction> {
            self.seen
                .lock()
                .unwrap()
                .push((event.tool_use.name.clone(), self.providers.steering_context()));
            Ok((self.action)().into())
        }
    }

    fn tool_use() -> ToolUse {
        ToolUse {
            name: "searchWeb".to_owned(),
            tool_use_id: "tu-1".to_owned(),
            input: json!({ "q": "hi" }),
        }
    }

    fn before_tool_call_event(agent: &Agent) -> BeforeToolCallEvent {
        BeforeToolCallEvent {
            agent: agent.clone(),
            tool_use: tool_use(),
            tool: None,
            invocation_state: Default::default(),
            cancel: Cancel::No,
            selected_tool: None,
        }
    }

    async fn agent_with(handler: Arc<dyn InterventionHandler>) -> Agent {
        let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("hi"))]);
        let agent = Agent::builder()
            .model(model)
            .interventions([handler])
            .build()
            .unwrap();
        agent.initialize().await.unwrap();
        agent
    }

    #[tokio::test]
    async fn routes_before_tool_call_to_the_handler_with_the_event() {
        let handler = Arc::new(Spy::new(|| SteeringAction::Guide("try again".to_owned())));
        let agent = agent_with(handler.clone()).await;

        let mut event = before_tool_call_event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(handler.seen.lock().unwrap()[0].0, "searchWeb");
        let Cancel::Reason(reason) = &event.cancel else {
            panic!("expected a guided cancel, got {:?}", event.cancel);
        };
        assert!(reason.contains("GUIDANCE:"), "{reason}");
        assert!(reason.contains("try again"), "{reason}");
    }

    #[tokio::test]
    async fn exposes_provider_context_to_the_handler_via_steering_context() {
        let handler = Arc::new(
            Spy::new(|| SteeringAction::Proceed)
                .with_providers(SteeringProviders::new([Arc::new(FakeProvider) as _])),
        );
        let agent = agent_with(handler.clone()).await;

        let mut event = before_tool_call_event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(
            handler.seen.lock().unwrap()[0].1,
            vec![SteeringContextData::new("fake").with("tokens", 42)]
        );
    }

    #[tokio::test]
    async fn siblings_with_distinct_names_can_coexist_on_one_agent() {
        struct Named(&'static str);

        #[async_trait]
        impl InterventionHandler for Named {
            fn name(&self) -> &str {
                self.0
            }
        }

        let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("hi"))]);
        let built = Agent::builder()
            .model(model)
            .interventions([
                Arc::new(Named("steer:tool")) as Arc<dyn InterventionHandler>,
                Arc::new(Named("steer:model")) as Arc<dyn InterventionHandler>,
            ])
            .build();

        assert!(built.is_ok());
    }

    #[tokio::test]
    async fn confirm_decision_flows_through_the_interrupt_system_on_resume_approved() {
        let handler = Arc::new(Spy::new(|| {
            SteeringAction::Confirm(Confirm::new("approve searchWeb?").with_response("yes"))
        }));
        let agent = agent_with(handler).await;

        let mut event = before_tool_call_event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(event.cancel, Cancel::No);
    }

    #[tokio::test]
    async fn confirm_decision_sets_cancel_when_human_denies() {
        let handler = Arc::new(Spy::new(|| {
            SteeringAction::Confirm(Confirm::new("approve searchWeb?").with_response("no"))
        }));
        let agent = agent_with(handler).await;

        let mut event = before_tool_call_event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(
            event.cancel,
            Cancel::Reason(format!("{CONFIRMATION_FAILED_PREFIX}approve searchWeb?"))
        );
    }
}
