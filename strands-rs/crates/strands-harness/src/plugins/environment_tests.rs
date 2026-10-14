use std::path::PathBuf;

use tempfile::TempDir;

use super::*;

async fn write_file(root: &std::path::Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.expect("mkdir");
    }
    tokio::fs::write(path, body).await.expect("write");
}

fn root_of(dir: &TempDir) -> Option<PathBuf> {
    Some(dir.path().to_path_buf())
}

#[tokio::test]
async fn injects_env_agents_md_and_nearby_links() {
    let dir = TempDir::new().expect("tempdir");
    write_file(dir.path(), "AGENTS.md", "root rules").await;
    write_file(dir.path(), "pkg/AGENTS.md", "pkg rules").await;
    write_file(dir.path(), "README.md", "readme").await;
    let block = render_environment(root_of(&dir)).await;
    assert!(
        block.starts_with("<system-reminder>\n<environment>\nPlatform: "),
        "{block}"
    );
    assert!(
        block.contains(&format!("Working directory: {}", dir.path().display())),
        "{block}"
    );
    assert!(block.contains("Date: 20"), "{block}");
    assert!(block.contains("<AGENTS.md>\nroot rules\n</AGENTS.md>"), "{block}");
    assert!(
        block.contains("Other AGENTS.md files nearby (read as needed): pkg/AGENTS.md"),
        "{block}"
    );
    assert!(
        block.contains("README files nearby (read as needed): README.md"),
        "{block}"
    );
    assert!(block.ends_with("</system-reminder>"), "{block}");
}

#[tokio::test]
async fn reaches_nested_agents_md_within_the_depth_limit() {
    let dir = TempDir::new().expect("tempdir");
    write_file(dir.path(), "a/b/AGENTS.md", "deep").await;
    write_file(dir.path(), "a/b/c/AGENTS.md", "too deep").await;
    write_file(dir.path(), "node_modules/AGENTS.md", "skipped").await;
    write_file(dir.path(), ".hidden/AGENTS.md", "skipped").await;
    let block = render_environment(root_of(&dir)).await;
    assert!(block.contains("a/b/AGENTS.md"), "{block}");
    assert!(!block.contains("a/b/c/AGENTS.md"), "{block}");
    assert!(!block.contains("node_modules"), "{block}");
    assert!(!block.contains(".hidden"), "{block}");
}

#[tokio::test]
async fn omits_the_agents_md_section_when_there_is_none() {
    let dir = TempDir::new().expect("tempdir");
    let block = render_environment(root_of(&dir)).await;
    assert!(!block.contains("<AGENTS.md>"), "{block}");
    assert!(!block.contains("nearby"), "{block}");
}

#[tokio::test]
async fn truncates_an_oversize_agents_md() {
    let dir = TempDir::new().expect("tempdir");
    write_file(dir.path(), "AGENTS.md", &"x".repeat(AGENTS_MD_CAP + 500)).await;
    let block = render_environment(root_of(&dir)).await;
    assert!(
        block.contains("\n… (truncated — read the full file if you need the rest)"),
        "truncation marker missing"
    );
    assert!(
        block.len() < AGENTS_MD_CAP + 400,
        "block not truncated: {}",
        block.len()
    );
}

#[test]
fn omits_platform_and_working_directory_when_the_probes_fail() {
    let block = render(&Gathered::default());
    assert!(!block.contains("Platform:"), "{block}");
    assert!(!block.contains("Working directory:"), "{block}");
    assert!(block.contains("Date: 20"), "{block}");
}

#[tokio::test]
async fn memoizes_discovery_across_turns() {
    use std::sync::Arc;

    use strands_agents::models::Model;
    use strands_agents::test_support::{MockMessageModel, MockResponse};
    use strands_agents::types::Message;

    let dir = TempDir::new().expect("tempdir");
    write_file(dir.path(), "AGENTS.md", "first").await;
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::assistant("one")),
        MockResponse::message(Message::assistant("two")),
    ]);
    let agent = strands_agents::Agent::builder()
        .model(model.clone() as Arc<dyn Model>)
        .plugin(EnvironmentContext::new(EnvironmentContextConfig {
            name: None,
            root: root_of(&dir),
        }))
        .build()
        .await
        .expect("agent builds");
    agent.invoke("hi").await.expect("first turn");
    write_file(dir.path(), "AGENTS.md", "changed").await;
    agent.invoke("again").await.expect("second turn");
    let calls = model.calls();
    let second = calls[1]
        .0
        .iter()
        .map(Message::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(second.contains("<AGENTS.md>\nfirst\n</AGENTS.md>"), "{second}");
    assert!(!second.contains("changed"), "{second}");
}

#[tokio::test]
async fn is_enabled_by_default_and_opts_out_with_an_empty_list() {
    let selected = crate::builtin_tools::select_builtin_plugins(&["environment"], &[]).expect("resolves");
    assert_eq!(selected[0].name(), "strands:environment");
    assert!(crate::builtin_tools::select_builtin_plugins(&[], &[])
        .expect("resolves")
        .is_empty());
}
