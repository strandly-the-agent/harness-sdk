use std::sync::Arc;
use strands_harness::*;

fn assert_send<T: Send>(_: &T) {}
fn assert_send_sync_static<T: Send + Sync + 'static>() {}

#[allow(dead_code)]
fn forms(model: Arc<dyn strands::models::Model>, sm: strands::session::SessionManager) {
    assert_send_sync_static::<HarnessError>();
    assert_send_sync_static::<HarnessAgentOptions>();
    let fut = create_harness(HarnessAgentOptions::default());
    assert_send(&fut);
    let _ = HarnessAgentOptions::builder().caching(false).build();
    let _ = HarnessAgentOptions::builder().session(false).build();
    let _ = HarnessAgentOptions::builder().session(SessionConfig { id: Some("a".into()), dir: None }).build();
    let _ = HarnessAgentOptions::builder().session(sm).build();
    let _ = HarnessAgentOptions::builder().skills(false).build();
    let _ = HarnessAgentOptions::builder().memory(false).build();
    let _ = HarnessAgentOptions::builder().background_tasks(false).build();
    let _ = HarnessAgentOptions::builder().builtin_tools(vec![BuiltinToolName::Read]).build();
    let _ = HarnessAgentOptions::builder().builtin_tools(BuiltinToolsConfig { web_search: Some(WebSearchSetting::Exa), ..Default::default() }).build();
    let _ = HarnessAgentOptions::builder().builtin_tools(BuiltinToolsConfig::builder().web_search(false).shell(false).build()).build();
    let _ = HarnessAgentOptions::builder().builtin_plugins(vec![]).build();
    let _ = HarnessAgentOptions::builder().interventions(["a.cedar", "ask"]).build();
    let _ = HarnessAgentOptions::builder().model(model).build();
    let _ = HarnessAgentOptions::builder().instructions("x").build();
    let _ = HarnessAgentOptions::builder().context_manager(strands::experimental::context_manager::ContextManagerStrategy::Off).build();
}
