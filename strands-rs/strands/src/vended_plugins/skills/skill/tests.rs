//! Ported from strands-ts/src/vended-plugins/skills/__tests__/skill.test.node.ts.

use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::test_support::WarnRecorder;

fn create_skill_dir_as(root: &TempDir, name: &str, content: &str, filename: &str) -> PathBuf {
    let dir = root.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(filename), content).unwrap();
    dir
}

fn create_skill_dir(root: &TempDir, name: &str, content: &str) -> PathBuf {
    create_skill_dir_as(root, name, content, "SKILL.md")
}

fn lenient() -> SkillOptions {
    SkillOptions::default()
}

fn strict() -> SkillOptions {
    SkillOptions::strict(true)
}

fn parse(content: &str) -> Skill {
    Skill::from_content(content, &lenient()).unwrap()
}

fn error_of<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().to_string()
}

fn path_text(path: &Path) -> &str {
    path.to_str().unwrap()
}

const VALID: &str = "---\nname: valid-skill\ndescription: Valid\n---\nBody.";

mod constructor {
    use super::*;

    #[test]
    fn creates_a_skill_with_required_fields() {
        let skill = Skill::new(SkillConfig {
            name: "test-skill".to_owned(),
            description: "A test skill".to_owned(),
            ..SkillConfig::default()
        });
        assert_eq!(skill.name(), "test-skill");
        assert_eq!(skill.description(), "A test skill");
        assert_eq!(skill.instructions(), "");
        assert_eq!(skill.path(), None);
        assert_eq!(skill.allowed_tools(), None);
        assert!(skill.metadata().is_empty());
        assert_eq!(skill.license(), None);
        assert_eq!(skill.compatibility(), None);
    }

    #[test]
    fn creates_a_skill_with_all_fields() {
        let metadata = json!({ "author": "test" }).as_object().unwrap().clone();
        let skill = Skill::new(
            SkillConfig::builder()
                .name("full-skill")
                .description("Full description")
                .instructions("# Instructions\nDo things")
                .path("/some/path")
                .allowed_tools(vec!["bash".to_owned(), "file-editor".to_owned()])
                .metadata(metadata.clone())
                .license("Apache-2.0")
                .compatibility("v1.0+")
                .build(),
        );
        assert_eq!(skill.name(), "full-skill");
        assert_eq!(skill.description(), "Full description");
        assert_eq!(skill.instructions(), "# Instructions\nDo things");
        assert_eq!(skill.path(), Some("/some/path"));
        assert_eq!(
            skill.allowed_tools(),
            Some(&["bash".to_owned(), "file-editor".to_owned()][..])
        );
        assert_eq!(skill.metadata(), &metadata);
        assert_eq!(skill.license(), Some("Apache-2.0"));
        assert_eq!(skill.compatibility(), Some("v1.0+"));
    }
}

mod from_content {
    use super::*;

    fn tools(skill: &Skill) -> Option<Vec<&str>> {
        skill
            .allowed_tools()
            .map(|tools| tools.iter().map(String::as_str).collect())
    }

    #[test]
    fn parses_valid_skill_md_content() {
        let skill = parse(
            "---\nname: my-skill\ndescription: Does something useful\n---\n# Instructions\nFollow these steps.",
        );
        assert_eq!(skill.name(), "my-skill");
        assert_eq!(skill.description(), "Does something useful");
        assert_eq!(skill.instructions(), "# Instructions\nFollow these steps.");
    }

    #[test]
    fn parses_content_with_allowed_tools_as_space_delimited_string() {
        let skill = parse(
            "---\nname: my-skill\ndescription: A skill\nallowed-tools: bash file-editor\n---\nInstructions here.",
        );
        assert_eq!(tools(&skill), Some(vec!["bash", "file-editor"]));
    }

    #[test]
    fn parses_content_with_allowed_tools_as_yaml_list() {
        let skill = parse(
            "---\nname: my-skill\ndescription: A skill\nallowed-tools:\n  - bash\n  - file-editor\n---\nInstructions here.",
        );
        assert_eq!(tools(&skill), Some(vec!["bash", "file-editor"]));
    }

    #[test]
    fn parses_content_with_allowed_tools_underscore_variant() {
        let skill = parse(
            "---\nname: my-skill\ndescription: A skill\nallowed_tools: bash notebook\n---\nInstructions here.",
        );
        assert_eq!(tools(&skill), Some(vec!["bash", "notebook"]));
    }

    // No TS case: TS delegates to the `yaml` package; the cases and expected values in
    // `frontmatter_cases.json` are the output of TS `Skill.fromContent` for block scalars,
    // nested mappings and double-quote escapes.
    #[test]
    fn block_scalars_nested_mappings_and_escapes_parse_as_the_ts_yaml_parser_does() {
        let cases: JsonObject = serde_json::from_str(include_str!("frontmatter_cases.json")).unwrap();
        for (name, case) in &cases {
            let skill = parse(case["content"].as_str().unwrap());
            assert_eq!(
                json!(skill.description()),
                case["description"],
                "{name}: description"
            );
            assert_eq!(
                json!(skill.allowed_tools()),
                case["allowedTools"],
                "{name}: allowedTools"
            );
            assert_eq!(json!(skill.metadata()), case["metadata"], "{name}: metadata");
            assert_eq!(json!(skill.license()), case["license"], "{name}: license");
        }
    }

    #[test]
    fn parses_content_with_metadata() {
        let skill = parse(
            "---\nname: my-skill\ndescription: A skill\nmetadata:\n  author: test-user\n  version: 1\n---\nBody.",
        );
        assert_eq!(
            JsonValue::Object(skill.metadata().clone()),
            json!({ "author": "test-user", "version": 1 })
        );
    }

    #[test]
    fn parses_content_with_license_and_compatibility() {
        let skill = parse(
            "---\nname: my-skill\ndescription: A skill\nlicense: MIT\ncompatibility: strands-agents >= 1.0\n---\nBody.",
        );
        assert_eq!(skill.license(), Some("MIT"));
        assert_eq!(skill.compatibility(), Some("strands-agents >= 1.0"));
    }

    #[test]
    fn throws_if_content_does_not_start_with() {
        assert_eq!(
            error_of(Skill::from_content("no frontmatter", &lenient())),
            "SKILL.md must start with --- frontmatter delimiter"
        );
    }

    #[test]
    fn throws_if_closing_is_missing() {
        assert_eq!(
            error_of(Skill::from_content("---\nname: test\n", &lenient())),
            "SKILL.md frontmatter missing closing --- delimiter"
        );
    }

    #[test]
    fn throws_if_name_is_missing() {
        assert_eq!(
            error_of(Skill::from_content(
                "---\ndescription: no name\n---\nBody.",
                &lenient()
            )),
            "SKILL.md content must have a 'name' field in frontmatter"
        );
    }

    #[test]
    fn throws_if_description_is_missing() {
        assert_eq!(
            error_of(Skill::from_content("---\nname: my-skill\n---\nBody.", &lenient())),
            "SKILL.md content must have a 'description' field in frontmatter"
        );
    }

    #[test]
    fn handles_empty_body() {
        let skill = parse("---\nname: my-skill\ndescription: A skill\n---");
        assert_eq!(skill.instructions(), "");
    }

    #[test]
    fn warns_but_does_not_throw_for_invalid_name_in_lenient_mode() {
        let (skill, warnings) =
            WarnRecorder::capture(|| parse("---\nname: INVALID_NAME\ndescription: A skill\n---\nBody."));
        assert_eq!(skill.name(), "INVALID_NAME");
        assert_eq!(
            warnings,
            [
                "name=<INVALID_NAME> | skill name should be 1-64 lowercase alphanumeric characters or hyphens, should not start/end with hyphen"
            ]
        );
    }

    #[test]
    fn throws_for_invalid_name_in_strict_mode() {
        let content = "---\nname: INVALID_NAME\ndescription: A skill\n---\nBody.";
        assert_eq!(
            error_of(Skill::from_content(content, &strict())),
            "name=<INVALID_NAME> | skill name should be 1-64 lowercase alphanumeric characters or hyphens, should not start/end with hyphen"
        );
    }

    #[test]
    fn throws_for_empty_name_in_strict_mode() {
        let content = "---\nname: \"\"\ndescription: A skill\n---\nBody.";
        assert_eq!(
            error_of(Skill::from_content(content, &lenient())),
            "SKILL.md content must have a 'name' field in frontmatter"
        );
    }

    #[test]
    fn throws_for_name_exceeding_length_limit_in_strict_mode() {
        let long_name = "a".repeat(65);
        let content = format!("---\nname: {long_name}\ndescription: A skill\n---\nBody.");
        assert_eq!(
            error_of(Skill::from_content(&content, &strict())),
            format!("name=<{long_name}> | skill name exceeds 64 character limit")
        );
    }

    #[test]
    fn throws_for_consecutive_hyphens_in_strict_mode() {
        let content = "---\nname: my--skill\ndescription: A skill\n---\nBody.";
        assert_eq!(
            error_of(Skill::from_content(content, &strict())),
            "name=<my--skill> | skill name contains consecutive hyphens"
        );
    }

    #[test]
    fn handles_body_containing_horizontal_rules() {
        let skill = parse(
            "---\nname: my-skill\ndescription: A skill\n---\n# Instructions\nFirst section.\n---\nSecond section after horizontal rule.\n---\nThird section.",
        );
        assert_eq!(skill.name(), "my-skill");
        assert_eq!(
            skill.instructions(),
            "# Instructions\nFirst section.\n---\nSecond section after horizontal rule.\n---\nThird section."
        );
    }

    #[test]
    fn handles_body_with_only_whitespace_after_frontmatter() {
        let skill = parse("---\nname: my-skill\ndescription: A skill\n---\n   ");
        assert_eq!(skill.name(), "my-skill");
        assert_eq!(skill.instructions(), "");
    }

    #[test]
    fn handles_frontmatter_value_containing_inline() {
        let skill = parse("---\nname: my-skill\ndescription: Use this --- for special cases\n---\nBody.");
        assert_eq!(skill.name(), "my-skill");
        assert_eq!(skill.description(), "Use this --- for special cases");
    }

    #[test]
    fn ignores_non_object_metadata() {
        let skill = parse("---\nname: my-skill\ndescription: A skill\nmetadata: just-a-string\n---\nBody.");
        assert!(skill.metadata().is_empty());
    }

    #[test]
    fn ignores_array_metadata() {
        let skill =
            parse("---\nname: my-skill\ndescription: A skill\nmetadata:\n  - item1\n  - item2\n---\nBody.");
        assert!(skill.metadata().is_empty());
    }

    #[test]
    fn handles_allowed_tools_as_empty_string() {
        let skill = parse("---\nname: my-skill\ndescription: A skill\nallowed-tools: \"\"\n---\nBody.");
        assert_eq!(skill.allowed_tools(), None);
    }

    #[test]
    fn filters_null_entries_from_allowed_tools_array() {
        let skill = parse(
            "---\nname: my-skill\ndescription: A skill\nallowed-tools:\n  - bash\n  - null\n  - file-editor\n---\nBody.",
        );
        assert_eq!(tools(&skill), Some(vec!["bash", "file-editor"]));
    }

    // No TS case: TS reaches this through its colon-quoting retry, which the Rust parser does not need.
    /// The colon-quoting fallback of TS: a value holding a colon needs no quotes.
    #[test]
    fn keeps_a_value_holding_a_colon() {
        let skill = parse("---\nname: my-skill\ndescription: Use when: the user asks\n---\nBody.");
        assert_eq!(skill.description(), "Use when: the user asks");
    }
}

mod from_file {
    use super::*;

    #[test]
    fn loads_a_skill_from_a_directory() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "my-skill",
            "---\nname: my-skill\ndescription: A test skill\n---\n# Instructions\nDo the thing.",
        );
        let skill = Skill::from_file(&dir, &lenient()).unwrap();
        assert_eq!(skill.name(), "my-skill");
        assert_eq!(skill.description(), "A test skill");
        assert_eq!(skill.instructions(), "# Instructions\nDo the thing.");
        assert_eq!(skill.path(), Some(path_text(&dir)));
    }

    #[test]
    fn loads_a_skill_from_a_skill_md_file_path() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "my-skill",
            "---\nname: my-skill\ndescription: A skill\n---\nBody.",
        );
        let skill = Skill::from_file(dir.join("SKILL.md"), &lenient()).unwrap();
        assert_eq!(skill.name(), "my-skill");
        assert_eq!(skill.path(), Some(path_text(&dir)));
    }

    #[test]
    fn finds_lowercase_skill_md_as_fallback() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir_as(
            &root,
            "my-skill",
            "---\nname: my-skill\ndescription: A skill\n---\nBody.",
            "skill.md",
        );
        assert_eq!(Skill::from_file(&dir, &lenient()).unwrap().name(), "my-skill");
    }

    #[test]
    fn throws_for_non_existent_path() {
        assert_eq!(
            error_of(Skill::from_file("/does/not/exist", &lenient())),
            "path=</does/not/exist> | skill path does not exist or is not a valid skill directory"
        );
    }

    #[test]
    fn warns_when_skill_name_does_not_match_directory_name() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "wrong-dir-name",
            "---\nname: actual-skill-name\ndescription: A skill\n---\nBody.",
        );
        let (skill, warnings) = WarnRecorder::capture(|| Skill::from_file(&dir, &lenient()).unwrap());
        assert_eq!(skill.name(), "actual-skill-name");
        assert_eq!(
            warnings,
            [
                "name=<actual-skill-name>, directory=<wrong-dir-name> | skill name does not match parent directory name"
            ]
        );
    }

    #[test]
    fn throws_when_skill_name_does_not_match_directory_in_strict_mode() {
        let root = TempDir::new().unwrap();
        let dir = create_skill_dir(
            &root,
            "wrong-dir-name",
            "---\nname: actual-skill-name\ndescription: A skill\n---\nBody.",
        );
        assert_eq!(
            error_of(Skill::from_file(&dir, &strict())),
            "name=<actual-skill-name>, directory=<wrong-dir-name> | skill name does not match parent directory name"
        );
    }
}

mod from_directory {
    use super::*;

    fn names(skills: &[Skill]) -> Vec<&str> {
        skills.iter().map(Skill::name).collect()
    }

    #[test]
    fn loads_all_skills_from_a_directory() {
        let root = TempDir::new().unwrap();
        create_skill_dir(
            &root,
            "skill-a",
            "---\nname: skill-a\ndescription: Skill A\n---\nInstructions A.",
        );
        create_skill_dir(
            &root,
            "skill-b",
            "---\nname: skill-b\ndescription: Skill B\n---\nInstructions B.",
        );
        let skills = Skill::from_directory(root.path(), &lenient()).unwrap();
        assert_eq!(names(&skills), ["skill-a", "skill-b"]);
    }

    #[test]
    fn skips_directories_without_skill_md() {
        let root = TempDir::new().unwrap();
        create_skill_dir(&root, "valid-skill", VALID);
        create_skill_dir_as(&root, "no-skill-md", "not a skill", "README.md");
        let skills = Skill::from_directory(root.path(), &lenient()).unwrap();
        assert_eq!(names(&skills), ["valid-skill"]);
    }

    #[test]
    fn skips_non_directory_children() {
        let root = TempDir::new().unwrap();
        create_skill_dir(&root, "valid-skill", VALID);
        std::fs::write(root.path().join("some-file.txt"), "not a directory").unwrap();
        let skills = Skill::from_directory(root.path(), &lenient()).unwrap();
        assert_eq!(skills.len(), 1);
    }

    #[test]
    fn skips_skills_with_invalid_content() {
        let root = TempDir::new().unwrap();
        create_skill_dir(&root, "valid-skill", VALID);
        create_skill_dir(&root, "bad-skill", "---\ndescription: Missing name\n---\nBody.");
        let skills = Skill::from_directory(root.path(), &lenient()).unwrap();
        assert_eq!(names(&skills), ["valid-skill"]);
    }

    #[test]
    fn throws_for_non_existent_directory() {
        assert_eq!(
            error_of(Skill::from_directory("/does/not/exist", &lenient())),
            "path=</does/not/exist> | skills directory does not exist"
        );
    }

    #[test]
    fn returns_empty_array_for_directory_with_no_skills() {
        let root = TempDir::new().unwrap();
        assert_eq!(Skill::from_directory(root.path(), &lenient()).unwrap(), []);
    }

    #[test]
    fn skips_skills_with_completely_broken_skill_md_no_frontmatter() {
        let root = TempDir::new().unwrap();
        create_skill_dir(&root, "valid-skill", VALID);
        let broken = create_skill_dir(&root, "broken-skill", "totally broken, no frontmatter at all");
        let (skills, warnings) =
            WarnRecorder::capture(|| Skill::from_directory(root.path(), &lenient()).unwrap());
        assert_eq!(names(&skills), ["valid-skill"]);
        assert_eq!(
            warnings,
            [format!(
                "path=<{}> | skipping skill due to error: Error: SKILL.md must start with --- frontmatter delimiter",
                broken.display()
            )]
        );
    }
}

mod from_url {
    use super::*;

    const SAMPLE_CONTENT: &str =
        "---\nname: my-skill\ndescription: A remote skill\n---\nRemote instructions.\n";

    fn fetch_success(content: &'static str) -> SkillFetch {
        skill_fetch(move |_url| async move {
            Ok(FetchReply {
                ok: true,
                status: 200,
                status_text: "OK".to_owned(),
                body: content.to_owned(),
            })
        })
    }

    #[tokio::test]
    async fn returns_a_skill_from_a_valid_url() {
        let url = "https://raw.githubusercontent.com/org/repo/main/SKILL.md";
        let skill = Skill::from_url_with(url, &lenient(), &fetch_success(SAMPLE_CONTENT))
            .await
            .unwrap();
        assert_eq!(skill.name(), "my-skill");
        assert_eq!(skill.description(), "A remote skill");
        assert_eq!(skill.instructions(), "Remote instructions.");
        assert_eq!(skill.path(), None);
    }

    #[tokio::test]
    async fn rejects_non_https_urls() {
        assert_eq!(
            error_of(Skill::from_url("./local-path", &lenient()).await),
            "url=<./local-path> | not a valid HTTPS URL"
        );
    }

    #[tokio::test]
    async fn rejects_http_urls() {
        assert_eq!(
            error_of(Skill::from_url("http://example.com/SKILL.md", &lenient()).await),
            "url=<http://example.com/SKILL.md> | not a valid HTTPS URL"
        );
    }

    #[tokio::test]
    async fn throws_on_http_error_responses() {
        let fetch = skill_fetch(|_url| async {
            Ok(FetchReply {
                ok: false,
                status: 404,
                status_text: "Not Found".to_owned(),
                body: String::new(),
            })
        });
        assert_eq!(
            error_of(Skill::from_url_with("https://example.com/SKILL.md", &lenient(), &fetch).await),
            "url=<https://example.com/SKILL.md> | HTTP 404: Not Found"
        );
    }

    #[tokio::test]
    async fn throws_on_network_errors() {
        let fetch = skill_fetch(|_url| async { Err("Connection refused".to_owned()) });
        assert_eq!(
            error_of(Skill::from_url_with("https://example.com/SKILL.md", &lenient(), &fetch).await),
            "url=<https://example.com/SKILL.md> | failed to fetch skill: Connection refused"
        );
    }

    #[tokio::test]
    async fn forwards_strict_mode_to_from_content() {
        let fetch = fetch_success("---\nname: BAD_NAME\ndescription: Bad\n---\nBody.");
        let error = error_of(Skill::from_url_with("https://example.com/SKILL.md", &strict(), &fetch).await);
        assert!(error.contains("skill name should be"), "{error}");
    }

    #[tokio::test]
    async fn throws_on_invalid_content_e_g_html_page() {
        let fetch = fetch_success("<html><body>Not a SKILL.md</body></html>");
        assert_eq!(
            error_of(Skill::from_url_with("https://example.com/SKILL.md", &lenient(), &fetch).await),
            "SKILL.md must start with --- frontmatter delimiter"
        );
    }
}

mod classmethods {
    use super::*;

    /// TS checks the static methods exist; here they are named, so their absence fails the build.
    #[test]
    fn has_from_file_from_content_from_directory_and_from_url() {
        let _from_content: fn(&str, &SkillOptions) -> Result<Skill> = Skill::from_content;
        let _from_file: fn(PathBuf, &SkillOptions) -> Result<Skill> = Skill::from_file;
        let _from_directory: fn(PathBuf, &SkillOptions) -> Result<Vec<Skill>> = Skill::from_directory;
        let _from_url = Skill::from_url;
    }
}
