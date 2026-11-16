use strands::hooks::*;
use strands::testing::MockMessageModel;
use strands::{Agent, AgentResult, AgentStream, AgentStreamEvent, CancelHandle, CancelSignal, Error, ToolContext};

fn send<T: Send>() {}
fn sync<T: Sync>() {}
fn send_val<T: Send>(_: &T) {}

#[test]
fn types() {
    send::<Agent>();
    sync::<Agent>();
    send::<AgentResult>();
    sync::<AgentResult>();
    send::<AgentStream<'static>>();
    send::<AgentStreamEvent>();
    sync::<AgentStreamEvent>();
    send::<Error>();
    sync::<Error>();
    send::<ToolContext>();
    sync::<ToolContext>();
    send::<CancelHandle>();
    sync::<CancelHandle>();
    send::<CancelSignal>();
    sync::<CancelSignal>();
    send::<strands::types::agent::ToolAgent>();
    sync::<strands::types::agent::ToolAgent>();
    send::<strands::types::agent::LocalAgent>();
    sync::<strands::types::agent::LocalAgent>();
    send::<strands::memory::MemoryManager>();
    sync::<strands::memory::MemoryManager>();
    send::<strands::session::SessionManager>();
    sync::<strands::session::SessionManager>();
    send::<strands::models::routing::ModelRouter>();
    sync::<strands::models::routing::ModelRouter>();
    send::<strands::background_tasks::BackgroundTasks>();
    send::<strands::types::snapshot::Snapshot>();
    sync::<strands::types::snapshot::Snapshot>();
    send::<strands::tools::ToolStream>();
    send::<BeforeInvocationEvent<'static>>();
    send::<AfterInvocationEvent<'static>>();
    send::<MessageAddedEvent<'static>>();
    send::<BeforeModelCallEvent<'static>>();
    send::<AfterModelCallEvent<'static>>();
    send::<BeforeToolsEvent<'static>>();
    send::<AfterToolsEvent<'static>>();
    send::<BeforeToolCallEvent>();
    send::<AfterToolCallEvent>();
    send::<AgentResultEvent<'static>>();
    send::<InitializedEvent<'static>>();
    send::<InterruptEvent<'static>>();
    send::<HookRegistry>();
    sync::<HookRegistry>();
    send::<HookCleanup>();
}

#[test]
fn futures() {
    let mut agent = Agent::builder().model(MockMessageModel::new()).printer(false).build().unwrap();
    send_val(&agent.invoke("x"));
    send_val(&agent.stream("x"));
    send_val(&agent.initialize());
    send_val(&agent.shutdown());
}
