//! Rust snippets for `interventions/index.mdx`.

use serde_json::json;
use strands_agents::tools::{FunctionTool, ToolContext};
use strands_agents::types::JsonValue;

// Mock tools for examples
fn search_tool() -> FunctionTool {
    FunctionTool::builder("search")
        .description("Search for information")
        .input_schema(json!({
            "type": "object",
            "properties": { "query": { "type": "string" } }
        }))
        .handler(
            |_input: JsonValue, _ctx: ToolContext| async move { Ok("search results") },
        )
}

fn delete_tool() -> FunctionTool {
    FunctionTool::builder("delete_file")
        .description("Delete a file")
        .input_schema(json!({
            "type": "object",
            "properties": { "path": { "type": "string" } }
        }))
        .handler(|_input: JsonValue, _ctx: ToolContext| async move { Ok("deleted") })
}

/// Stands in for the reader's own redactor.
fn redact_pii(body: &str) -> String {
    body.to_owned()
}

// =====================
// Basic Usage
// =====================

async fn basic_usage_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:basic_usage]
    use async_trait::async_trait;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::interventions::{InterventionHandler, ToolCallAction};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{Agent, Result};

    struct ToolGuard {
        blocked_tools: Vec<String>,
    }

    #[async_trait]
    impl InterventionHandler for ToolGuard {
        fn name(&self) -> &str {
            "tool-guard"
        }

        async fn before_tool_call(
            &self,
            event: &BeforeToolCallEvent,
        ) -> Result<ToolCallAction> {
            if self.blocked_tools.contains(&event.tool_use.name) {
                return Ok(ToolCallAction::Deny(format!(
                    "Tool '{}' is not allowed in this environment",
                    event.tool_use.name
                )));
            }
            Ok(ToolCallAction::Proceed)
        }
    }

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(search_tool())
        .tool(delete_tool())
        .intervention(ToolGuard { blocked_tools: vec!["delete_file".to_owned()] })
        .build()?;

    // The agent can search freely, but any attempt to call delete_file
    // is blocked before execution: the model sees the denial reason
    // and adjusts its approach
    agent.invoke("Clean up the temp directory").await?;
    // --8<-- [end:basic_usage]
    Ok(())
}

// =====================
// Action Types
// =====================

fn action_types_example() {
    // --8<-- [start:action_types]
    use async_trait::async_trait;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::interventions::{Confirm, InterventionHandler, ToolCallAction};
    use strands_agents::Result;

    // Deny: block tool calls that access production resources
    struct EnvironmentGuard;

    #[async_trait]
    impl InterventionHandler for EnvironmentGuard {
        fn name(&self) -> &str {
            "environment-guard"
        }

        async fn before_tool_call(
            &self,
            event: &BeforeToolCallEvent,
        ) -> Result<ToolCallAction> {
            let database = event.tool_use.input["database"].as_str().unwrap_or_default();
            if database.contains("prod") {
                return Ok(ToolCallAction::Deny(
                    "Production database access is not allowed".to_owned(),
                ));
            }
            Ok(ToolCallAction::Proceed)
        }
    }

    // Guide: steer the model when it tries to send emails without a subject
    struct EmailValidator;

    #[async_trait]
    impl InterventionHandler for EmailValidator {
        fn name(&self) -> &str {
            "email-validator"
        }

        async fn before_tool_call(
            &self,
            event: &BeforeToolCallEvent,
        ) -> Result<ToolCallAction> {
            if event.tool_use.name == "send_email"
                && event.tool_use.input["subject"].is_null()
            {
                return Ok(ToolCallAction::Guide(
                    "All emails must include a subject line.".to_owned(),
                ));
            }
            Ok(ToolCallAction::Proceed)
        }
    }

    // Confirm: require human approval before deleting files
    struct DeleteApproval;

    #[async_trait]
    impl InterventionHandler for DeleteApproval {
        fn name(&self) -> &str {
            "delete-approval"
        }

        async fn before_tool_call(
            &self,
            event: &BeforeToolCallEvent,
        ) -> Result<ToolCallAction> {
            if event.tool_use.name == "delete_file" {
                let path = event.tool_use.input["path"].as_str().unwrap_or_default();
                return Ok(ToolCallAction::Confirm(Confirm::new(format!(
                    "Approve deleting \"{path}\"?"
                ))));
            }
            Ok(ToolCallAction::Proceed)
        }
    }

    // Transform: redact PII from outgoing email bodies
    struct PiiRedactor;

    #[async_trait]
    impl InterventionHandler for PiiRedactor {
        fn name(&self) -> &str {
            "pii-redactor"
        }

        async fn before_tool_call(
            &self,
            event: &BeforeToolCallEvent,
        ) -> Result<ToolCallAction> {
            if event.tool_use.name != "send_email" {
                return Ok(ToolCallAction::Proceed);
            }
            Ok(ToolCallAction::Transform(Box::new(|event: &mut BeforeToolCallEvent| {
                // `redact_pii` is your own function; the SDK vends no redactor
                let body = event.tool_use.input["body"].as_str().unwrap_or_default();
                let redacted = redact_pii(body);
                event.tool_use.input["body"] = redacted.into();
            })))
        }
    }
    // --8<-- [end:action_types]
}

// =====================
// Short-Circuiting
// =====================

async fn short_circuiting_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:short_circuiting]
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use strands_agents::hooks::{AfterModelCallEvent, BeforeToolCallEvent};
    use strands_agents::interventions::{Action, InterventionHandler, ToolCallAction};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{Agent, Result};

    #[derive(Default)]
    struct RateLimiter {
        call_count: AtomicUsize,
    }

    #[async_trait]
    impl InterventionHandler for RateLimiter {
        fn name(&self) -> &str {
            "rate-limiter"
        }

        async fn before_tool_call(
            &self,
            _event: &BeforeToolCallEvent,
        ) -> Result<ToolCallAction> {
            if self.call_count.fetch_add(1, Ordering::Relaxed) >= 10 {
                // Deny short-circuits: handlers registered after this one are skipped
                return Ok(ToolCallAction::Deny("Rate limit exceeded".to_owned()));
            }
            Ok(ToolCallAction::Proceed)
        }
    }

    struct ToneSteeringHandler;

    #[async_trait]
    impl InterventionHandler for ToneSteeringHandler {
        fn name(&self) -> &str {
            "tone-steering"
        }

        async fn after_model_call(
            &self,
            _event: &AfterModelCallEvent,
        ) -> Result<Action<AfterModelCallEvent>> {
            // This handler never runs for denied tool calls
            Ok(Action::Guide("Use a more professional tone.".to_owned()))
        }
    }

    // Handlers evaluate in registration order
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(search_tool())
        .intervention(RateLimiter::default()) // Evaluates first
        .intervention(ToneSteeringHandler) // Skipped if RateLimiter denies
        .build()?;
    // --8<-- [end:short_circuiting]
    let _ = agent;
    Ok(())
}

// =====================
// Confirm Action
// =====================

fn confirm_example() {
    // --8<-- [start:confirm]
    use async_trait::async_trait;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::interventions::{Confirm, InterventionHandler, ToolCallAction};
    use strands_agents::Result;

    struct DeleteApproval;

    #[async_trait]
    impl InterventionHandler for DeleteApproval {
        fn name(&self) -> &str {
            "delete-approval"
        }

        async fn before_tool_call(
            &self,
            event: &BeforeToolCallEvent,
        ) -> Result<ToolCallAction> {
            if event.tool_use.name == "delete_file" {
                let path = event.tool_use.input["path"].as_str().unwrap_or_default();
                // With no `with_response`, the agent interrupts and waits for a human;
                // `Confirm::new(..).with_response(value)` answers up front instead
                return Ok(ToolCallAction::Confirm(Confirm::new(format!(
                    "Approve deleting \"{path}\"?"
                ))));
            }
            Ok(ToolCallAction::Proceed)
        }
    }
    // --8<-- [end:confirm]
}
