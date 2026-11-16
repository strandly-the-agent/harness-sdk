use strands::Agent;
use strands::testing::MockMessageModel;
use strands::types::messages::TextBlock;
use strands::vended_plugins::skills::{AgentSkills, AgentSkillsConfig, SkillSource};

fn skill(root: &std::path::Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\nBody.\n")).unwrap();
}

fn agent(plugin: &AgentSkills) -> Agent {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("ok").into()]);
    model.add_turn(vec![TextBlock::new("ok").into()]);
    Agent::builder().model(model).printer(false).plugin(plugin.clone()).build().unwrap()
}

#[tokio::test]
async fn two_default_id_agents_do_not_share_skills() {
    let root = std::env::temp_dir().join(format!("reviewsdk_correctness_skills_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    skill(&root, "one");
    let source: SkillSource = root.to_str().unwrap().into();
    let plugin = AgentSkills::new(AgentSkillsConfig::builder().skills(vec![source]).build());
    let (mut a, mut b) = (agent(&plugin), agent(&plugin));
    a.invoke("hi").await.unwrap();
    let before = format!("{:?}", a.system_prompt());
    assert!(before.contains("one") && !before.contains("<name>two"), "{before}");
    skill(&root, "two");
    b.invoke("hi").await.unwrap();
    a.invoke("hi").await.unwrap();
    let after = format!("{:?}", a.system_prompt());
    let _ = std::fs::remove_dir_all(&root);
    assert!(!after.contains("two"), "agent A picked up the skills agent B loaded: {after}");
}
