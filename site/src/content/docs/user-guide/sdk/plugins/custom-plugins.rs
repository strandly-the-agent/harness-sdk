//! Rust snippets for `custom-plugins.mdx`. Each snippet has its own module for scoping.

mod using_plugins {
    use async_trait::async_trait;
    use strands_agents::hooks::BeforeModelCallEvent;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::plugins::Plugin;
    use strands_agents::tools::FunctionTool;
    use strands_agents::{Agent, Result};

    struct GuidancePlugin {
        system_prompt: String,
    }

    #[async_trait]
    impl Plugin for GuidancePlugin {
        fn name(&self) -> &str {
            "guidance-plugin"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            let system_prompt = self.system_prompt.clone();
            agent.hooks().add_sync_callback(move |_event: &mut BeforeModelCallEvent| {
                println!("[Guidance] System prompt: {system_prompt}");
                Ok(())
            });
            Ok(())
        }
    }

    fn my_tool() -> FunctionTool {
        FunctionTool::builder("my_tool")
            .description("A sample tool")
            .handler(|_input, _ctx| async move { Ok("result") })
    }

    async fn build() -> Result<Agent> {
        // --8<-- [start:using_plugins]
        use strands_agents::models::{BedrockModel, BedrockModelConfig};
        use strands_agents::Agent;

        let model = BedrockModel::new(BedrockModelConfig::default()).await?;

        // Create an agent with plugins
        let agent = Agent::builder()
            .model(model)
            .tool(my_tool())
            .plugin(GuidancePlugin {
                system_prompt: "Guide the agent...".to_owned(),
            })
            .build()?;
        // --8<-- [end:using_plugins]

        Ok(agent)
    }
}

mod basic_plugin {
    // --8<-- [start:basic_plugin]
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde::Deserialize;
    use strands_agents::hooks::{AfterToolCallEvent, BeforeToolCallEvent};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::plugins::Plugin;
    use strands_agents::tools::{FunctionTool, ToolRef};
    use strands_agents::{Agent, Result};

    struct LoggingPlugin;

    #[async_trait]
    impl Plugin for LoggingPlugin {
        fn name(&self) -> &str {
            "logging-plugin"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            // Register hooks in init_agent
            agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
                println!("[LOG] Calling tool: {}", event.tool_use.name);
                println!("[LOG] Input: {}", event.tool_use.input);
                Ok(())
            });

            agent.hooks().add_sync_callback(|event: &mut AfterToolCallEvent| {
                println!("[LOG] Tool completed: {}", event.tool_use.name);
                Ok(())
            });
            Ok(())
        }

        fn tools(&self) -> Vec<ToolRef> {
            // Provide additional tools via the plugin
            vec![Arc::new(debug_print_tool())]
        }
    }

    // Custom tool to add
    #[derive(Deserialize, schemars::JsonSchema)]
    struct DebugPrintInput {
        /// The message to print
        message: String,
    }

    fn debug_print_tool() -> FunctionTool {
        FunctionTool::builder("debug_print")
            .description("Print a debug message")
            .typed_handler(|input: DebugPrintInput, _ctx| async move {
                println!("[DEBUG] {}", input.message);
                Ok(format!("Printed: {}", input.message))
            })
    }

    // Using the plugin
    async fn build() -> Result<Agent> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        Agent::builder().model(model).plugin(LoggingPlugin).build()
    }
    // --8<-- [end:basic_plugin]
}

mod hook_decorator_alternative {
    // --8<-- [start:hook_decorator_alternative]
    use async_trait::async_trait;
    use strands_agents::hooks::{AfterModelCallEvent, BeforeModelCallEvent};
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    struct ModelMonitorPlugin;

    #[async_trait]
    impl Plugin for ModelMonitorPlugin {
        fn name(&self) -> &str {
            "model-monitor"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            // Register a hook for a single event type
            agent.hooks().add_sync_callback(|_event: &mut BeforeModelCallEvent| {
                println!("Model call starting...");
                Ok(())
            });

            // The registry is keyed by event type: one callback per type, shared handler
            fn on_model_event(event: &str) {
                println!("Model event: {event}");
            }
            agent.hooks().add_sync_callback(|_event: &mut BeforeModelCallEvent| {
                on_model_event("BeforeModelCallEvent");
                Ok(())
            });
            agent.hooks().add_sync_callback(|_event: &mut AfterModelCallEvent| {
                on_model_event("AfterModelCallEvent");
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:hook_decorator_alternative]
}

mod manual_registration {
    // --8<-- [start:manual_registration]
    use async_trait::async_trait;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    struct ManualPlugin {
        verbose: bool,
    }

    #[async_trait]
    impl Plugin for ManualPlugin {
        fn name(&self) -> &str {
            "manual-plugin"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            // Conditionally register additional hooks
            if self.verbose {
                agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
                    println!("[VERBOSE] {}", serde_json::json!(event.tool_use));
                    Ok(())
                });
            }

            // Access the agent's tools
            println!("Attached to agent with {} tools", agent.tools().len());
            Ok(())
        }
    }
    // --8<-- [end:manual_registration]
}

mod state_management {
    // --8<-- [start:state_management]
    use async_trait::async_trait;
    use serde_json::json;
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    struct MetricsPlugin;

    #[async_trait]
    impl Plugin for MetricsPlugin {
        fn name(&self) -> &str {
            "metrics-plugin"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            // Initialize state values if not present
            if agent.state().get("metrics_call_count").is_none() {
                agent.state().set("metrics_call_count", json!(0));
            }

            agent.hooks().add_sync_callback(|event: &mut BeforeToolCallEvent| {
                let state = event.agent.state();
                let current = state.get_as::<u64>("metrics_call_count").unwrap_or(0);
                state.set("metrics_call_count", json!(current + 1));
                Ok(())
            });
            Ok(())
        }
    }

    // Usage
    async fn build() -> Result<Agent> {
        let model = BedrockModel::new(BedrockModelConfig::default()).await?;
        let agent = Agent::builder().model(model).plugin(MetricsPlugin).build()?;
        agent.initialize().await?;

        println!("Tool calls: {:?}", agent.state().get("metrics_call_count"));
        Ok(agent)
    }
    // --8<-- [end:state_management]
}

mod async_initialization {
    // --8<-- [start:async_initialization]
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{json, Map, Value};
    use strands_agents::hooks::BeforeToolCallEvent;
    use strands_agents::plugins::Plugin;
    use strands_agents::{Agent, Result};

    struct AsyncConfigPlugin;

    impl AsyncConfigPlugin {
        async fn load_config(&self) -> Map<String, Value> {
            // Simulate async operation
            tokio::time::sleep(Duration::from_millis(100)).await;
            json!({ "setting": "value" }).as_object().cloned().unwrap_or_default()
        }
    }

    #[async_trait]
    impl Plugin for AsyncConfigPlugin {
        fn name(&self) -> &str {
            "async-config"
        }

        async fn init_agent(&self, agent: &Agent) -> Result<()> {
            // Async initialization
            let config = self.load_config().await;

            agent.hooks().add_sync_callback(move |_event: &mut BeforeToolCallEvent| {
                println!("Config: {}", Value::Object(config.clone()));
                Ok(())
            });
            Ok(())
        }
    }
    // --8<-- [end:async_initialization]
}

