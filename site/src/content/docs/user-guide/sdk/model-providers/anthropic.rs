//! Rust examples for the Anthropic model provider documentation.
//! These examples demonstrate common usage patterns for the AnthropicModel.

use strands_agents::Error;

async fn basic_usage() -> Result<(), Error> {
    // --8<-- [start:basic_usage]
    use strands_agents::models::{AnthropicModel, AnthropicModelConfig};
    use strands_agents::Agent;

    let model = AnthropicModel::new(AnthropicModelConfig {
        api_key: Some("<KEY>".to_owned()),
        model_id: Some("claude-sonnet-5".to_owned()),
        max_tokens: Some(1028),
        temperature: Some(0.7),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    println!("{}", agent.invoke("What is 2+2").await?);
    // --8<-- [end:basic_usage]
    Ok(())
}

async fn structured_output() -> Result<(), Error> {
    // --8<-- [start:structured_output]
    use schemars::JsonSchema;
    use serde::Deserialize;
    use strands_agents::models::{AnthropicModel, AnthropicModelConfig};
    use strands_agents::Agent;

    /// Analyze a movie review.
    #[derive(Debug, Deserialize, JsonSchema)]
    struct MovieReview {
        /// Movie title.
        title: String,
        /// Overall sentiment: positive, negative, or neutral.
        sentiment: String,
        /// Rating from 1-10.
        rating: u8,
        /// Primary genre.
        genre: String,
        /// Brief summary of the review.
        summary: String,
    }

    let model = AnthropicModel::new(AnthropicModelConfig {
        api_key: Some("<KEY>".to_owned()),
        model_id: Some("claude-sonnet-5".to_owned()),
        max_tokens: Some(1028),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;

    let review: MovieReview = agent
        .structured_output(
            r#"Just watched "The Matrix" - what an incredible sci-fi masterpiece!
            The groundbreaking visual effects and philosophical themes make this
            a must-watch. Keanu Reeves delivers a solid performance. 9/10!"#,
        )
        .await?;

    println!("Movie: {}", review.title);
    println!("Rating: {}/10", review.rating);
    println!("Genre: {}", review.genre);
    println!("Sentiment: {}", review.sentiment);
    // --8<-- [end:structured_output]
    let _ = review.summary;
    Ok(())
}

async fn builtin_tools() -> Result<(), Error> {
    // --8<-- [start:builtin_tools]
    use serde_json::json;
    use strands_agents::models::{AnthropicModel, AnthropicModelConfig};
    use strands_agents::Agent;

    let model = AnthropicModel::new(AnthropicModelConfig {
        api_key: Some("<KEY>".to_owned()),
        model_id: Some("claude-sonnet-4-6".to_owned()),
        max_tokens: Some(1028),
        anthropic_tools: Some(vec![json!({
            "type": "web_search_20260318",
            "name": "web_search",
            "max_uses": 5,
        })]),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    println!("{}", agent.invoke("What are the latest AI news today?").await?);
    // --8<-- [end:builtin_tools]
    Ok(())
}

async fn prompt_caching() -> Result<(), Error> {
    // --8<-- [start:prompt_caching]
    use strands_agents::models::{AnthropicCacheConfig, AnthropicCacheSection};
    use strands_agents::models::{AnthropicModel, AnthropicModelConfig};
    use strands_agents::Agent;

    let model = AnthropicModel::new(AnthropicModelConfig {
        model_id: Some("claude-sonnet-5".to_owned()),
        max_tokens: Some(1028),
        cache_config: Some(AnthropicCacheConfig {
            ttl: Some("1h".to_owned()),
            tools_ttl: Some(AnthropicCacheSection::Ttl("1h".to_owned())),
            ..Default::default()
        }),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    let result = agent.invoke("Summarize the attached report.").await?;
    println!("{:?}", result.metrics.accumulated_usage);

    // Typical output:
    // Usage { input_tokens: 12, ..., cache_read_input_tokens: Some(2505) }
    // --8<-- [end:prompt_caching]
    Ok(())
}
