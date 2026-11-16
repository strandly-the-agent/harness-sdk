//! Ports `harness-ts/test/plugins/environment.test.ts`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::StreamExt;
use futures::stream::{self, BoxStream};
use strands::sandbox::{ExecutionEvent, ExecutionResult, FileInfo, NotASandboxLocalEnvironment};

use super::*;
use crate::sandbox::SHELL_PROBE;

/// What the fake's shell answers.
enum Answers {
    /// `sh`: `uname` prints `Linux`, anything else the working directory.
    Sh { cwd: &'static str },
    /// The SDK's local sandbox on Windows: PowerShell, which has no `uname`.
    PowerShell {
        platform: &'static str,
        cwd: &'static str,
    },
    /// Every command fails to run.
    None,
}

struct FakeSandbox {
    files: HashMap<&'static str, String>,
    tree: HashMap<&'static str, Vec<FileInfo>>,
    shell: Answers,
    list_calls: AtomicUsize,
}

impl FakeSandbox {
    fn new(files: &[(&'static str, &str)], tree: Vec<(&'static str, Vec<FileInfo>)>) -> Self {
        Self {
            files: files
                .iter()
                .map(|(path, text)| (*path, (*text).to_owned()))
                .collect(),
            tree: tree.into_iter().collect(),
            shell: Answers::Sh { cwd: "/work" },
            list_calls: AtomicUsize::new(0),
        }
    }

    fn with_shell(mut self, shell: Answers) -> Self {
        self.shell = shell;
        self
    }

    fn run(&self, command: &str) -> strands::Result<ExecutionResult> {
        match self.shell {
            Answers::None => Err(strands::Error::other("no shell")),
            Answers::Sh { cwd } => Ok(if command == SHELL_PROBE {
                ExecutionResult::new(0, "\n", "")
            } else if command.starts_with("uname") {
                ExecutionResult::new(0, "Linux\n", "")
            } else {
                ExecutionResult::new(0, format!("{cwd}\n"), "")
            }),
            Answers::PowerShell { platform, cwd } => Ok(if command == SHELL_PROBE {
                ExecutionResult::new(0, "Desktop\r\n", "")
            } else if command.starts_with("if ($IsLinux") {
                ExecutionResult::new(0, format!("{platform}\r\n"), "")
            } else if command.starts_with("uname") {
                ExecutionResult::new(1, "", "The term 'uname' is not recognized")
            } else {
                ExecutionResult::new(0, format!("{cwd}\r\n"), "")
            }),
        }
    }
}

#[async_trait]
impl Sandbox for FakeSandbox {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        stream::iter([self.run(command).map(ExecutionEvent::ExecutionResult)]).boxed()
    }

    fn execute_code_streaming<'a>(
        &'a self,
        _code: &'a str,
        _language: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        stream::iter([Err(strands::Error::other("not supported"))]).boxed()
    }

    async fn read_file(&self, path: &str) -> strands::Result<Vec<u8>> {
        self.files
            .get(path)
            .map(|text| text.clone().into_bytes())
            .ok_or_else(|| strands::Error::other(format!("not found: {path}")))
    }

    async fn write_file(&self, _path: &str, _content: &[u8]) -> strands::Result<()> {
        Err(strands::Error::other("not supported"))
    }

    async fn remove_file(&self, _path: &str) -> strands::Result<()> {
        Err(strands::Error::other("not supported"))
    }

    async fn list_files(&self, path: &str) -> strands::Result<Vec<FileInfo>> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        self.tree
            .get(path)
            .cloned()
            .ok_or_else(|| strands::Error::other(format!("not found: {path}")))
    }
}

fn file(name: &str) -> FileInfo {
    FileInfo::new(name).with_is_dir(false)
}

fn dir(name: &str) -> FileInfo {
    FileInfo::new(name).with_is_dir(true)
}

fn repo_tree() -> Vec<(&'static str, Vec<FileInfo>)> {
    vec![
        (
            ".",
            vec![
                file("AGENTS.md"),
                file("README.md"),
                dir("harness-py"),
                dir("node_modules"),
                dir(".git"),
            ],
        ),
        (
            "harness-py",
            vec![file("AGENTS.md"), file("README.md"), dir("src")],
        ),
        ("harness-py/src", vec![file("AGENTS.md")]),
    ]
}

async fn render_with(sandbox: FakeSandbox) -> String {
    let sandbox: Arc<dyn Sandbox> = Arc::new(sandbox);
    render_environment(
        Some(&sandbox),
        &ShellCache::default(),
        &EnvironmentMemo::default(),
    )
    .await
}

#[tokio::test]
async fn injects_env_agents_md_contents_and_nearby_links() {
    let out = render_with(FakeSandbox::new(&[("AGENTS.md", "root agents doc")], repo_tree())).await;
    assert!(out.contains("<environment>"));
    // Platform and cwd come from the sandbox probe (uname/$PWD), not the host process.
    assert!(out.contains("Platform: Linux"));
    assert!(out.contains("Date:"));
    assert!(out.contains("Working directory: /work"));
    assert!(out.contains("<AGENTS.md>\nroot agents doc\n</AGENTS.md>"));
    assert!(out.contains("harness-py/AGENTS.md"));
    assert!(out.contains("harness-py/README.md"));
    assert!(!out.contains("node_modules"));
    assert!(!out.contains(".git"));
}

/// TS changes the process working directory to a fresh one; a test here must not, so this
/// compares the probe with the directory the test process already runs in.
#[cfg(unix)]
#[tokio::test]
async fn reports_the_real_working_directory_from_the_local_sandbox_shell() {
    let sandbox: Arc<dyn Sandbox> = Arc::new(NotASandboxLocalEnvironment::new());
    let out = render_environment(
        Some(&sandbox),
        &ShellCache::default(),
        &EnvironmentMemo::default(),
    )
    .await;
    let line = out
        .lines()
        .find(|line| line.starts_with("Working directory: "))
        .unwrap_or_else(|| panic!("no working directory line in {out}"));
    let cwd = std::env::current_dir().unwrap();
    let name = cwd.file_name().unwrap().to_str().unwrap();
    assert!(line.ends_with(&format!("/{name}")), "{out}");
    let platform = out
        .lines()
        .find_map(|line| line.strip_prefix("Platform: "))
        .unwrap_or_else(|| panic!("no platform line in {out}"));
    let expected = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    assert_eq!(platform, expected);
}

#[tokio::test]
async fn reaches_nested_agents_md_within_the_depth_limit() {
    let out = render_with(FakeSandbox::new(&[("AGENTS.md", "x")], repo_tree())).await;
    assert!(out.contains("harness-py/src/AGENTS.md"));
}

#[tokio::test]
async fn omits_the_agents_md_section_when_there_is_none() {
    let out = render_with(FakeSandbox::new(&[], vec![(".", vec![file("main.ts")])])).await;
    assert!(!out.contains("<AGENTS.md>"));
    assert!(out.contains("<environment>"));
}

#[tokio::test]
async fn truncates_an_oversize_agents_md() {
    let big = "a".repeat(AGENTS_MD_CAP + 500);
    let out = render_with(FakeSandbox::new(
        &[("AGENTS.md", &big)],
        vec![(".", vec![file("AGENTS.md")])],
    ))
    .await;
    assert!(out.contains("truncated"));
    assert!(out.len() < big.len());
    assert!(out.contains(&format!(
        "<AGENTS.md>\n{}\n… (truncated — read the full file if you need the rest)\n</AGENTS.md>",
        "a".repeat(AGENTS_MD_CAP)
    )));
}

#[tokio::test]
async fn reports_windows_and_the_power_shell_shell_when_the_sandbox_runs_power_shell() {
    let sandbox = FakeSandbox::new(&[("AGENTS.md", "x")], vec![(".", vec![file("AGENTS.md")])]).with_shell(
        Answers::PowerShell {
            platform: "Windows",
            cwd: "C:\\Users\\me\\proj",
        },
    );
    let out = render_with(sandbox).await;
    assert!(out.contains("Platform: Windows"));
    assert!(out.contains("Shell: PowerShell"));
    assert!(out.contains("Working directory: C:\\Users\\me\\proj"));
}

#[tokio::test]
async fn asks_a_pwsh_sandbox_for_its_platform_instead_of_assuming_windows() {
    let sandbox = FakeSandbox::new(&[], vec![(".", vec![])]).with_shell(Answers::PowerShell {
        platform: "Linux",
        cwd: "/work",
    });
    let out = render_with(sandbox).await;
    assert!(out.contains("Platform: Linux"));
    assert!(out.contains("Shell: PowerShell"));
}

#[tokio::test]
async fn adds_no_shell_line_for_an_sh_sandbox() {
    let out = render_with(FakeSandbox::new(&[], vec![(".", vec![])])).await;
    assert!(out.contains("Platform: Linux"));
    assert!(!out.contains("Shell:"));
}

#[tokio::test]
async fn omits_platform_and_working_directory_when_the_probes_fail() {
    let sandbox = FakeSandbox::new(&[("AGENTS.md", "x")], vec![(".", vec![file("AGENTS.md")])])
        .with_shell(Answers::None);
    let out = render_with(sandbox).await;
    assert!(!out.contains("Working directory"));
    assert!(!out.contains("Platform"));
    assert!(out.contains("Date:"));
}

#[tokio::test]
async fn memoizes_discovery_across_turns() {
    let fake = Arc::new(FakeSandbox::new(&[("AGENTS.md", "x")], repo_tree()));
    let sandbox: Arc<dyn Sandbox> = Arc::clone(&fake) as Arc<dyn Sandbox>;
    let (shells, memo) = (ShellCache::default(), EnvironmentMemo::default());
    render_environment(Some(&sandbox), &shells, &memo).await;
    let after_first = fake.list_calls.load(Ordering::SeqCst);
    assert_eq!(after_first, 3);
    render_environment(Some(&sandbox), &shells, &memo).await;
    assert_eq!(fake.list_calls.load(Ordering::SeqCst), after_first);
}

// Rust-only cases.

#[test]
fn the_block_has_the_ts_layout() {
    let gathered = Gathered {
        platform: Some("Linux".to_owned()),
        cwd: Some("/work".to_owned()),
        shell: Some(SandboxShell::PowerShell),
        agents_md: Some("doc".to_owned()),
        other_agents: vec!["a/AGENTS.md".to_owned(), "b/AGENTS.md".to_owned()],
        readmes: vec!["README.md".to_owned(), "a/README.md".to_owned()],
    };
    assert_eq!(
        render(&gathered, "2026-01-02"),
        "<system-reminder>\n<environment>\nPlatform: Linux\nShell: PowerShell (the shell tool runs commands in it, not sh)\nDate: 2026-01-02\nWorking directory: /work\n</environment>\n\n<AGENTS.md>\ndoc\n</AGENTS.md>\n\nOther AGENTS.md files nearby (read as needed): a/AGENTS.md, b/AGENTS.md\n\nREADME files nearby (read as needed): README.md, a/README.md\n</system-reminder>"
    );
    assert_eq!(
        render(&Gathered::default(), "2026-01-02"),
        "<system-reminder>\n<environment>\nDate: 2026-01-02\n</environment>\n</system-reminder>"
    );
}

#[test]
fn the_strings_are_the_ones_in_the_ts_source() {
    let Ok(source) = std::fs::read_to_string("/tmp/ts-spec/harness-ts/src/plugins/environment.ts") else {
        return;
    };
    for text in [
        TRUNCATION_MARKER.trim_start_matches('\n'),
        PWSH_PLATFORM_PROBE,
        "Shell: PowerShell (the shell tool runs commands in it, not sh)",
        "Other AGENTS.md files nearby (read as needed): ",
        "README files nearby (read as needed): ",
        "AGENTS_MD_CAP = 16_000",
        "DISCOVERY_DEPTH = 2",
    ] {
        assert!(source.contains(text), "not in the TS source: {text}");
    }
    for name in SKIP_DIRS {
        assert!(source.contains(&format!("'{name}'")), "not a TS skip dir: {name}");
    }
}

#[test]
fn today_is_an_iso_calendar_date() {
    let today = today_iso();
    let parts: Vec<&str> = today.split('-').collect();
    assert_eq!(parts.iter().map(|part| part.len()).collect::<Vec<_>>(), [4, 2, 2]);
    assert!(
        parts
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    );
}

#[test]
fn the_cap_counts_utf16_code_units_and_never_splits_a_character() {
    let exact = "a".repeat(AGENTS_MD_CAP);
    assert_eq!(cap_agents_md(exact.clone()), exact);
    // 😀 is two UTF-16 code units: the cap falls inside the last one, which is dropped whole.
    let text = format!("{}😀b", "a".repeat(AGENTS_MD_CAP - 1));
    assert_eq!(
        cap_agents_md(text),
        format!("{}{TRUNCATION_MARKER}", "a".repeat(AGENTS_MD_CAP - 1))
    );
    let text = "😀".repeat(AGENTS_MD_CAP / 2 + 1);
    assert_eq!(
        cap_agents_md(text),
        format!("{}{TRUNCATION_MARKER}", "😀".repeat(AGENTS_MD_CAP / 2))
    );
}

#[tokio::test]
async fn discovery_stops_two_levels_down_and_skips_build_and_hidden_directories() {
    let tree = vec![
        (
            ".",
            vec![dir("a"), dir("target"), dir(".hidden"), file("README.md")],
        ),
        ("a", vec![dir("b"), file("README.md")]),
        ("a/b", vec![dir("c"), file("AGENTS.md")]),
        ("a/b/c", vec![file("AGENTS.md")]),
        ("target", vec![file("README.md")]),
        (".hidden", vec![file("README.md")]),
    ];
    let out = render_with(FakeSandbox::new(&[], tree)).await;
    assert!(out.contains("Other AGENTS.md files nearby (read as needed): a/b/AGENTS.md\n"));
    assert!(out.contains("README files nearby (read as needed): a/README.md, README.md\n"));
}

#[tokio::test]
async fn without_a_sandbox_the_block_carries_only_the_date() {
    let out = render_environment(None, &ShellCache::default(), &EnvironmentMemo::default()).await;
    assert_eq!(
        out,
        format!(
            "<system-reminder>\n<environment>\nDate: {}\n</environment>\n</system-reminder>",
            today_iso()
        )
    );
}

#[tokio::test]
async fn the_plugin_injects_the_block_on_a_user_turn() {
    use strands::Agent;
    use strands::testing::MockMessageModel;
    use strands::types::messages::TextBlock;

    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("hi").into()]);
    let plugin = EnvironmentContext::new(EnvironmentContextConfig::builder().name("my:env").build());
    assert_eq!(plugin.name(), "my:env");
    assert_eq!(EnvironmentContext::default().name(), "strands:environment");
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .printer(false)
        .sandbox(Arc::new(FakeSandbox::new(
            &[("AGENTS.md", "root agents doc")],
            repo_tree(),
        )))
        .plugin(plugin)
        .build()
        .unwrap();
    agent.invoke("hello").await.unwrap();
    let requests = model.requests();
    let sent = serde_json::to_string(requests[0].messages.last().unwrap().content()).unwrap();
    assert!(sent.contains("Working directory: /work"), "{sent}");
    assert!(sent.contains("root agents doc"), "{sent}");
}
