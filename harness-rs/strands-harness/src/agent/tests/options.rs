//! The conversions the options builder accepts: one per member of each TS union of
//! `HarnessAgentOptions` (`harness-ts/src/agent.ts`).

use strands::types::json::JsonObject;
use strands::vended_plugins::SkillSource;

use super::*;

#[test]
fn the_documented_builder_snippet_compiles_with_a_path_for_mcp_servers() {
    let options = HarnessAgentOptions::builder()
        .model("anthropic/claude-opus-5")
        .effort(Effort::High)
        .mcp_servers("./mcp.json")
        .interventions("ask")
        .build();
    assert_eq!(
        options.mcp_servers,
        Some(McpServersOption::Path("./mcp.json".to_owned()))
    );
}

#[test]
fn mcp_servers_takes_a_path_or_a_server_map() {
    let owned = String::from("./mcp.json");
    let path = McpServersOption::Path(owned.clone());
    assert_eq!(McpServersOption::from(&owned), path);
    assert_eq!(McpServersOption::from(owned), path);

    let mut servers = JsonObject::new();
    servers.insert("docs".to_owned(), serde_json::json!({ "command": "docs-server" }));
    let options = HarnessAgentOptions::builder()
        .mcp_servers(servers.clone())
        .build();
    assert_eq!(options.mcp_servers, Some(McpServersOption::Servers(servers)));
}

#[test]
fn skills_takes_one_source_as_ts_does() {
    let one = HarnessAgentOptions::builder()
        .skills(SkillSource::from("./skills"))
        .build();
    assert!(matches!(&one.skills, SkillsOption::Sources(sources) if sources.len() == 1));
    let text = HarnessAgentOptions::builder().skills("./skills").build();
    assert!(matches!(&text.skills, SkillsOption::Sources(sources) if sources.len() == 1));
    let owned = HarnessAgentOptions::builder()
        .skills(String::from("./skills"))
        .build();
    assert!(matches!(&owned.skills, SkillsOption::Sources(sources) if sources.len() == 1));
}

#[test]
fn builtin_tools_takes_an_array_or_a_slice_of_names() {
    let array = HarnessAgentOptions::builder()
        .builtin_tools([BuiltinToolName::Read, BuiltinToolName::Shell])
        .build();
    assert!(matches!(
        array.builtin_tools,
        Some(BuiltinToolsOption::Names(names)) if names == [BuiltinToolName::Read, BuiltinToolName::Shell]
    ));
    let slice = HarnessAgentOptions::builder()
        .builtin_tools(&[BuiltinToolName::Read][..])
        .build();
    assert!(matches!(
        slice.builtin_tools,
        Some(BuiltinToolsOption::Names(names)) if names == [BuiltinToolName::Read]
    ));
}

#[test]
fn the_builder_takes_tools_as_the_sdk_builder_does_and_extends() {
    let shared: Arc<dyn Tool> = Arc::new(crate::tools::edit());
    let options = HarnessAgentOptions::builder()
        .tool(crate::tools::read())
        .tools([crate::tools::write()])
        .tools(vec![shared])
        .tools(vec![consumer_tool("sample_tool")])
        .build();
    let names: Vec<&str> = options
        .tools
        .iter()
        .map(|source| match source {
            ToolSource::Tool(tool) => tool.name(),
            _ => "not a tool",
        })
        .collect();
    assert_eq!(names, ["read", "write", "edit", "sample_tool"]);
}

#[test]
fn the_builder_takes_plugins_by_value_or_shared_and_extends() {
    let kept = Arc::new(Todos::default());
    let erased: Arc<dyn Plugin> = Arc::new(EnvironmentContext::default());
    let options = HarnessAgentOptions::builder()
        .plugin(Arc::clone(&kept))
        .plugins(vec![Todos::default()])
        .plugins([erased])
        .build();
    let names: Vec<&str> = options.plugins.iter().map(|plugin| plugin.name()).collect();
    let (todos, environment) = (Todos::default(), EnvironmentContext::default());
    assert_eq!(names, [todos.name(), todos.name(), environment.name()]);
    assert!(Arc::ptr_eq(&options.plugins[0], &(kept as Arc<dyn Plugin>)));
}

#[test]
fn a_builder_without_tools_or_plugins_has_empty_lists() {
    let options = HarnessAgentOptions::builder().build();
    assert!(options.tools.is_empty());
    assert!(options.plugins.is_empty());
}
