//! The LLM steering handler
//! (`strands-ts/src/vended-interventions/steering/handlers/llm.ts`).

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::events::BeforeToolCallEvent;
use crate::interventions::{Confirm, InterventionHandler, ToolCallAction};
use crate::models::Model;
use crate::types::{CachePointBlock, ContentBlock, SystemPrompt, ToolUse};

use super::context::{SteeringContextData, SteeringContextProvider};
use super::handler::{SteeringAction, SteeringProviders};
use super::tool_ledger::ToolLedgerProvider;

/// Default handler name.
pub const DEFAULT_LLM_STEERING_NAME: &str = "strands:llm-steering-handler";

/// Builds the evaluation prompt sent to the steering model from the provider context and the tool
/// call under evaluation.
pub type PromptBuilder =
    Arc<dyn Fn(&[SteeringContextData], Option<&ToolUse>) -> Vec<ContentBlock> + Send + Sync>;

/// The decision the steering model returns.
#[derive(Debug, Deserialize, JsonSchema)]
struct SteeringDecision {
    /// Steering decision: 'proceed' to continue, 'guide' to provide feedback, 'confirm' to pause
    /// for human approval
    #[serde(rename = "type")]
    kind: DecisionKind,
    /// Clear explanation of the steering decision and any guidance provided
    reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum DecisionKind {
    Proceed,
    Guide,
    Confirm,
}

/// Default prompt builder: static instructions, a cache point, then the per-call context and
/// event payload.
///
/// See: <https://github.com/strands-agents/agent-sop>
fn default_prompt_builder(context: &[SteeringContextData], tool_use: Option<&ToolUse>) -> Vec<ContentBlock> {
    let context_str = if context.is_empty() {
        "No context available".to_owned()
    } else {
        serde_json::to_string_pretty(context).unwrap_or_else(|_| "No context available".to_owned())
    };

    let action_type = if tool_use.is_some() { "tool call" } else { "action" };
    let action_type_title = if tool_use.is_some() { "Tool Call" } else { "Action" };
    let event_description = match tool_use {
        Some(tool_use) => format!(
            "Tool: {}\nArguments: {}",
            tool_use.name,
            serde_json::to_string_pretty(&tool_use.input).unwrap_or_default()
        ),
        None => "General evaluation".to_owned(),
    };

    let has_ledger = context.iter().any(|c| c.type_name == "toolLedger");
    let ledger_explanation = if has_ledger { LEDGER_EXPLANATION } else { "" };

    // Static framing (cached): role, constraints, decision criteria, ledger semantics.
    let instructions = format!(
        "# Steering Evaluation

## Overview

You are a STEERING AGENT that evaluates a {action_type} that ANOTHER AGENT is attempting to make.
Your job is to provide contextual guidance to help the other agent navigate workflows effectively.
You act as a safety net that can intervene when patterns in the context data suggest the agent
should try a different approach or get human input.

**YOUR ROLE:**
- Analyze context data for concerning patterns (repeated failures, inappropriate timing, etc.)
- Provide just-in-time guidance when the agent is going down an ineffective path
- Allow normal operations to proceed when context shows no issues

**CRITICAL CONSTRAINTS:**
- Base decisions ONLY on the context data provided
- Do NOT use external knowledge about domains, URLs, or tool purposes
- Do NOT make assumptions about what tools \"should\" or \"shouldn't\" do
- Focus ONLY on patterns in the context data{ledger_explanation}

## Steps

### 1. Analyze the {action_type_title}

Review ONLY the context data. Look for patterns in the data that indicate:

- Previous failures or successes with this tool
- Frequency of attempts
- Any relevant tracking information

**Constraints:**
- You MUST base analysis ONLY on the provided context data
- You MUST NOT use external knowledge about tool purposes or domains
- You SHOULD identify patterns in the context data
- You MAY reference relevant context data to inform your decision

### 2. Make Steering Decision

**Constraints:**
- You MUST respond with exactly one of: \"proceed\", \"guide\", or \"confirm\"
- You MUST base the decision ONLY on context data patterns
- Your reason will be shown to the AGENT as guidance

**Decision Options:**
- \"proceed\" if context data shows no concerning patterns
- \"guide\" if context data shows patterns requiring intervention
- \"confirm\" if context data shows patterns requiring human input"
    );

    // Dynamic block (uncached): per-call context and event payload.
    let dynamic = format!(
        "## Context

{context_str}

## Event to Evaluate

{event_description}"
    );

    vec![
        ContentBlock::text(instructions),
        ContentBlock::CachePoint(CachePointBlock::default()),
        ContentBlock::text(dynamic),
    ]
}

const LEDGER_EXPLANATION: &str = "

### Understanding Ledger Tool States

If the context includes a ledger with tool_calls, the \"status\" field indicates:

- **\"pending\"**: The tool is CURRENTLY being evaluated by you (the steering agent).
This is NOT a duplicate call — it's the tool you're deciding whether to approve.
The tool has NOT started executing yet.
- **\"success\"**: The tool completed successfully in a previous turn
- **\"error\"**: The tool failed or was cancelled in a previous turn

**IMPORTANT**: When you see a tool with status=\"pending\" that matches the tool you're evaluating,
that IS the current tool being evaluated. It is NOT already executing or a duplicate.";

/// Steering handler that uses a model to provide contextual guidance.
///
/// Judges every tool call against the natural-language rules in its system prompt and turns the
/// model's answer into an intervention action: proceed, guide the agent with feedback, or ask a
/// human. Register it with `AgentBuilder::intervention`.
///
/// Only the tool-call gate is steered: Rust's
/// [`InterventionHandler`](crate::interventions::InterventionHandler) has no after-model gate, so
/// TS's `afterModelCall` steering has no counterpart here.
///
/// ```
/// use strands_agents::interventions::vended::steering::LlmSteeringHandler;
///
/// let handler = LlmSteeringHandler::new("You ensure emails maintain a cheerful, positive tone.");
/// // Agent::builder().model(model).tool(send_email).intervention(handler)
/// ```
#[derive(Clone)]
pub struct LlmSteeringHandler {
    name: String,
    system_prompt: SystemPrompt,
    model: Option<Arc<dyn Model>>,
    prompt_builder: PromptBuilder,
    providers: SteeringProviders,
}

impl LlmSteeringHandler {
    /// Handler judging tool calls against `system_prompt`, with a [`ToolLedgerProvider`] attached.
    pub fn new(system_prompt: impl Into<SystemPrompt>) -> Self {
        Self {
            name: DEFAULT_LLM_STEERING_NAME.to_owned(),
            system_prompt: system_prompt.into(),
            model: None,
            prompt_builder: Arc::new(default_prompt_builder),
            providers: SteeringProviders::new([ToolLedgerProvider::new().shared()]),
        }
    }

    /// Model used for the evaluation. Resolution order: this model, then
    /// [`Agent::aux_model`] (which itself falls back to the agent's model).
    #[must_use]
    pub fn model(mut self, model: impl Model + 'static) -> Self {
        self.model = Some(Arc::new(model));
        self
    }

    /// Replace the prompt builder ([`default_prompt_builder`] otherwise).
    #[must_use]
    pub fn prompt_builder<F>(mut self, builder: F) -> Self
    where
        F: Fn(&[SteeringContextData], Option<&ToolUse>) -> Vec<ContentBlock> + Send + Sync + 'static,
    {
        self.prompt_builder = Arc::new(builder);
        self
    }

    /// Replace the context providers; an empty list disables context collection.
    #[must_use]
    pub fn context_providers(
        mut self,
        providers: impl IntoIterator<Item = Arc<dyn SteeringContextProvider>>,
    ) -> Self {
        self.providers = SteeringProviders::new(providers);
        self
    }

    /// Override the handler name (needed when attaching more than one steering handler).
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// The context providers this handler evaluates against.
    pub fn providers(&self) -> &SteeringProviders {
        &self.providers
    }

    /// Ask `model` for a decision. A fresh inner agent per call keeps the handler free of shared
    /// mutable state, so it stays safe to attach to agents whose tool calls evaluate concurrently.
    async fn decide(&self, prompt: Vec<ContentBlock>, model: Arc<dyn Model>) -> Result<SteeringDecision> {
        let inner = Agent::builder()
            .model(model)
            .system_prompt(self.system_prompt.clone())
            .build()?;
        inner.structured_output(prompt).await
    }
}

#[async_trait]
impl InterventionHandler for LlmSteeringHandler {
    fn name(&self) -> &str {
        &self.name
    }

    async fn observe_agent(&self, agent: &Agent) -> Result<()> {
        self.providers.observe_agent(agent).await
    }

    async fn before_tool_call(&self, event: &BeforeToolCallEvent) -> Result<ToolCallAction> {
        let context = self.providers.steering_context();
        let prompt = (self.prompt_builder)(&context, Some(&event.tool_use));
        let model = self.model.clone().unwrap_or_else(|| event.agent.aux_model());
        let decision = self.decide(prompt, model).await?;

        let action = match decision.kind {
            DecisionKind::Proceed => {
                tracing::debug!(
                    tool_name = %event.tool_use.name,
                    reason = %decision.reason,
                    "steering: proceed"
                );
                SteeringAction::Proceed
            }
            DecisionKind::Guide => SteeringAction::Guide(decision.reason),
            DecisionKind::Confirm => {
                let mut confirm = Confirm::new(decision.reason.clone());
                confirm.reason = Some(decision.reason);
                SteeringAction::Confirm(confirm)
            }
        };
        Ok(action.into())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::hooks::events::Cancel;
    use crate::interventions::GUIDANCE_PREFIX;
    use crate::testing::{MockMessageModel, MockResponse};
    use crate::types::{Message, Role, ToolUseBlock};

    /// A model that answers the steering question with `decision` via the structured-output tool.
    fn judge(decision: Value) -> Arc<MockMessageModel> {
        MockMessageModel::shared(vec![MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "strands_structured_output",
                "inner-1",
                decision,
            ))],
        ))])
    }

    fn tool_use() -> ToolUse {
        ToolUse {
            name: "searchWeb".to_owned(),
            tool_use_id: "tu-1".to_owned(),
            input: json!({ "q": "hi" }),
        }
    }

    fn event(agent: &Agent) -> BeforeToolCallEvent {
        BeforeToolCallEvent {
            agent: agent.clone(),
            tool_use: tool_use(),
            tool: None,
            invocation_state: Default::default(),
            cancel: Cancel::No,
            selected_tool: None,
        }
    }

    fn handler() -> LlmSteeringHandler {
        LlmSteeringHandler::new("You are a steering agent.").context_providers([])
    }

    #[tokio::test]
    async fn defaults_to_the_parent_agents_model_when_none_is_configured() {
        let model = judge(json!({ "type": "proceed", "reason": "no concerning patterns" }));
        let agent = Agent::builder()
            .model(model.clone())
            .intervention(handler())
            .build()
            .unwrap();
        agent.initialize().await.unwrap();

        let mut event = event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(model.call_count(), 1);
        assert_eq!(event.cancel, Cancel::No);
    }

    #[tokio::test]
    async fn uses_the_agents_aux_model_when_none_is_configured() {
        let agent_model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("unused"))]);
        let aux_model = judge(json!({ "type": "proceed", "reason": "ok" }));
        let agent = Agent::builder()
            .model(agent_model.clone())
            .aux_model(aux_model.clone())
            .intervention(handler())
            .build()
            .unwrap();
        agent.initialize().await.unwrap();

        let mut event = event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(aux_model.call_count(), 1);
        assert_eq!(agent_model.call_count(), 0);
    }

    #[tokio::test]
    async fn uses_the_configured_model_in_preference_to_the_agent_model() {
        let agent_model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("unused"))]);
        let configured = judge(json!({ "type": "proceed", "reason": "ok" }));
        let agent = Agent::builder()
            .model(agent_model.clone())
            .intervention(handler().model(configured.clone()))
            .build()
            .unwrap();
        agent.initialize().await.unwrap();

        let mut event = event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(configured.call_count(), 1);
        assert_eq!(agent_model.call_count(), 0);
    }

    #[tokio::test]
    async fn resolves_the_agents_model_at_tool_call_time_so_reassignment_takes_effect() {
        let first = judge(json!({ "type": "proceed", "reason": "ok" }));
        let second = judge(json!({ "type": "proceed", "reason": "ok" }));
        let agent = Agent::builder()
            .model(first.clone())
            .intervention(handler())
            .build()
            .unwrap();
        agent.initialize().await.unwrap();
        agent.set_model(second.clone());

        let mut event = event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(second.call_count(), 1);
        assert_eq!(first.call_count(), 0);
    }

    #[tokio::test]
    async fn guide_decision_cancels_the_tool_call_with_the_models_reason() {
        let model = judge(json!({ "type": "guide", "reason": "search the docs first" }));
        let agent = Agent::builder()
            .model(model)
            .intervention(handler())
            .build()
            .unwrap();
        agent.initialize().await.unwrap();

        let mut event = event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(
            event.cancel,
            Cancel::Reason(format!(
                "{GUIDANCE_PREFIX}[{DEFAULT_LLM_STEERING_NAME}] search the docs first"
            ))
        );
    }

    #[tokio::test]
    async fn confirm_decision_asks_the_human_with_the_models_reason() {
        let model = judge(json!({ "type": "confirm", "reason": "this deletes data" }));
        let agent = Agent::builder()
            .model(model)
            .intervention(handler())
            .build()
            .unwrap();
        agent.initialize().await.unwrap();

        let mut event = event(&agent);
        let error = agent.hooks().invoke_callbacks(&mut event).await.unwrap_err();

        let crate::error::Error::Interrupt(interrupts) = error else {
            panic!("expected an interrupt, got {error}");
        };
        assert_eq!(interrupts[0].name, DEFAULT_LLM_STEERING_NAME);
        assert_eq!(interrupts[0].reason, Some(json!("this deletes data")));
    }

    #[tokio::test]
    async fn attaches_a_tool_ledger_provider_by_default() {
        let handler = LlmSteeringHandler::new("steer");
        let names: Vec<_> = handler
            .providers()
            .providers()
            .iter()
            .map(|p| p.name().to_owned())
            .collect();
        assert_eq!(names, vec![super::super::DEFAULT_TOOL_LEDGER_NAME]);
    }

    #[tokio::test]
    async fn the_ledger_entry_for_the_evaluated_call_is_pending_in_the_prompt() {
        let model = judge(json!({ "type": "proceed", "reason": "ok" }));
        let agent = Agent::builder()
            .model(model.clone())
            .intervention(LlmSteeringHandler::new("You are a steering agent."))
            .build()
            .unwrap();
        agent.initialize().await.unwrap();

        let mut event = event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        let prompt = model.calls()[0].0[0].text();
        assert!(prompt.contains("\"status\": \"pending\""), "{prompt}");
        assert!(prompt.contains("Tool: searchWeb"), "{prompt}");
        assert!(prompt.contains("Understanding Ledger Tool States"), "{prompt}");
    }

    #[test]
    fn default_prompt_says_no_context_available_and_omits_ledger_help_without_providers() {
        let blocks = default_prompt_builder(&[], Some(&tool_use()));
        let instructions = match &blocks[0] {
            ContentBlock::Text(text) => text.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        let dynamic = match &blocks[2] {
            ContentBlock::Text(text) => text.clone(),
            other => panic!("expected text, got {other:?}"),
        };

        assert!(matches!(blocks[1], ContentBlock::CachePoint(_)));
        assert!(!instructions.contains("Understanding Ledger Tool States"));
        assert!(dynamic.contains("No context available"), "{dynamic}");
        assert!(
            dynamic.contains("Tool: searchWeb\nArguments: {\n  \"q\": \"hi\"\n}"),
            "{dynamic}"
        );
    }

    #[test]
    fn default_prompt_describes_a_general_evaluation_without_a_tool_call() {
        let blocks = default_prompt_builder(&[], None);
        let ContentBlock::Text(instructions) = &blocks[0] else {
            panic!("expected text");
        };
        let ContentBlock::Text(dynamic) = &blocks[2] else {
            panic!("expected text");
        };
        assert!(
            instructions.contains("evaluates a action that ANOTHER AGENT"),
            "{instructions}"
        );
        assert!(
            instructions.contains("### 1. Analyze the Action"),
            "{instructions}"
        );
        assert!(dynamic.contains("General evaluation"), "{dynamic}");
    }

    #[tokio::test]
    async fn a_custom_prompt_builder_replaces_the_default() {
        let model = judge(json!({ "type": "proceed", "reason": "ok" }));
        let agent = Agent::builder()
            .model(model.clone())
            .intervention(handler().prompt_builder(|_context, tool_use| {
                vec![ContentBlock::text(format!(
                    "judge {}",
                    tool_use.map_or("nothing", |t| t.name.as_str())
                ))]
            }))
            .build()
            .unwrap();
        agent.initialize().await.unwrap();

        let mut event = event(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(model.calls()[0].0[0].text(), "judge searchWeb");
    }
}
