//! Ported from strands-ts/src/vended-plugins/skills/__tests__/agent-skills.test.node.ts.

use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;

use super::super::skill::{FetchReply, SkillConfig, skill_fetch};
use super::*;
use crate::test_support::WarnRecorder;
use crate::types::agent::InvocationState;
use crate::types::messages::{CachePointBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;
use crate::vended_tools::test_support::{agent_with, context_for, local, stream_result};

fn create_skill_dir(root: &TempDir, name: &str, content: &str, extra_files: &[(&str, &str)]) -> PathBuf {
    let dir = root.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), content).unwrap();
    for (file, file_content) in extra_files {
        let full = dir.join(file);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, file_content).unwrap();
    }
    dir
}

fn make_skill_with(name: &str, description: &str) -> Skill {
    Skill::new(
        SkillConfig::builder()
            .name(name)
            .description(description)
            .instructions(format!("Instructions for {name}"))
            .build(),
    )
}

fn make_skill(name: &str) -> Skill {
    make_skill_with(name, &format!("Description of {name}"))
}

fn plugin_of(skills: Vec<SkillSource>) -> AgentSkills {
    AgentSkills::new(AgentSkillsConfig::builder().skills(skills).build())
}

fn path_source(path: &Path) -> SkillSource {
    path.to_str().unwrap().into()
}

fn mock_agent() -> LocalAgent {
    agent_with(Some(local()))
}

async fn init(plugin: &AgentSkills, agent: &mut LocalAgent) {
    Arc::new(plugin.clone()).init_agent(agent).await.unwrap();
}

async fn fire_before_invocation(agent: &mut LocalAgent) {
    let hooks = agent.hooks().clone();
    let mut event = BeforeInvocationEvent::new(agent, InvocationState::new(), Vec::new());
    hooks
        .invoke::<BeforeInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
}

fn prompt_text(agent: &LocalAgent) -> String {
    match agent.system_prompt() {
        Some(SystemPrompt::Text(text)) => text.clone(),
        other => panic!("expected a string system prompt, got {other:?}"),
    }
}

fn prompt_blocks(agent: &LocalAgent) -> Vec<SystemContentBlock> {
    match agent.system_prompt() {
        Some(SystemPrompt::Blocks(blocks)) => blocks.clone(),
        other => panic!("expected system prompt blocks, got {other:?}"),
    }
}

fn names(skills: &[Skill]) -> Vec<&str> {
    skills.iter().map(Skill::name).collect()
}

async fn invoke_tool_result(
    plugin: &AgentSkills,
    agent: &LocalAgent,
    skill_name: &str,
) -> (ToolResultStatus, String) {
    let tools = plugin.get_tools();
    let context = context_for(agent, "skills", json!({ "skill_name": skill_name }));
    let result = stream_result(tools[0].as_ref(), context).await;
    let text = result
        .content()
        .iter()
        .map(|content| match content {
            ToolResultContent::Text(text) => text.text(),
            _ => "",
        })
        .collect();
    (result.status().clone(), text)
}

async fn invoke_tool(plugin: &AgentSkills, agent: &LocalAgent, skill_name: &str) -> String {
    invoke_tool_result(plugin, agent, skill_name).await.1
}

mod constructor {
    use super::*;

    #[tokio::test]
    async fn resolves_skill_instances_directly() {
        let plugin = plugin_of(vec![make_skill("my-skill").into()]);
        assert_eq!(names(&plugin.get_available_skills(None).await), ["my-skill"]);
    }

    #[tokio::test]
    async fn resolves_a_skill_directory_path() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "my-skill",
            "---\nname: my-skill\ndescription: A skill\n---\nBody.",
            &[],
        );
        let plugin = plugin_of(vec![path_source(&dir)]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(plugin.get_available_skills(Some(agent.id())).await.len(), 1);
    }

    #[tokio::test]
    async fn resolves_a_parent_directory_with_multiple_skills() {
        let root = TempDir::new().unwrap();
        create_skill_dir(
            &root,
            "skill-a",
            "---\nname: skill-a\ndescription: Skill A\n---\nA.",
            &[],
        );
        create_skill_dir(
            &root,
            "skill-b",
            "---\nname: skill-b\ndescription: Skill B\n---\nB.",
            &[],
        );
        let plugin = plugin_of(vec![path_source(root.path())]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(
            names(&plugin.get_available_skills(Some(agent.id())).await),
            ["skill-a", "skill-b"]
        );
    }

    #[tokio::test]
    async fn handles_mixed_sources() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "file-skill",
            "---\nname: file-skill\ndescription: From file\n---\nBody.",
            &[],
        );
        let plugin = plugin_of(vec![make_skill("direct-skill").into(), path_source(&dir)]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(
            names(&plugin.get_available_skills(Some(agent.id())).await),
            ["direct-skill", "file-skill"]
        );
    }

    #[test]
    fn warns_on_duplicate_names_and_keeps_the_last() {
        let (plugin, warnings) = WarnRecorder::capture(|| {
            plugin_of(vec![
                make_skill_with("dup", "First").into(),
                make_skill_with("dup", "Second").into(),
            ])
        });
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let skills = runtime.block_on(plugin.get_available_skills(None));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description(), "Second");
        assert_eq!(
            warnings,
            ["name=<dup> | duplicate skill name, overwriting previous skill"]
        );
    }

    #[tokio::test]
    async fn warns_and_skips_non_existent_paths() {
        let plugin = plugin_of(vec!["/does/not/exist".into()]);
        assert_eq!(plugin.get_available_skills(None).await.len(), 0);
    }

    // No TS case: the TS spec has no case for the `skill source does not exist` warning.
    #[test]
    fn warns_about_a_non_existent_path_when_the_agent_initializes() {
        let plugin = plugin_of(vec!["/does/not/exist".into()]);
        let mut agent = mock_agent();
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let ((), warnings) = WarnRecorder::capture(|| runtime.block_on(init(&plugin, &mut agent)));
        assert_eq!(
            warnings,
            [
                "path=</does/not/exist> | skill source does not exist or is not a valid path",
                "no skills were loaded, the agent will have no skills available",
            ]
        );
    }

    #[tokio::test]
    async fn gracefully_handles_a_path_with_malformed_skill_md() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(&root, "bad-skill", "totally broken, no frontmatter at all", &[]);
        let plugin = plugin_of(vec![path_source(&dir)]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(plugin.get_available_skills(None).await.len(), 0);
        assert_eq!(plugin.get_available_skills(Some(agent.id())).await.len(), 0);
    }

    #[tokio::test]
    async fn loads_valid_skills_from_a_parent_dir_containing_malformed_siblings() {
        let root = TempDir::new().unwrap();
        create_skill_dir(
            &root,
            "good-skill",
            "---\nname: good-skill\ndescription: Works\n---\nBody.",
            &[],
        );
        create_skill_dir(&root, "bad-skill", "no frontmatter", &[]);
        let plugin = plugin_of(vec![path_source(root.path())]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(
            names(&plugin.get_available_skills(Some(agent.id())).await),
            ["good-skill"]
        );
    }
}

mod plugin_interface {
    use super::*;

    #[test]
    fn has_the_correct_name() {
        assert_eq!(
            plugin_of(vec![make_skill("s").into()]).name(),
            "strands:agent-skills"
        );
    }

    #[test]
    fn returns_one_tool_named_skills_from_get_tools() {
        let tools = plugin_of(vec![make_skill("s").into()]).get_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "skills");
        assert_eq!(
            serde_json::to_value(tools[0].tool_spec()).unwrap(),
            json!({
                "name": "skills",
                "description": "Activate a skill to load its full instructions. Use this tool to load the complete instructions for a skill listed in the available_skills section of your system prompt.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "skill_name": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Name of the skill to activate",
                        },
                    },
                    "required": ["skill_name"],
                    "additionalProperties": false,
                },
            })
        );
    }

    #[tokio::test]
    async fn registers_a_before_invocation_event_hook_in_init_agent() {
        let plugin = plugin_of(vec![make_skill("s").into()]);
        let mut agent = mock_agent();
        assert!(!agent.hooks().has_callbacks::<BeforeInvocationEvent<'static>>());
        init(&plugin, &mut agent).await;
        assert!(agent.hooks().has_callbacks::<BeforeInvocationEvent<'static>>());
    }
}

mod system_prompt_injection {
    use super::*;

    async fn setup() -> (AgentSkills, LocalAgent) {
        let plugin = plugin_of(vec![make_skill_with("pdf-skill", "Process PDFs").into()]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        (plugin, agent)
    }

    const PDF_XML: &str = "<available_skills>\n<skill>\n<name>pdf-skill</name>\n<description>Process PDFs</description>\n</skill>\n</available_skills>";

    #[tokio::test]
    async fn injects_into_undefined_system_prompt() {
        let (_plugin, mut agent) = setup().await;
        agent.set_system_prompt(None);
        fire_before_invocation(&mut agent).await;
        assert_eq!(prompt_text(&agent), PDF_XML);
    }

    #[tokio::test]
    async fn injects_into_string_system_prompt() {
        let (_plugin, mut agent) = setup().await;
        agent.set_system_prompt(Some(SystemPrompt::Text(
            "You are a helpful assistant.".to_owned(),
        )));
        fire_before_invocation(&mut agent).await;
        assert_eq!(
            prompt_text(&agent),
            format!("You are a helpful assistant.\n\n{PDF_XML}")
        );
    }

    #[tokio::test]
    async fn injects_into_system_content_block_prompt() {
        let (_plugin, mut agent) = setup().await;
        let original = vec![
            SystemContentBlock::Text(TextBlock::new("You are helpful.")),
            SystemContentBlock::CachePoint(CachePointBlock::new()),
        ];
        agent.set_system_prompt(Some(SystemPrompt::Blocks(original.clone())));
        fire_before_invocation(&mut agent).await;
        let blocks = prompt_blocks(&agent);
        assert_eq!(blocks.len(), 3);
        // Original blocks preserved
        assert_eq!(blocks[..2], original[..]);
        // New skills block appended
        assert_eq!(blocks[2], SystemContentBlock::Text(TextBlock::new(PDF_XML)));
    }

    #[tokio::test]
    async fn is_idempotent_re_injection_replaces_previous_block() {
        let (_plugin, mut agent) = setup().await;
        agent.set_system_prompt(Some(SystemPrompt::Text("Base prompt.".to_owned())));
        fire_before_invocation(&mut agent).await;
        let first = prompt_text(&agent);
        assert_eq!(first.matches("<available_skills>").count(), 1);
        // Fire again
        fire_before_invocation(&mut agent).await;
        let second = prompt_text(&agent);
        assert_eq!(second.matches("<available_skills>").count(), 1);
        assert_eq!(second, format!("Base prompt.\n\n{PDF_XML}"));
    }

    #[tokio::test]
    async fn is_idempotent_with_system_content_block_prompt() {
        let (_plugin, mut agent) = setup().await;
        let base = SystemContentBlock::Text(TextBlock::new("Base."));
        agent.set_system_prompt(Some(SystemPrompt::Blocks(vec![base.clone()])));
        fire_before_invocation(&mut agent).await;
        fire_before_invocation(&mut agent).await;
        // Original block + one skills block (not two)
        assert_eq!(
            prompt_blocks(&agent),
            [base, SystemContentBlock::Text(TextBlock::new(PDF_XML))]
        );
    }

    #[tokio::test]
    async fn preserves_external_modifications_to_system_prompt() {
        let (_plugin, mut agent) = setup().await;
        agent.set_system_prompt(Some(SystemPrompt::Text("Original.".to_owned())));
        fire_before_invocation(&mut agent).await;
        // Simulate external modification
        let modified = prompt_text(&agent).replace("Original.", "Modified.");
        agent.set_system_prompt(Some(SystemPrompt::Text(modified)));
        fire_before_invocation(&mut agent).await;
        assert_eq!(prompt_text(&agent), format!("Modified.\n\n{PDF_XML}"));
    }

    #[tokio::test]
    async fn xml_escapes_special_characters_in_skill_metadata() {
        let plugin = plugin_of(vec![
            make_skill_with("test-skill", "Use when: user says <hello> & \"goodbye\"").into(),
        ]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        fire_before_invocation(&mut agent).await;
        assert_eq!(
            prompt_text(&agent),
            "<available_skills>\n<skill>\n<name>test-skill</name>\n<description>Use when: user says &lt;hello&gt; &amp; &quot;goodbye&quot;</description>\n</skill>\n</available_skills>"
        );
    }

    #[tokio::test]
    async fn includes_skill_location_when_path_is_set() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "located-skill",
            "---\nname: located-skill\ndescription: Has a path\n---\nBody.",
            &[],
        );
        let plugin = plugin_of(vec![path_source(&dir)]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        fire_before_invocation(&mut agent).await;
        let location = format!("<location>{}/SKILL.md</location>", dir.display());
        assert!(prompt_text(&agent).contains(&location), "{}", prompt_text(&agent));
    }

    #[tokio::test]
    async fn shows_no_skills_available_when_empty() {
        let plugin = plugin_of(Vec::new());
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        fire_before_invocation(&mut agent).await;
        assert_eq!(
            prompt_text(&agent),
            "<available_skills>\nNo skills are currently available.\n</available_skills>"
        );
    }

    /// TS `null`; Rust has one absent value.
    #[tokio::test]
    async fn injects_into_null_system_prompt() {
        let (_plugin, mut agent) = setup().await;
        agent.set_system_prompt(None);
        fire_before_invocation(&mut agent).await;
        fire_before_invocation(&mut agent).await;
        assert_eq!(prompt_text(&agent), PDF_XML);
    }

    #[tokio::test]
    async fn reflects_updated_skills_after_set_available_skills() {
        let (plugin, mut agent) = setup().await;
        agent.set_system_prompt(Some(SystemPrompt::Text("Base.".to_owned())));
        fire_before_invocation(&mut agent).await;
        assert!(prompt_text(&agent).contains("pdf-skill"));
        plugin.set_available_skills(vec![make_skill_with("new-skill", "A new skill").into()]);
        fire_before_invocation(&mut agent).await;
        let prompt = prompt_text(&agent);
        assert!(prompt.contains("new-skill"));
        assert!(!prompt.contains("pdf-skill"));
        assert!(prompt.starts_with("Base.\n\n<available_skills>"));
    }

    #[tokio::test]
    async fn lists_all_skills_when_multiple_are_available() {
        let plugin = plugin_of(vec![
            make_skill_with("skill-a", "First").into(),
            make_skill_with("skill-b", "Second").into(),
            make_skill_with("skill-c", "Third").into(),
        ]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        fire_before_invocation(&mut agent).await;
        assert_eq!(
            prompt_text(&agent),
            "<available_skills>\n<skill>\n<name>skill-a</name>\n<description>First</description>\n</skill>\n<skill>\n<name>skill-b</name>\n<description>Second</description>\n</skill>\n<skill>\n<name>skill-c</name>\n<description>Third</description>\n</skill>\n</available_skills>"
        );
    }
}

mod tool_callback {
    use super::*;

    async fn setup() -> (AgentSkills, LocalAgent) {
        let skill = Skill::new(
            SkillConfig::builder()
                .name("test-skill")
                .description("A test skill")
                .instructions("# Test\nDo the thing.")
                .allowed_tools(vec!["bash".to_owned()])
                .compatibility("v1.0+")
                .build(),
        );
        let plugin = plugin_of(vec![skill.into()]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        (plugin, agent)
    }

    #[tokio::test]
    async fn returns_instructions_for_a_valid_skill() {
        let (plugin, agent) = setup().await;
        let result = invoke_tool(&plugin, &agent, "test-skill").await;
        assert!(result.starts_with("# Test\nDo the thing."), "{result}");
    }

    #[tokio::test]
    async fn includes_metadata_in_the_response() {
        let (plugin, agent) = setup().await;
        assert_eq!(
            invoke_tool(&plugin, &agent, "test-skill").await,
            "# Test\nDo the thing.\n\n---\nAllowed tools: bash\nCompatibility: v1.0+"
        );
    }

    #[tokio::test]
    async fn returns_error_for_unknown_skill() {
        let (plugin, agent) = setup().await;
        assert_eq!(
            invoke_tool(&plugin, &agent, "nonexistent").await,
            "Skill 'nonexistent' not found. Available skills: test-skill"
        );
    }

    #[tokio::test]
    async fn tracks_activated_skills_in_app_state() {
        let (plugin, agent) = setup().await;
        invoke_tool(&plugin, &agent, "test-skill").await;
        assert_eq!(plugin.get_activated_skills(agent.app_state()), ["test-skill"]);
        assert_eq!(
            agent.app_state().get_value("agent_skills"),
            Some(json!({ "activatedSkills": ["test-skill"] }))
        );
    }

    #[tokio::test]
    async fn maintains_activation_order_without_duplicates() {
        let (plugin, agent) = setup().await;
        // Add a second skill
        plugin.set_available_skills(vec![make_skill("skill-a").into(), make_skill("skill-b").into()]);
        invoke_tool(&plugin, &agent, "skill-a").await;
        invoke_tool(&plugin, &agent, "skill-b").await;
        invoke_tool(&plugin, &agent, "skill-a").await; // re-activate
        assert_eq!(
            plugin.get_activated_skills(agent.app_state()),
            ["skill-b", "skill-a"]
        );
    }

    #[tokio::test]
    async fn handles_skill_with_no_instructions() {
        let (plugin, agent) = setup().await;
        let empty = SkillConfig::builder()
            .name("empty")
            .description("No instructions")
            .build();
        plugin.set_available_skills(vec![Skill::new(empty).into()]);
        assert_eq!(
            invoke_tool(&plugin, &agent, "empty").await,
            "Skill 'empty' activated (no instructions available)."
        );
    }

    /// TS reports zod's `too_small` issue; the crate's validator reports the schema violation.
    #[tokio::test]
    async fn returns_validation_error_for_empty_skill_name() {
        let (plugin, agent) = setup().await;
        let (status, text) = invoke_tool_result(&plugin, &agent, "").await;
        assert_eq!(status, ToolResultStatus::Error);
        assert!(text.contains("Invalid input at /skill_name"), "{text}");
        assert!(plugin.get_activated_skills(agent.app_state()).is_empty());
    }

    // No TS case: the TS spec has no case for the `expected object for state key` error.
    #[tokio::test]
    async fn fails_when_the_state_key_holds_something_else() {
        let (plugin, agent) = setup().await;
        agent.app_state().set("agent_skills", json!("text")).unwrap();
        let (status, text) = invoke_tool_result(&plugin, &agent, "test-skill").await;
        assert_eq!(status, ToolResultStatus::Error);
        assert!(
            text.contains("expected object for state key 'agent_skills', got string"),
            "{text}"
        );
    }
}

mod resource_listing {
    use super::*;

    #[tokio::test]
    async fn lists_files_from_scripts_references_assets() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "resource-skill",
            "---\nname: resource-skill\ndescription: Has resources\n---\nBody.",
            &[
                ("scripts/setup.sh", "#!/bin/bash"),
                ("references/api.md", "# API Docs"),
                ("assets/logo.png", "binary"),
            ],
        );
        let plugin = plugin_of(vec![path_source(&dir)]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(
            invoke_tool(&plugin, &agent, "resource-skill").await,
            format!(
                "Body.\n\n---\nLocation: {}/SKILL.md\n\nAvailable resources:\n  scripts/setup.sh\n  references/api.md\n  assets/logo.png",
                dir.display()
            )
        );
    }

    #[tokio::test]
    async fn handles_missing_resource_directories_gracefully() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "no-resources",
            "---\nname: no-resources\ndescription: No resources\n---\nBody.",
            &[],
        );
        let plugin = plugin_of(vec![path_source(&dir)]);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        let text = invoke_tool(&plugin, &agent, "no-resources").await;
        assert!(!text.contains("Available resources"), "{text}");
    }

    #[tokio::test]
    async fn truncates_at_max_resource_files() {
        let root = TempDir::new().unwrap();
        // Create more files than the limit
        let files: Vec<_> = (0..5)
            .map(|index| (format!("scripts/file{index}.sh"), format!("script {index}")))
            .collect();
        let files: Vec<_> = files
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_str()))
            .collect();
        let dir = create_skill_dir(
            &root,
            "many-files",
            "---\nname: many-files\ndescription: Many resources\n---\nBody.",
            &files,
        );
        let config = AgentSkillsConfig::builder()
            .skills(vec![path_source(&dir)])
            .max_resource_files(3)
            .build();
        let plugin = AgentSkills::new(config);
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        let text = invoke_tool(&plugin, &agent, "many-files").await;
        assert!(
            text.ends_with(
                "\nAvailable resources:\n  scripts/file0.sh\n  scripts/file1.sh\n  scripts/file2.sh\n  ... (truncated at 3 files)"
            ),
            "{text}"
        );
    }
}

mod set_available_skills {
    use super::*;

    #[tokio::test]
    async fn replaces_all_skills() {
        let plugin = plugin_of(vec![make_skill("original").into()]);
        assert_eq!(plugin.get_available_skills(None).await.len(), 1);
        plugin.set_available_skills(vec![make_skill("new-a").into(), make_skill("new-b").into()]);
        assert_eq!(
            names(&plugin.get_available_skills(None).await),
            ["new-a", "new-b"]
        );
    }
}

mod url_skill_resolution {
    use super::*;

    const SAMPLE_CONTENT: &str = "---\nname: url-skill\ndescription: A URL skill\n---\n# Instructions\n";

    fn fetch_success() -> SkillFetch {
        skill_fetch(|_url| async {
            Ok(FetchReply {
                ok: true,
                status: 200,
                status_text: "OK".to_owned(),
                body: SAMPLE_CONTENT.to_owned(),
            })
        })
    }

    fn url_plugin(skills: Vec<SkillSource>, fetch: SkillFetch) -> AgentSkills {
        AgentSkills::with_fetch(AgentSkillsConfig::builder().skills(skills).build(), fetch)
    }

    // No TS case: a JS promise can't be dropped mid-fetch; a Rust future can.
    #[tokio::test]
    async fn a_fetch_dropped_midway_is_retried_by_the_next_call() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fetch = skill_fetch(move |_url| {
            let first = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            async move {
                if first {
                    std::future::pending::<()>().await;
                }
                Ok(FetchReply {
                    ok: true,
                    status: 200,
                    status_text: "OK".to_owned(),
                    body: SAMPLE_CONTENT.to_owned(),
                })
            }
        });
        let plugin = url_plugin(vec!["https://example.com/SKILL.md".into()], fetch);
        {
            let mut dropped = std::pin::pin!(plugin.get_available_skills(None));
            assert!(futures::poll!(dropped.as_mut()).is_pending());
        }
        assert_eq!(names(&plugin.get_available_skills(None).await), ["url-skill"]);
    }

    // No TS case: TS inserts fetch results into the map of the `_resolveSkills` call that started
    // them; this pins the same outcome for the shared Rust state.
    #[tokio::test]
    async fn a_set_replaced_during_a_fetch_does_not_receive_the_old_sets_url_skills() {
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let fetch = {
            let gate = gate.clone();
            skill_fetch(move |_url| {
                let gate = gate.clone();
                async move {
                    gate.acquire().await.unwrap().forget();
                    Ok(FetchReply {
                        ok: true,
                        status: 200,
                        status_text: "OK".to_owned(),
                        body: SAMPLE_CONTENT.to_owned(),
                    })
                }
            })
        };
        let plugin = url_plugin(vec!["https://example.com/SKILL.md".into()], fetch);
        let mut first = std::pin::pin!(plugin.get_available_skills(None));
        assert!(futures::poll!(first.as_mut()).is_pending());
        plugin.set_available_skills(Vec::new());
        gate.add_permits(1);
        assert!(first.await.is_empty());
    }

    #[tokio::test]
    async fn resolves_a_url_string_as_a_skill_source() {
        let plugin = url_plugin(vec!["https://example.com/SKILL.md".into()], fetch_success());
        init(&plugin, &mut mock_agent()).await;
        assert_eq!(names(&plugin.get_available_skills(None).await), ["url-skill"]);
    }

    #[tokio::test]
    async fn resolves_a_mix_of_url_and_local_filesystem_sources() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "local-skill",
            "---\nname: local-skill\ndescription: A local skill\n---\nBody.",
            &[],
        );
        let plugin = url_plugin(
            vec!["https://example.com/SKILL.md".into(), path_source(&dir)],
            fetch_success(),
        );
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(
            names(&plugin.get_available_skills(Some(agent.id())).await),
            ["url-skill", "local-skill"]
        );
    }

    #[test]
    fn skips_a_failed_url_fetch_gracefully() {
        let fetch = skill_fetch(|_url| async {
            Ok(FetchReply {
                ok: false,
                status: 404,
                status_text: "Not Found".to_owned(),
                body: String::new(),
            })
        });
        let plugin = url_plugin(vec!["https://example.com/broken/SKILL.md".into()], fetch);
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (skills, warnings) = WarnRecorder::capture(|| {
            runtime.block_on(async {
                init(&plugin, &mut mock_agent()).await;
                plugin.get_available_skills(None).await
            })
        });
        assert_eq!(skills.len(), 0);
        assert_eq!(
            warnings,
            [
                "url=<https://example.com/broken/SKILL.md> | failed to load skill from URL: Error: url=<https://example.com/broken/SKILL.md> | HTTP 404: Not Found",
                "no skills were loaded, the agent will have no skills available",
            ]
        );
    }

    #[test]
    fn warns_on_duplicate_skill_names_from_urls() {
        let plugin = url_plugin(
            vec![
                "https://example.com/a/SKILL.md".into(),
                "https://example.com/b/SKILL.md".into(),
            ],
            fetch_success(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (skills, warnings) = WarnRecorder::capture(|| {
            runtime.block_on(async {
                init(&plugin, &mut mock_agent()).await;
                plugin.get_available_skills(None).await
            })
        });
        assert_eq!(skills.len(), 1);
        assert_eq!(
            warnings,
            ["name=<url-skill> | duplicate skill name, overwriting previous skill"]
        );
    }

    #[tokio::test]
    async fn awaits_url_sources_in_init_agent() {
        let plugin = url_plugin(vec!["https://example.com/SKILL.md".into()], fetch_success());
        let mut agent = mock_agent();
        init(&plugin, &mut agent).await;
        assert_eq!(plugin.get_available_skills(None).await.len(), 1);
        assert!(agent.hooks().has_callbacks::<BeforeInvocationEvent<'static>>());
    }
}

/// Rust-only cases: the per-agent skills are keyed by the agent's identity, as the TS `WeakMap`.
mod rust_only {
    use super::*;

    fn skill_md(name: &str) -> String {
        format!("---\nname: {name}\ndescription: A skill\n---\nBody.")
    }

    #[tokio::test]
    async fn agents_with_the_same_id_sharing_a_plugin_keep_their_own_skills() {
        let root = TempDir::new().unwrap();
        create_skill_dir(&root, "one", &skill_md("one"), &[]);
        let plugin = plugin_of(vec![path_source(root.path())]);
        let (mut first, mut second) = (mock_agent(), mock_agent());
        assert_eq!(first.id(), second.id());
        init(&plugin, &mut first).await;
        create_skill_dir(&root, "two", &skill_md("two"), &[]);
        init(&plugin, &mut second).await;

        fire_before_invocation(&mut first).await;
        fire_before_invocation(&mut second).await;

        assert!(prompt_text(&first).contains("<name>one</name>"));
        assert!(!prompt_text(&first).contains("<name>two</name>"));
        assert!(prompt_text(&second).contains("<name>two</name>"));
        let (_, text) = invoke_tool_result(&plugin, &first, "two").await;
        assert_eq!(text, "Skill 'two' not found. Available skills: one");
    }

    #[tokio::test]
    async fn the_skills_of_a_dropped_agent_are_pruned() {
        let plugin = plugin_of(vec![make_skill("a").into()]);
        let mut first = mock_agent();
        init(&plugin, &mut first).await;
        let id = first.id().to_owned();
        drop(first);
        plugin.set_available_skills(vec![make_skill("b").into()]);
        assert_eq!(
            names(&plugin.get_available_skills(Some(&id)).await),
            ["b"],
            "a dropped agent's entry is not read"
        );

        let (mut second, mut third) = (mock_agent(), mock_agent());
        second.set_id("second".to_owned());
        init(&plugin, &mut second).await;
        drop(second);
        init(&plugin, &mut third).await;
        assert_eq!(plugin.inner.lock().agent_skills.len(), 1);
    }
}
