//! Rust snippets for `skills.mdx`.

// =====================
// Usage
// =====================

fn usage() -> Result<(), strands_agents::Error> {
    // --8<-- [start:usage]
    use strands_agents::plugins::{AgentSkills, Skill, SkillSource};
    use strands_agents::testing::MockMessageModel;
    use strands_agents::Agent;

    // Single skill directory
    let plugin = AgentSkills::new(["./skills/pdf-processing"]);

    // Parent directory loads all child directories
    // containing SKILL.md
    let plugin_from_dir = AgentSkills::new(["./skills/"]);

    // Mixed sources: paths and programmatic skills
    let plugin_mixed = AgentSkills::new([
        SkillSource::from("./skills/pdf-processing"),
        SkillSource::from("./skills/"),
        SkillSource::from(Skill::new(
            "custom-greeting",
            "Generate custom greetings",
            "Always greet the user by name with enthusiasm.",
        )),
    ]);

    let agent = Agent::builder()
        .model(MockMessageModel::shared(vec![]))
        .plugin(plugin_mixed)
        .build()?;
    // --8<-- [end:usage]

    let _ = (plugin, plugin_from_dir, agent);
    Ok(())
}

// =====================
// Providing tools for resource access
// =====================

fn tools() -> Result<(), strands_agents::Error> {
    // --8<-- [start:tools]
    use strands_agents::plugins::AgentSkills;
    use strands_agents::testing::MockMessageModel;
    use strands_agents::tools::vended::{FileEditorTool, ShellTool};
    use strands_agents::Agent;

    let plugin = AgentSkills::new(["./skills/"]);

    let agent = Agent::builder()
        .model(MockMessageModel::shared(vec![]))
        .plugin(plugin)
        .tool(ShellTool::new())
        .tool(FileEditorTool::new())
        .build()?;
    // --8<-- [end:tools]

    let _ = agent;
    Ok(())
}

// =====================
// Programmatic skill creation
// =====================

fn programmatic() -> Result<(), strands_agents::Error> {
    // --8<-- [start:programmatic]
    use strands_agents::plugins::Skill;

    // Create directly
    let skill = Skill::new(
        "code-review",
        "Review code for best practices and bugs",
        "Review the provided code. Check for...",
    );

    // Parse from SKILL.md content (`strict` turns validation warnings into errors)
    let parsed = Skill::from_content(
        "---\n\
         name: code-review\n\
         description: Review code for best practices\n\
         ---\n\
         Review the provided code. Check for...\n",
        false,
        None,
    )?;

    // Load from a specific directory
    let loaded = Skill::from_file("./skills/code-review", false)?;

    // Load all skills from a parent directory
    let skills = Skill::from_directory("./skills/", false)?;
    // --8<-- [end:programmatic]

    let _ = (skill, parsed, loaded, skills);
    Ok(())
}

// =====================
// Managing skills at runtime
// =====================

fn runtime() -> Result<(), strands_agents::Error> {
    // --8<-- [start:runtime]
    use strands_agents::plugins::{AgentSkills, Skill, SkillSource};
    use strands_agents::testing::MockMessageModel;
    use strands_agents::Agent;

    let plugin = AgentSkills::new(["./skills/pdf-processing"]);
    let agent = Agent::builder()
        .model(MockMessageModel::shared(vec![]))
        .plugin(plugin.clone())
        .build()?;

    // View available skills
    let available = plugin.available_skills();
    for skill in &available {
        println!("{}: {}", skill.name, skill.description);
    }

    // Add a new skill at runtime
    let new_skill = Skill::new(
        "summarize",
        "Summarize long documents",
        "Read the document and produce a concise summary...",
    );
    let mut sources: Vec<SkillSource> =
        available.into_iter().map(SkillSource::from).collect();
    sources.push(new_skill.into());
    plugin.set_available_skills(sources);

    // Replace all skills
    plugin.set_available_skills(["./skills/new-set/"]);

    // Check which skills the agent has activated
    let activated = plugin.activated_skills(&agent);
    println!("Activated skills: {activated:?}");
    // --8<-- [end:runtime]

    Ok(())
}
