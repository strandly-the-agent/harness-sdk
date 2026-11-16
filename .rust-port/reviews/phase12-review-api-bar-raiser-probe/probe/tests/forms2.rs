use strands_harness::*;
#[allow(dead_code)]
fn a() { let _ = HarnessAgentOptions::builder().context_manager("agentic").build(); }
#[allow(dead_code)]
fn b() { let _ = HarnessAgentOptions::builder().context_manager(false).build(); }
#[allow(dead_code)]
fn c() { let _ = HarnessAgentOptions::builder().skills(vec!["./skills"]).build(); }
#[allow(dead_code)]
fn d() { let _ = HarnessAgentOptions::builder().builtin_tools([BuiltinToolName::Read]).build(); }
#[allow(dead_code)]
fn e() { let _ = HarnessAgentOptions::builder().builtin_tools(BuiltinToolsConfig::builder().web_search("exa").build()).build(); }
#[allow(dead_code)]
fn f() { let _ = HarnessAgentOptions::builder().memory(MemoryConfig::builder().dir("./m").build()).build(); }
