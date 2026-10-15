//! Rust snippets for `hooks.mdx`. Each snippet has its own module for scoping.

mod individual_callback {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Result;

    async fn example() -> Result<()> {
        // --8<-- [start:individual_callback]
        use strands_agents::hooks::{BeforeInvocationEvent, BeforeToolCallEvent};
        use strands_agents::Agent;

        let model = BedrockModel::new(BedrockModelConfig::default()).await?;

        // Register a callback while building the agent
        let agent = Agent::builder()
            .model(model)
            .sync_hook(|_event: &mut BeforeInvocationEvent| {
                println!("Custom callback triggered");
                Ok(())
            })
            .build()?;

        // Or register one on a built agent. The event type picks the slot in the
        // registry, so the closure's parameter type is the subscription.
        agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
            println!("Tool called: {}", event.tool_use.name);
            Ok(())
        });
        // --8<-- [end:individual_callback]

        Ok(())
    }
}

mod plugin_for_hooks {
    // --8<-- [start:plugin_for_hooks]
    use async_trait::async_trait;
    use strands_agents::hooks::{AfterToolCallEvent, BeforeToolCallEvent};
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    struct LoggingPlugin;

    #[async_trait]
    impl Plugin for LoggingPlugin {
        fn name(&self) -> &str {
            "logging-plugin"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
                println!("Calling: {}", event.tool_use.name);
                Ok(())
            });
            agent.hooks().add_sync_callback(|event: &mut AfterToolCallEvent| {
                println!("Completed: {}", event.tool_use.name);
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:plugin_for_hooks]

    use strands_agents::models::{BedrockModel, BedrockModelConfig};

    async fn usage() -> Result<Agent> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        Agent::builder().model(model).plugin(LoggingPlugin).build()
    }
}

mod hook_ordering {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Result;

    async fn example() -> Result<()> {
        // --8<-- [start:hook_ordering]
        use strands_agents::hooks::{hook_order, BeforeToolCallEvent};
        use strands_agents::Agent;

        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        let agent = Agent::builder().model(model).build()?;

        agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
            println!("[logging] Tool called: {}", event.tool_use.name);
            Ok(())
        }); // hook_order::DEFAULT (0)

        // Run before the SDK's earliest hooks
        agent.hooks().add_sync_callback_with_order(
            hook_order::SDK_FIRST - 1,
            |_event: &mut BeforeToolCallEvent| {
                println!("[guardrail] Runs before SDK hooks");
                Ok(())
            },
        );

        // Arbitrary numbers for fine-grained control
        agent
            .hooks()
            .add_sync_callback_with_order(-50, |_event: &mut BeforeToolCallEvent| {
                println!("[validation] Validating input");
                Ok(())
            });

        // Use i32::MIN/i32::MAX for guaranteed absolute first/last
        agent
            .hooks()
            .add_sync_callback_with_order(i32::MIN, |_event: &mut BeforeToolCallEvent| {
                println!("[absolute] Always runs first, no matter what");
                Ok(())
            });
        // --8<-- [end:hook_ordering]

        Ok(())
    }
}

mod invocation_state_in_hooks {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Result;

    async fn example() -> Result<()> {
        // --8<-- [start:invocation_state_in_hooks]
        use strands_agents::agent::{InvocationState, InvokeOptions};
        use strands_agents::hooks::BeforeToolCallEvent;
        use strands_agents::Agent;

        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        let agent = Agent::builder().model(model).build()?;

        agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
            // Read caller-provided context
            let user_id = event.invocation_state.get::<String>("user_id");
            let session_id = event.invocation_state.get::<String>("session_id");

            println!(
                "User {:?} (session {:?}) invoking tool: {}",
                user_id, session_id, event.tool_use.name
            );
            Ok(())
        });

        // Pass invocation state when invoking the agent
        let state = InvocationState::new();
        state.insert("user_id", "user123".to_owned());
        state.insert("session_id", "sess456".to_owned());

        let result = agent
            .invoke_with("Process the data", InvokeOptions::default().with_invocation_state(state))
            .await?;

        // The same handle is returned on the result
        println!("{:?}", result.invocation_state.get::<String>("user_id")); // "user123"
        // --8<-- [end:invocation_state_in_hooks]

        Ok(())
    }
}

mod tool_interception {
    // --8<-- [start:tool_interception]
    use std::sync::Arc;

    use async_trait::async_trait;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::tools::ToolRef;
    use strands_agents::{Agent, Result};

    struct ToolInterceptor {
        safe_alternative: ToolRef,
    }

    #[async_trait]
    impl Plugin for ToolInterceptor {
        fn name(&self) -> &str {
            "tool-interceptor"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            let safe_alternative = Arc::clone(&self.safe_alternative);
            agent.hooks().add_sync_callback(move |event: &mut BeforeToolCallEvent| {
                if event.tool_use.name != "sensitive_tool" {
                    return Ok(());
                }
                // Run a safer tool in place of the registry's match for this call.
                event.selected_tool = Some(Arc::clone(&safe_alternative));
                // Mirror the rename on tool_use so the model sees the substitution.
                event.tool_use.name = safe_alternative.name().to_owned();
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:tool_interception]
}

mod result_modification {
    // --8<-- [start:result_modification]
    use async_trait::async_trait;
    use strands_agents::hooks::AfterToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::types::ToolResultContent;
    use strands_agents::{Agent, Result};

    struct ResultProcessor;

    #[async_trait]
    impl Plugin for ResultProcessor {
        fn name(&self) -> &str {
            "result-processor"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            agent.hooks().add_sync_callback(|event: &mut AfterToolCallEvent| {
                if event.tool_use.name != "calculator" {
                    return Ok(());
                }
                // Prefix calculator output before it propagates to the model.
                for block in &mut event.result.content {
                    if let ToolResultContent::Text(text) = block {
                        *block = ToolResultContent::Text(format!("Result: {text}"));
                    }
                }
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:result_modification]
}

mod composability {
    use async_trait::async_trait;
    use strands_agents::hooks::{AfterInvocationEvent, BeforeInvocationEvent, BeforeToolCallEvent};
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    // --8<-- [start:composability]
    struct RequestLoggingHook;

    #[async_trait]
    impl Plugin for RequestLoggingHook {
        fn name(&self) -> &str {
            "request-logging"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            agent.hooks().add_sync_callback(log_request);
            agent.hooks().add_sync_callback(log_response);
            agent.hooks().add_sync_callback(log_tool_use);
            Ok(())
        }
    }

    // ...
    // --8<-- [end:composability]

    fn log_request(_event: &mut BeforeInvocationEvent) -> Result<()> {
        Ok(())
    }

    fn log_response(_event: &mut AfterInvocationEvent) -> Result<()> {
        Ok(())
    }

    fn log_tool_use(_event: &mut BeforeToolCallEvent) -> Result<()> {
        Ok(())
    }
}

mod logging_modifications {
    // --8<-- [start:logging_modifications]
    use async_trait::async_trait;
    use strands_agents::hooks::AfterToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::types::ToolResultContent;
    use strands_agents::{Agent, Result};

    struct ResultProcessor;

    #[async_trait]
    impl Plugin for ResultProcessor {
        fn name(&self) -> &str {
            "result-processor"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            agent.hooks().add_sync_callback(|event: &mut AfterToolCallEvent| {
                if event.tool_use.name != "calculator" {
                    return Ok(());
                }
                for block in &mut event.result.content {
                    let ToolResultContent::Text(text) = block else {
                        continue;
                    };
                    // Log the change before mutating so the audit trail captures both states.
                    println!("Modifying calculator result: {text}");
                    *block = ToolResultContent::Text(format!("Result: {text}"));
                }
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:logging_modifications]
}

mod fixed_tool_arguments {
    // --8<-- [start:fixed_tool_arguments_class]
    use std::collections::HashMap;

    use async_trait::async_trait;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::types::JsonValue;
    use strands_agents::{Agent, Result};

    /// Fixed parameter values per tool. These values override whatever the agent
    /// provided when the tool is invoked.
    struct ConstantToolArguments {
        fixed_tool_arguments: HashMap<String, JsonValue>,
    }

    #[async_trait]
    impl Plugin for ConstantToolArguments {
        fn name(&self) -> &str {
            "constant-tool-arguments"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            let fixed = self.fixed_tool_arguments.clone();
            agent.hooks().add_sync_callback(move |event: &mut BeforeToolCallEvent| {
                // If the tool is in our list of parameters, then use those parameters
                let Some(parameters_to_fix) = fixed.get(&event.tool_use.name).and_then(JsonValue::as_object)
                else {
                    return Ok(());
                };
                if let Some(tool_input) = event.tool_use.input.as_object_mut() {
                    for (name, value) in parameters_to_fix {
                        tool_input.insert(name.clone(), value.clone());
                    }
                }
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:fixed_tool_arguments_class]

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::FunctionTool;

    fn calculator() -> FunctionTool {
        FunctionTool::builder("calculator")
            .description("Evaluate an expression")
            .handler(|_input: JsonValue, _ctx| async move { Ok("0.67") })
    }

    async fn usage() -> Result<()> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        // --8<-- [start:fixed_tool_arguments_usage]
        use serde_json::json;

        let fix_parameters = ConstantToolArguments {
            fixed_tool_arguments: HashMap::from([("calculator".to_owned(), json!({ "precision": 1 }))]),
        };

        let agent = Agent::builder()
            .model(model)
            .tool(calculator())
            .plugin(fix_parameters)
            .build()?;
        let result = agent.invoke("What is 2 / 3?").await?;
        // --8<-- [end:fixed_tool_arguments_usage]

        println!("{result}");
        Ok(())
    }
}

mod limit_tool_counts {
    // --8<-- [start:limit_tool_counts_class]
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use strands_agents::hooks::{BeforeInvocationEvent, BeforeToolCallEvent};
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    /// Maximum allowed invocations per tool, per agent invocation.
    struct LimitToolCounts {
        max_tool_counts: HashMap<String, u32>,
        tool_counts: Arc<Mutex<HashMap<String, u32>>>,
    }

    #[async_trait]
    impl Plugin for LimitToolCounts {
        fn name(&self) -> &str {
            "limit-tool-counts"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            let counts = Arc::clone(&self.tool_counts);
            agent.hooks().add_sync_callback(move |_event: &mut BeforeInvocationEvent| {
                counts.lock().unwrap().clear();
                Ok(())
            });

            let counts = Arc::clone(&self.tool_counts);
            let max_tool_counts = self.max_tool_counts.clone();
            agent.hooks().add_sync_callback(move |event: &mut BeforeToolCallEvent| {
                let tool_name = event.tool_use.name.clone();
                let tool_count = {
                    let mut counts = counts.lock().unwrap();
                    let count = counts.entry(tool_name.clone()).or_insert(0);
                    *count += 1;
                    *count
                };

                if max_tool_counts.get(&tool_name).is_some_and(|max| tool_count > *max) {
                    event.cancel = format!(
                        "Tool '{tool_name}' has been invoked too many times and is now being \
                         throttled. DO NOT CALL THIS TOOL ANYMORE"
                    )
                    .into();
                }
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:limit_tool_counts_class]

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::FunctionTool;
    use strands_agents::types::JsonValue;

    fn sleep_tool() -> FunctionTool {
        FunctionTool::builder("sleep")
            .description("Sleep for a few milliseconds")
            .handler(|_input: JsonValue, _ctx| async move { Ok("slept") })
    }

    async fn usage() -> Result<()> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        // --8<-- [start:limit_tool_counts_usage]
        let limit_plugin = LimitToolCounts {
            max_tool_counts: HashMap::from([("sleep".to_owned(), 3)]),
            tool_counts: Arc::new(Mutex::new(HashMap::new())),
        };

        let agent = Agent::builder()
            .model(model)
            .tool(sleep_tool())
            .plugin(limit_plugin)
            .build()?;

        // This call will only have 3 successful sleeps
        agent
            .invoke("Sleep 5 times for 10ms each or until you can't anymore")
            .await?;
        // This will sleep successfully again because the count resets every invocation
        agent.invoke("Sleep once").await?;
        // --8<-- [end:limit_tool_counts_usage]

        Ok(())
    }
}

mod model_call_retry {
    // --8<-- [start:model_call_retry_class]
    use async_trait::async_trait;
    use strands_agents::hooks::AfterModelCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    struct RetryOnServiceUnavailable {
        max_retries: u32,
    }

    #[async_trait]
    impl Plugin for RetryOnServiceUnavailable {
        fn name(&self) -> &str {
            "retry-on-service-unavailable"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            let max_retries = self.max_retries;
            agent.hooks().add_sync_callback(move |event: &mut AfterModelCallEvent| {
                // `attempt_count` is 1-indexed and includes the attempt that just failed,
                // so no manual counter is needed to cap retries.
                let failed_with_503 = event
                    .error
                    .as_ref()
                    .is_some_and(|error| error.to_string().contains("ServiceUnavailable"));
                if failed_with_503 && event.attempt_count <= max_retries {
                    event.retry = true;
                }
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:model_call_retry_class]

    use strands_agents::models::{BedrockModel, BedrockModelConfig};

    async fn usage() -> Result<()> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        // --8<-- [start:model_call_retry_usage]
        let agent = Agent::builder()
            .model(model)
            .plugin(RetryOnServiceUnavailable { max_retries: 3 })
            .build()?;

        let result = agent.invoke("What is the capital of France?").await?;
        // --8<-- [end:model_call_retry_usage]

        println!("{result}");
        Ok(())
    }
}

mod propagate_unexpected_errors {
    // --8<-- [start:propagate_unexpected_errors_class]
    use async_trait::async_trait;
    use strands_agents::hooks::AfterToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Error, Result};

    /// Propagate unexpected tool errors instead of returning them to the model.
    struct PropagateUnexpectedErrors {
        allowed: Vec<String>,
    }

    #[async_trait]
    impl Plugin for PropagateUnexpectedErrors {
        fn name(&self) -> &str {
            "propagate-unexpected-errors"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            let allowed = self.allowed.clone();
            agent.hooks().add_sync_callback(move |event: &mut AfterToolCallEvent| {
                let Some(error) = event.error.as_ref() else {
                    return Ok(()); // Tool succeeded
                };
                let message = error.to_string();
                if allowed.iter().any(|allowed| message.contains(allowed)) {
                    return Ok(()); // Let the model retry these
                }
                // Returning `Err` from a callback aborts the invocation.
                Err(Error::other(message))
            });
            Ok(())
        }
    }
    // --8<-- [end:propagate_unexpected_errors_class]

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::FunctionTool;
    use strands_agents::types::JsonValue;

    fn my_tool() -> FunctionTool {
        FunctionTool::builder("my_tool")
            .description("A sample tool")
            .handler(|_input: JsonValue, _ctx| async move { Ok("result") })
    }

    async fn usage() -> Result<Agent> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        // --8<-- [start:propagate_unexpected_errors_usage]
        let agent = Agent::builder()
            .model(model)
            .tool(my_tool())
            .plugin(PropagateUnexpectedErrors {
                allowed: vec!["invalid input".to_owned(), "validation".to_owned()],
            })
            .build()?;
        // --8<-- [end:propagate_unexpected_errors_usage]

        Ok(agent)
    }
}

mod tool_call_retry {
    // --8<-- [start:tool_call_retry_class]
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use strands_agents::hooks::AfterToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    struct RetryOnToolError {
        max_retries: u32,
        attempts: Arc<Mutex<HashMap<String, u32>>>,
    }

    #[async_trait]
    impl Plugin for RetryOnToolError {
        fn name(&self) -> &str {
            "retry-on-tool-error"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            let attempts = Arc::clone(&self.attempts);
            let max_retries = self.max_retries;
            agent.hooks().add_sync_callback(move |event: &mut AfterToolCallEvent| {
                let tool_use_id = event.result.tool_use_id.clone();
                let mut attempts = attempts.lock().unwrap();
                let attempt = attempts.entry(tool_use_id.clone()).or_insert(0);
                *attempt += 1;

                if event.error.is_some() {
                    if *attempt <= max_retries {
                        event.retry = true;
                    }
                } else {
                    // Clean up tracking once the tool call finally succeeds.
                    attempts.remove(&tool_use_id);
                }
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:tool_call_retry_class]

    use strands_agents::models::{BedrockModel, BedrockModelConfig};

    async fn usage() -> Result<()> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        // --8<-- [start:tool_call_retry_usage]
        let agent = Agent::builder()
            .model(model)
            .plugin(RetryOnToolError {
                max_retries: 1,
                attempts: Arc::new(Mutex::new(HashMap::new())),
            })
            .build()?;

        let result = agent.invoke("Fetch the latest metrics").await?;
        // --8<-- [end:tool_call_retry_usage]

        println!("{result}");
        Ok(())
    }
}

mod summarize_after_tools {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Result;

    async fn example() -> Result<()> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        // --8<-- [start:summarize_after_tools]
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        use strands_agents::hooks::AfterInvocationEvent;
        use strands_agents::Agent;

        let resume_count = Arc::new(AtomicU32::new(0));

        let agent = Agent::builder().model(model).build()?;
        agent.hooks().add_sync_callback(move |event: &mut AfterInvocationEvent| {
            // Resume once after a clean turn to ask the model for a one-line summary.
            if resume_count.fetch_add(1, Ordering::Relaxed) == 0 {
                event.resume = Some("Now summarize what you just did in one sentence.".into());
            }
            Ok(())
        });

        let result = agent.invoke("Look up the weather in Seattle").await?;
        // --8<-- [end:summarize_after_tools]

        println!("{result}");
        Ok(())
    }
}

mod iterative_refinement {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Result;

    async fn example() -> Result<()> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        // --8<-- [start:iterative_refinement]
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        use strands_agents::hooks::AfterInvocationEvent;
        use strands_agents::Agent;

        const MAX_ITERATIONS: u32 = 3;
        let iteration = Arc::new(AtomicU32::new(0));

        let agent = Agent::builder().model(model).build()?;
        agent.hooks().add_sync_callback(move |event: &mut AfterInvocationEvent| {
            let current = iteration.fetch_add(1, Ordering::Relaxed) + 1;
            if current > MAX_ITERATIONS {
                return Ok(());
            }
            event.resume = Some(
                format!(
                    "Review your previous response and improve it. \
                     Iteration {current} of {MAX_ITERATIONS}."
                )
                .into(),
            );
            Ok(())
        });

        let result = agent.invoke("Draft a haiku about programming").await?;
        // --8<-- [end:iterative_refinement]

        println!("{result}");
        Ok(())
    }
}
