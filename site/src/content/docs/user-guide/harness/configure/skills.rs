//! Snippets for `skills.mdx`.

async fn harness_skills() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_skills]
    use strands_harness::{create_harness, HarnessAgentOptions, SkillsOption};

    let agent = create_harness(HarnessAgentOptions {
        skills: SkillsOption::Sources(vec![
            "./team-skills".into(),
            "./project-skills".into(),
        ]),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_skills]

    let _ = agent;
    Ok(())
}

async fn sdk_skills() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_skills]
    use strands_agents::models::BedrockModel;
    use strands_agents::plugins::vended::AgentSkills;
    use strands_agents::Agent;

    let plugin = AgentSkills::new(["./team-skills", "./project-skills"]);
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .plugin(plugin)
        .build()?;
    // --8<-- [end:sdk_skills]

    let _ = agent;
    Ok(())
}

async fn harness_skills_off() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_skills_off]
    use strands_harness::{create_harness, HarnessAgentOptions, SkillsOption};

    let agent = create_harness(HarnessAgentOptions {
        skills: SkillsOption::Off,
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_skills_off]

    let _ = agent;
    Ok(())
}

async fn sdk_skills_off() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_skills_off]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Raw Strands Harness SDK loads no skills unless you add AgentSkills; omit it.
    let agent = Agent::new(BedrockModel::default_model().await?)?;
    // --8<-- [end:sdk_skills_off]

    let _ = agent;
    Ok(())
}
