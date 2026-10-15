//! Snippets for `quickstart.mdx`. Each program is its own module so the file compiles.

mod run_bedrock {
    // --8<-- [start:run_bedrock]
    use strands_harness::{create_harness, HarnessAgentOptions};

    #[tokio::main]
    async fn main() -> Result<(), Box<dyn std::error::Error>> {
        let agent = create_harness(HarnessAgentOptions::default()).await?;
        let result = agent
            .invoke(
                "Research the three most common strategies for versioning a REST API, \
                 compare their tradeoffs, and write a recommendation to \
                 api-versioning.md",
            )
            .await?;
        println!("{result}");
        Ok(())
    }
    // --8<-- [end:run_bedrock]
}

mod run_anthropic {
    // --8<-- [start:run_anthropic]
    use strands_harness::{create_harness, HarnessAgentOptions};

    #[tokio::main]
    async fn main() -> Result<(), Box<dyn std::error::Error>> {
        let agent = create_harness(HarnessAgentOptions {
            model: Some("anthropic/claude-sonnet-5".into()),
            ..Default::default()
        })
        .await?;
        let result = agent
            .invoke(
                "Research the three most common strategies for versioning a REST API, \
                 compare their tradeoffs, and write a recommendation to \
                 api-versioning.md",
            )
            .await?;
        println!("{result}");
        Ok(())
    }
    // --8<-- [end:run_anthropic]
}

mod run_openai {
    // --8<-- [start:run_openai]
    use strands_harness::{create_harness, HarnessAgentOptions};

    #[tokio::main]
    async fn main() -> Result<(), Box<dyn std::error::Error>> {
        let agent = create_harness(HarnessAgentOptions {
            model: Some("openai/gpt-5.4".into()),
            ..Default::default()
        })
        .await?;
        let result = agent
            .invoke(
                "Research the three most common strategies for versioning a REST API, \
                 compare their tradeoffs, and write a recommendation to \
                 api-versioning.md",
            )
            .await?;
        println!("{result}");
        Ok(())
    }
    // --8<-- [end:run_openai]
}

async fn keep_a_conversation() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:keep_a_conversation]
    use strands_harness::{
        create_harness, HarnessAgentOptions, SessionConfig, SessionOption,
    };

    let agent = create_harness(HarnessAgentOptions {
        session: SessionOption::Config(SessionConfig {
            id: Some("api-design".into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;
    agent
        .invoke(
            "Which of those would you pick for an API with external customers, and why?",
        )
        .await?;
    // --8<-- [end:keep_a_conversation]

    Ok(())
}
