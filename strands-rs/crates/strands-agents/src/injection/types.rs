//! Injection configuration shared by every injection consumer
//! (`strands-ts/src/injection/types.ts`).

use std::sync::Arc;

use crate::agent::Agent;
use crate::types::Message;

use super::message_injection::is_user_turn;

/// When an injector contributes text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InjectionTrigger {
    /// Only on a fresh user ask: the last message is from the user and carries no tool results.
    #[default]
    UserTurn,
    /// Every model call.
    EveryTurn,
}

/// What an injector sees when deciding what to inject.
pub struct InjectionContext {
    /// The conversation as it will be sent to the model.
    pub messages: Vec<Message>,
    /// The invoking agent (its [`state`](Agent::state) is the durable app state).
    pub agent: Agent,
}

type PredicateFn = Arc<dyn Fn(&InjectionContext) -> bool + Send + Sync>;

/// The resolved gate of an injection (TS `resolveTrigger`): a built-in [`InjectionTrigger`]
/// policy or a custom predicate over the [`InjectionContext`].
#[derive(Clone)]
pub struct Trigger(Kind);

#[derive(Clone)]
enum Kind {
    Builtin(InjectionTrigger),
    Predicate(PredicateFn),
}

impl Trigger {
    /// Gate on a built-in policy.
    pub fn builtin(trigger: InjectionTrigger) -> Self {
        Self(Kind::Builtin(trigger))
    }

    /// Gate on a custom predicate.
    pub fn predicate<F>(predicate: F) -> Self
    where
        F: Fn(&InjectionContext) -> bool + Send + Sync + 'static,
    {
        Self(Kind::Predicate(Arc::new(predicate)))
    }

    /// Whether this call should inject.
    pub fn should_inject(&self, context: &InjectionContext) -> bool {
        match &self.0 {
            Kind::Builtin(InjectionTrigger::EveryTurn) => true,
            Kind::Builtin(InjectionTrigger::UserTurn) => is_user_turn(&context.messages),
            Kind::Predicate(predicate) => predicate(context),
        }
    }
}

impl Default for Trigger {
    fn default() -> Self {
        Self::builtin(InjectionTrigger::default())
    }
}

impl From<InjectionTrigger> for Trigger {
    fn from(trigger: InjectionTrigger) -> Self {
        Self::builtin(trigger)
    }
}

impl std::fmt::Debug for Trigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Kind::Builtin(trigger) => f.debug_tuple("Trigger").field(trigger).finish(),
            Kind::Predicate(_) => f.write_str("Trigger(predicate)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_fixtures::test_agent_with;
    use crate::injection::tests::user_with_tool_result;

    fn context(messages: Vec<Message>) -> InjectionContext {
        InjectionContext {
            messages,
            agent: test_agent_with("trigger", None, Vec::new()),
        }
    }

    #[test]
    fn defaults_to_the_user_turn_policy() {
        let trigger = Trigger::default();
        assert!(trigger.should_inject(&context(vec![Message::user("ask")])));
        assert!(!trigger.should_inject(&context(vec![user_with_tool_result()])));
    }

    #[test]
    fn user_turn_uses_is_user_turn() {
        let trigger = Trigger::builtin(InjectionTrigger::UserTurn);
        assert!(trigger.should_inject(&context(vec![Message::user("ask")])));
        assert!(!trigger.should_inject(&context(vec![user_with_tool_result()])));
    }

    #[test]
    fn every_turn_always_fires() {
        let trigger = Trigger::from(InjectionTrigger::EveryTurn);
        assert!(trigger.should_inject(&context(Vec::new())));
        assert!(trigger.should_inject(&context(vec![user_with_tool_result()])));
    }

    #[test]
    fn uses_a_custom_predicate_over_the_context() {
        let trigger = Trigger::predicate(|context| context.messages.len() >= 2);
        assert!(!trigger.should_inject(&context(vec![Message::user("a")])));
        assert!(trigger.should_inject(&context(vec![
            Message::user("a"),
            Message::assistant("b")
        ])));
    }

    #[test]
    fn debug_names_the_policy_without_leaking_the_predicate() {
        assert_eq!(format!("{:?}", Trigger::default()), "Trigger(UserTurn)");
        assert_eq!(format!("{:?}", Trigger::predicate(|_| true)), "Trigger(predicate)");
    }
}
