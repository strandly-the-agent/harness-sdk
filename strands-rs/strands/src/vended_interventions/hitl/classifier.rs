//! Risk classifiers for [`HumanInTheLoop`](super::HumanInTheLoop) (TS
//! `vended-interventions/hitl/classifier.ts`).

use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::agent::{Agent, InvocationContext};
use crate::error::Result;
use crate::hooks::BeforeToolCallEvent;
use crate::models::model::Model;
use crate::tools::structured_output::StructuredOutputSchema;
use crate::types::agent::InvokeOptions;
use crate::types::json::to_js_string_pretty;

/// A classifier's verdict on one tool call (TS `ClassifierResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClassifierResult {
    /// Whether the call needs human approval.
    pub requires_human_in_the_loop: bool,
    /// A short reason, shown to the human in the approval prompt.
    pub reason: Option<String>,
}

impl ClassifierResult {
    /// A verdict with no reason.
    pub fn new(requires_human_in_the_loop: bool) -> Self {
        Self {
            requires_human_in_the_loop,
            reason: None,
        }
    }

    /// Sets the reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Decides whether a tool call needs approval (TS `HumanInTheLoopClassifier`). It reads what it
/// needs from the event and returns an owned future; a failure counts as approval required.
pub type HumanInTheLoopClassifier =
    Arc<dyn Fn(&BeforeToolCallEvent) -> BoxFuture<'static, Result<ClassifierResult>> + Send + Sync>;

/// Configuration of the built-in LLM risk classifier (TS `LlmClassifierConfig`).
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct LlmClassifierConfig {
    /// Replaces the built-in risk-evaluation prompt.
    #[builder(into)]
    pub system_prompt: Option<String>,
    /// The classifying model; order: this model > `agent.aux_model` > `agent.model`.
    pub model: Option<Arc<dyn Model>>,
}

impl fmt::Debug for LlmClassifierConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmClassifierConfig")
            .field("system_prompt", &self.system_prompt)
            .field("model", &self.model.is_some())
            .finish()
    }
}

// TS `RISK_DECISION`. A doc comment here would become the schema description, which TS doesn't send.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct RiskDecision {
    /// Whether this tool call requires human approval before executing
    requires_approval: bool,
    /// Brief reason (under 10 words) why approval is or is not required
    reason: String,
}

const DEFAULT_SYSTEM_PROMPT: &str = "You are a risk evaluator for an AI agent's tool calls. Your job is to decide whether each tool call requires human approval before executing.

## When to require approval

Require approval when the tool call:
- Is destructive or irreversible (deleting data, dropping tables, revoking access)
- Modifies important state in production or shared environments
- Accesses or transmits sensitive data (credentials, PII, financial records)
- Communicates externally (sending emails, posting messages, making payments)
- Has a large blast radius (affecting many records, users, or systems)

## When approval is NOT needed

Do not require approval when the tool call:
- Is read-only AND does not access sensitive data (listing non-sensitive files, querying public data, searching)
- Operates on local or temporary resources
- Has easily reversible effects
- Is scoped to a single non-critical resource

Note: even read-only operations that access credentials, secrets, PII, or financial data still require approval.

## Instructions

Evaluate the tool name and its input arguments. Consider what could go wrong if this specific call executes with these specific arguments. When uncertain, require approval.

Keep your reason under 10 words — it is shown to a human in a CLI prompt.";

/// The built-in classifier (TS `createLlmRiskClassifier`): asks a model, with structured
/// output, whether the tool call is risky. Model order: `config.model > agent.aux_model >
/// agent.model`; it runs in an auxiliary invocation context so its usage counts toward the request.
pub(crate) fn create_llm_risk_classifier(config: LlmClassifierConfig) -> HumanInTheLoopClassifier {
    let system_prompt = config
        .system_prompt
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_owned());
    let configured_model = config.model;
    Arc::new(move |event| {
        let model = configured_model
            .clone()
            .unwrap_or_else(|| Arc::clone(event.agent().aux_model()));
        let system_prompt = system_prompt.clone();
        let prompt = format!(
            "Should this tool call require human approval?\n\n{}",
            format_tool_call(event)
        );
        let invocation_context = InvocationContext::auxiliary(event.invocation_context());
        Box::pin(async move {
            let mut inner = Agent::builder()
                .model(model)
                .system_prompt(system_prompt)
                .structured_output_schema(StructuredOutputSchema::of::<RiskDecision>())
                .printer(false)
                .build()?;
            let options = InvokeOptions {
                invocation_context,
                ..InvokeOptions::default()
            };
            let result = inner.invoke_with(prompt, options).await?;
            let decision: RiskDecision = result.structured_output()?;
            Ok(ClassifierResult {
                requires_human_in_the_loop: decision.requires_approval,
                reason: Some(decision.reason),
            })
        })
    })
}

fn format_tool_call(event: &BeforeToolCallEvent) -> String {
    let input = to_js_string_pretty(event.tool_use().input()).unwrap_or_default();
    format!("Tool: {}\nInput: {input}", event.tool_use().name())
}
