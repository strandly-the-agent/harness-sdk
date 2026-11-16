//! `AgentSkills` through the public API of a running agent. The unit tests beside the plugin
//! port strands-ts/src/vended-plugins/skills/__tests__/agent-skills.test.node.ts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

use std::sync::Arc;

use serde_json::json;
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, SystemPrompt, TextBlock, ToolResultContent, ToolUseBlock};
use strands::vended_plugins::skills::{AgentSkills, AgentSkillsConfig, Skill, SkillConfig};

#[tokio::test]
async fn lists_the_skills_in_the_system_prompt_and_loads_one_through_the_tool() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("pdf-processing");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: pdf-processing\ndescription: Extract text from PDF files\nallowed-tools: shell\n---\n# Steps\nRun the script.",
    )
    .unwrap();
    std::fs::write(dir.join("scripts/extract.sh"), "#!/bin/sh").unwrap();
    let direct = Skill::new(
        SkillConfig::builder()
            .name("review")
            .description("Review a change")
            .instructions("Read the diff.")
            .build(),
    );
    let plugin = AgentSkills::new(
        AgentSkillsConfig::builder()
            .skills(vec![direct.into(), root.path().to_str().unwrap().into()])
            .build(),
    );
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![
        ToolUseBlock::new("skills", "tool-1", json!({ "skill_name": "pdf-processing" })).into(),
    ]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    model.add_turn(vec![TextBlock::new("welcome").into()]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .printer(false)
        .system_prompt("You are helpful.")
        .plugin(plugin.clone())
        .build()
        .unwrap();

    agent.invoke("extract the text").await.unwrap();
    agent.invoke("thanks").await.unwrap();

    let location = format!("{}/SKILL.md", dir.display());
    let prompt = format!(
        "You are helpful.\n\n<available_skills>\n<skill>\n<name>review</name>\n<description>Review a change</description>\n</skill>\n<skill>\n<name>pdf-processing</name>\n<description>Extract text from PDF files</description>\n<location>{location}</location>\n</skill>\n</available_skills>"
    );
    let requests = model.requests();
    assert_eq!(
        requests[0].system_prompt,
        Some(SystemPrompt::Text(prompt.clone()))
    );
    // The second invocation replaces the block rather than appending another.
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::Text(prompt)));
    let Some(ContentBlock::ToolResult(result)) = agent.messages()[2].content().first() else {
        panic!("expected a tool result");
    };
    let [ToolResultContent::Text(text)] = result.content() else {
        panic!("expected one text block");
    };
    assert_eq!(
        text.text(),
        format!(
            "# Steps\nRun the script.\n\n---\nAllowed tools: shell\nLocation: {location}\n\nAvailable resources:\n  scripts/extract.sh"
        )
    );
    assert_eq!(plugin.get_activated_skills(agent.app_state()), ["pdf-processing"]);
    let available = plugin.get_available_skills(Some(agent.id())).await;
    let names: Vec<_> = available.iter().map(Skill::name).collect();
    assert_eq!(names, ["review", "pdf-processing"]);
}
