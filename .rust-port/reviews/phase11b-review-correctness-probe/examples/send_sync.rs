use strands::mcp::{McpClient, McpClientConfig};
use strands::models::anthropic::AnthropicModel;
use strands::models::openai::OpenAIModel;
use strands::{Agent, AgentStream};

fn send_sync<T: Send + Sync>() {}
fn send<T: Send>() {}
fn assert_send<T: Send>(_: &T) {}

#[allow(dead_code, unreachable_code, clippy::diverging_sub_expression)]
async fn futures_are_send(agent: &mut Agent, client: &McpClient) {
    assert_send(&agent.invoke("x"));
    assert_send(&agent.shutdown());
    assert_send(&agent.initialize());
    assert_send(&client.list_tools());
    assert_send(&client.connect(false));
    assert_send(&client.disconnect());
    let tool: strands::mcp::McpTool = todo!();
    assert_send(&client.call_tool(&tool, serde_json::json!({})));
    assert_send(&agent.stream("x"));
}

fn main() {
    send_sync::<McpClient>();
    send_sync::<McpClientConfig>();
    send_sync::<strands::mcp::McpTool>();
    send_sync::<strands::mcp::McpTransport>();
    send_sync::<AnthropicModel>();
    send_sync::<OpenAIModel>();
    send_sync::<strands::models::routing::ModelRouter>();
    send_sync::<strands::memory::MemoryManager>();
    send_sync::<strands::session::SessionManager>();
    send_sync::<strands::experimental::context_manager::ContextManager>();
    send_sync::<strands::vended_interventions::HumanInTheLoop>();
    send_sync::<strands::vended_interventions::CedarAuthorization>();
    send_sync::<strands::telemetry::AgentMetrics>();
    send_sync::<strands::telemetry::AgentTrace>();
    send_sync::<strands::telemetry::BaggageSpanProcessor>();
    send_sync::<strands::telemetry::TracerConfig>();
    send_sync::<strands::telemetry::MeterConfig>();
    send_sync::<strands::CancelHandle>();
    send_sync::<strands::CancelSignal>();
    send_sync::<strands::AgentResult>();
    send_sync::<strands::Error>();
    send::<Agent>();
    send_sync::<Agent>();
    send::<AgentStream<'static>>();
}
