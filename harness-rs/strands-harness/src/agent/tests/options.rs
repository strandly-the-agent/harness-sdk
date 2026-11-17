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
