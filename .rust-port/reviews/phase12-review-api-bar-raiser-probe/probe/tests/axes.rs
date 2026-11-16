use strands_harness::*;
#[allow(dead_code)]
fn ok(b: AgentBuilder) {
    let _ = MakeSubagentOptions::builder().builder(b.clone()).instructions(Open).tools(Inherit).model(Inherit).context(Fixed(ContextMode::All)).build();
    let _ = MakeSubagentOptions::builder().builder(b.clone()).tools(Choice::multiple(["read", "write"])).build();
    let _ = MakeSubagentOptions::builder().builder(b.clone()).instructions(Fixed(Some("x".to_owned()))).build();
    let _ = MakeSubagentOptions::builder().builder(b.clone()).context(Choice::new([ChoiceOption::new("all").with_value(ContextMode::All)])).build();
    let _ = MakeSubagentOptions::builder().builder(b).model(Choice::new([ChoiceOption::new("fast").with_value("anthropic/claude-haiku-4-5")])).build();
}
#[allow(dead_code)]
fn a(b: AgentBuilder) { let _ = MakeSubagentOptions::builder().builder(b).instructions(Fixed("x")).build(); }
#[allow(dead_code)]
fn c(b: AgentBuilder) { let _ = MakeSubagentOptions::builder().builder(b).tools(Fixed(vec!["read".to_owned()])).build(); }
#[allow(dead_code)]
fn d(b: AgentBuilder) { let _ = MakeSubagentOptions::builder().builder(b).model(Fixed("anthropic/claude-haiku-4-5")).build(); }
#[allow(dead_code)]
fn e(b: AgentBuilder) { let _ = MakeSubagentOptions::builder().builder(b).context(ContextMode::All).build(); }
#[allow(dead_code)]
fn f(b: AgentBuilder) { let _ = MakeSubagentOptions::builder().builder(b).mcp_servers(Fixed(None)).build(); }
#[allow(dead_code)]
fn g() { let _ = MakeSubagentOptions::builder().builder(|spec: AgentSpec| async move { let _ = spec; Err::<strands::Agent, HarnessError>(HarnessError::EmptySessionId) }).build(); }
