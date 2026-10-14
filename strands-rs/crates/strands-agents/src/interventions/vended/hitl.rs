//! Human-in-the-loop (`strands-ts/src/vended-interventions/hitl/**`).
//!
//! Gates tool calls on a human's approval. With no [`ask`](HumanInTheLoop::ask) callback the agent
//! interrupts and the caller resumes it with the answer; with one, the callback is awaited inline.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;

use crate::error::Result;
use crate::hooks::events::BeforeToolCallEvent;
use crate::interventions::{default_evaluate, Confirm, EvaluateFn, InterventionHandler, ToolCallAction};
use crate::types::JsonValue;

/// Agent-state key holding the tools trusted during this run.
pub const TRUSTED_TOOLS_KEY: &str = "hitl:trustedTools";
/// Responses that mean "trust this tool from now on": `"t"`, `"trust"`.
pub const TRUST_RESPONSES: [&str; 2] = ["t", "trust"];

/// Asks the human `prompt` and resolves to their answer.
pub type AskFn = Arc<dyn Fn(String) -> BoxFuture<'static, Result<JsonValue>> + Send + Sync>;

/// Default trust test: `"t"` or `"trust"` (trimmed, case-insensitive).
pub fn default_evaluate_trust(response: &JsonValue) -> bool {
    match response {
        JsonValue::String(s) => TRUST_RESPONSES.contains(&s.trim().to_lowercase().as_str()),
        _ => false,
    }
}

/// Requires human approval before a tool runs.
///
/// Approval precedence: a negated tool (`!name`) always asks; a tool trusted at runtime runs
/// freely; `*` or an explicit entry in [`allowed_tools`](HumanInTheLoop::allowed_tools) runs
/// freely; otherwise approval is required.
#[derive(Clone, Default)]
pub struct HumanInTheLoop {
    allowed_tools: HashSet<String>,
    enable_trust: bool,
    evaluate: Option<EvaluateFn>,
    evaluate_trust: Option<EvaluateFn>,
    ask: Option<AskFn>,
}

impl HumanInTheLoop {
    /// Approval for every tool, in interrupt/resume mode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tools that may run without approval. `"*"` allows all; `"!name"` always asks for `name`.
    #[must_use]
    pub fn allowed_tools<S: Into<String>>(mut self, tools: impl IntoIterator<Item = S>) -> Self {
        self.allowed_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    /// Let the human answer `"t"` to trust a tool for the rest of the run.
    #[must_use]
    pub fn enable_trust(mut self, enable: bool) -> Self {
        self.enable_trust = enable;
        self
    }

    /// Ask inline instead of interrupting (the seam a CLI or UI plugs into).
    #[must_use]
    pub fn ask(mut self, ask: AskFn) -> Self {
        self.ask = Some(ask);
        self
    }

    /// Replace the approval test ([`default_evaluate`] otherwise).
    #[must_use]
    pub fn evaluate(mut self, evaluate: EvaluateFn) -> Self {
        self.evaluate = Some(evaluate);
        self
    }

    /// Replace the trust test ([`default_evaluate_trust`] otherwise).
    #[must_use]
    pub fn evaluate_trust(mut self, evaluate: EvaluateFn) -> Self {
        self.evaluate_trust = Some(evaluate);
        self
    }

    fn is_negated(&self, tool_name: &str) -> bool {
        self.allowed_tools.contains(&format!("!{tool_name}"))
    }

    fn approves(&self, response: &JsonValue) -> bool {
        match &self.evaluate {
            Some(evaluate) => evaluate(response),
            None => default_evaluate(response),
        }
    }

    fn trusts(&self, response: &JsonValue) -> bool {
        match &self.evaluate_trust {
            Some(evaluate) => evaluate(response),
            None => default_evaluate_trust(response),
        }
    }

    fn trusted_tools(&self, event: &BeforeToolCallEvent) -> Vec<String> {
        event
            .agent
            .state()
            .get_as::<Vec<String>>(TRUSTED_TOOLS_KEY)
            .unwrap_or_default()
    }

    fn trust_tool(&self, event: &BeforeToolCallEvent, tool_name: &str) {
        let mut trusted = self.trusted_tools(event);
        if !trusted.iter().any(|t| t == tool_name) {
            trusted.push(tool_name.to_owned());
        }
        event.agent.state().set(
            TRUSTED_TOOLS_KEY,
            serde_json::to_value(trusted).expect("tool names are serializable"),
        );
    }

    /// Whether this call needs a human, in TS's precedence order.
    fn requires_approval(&self, event: &BeforeToolCallEvent, tool_name: &str) -> bool {
        if self.is_negated(tool_name) {
            return true;
        }
        if self.enable_trust && self.trusted_tools(event).iter().any(|t| t == tool_name) {
            return false;
        }
        if self.allowed_tools.contains("*") || self.allowed_tools.contains(tool_name) {
            return false;
        }
        true
    }

    fn prompt(event: &BeforeToolCallEvent, tool_name: &str) -> String {
        let input = serde_json::to_string(&event.tool_use.input).unwrap_or_default();
        format!("Approve \"{tool_name}\"?\n  Input: {input}")
    }

    /// The approval test used for an interrupt-mode confirmation: trust answers approve and are
    /// remembered, everything else goes through the normal test.
    fn confirm_evaluate(&self, event: &BeforeToolCallEvent, tool_name: &str) -> EvaluateFn {
        let this = self.clone();
        let event = event.clone();
        let tool_name = tool_name.to_owned();
        Arc::new(move |response: &JsonValue| {
            if !this.is_negated(&tool_name) && this.enable_trust && this.trusts(response) {
                this.trust_tool(&event, &tool_name);
                return true;
            }
            this.approves(response)
        })
    }
}

#[async_trait]
impl InterventionHandler for HumanInTheLoop {
    fn name(&self) -> &str {
        "strands:human-in-the-loop"
    }

    async fn before_tool_call(&self, event: &BeforeToolCallEvent) -> Result<ToolCallAction> {
        let tool_name = event.tool_use.name.clone();
        if !self.requires_approval(event, &tool_name) {
            return Ok(ToolCallAction::Proceed);
        }
        let prompt = Self::prompt(event, &tool_name);

        let Some(ask) = &self.ask else {
            // Interrupt mode: the registry raises and the agent stops until the caller resumes.
            return Ok(ToolCallAction::Confirm(
                Confirm::new(prompt).with_evaluate(self.confirm_evaluate(event, &tool_name)),
            ));
        };

        let response = ask(prompt.clone()).await?;
        if !self.is_negated(&tool_name) && self.enable_trust && self.trusts(&response) {
            self.trust_tool(event, &tool_name);
            return Ok(ToolCallAction::Proceed);
        }
        // A preemptive response records the interrupt without ever pausing.
        Ok(ToolCallAction::Confirm(
            Confirm::new(prompt)
                .with_response(response)
                .with_evaluate(Arc::new({
                    let this = self.clone();
                    move |response: &JsonValue| this.approves(response)
                })),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_responses_are_recognized() {
        assert!(default_evaluate_trust(&serde_json::json!("t")));
        assert!(default_evaluate_trust(&serde_json::json!(" TRUST ")));
        assert!(!default_evaluate_trust(&serde_json::json!("y")));
        assert!(!default_evaluate_trust(&serde_json::json!(true)));
    }
}
