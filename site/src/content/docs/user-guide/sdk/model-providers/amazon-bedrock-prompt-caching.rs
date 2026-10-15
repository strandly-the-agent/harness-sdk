//! Rust examples for the Amazon Bedrock prompt caching documentation.
//! These examples demonstrate caching system prompts, tools, and messages.

use strands_agents::Error;

async fn system_prompt_caching_full() -> Result<(), Error> {
    // --8<-- [start:system_prompt_caching_full]
    use strands_agents::models::BedrockModel;
    use strands_agents::types::{CachePointBlock, SystemContentBlock, SystemPrompt};
    use strands_agents::Agent;

    let system_content = SystemPrompt::Blocks(vec![
        // needs to be at least 1,024 tokens
        SystemContentBlock::Text(
            "You are a helpful assistant that provides concise answers. \
             This is a long system prompt with detailed instructions..."
                .to_owned()
                + &"...".repeat(1600),
        ),
        SystemContentBlock::CachePoint(CachePointBlock::default()),
    ]);

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt(system_content)
        .build()?;

    // First request will cache the system prompt
    let result = agent.invoke("Tell me about Python").await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));

    // Second request will reuse the cached system prompt
    let result = agent.invoke("Tell me about JavaScript").await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));
    // --8<-- [end:system_prompt_caching_full]
    Ok(())
}

async fn tool_caching_full() -> Result<(), Error> {
    // --8<-- [start:tool_caching_full]
    use strands_agents::models::{
        BedrockModel, BedrockModelConfig, CacheConfig, CacheStrategy,
    };
    use strands_agents::Agent;

    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Auto),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;

    let agent = Agent::builder()
        .model(bedrock_model)
        // Add your tools here
        .build()?;

    // First request will cache the tools
    let result = agent.invoke("What time is it?").await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));

    // Second request will reuse the cached tools
    let result = agent.invoke("What is the square root of 1764?").await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));
    // --8<-- [end:tool_caching_full]
    Ok(())
}

async fn automatic_cache_strategy() -> Result<(), Error> {
    // --8<-- [start:automatic_cache_strategy]
    use strands_agents::models::{
        BedrockModel, BedrockModelConfig, CacheConfig, CacheStrategy,
    };
    use strands_agents::Agent;

    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_owned()),
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Auto),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;

    let agent = Agent::new(bedrock_model)?;

    // Agent call - cache write and read occur as context accumulates
    let result = agent
        .invoke("Search for Python async patterns, then compare with error handling")
        .await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));

    // Follow-up reuses cached context from previous conversation
    let result = agent.invoke("Summarize the key differences").await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));
    // --8<-- [end:automatic_cache_strategy]
    Ok(())
}

async fn messages_caching_full() -> Result<(), Error> {
    // --8<-- [start:messages_caching_full]
    use strands_agents::models::BedrockModel;
    use strands_agents::types::{
        CachePointBlock, ContentBlock, DocumentBlock, DocumentFormat, DocumentSource,
        Message, Role,
    };
    use strands_agents::Agent;

    let document_bytes = b"This is a sample document!".to_vec();

    let user_message = Message::new(
        Role::User,
        vec![
            ContentBlock::Document(DocumentBlock {
                format: DocumentFormat::Txt,
                name: "example".to_owned(),
                source: DocumentSource::Bytes(document_bytes),
                citations: None,
                context: None,
            }),
            ContentBlock::Text("Use this document in your response.".to_owned()),
            // Cache only up to this point
            ContentBlock::CachePoint(CachePointBlock::default()),
        ],
    );

    let assistant_message =
        Message::assistant("I will reference that document in my following responses.");

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .messages([user_message, assistant_message])
        .build()?;

    // First request will cache the message
    let result = agent.invoke("What is in that document?").await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));

    // Second request will reuse the cached message
    let result = agent.invoke("How long is the document?").await?;
    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));
    // --8<-- [end:messages_caching_full]
    Ok(())
}

async fn cache_metrics() -> Result<(), Error> {
    // --8<-- [start:cache_metrics]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;
    let result = agent.invoke("Hello!").await?;

    let usage = &result.metrics.accumulated_usage;
    println!("Cache write tokens: {}", usage.cache_write_input_tokens.unwrap_or(0));
    println!("Cache read tokens: {}", usage.cache_read_input_tokens.unwrap_or(0));
    // --8<-- [end:cache_metrics]
    Ok(())
}
