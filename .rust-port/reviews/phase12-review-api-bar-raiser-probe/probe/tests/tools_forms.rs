use std::sync::Arc;
use strands_harness::*;

#[allow(dead_code)]
fn sdk_builder_accepts() {
    // The SDK builder the harness is meant to follow (#32).
    let _ = strands::Agent::builder().tool(read()).tools([write(), edit()]).plugin(Todos::default());
}
#[allow(dead_code)]
fn ok() {
    let _ = HarnessAgentOptions::builder().tools(vec![read().into()]).build();
    let _ = HarnessAgentOptions::builder().plugins(vec![Arc::new(Todos::default())]).build();
}
#[allow(dead_code)]
fn a() { let _ = HarnessAgentOptions::builder().tools(vec![read()]).build(); }
#[allow(dead_code)]
fn b() { let _ = HarnessAgentOptions::builder().tools([read()]).build(); }
#[allow(dead_code)]
fn c() { let _ = HarnessAgentOptions::builder().plugins(vec![Todos::default()]).build(); }
