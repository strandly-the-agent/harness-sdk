//! Rust examples for the model routing documentation.

use strands_agents::Error;

async fn model_routing_ordered_fallback() -> Result<(), Error> {
    // --8<-- [start:mr_fallback]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, Model, ModelRouter};

    let primary_model: Arc<dyn Model> = Arc::new(
        BedrockModel::builder()
            .model_id("us.amazon.nova-pro-v1:0")
            .build()
            .await?,
    );
    let backup_model: Arc<dyn Model> = Arc::new(
        BedrockModel::builder()
            .model_id("us.anthropic.claude-haiku-4-5-20251001-v1:0")
            .build()
            .await?,
    );

    let router =
        ModelRouter::new(vec![primary_model, backup_model])?.with_max_switches(1);
    let agent = Agent::builder().model_router(router).build()?;

    agent
        .invoke("Summarize the tradeoffs of active-active deployment.")
        .await?;
    // --8<-- [end:mr_fallback]
    Ok(())
}

async fn model_routing_classifier() -> Result<(), Error> {
    // --8<-- [start:mr_classify]
    use std::sync::Arc;

    use serde_json::json;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{Agent, ClassifierStrategy, Model, ModelRouter, RoutingCandidate};

    let routing_policy = "Choose the lowest-latency candidate that satisfies every \
                          request requirement. Use candidate metadata as evidence.";
    let classifier_model: Arc<dyn Model> = Arc::new(
        BedrockModel::new(BedrockModelConfig {
            model_id: Some("us.anthropic.claude-haiku-4-5-20251001-v1:0".to_owned()),
            max_tokens: Some(64),
            temperature: Some(0.0),
            ..Default::default()
        })
        .await?,
    );
    let routine_model: Arc<dyn Model> = Arc::new(
        BedrockModel::builder()
            .model_id("us.amazon.nova-lite-v1:0")
            .build()
            .await?,
    );
    let advanced_model: Arc<dyn Model> = Arc::new(
        BedrockModel::builder()
            .model_id("us.amazon.nova-pro-v1:0")
            .build()
            .await?,
    );

    let router = ModelRouter::new(vec![
        RoutingCandidate::new(routine_model)
            .with_name("routine")
            .with_description("Concise factual questions and routine requests.")
            .with_metadata(json!({
                "cost": "low",
                "latency": "low",
                "complexity": "routine",
            }))?,
        RoutingCandidate::new(advanced_model)
            .with_name("advanced")
            .with_description("Systems design with several interacting constraints.")
            .with_metadata(json!({
                "cost": "high",
                "latency": "medium",
                "complexity": "advanced",
            }))?,
    ])?
    .with_strategy(
        ClassifierStrategy::new(classifier_model).with_system_prompt(routing_policy),
    );
    let agent = Agent::builder().model_router(router).build()?;

    agent
        .invoke(
            "Design a rollback-safe migration from regional to global idempotency keys.",
        )
        .await?;
    // --8<-- [end:mr_classify]
    Ok(())
}

async fn model_routing_composed_strategy() -> Result<(), Error> {
    // --8<-- [start:mr_composed]
    use std::sync::Arc;

    use async_trait::async_trait;
    use strands_agents::models::routing::RoutingContext;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{
        Agent, ClassifierStrategy, FallbackStrategy, Model, ModelRouter, Result,
        RoutingCandidate, RoutingStrategy,
    };

    struct ClassifyThenFallback {
        classifier: ClassifierStrategy,
        fallback: FallbackStrategy,
    }

    #[async_trait]
    impl RoutingStrategy for ClassifyThenFallback {
        fn name(&self) -> &str {
            "ClassifyThenFallback"
        }

        async fn select(
            &self,
            context: &RoutingContext,
        ) -> Result<Option<RoutingCandidate>> {
            if context.attempts.is_empty() {
                self.classifier.select(context).await
            } else {
                self.fallback.select(context).await
            }
        }
    }

    let classifier_model: Arc<dyn Model> = Arc::new(
        BedrockModel::new(BedrockModelConfig {
            model_id: Some("us.anthropic.claude-haiku-4-5-20251001-v1:0".to_owned()),
            max_tokens: Some(64),
            temperature: Some(0.0),
            ..Default::default()
        })
        .await?,
    );
    let strategy = ClassifyThenFallback {
        classifier: ClassifierStrategy::new(classifier_model),
        fallback: FallbackStrategy,
    };
    let routine_model: Arc<dyn Model> = Arc::new(
        BedrockModel::builder()
            .model_id("us.amazon.nova-lite-v1:0")
            .build()
            .await?,
    );
    let advanced_model: Arc<dyn Model> = Arc::new(
        BedrockModel::builder()
            .model_id("us.amazon.nova-pro-v1:0")
            .build()
            .await?,
    );
    let router = ModelRouter::new(vec![
        RoutingCandidate::new(routine_model)
            .with_name("routine")
            .with_description("Concise factual questions and routine requests."),
        RoutingCandidate::new(advanced_model)
            .with_name("advanced")
            .with_description("Systems design with several interacting constraints."),
    ])?
    .with_strategy(strategy);
    let agent = Agent::builder().model_router(router).build()?;

    agent
        .invoke(
            "Design a rollback-safe migration from regional to global idempotency keys.",
        )
        .await?;
    // --8<-- [end:mr_composed]
    Ok(())
}
