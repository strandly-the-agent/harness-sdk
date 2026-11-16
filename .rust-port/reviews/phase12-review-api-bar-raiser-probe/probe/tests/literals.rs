// Struct-literal construction with `..Default::default()` from another crate (#65).
use strands_harness::*;

#[test]
fn literals() {
    let _ = HarnessAgentOptions { effort: Effort::High, ..Default::default() };
    let _ = ReadConfig { media: Some(true), ..Default::default() };
    let _ = ShellConfig { description: None, ..Default::default() };
    let _ = HarnessConfigWebFetch { transport: None, ..Default::default() };
    let _ = ProgrammaticToolCallerConfig { timeout: Some(None), ..Default::default() };
    let _ = SubagentConfig { max_depth: Some(1), ..Default::default() };
    let _ = HarnessConfigBuiltinTools { all: Some(false), ..Default::default() };
    let _ = HarnessConfigMemory { dir: None, ..Default::default() };
    let _ = HarnessDependencies { python: vec![], ..Default::default() };
    let _ = HarnessAgentConfig { caching: false, ..Default::default() };
    let _ = WebFetchConfig { transport: None, ..Default::default() };
    let _ = BuiltinToolsConfig { all: Some(false), ..Default::default() };
    let _ = SessionConfig { id: Some("x".into()), ..Default::default() };
    let _ = MemoryConfig { dir: None, ..Default::default() };
    let _ = EnvironmentContextConfig { name: None, ..Default::default() };
    let _ = TodosConfig { name: None, ..Default::default() };
    let _ = Preset { description: "d".into(), ..Default::default() };
    let _ = MakeProgrammaticToolCallerOptions { name: None, ..Default::default() };
    let _ = TodoItem { content: "a".into(), active_form: "b".into(), status: TodoStatus::Pending };
    let _ = AgentSpec::new("t", None);
    let _ = HarnessModuleReference::new(HarnessModuleKind::Tool, "m");
}
