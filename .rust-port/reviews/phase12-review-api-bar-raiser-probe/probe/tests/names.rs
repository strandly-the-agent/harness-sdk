// Each item is a separate compile probe; errors are the evidence.
use strands_harness::*;

fn _builder_nameable(b: strands_harness::HarnessAgentOptionsBuilder) {}
fn _effort_parse() { let _: Effort = "high".parse().unwrap(); }
fn _effort_display() { let _ = Effort::High.to_string(); }
fn _todos_builder_debug() { let _ = format!("{:?}", TodosConfig::builder()); }
fn _env_builder_debug() { let _ = format!("{:?}", EnvironmentContextConfig::builder()); }
fn _web_fetch_builder_debug(m: std::sync::Arc<dyn strands::models::Model>) { let _ = format!("{:?}", MakeWebFetchOptions::builder().model(m)); }
fn _subagent_builder_debug(b: AgentBuilder) { let _ = format!("{:?}", MakeSubagentOptions::builder().builder(b)); }
fn _todo_status_str() { let _ = TodoStatus::Pending.as_str(); }
fn _skills_single(s: strands::vended_plugins::SkillSource) { let _: SkillsOption = s.into(); }
fn _mcp_from_json(o: strands::types::json::JsonObject) { let _: McpServersOption = o.into(); }
fn _mcp_from_string() { let _: McpServersOption = String::from("./mcp.json").into(); }
