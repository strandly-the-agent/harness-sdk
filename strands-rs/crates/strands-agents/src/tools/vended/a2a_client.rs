//! The `a2a_client` tool (`strands-ts/src/vended-tools/a2a-client/`).
//!
//! Lets a local agent discover and talk to remote A2A agents, restricted to an allow-list of
//! endpoints. Stateless: a fresh [`A2AAgent`] is built on every call, like TS.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::a2a::types::TaskState;
use crate::a2a::{A2AAgent, A2AStreamEvent};
use crate::error::{Error, Result};
use crate::tools::tool::{create_error_result, Tool, ToolContext};
use crate::tools::wrap_in_tool_result;
use crate::types::{ToolResultBlock, ToolSpec};
use futures::StreamExt;

/// Name of the A2A client tool.
pub const A2A_CLIENT_TOOL_NAME: &str = "a2a_client";

/// Description of the A2A client tool, verbatim from TS (`DEFAULT_A2A_CLIENT_DESCRIPTION`).
/// The permitted endpoints are appended to it unless a description is configured.
pub const DEFAULT_A2A_CLIENT_DESCRIPTION: &str = "Interacts with remote A2A (Agent-to-Agent) protocol agents. Use operation='discover' to fetch an agent card from an endpoint. Use operation='send_message' to send a message and receive a response. Only the listed endpoints are permitted.";

/// What the model asked the tool to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum A2AClientOperation {
    /// Fetch the remote agent's card.
    Discover,
    /// Send a message and return the remote agent's reply.
    SendMessage,
}

#[derive(Debug, Deserialize)]
struct A2AClientInput {
    operation: A2AClientOperation,
    endpoint: String,
    #[serde(default)]
    message: Option<String>,
}

/// A remote-A2A-agent tool over an allow-list of endpoints (`makeA2AClient` in TS).
///
/// Each endpoint may carry its own [`reqwest::Client`], which is where per-endpoint
/// authentication lives (TS passes a `ClientFactory` per endpoint).
#[derive(Debug)]
pub struct A2AClientTool {
    endpoints: BTreeMap<String, Option<reqwest::Client>>,
    spec: ToolSpec,
}

impl A2AClientTool {
    /// Tool permitting `endpoints`, with the default name and description.
    ///
    /// # Errors
    /// Fails when `endpoints` is empty, like TS.
    pub fn new<I, S>(endpoints: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::builder(endpoints).build()
    }

    /// Start building a tool permitting `endpoints`.
    pub fn builder<I, S>(endpoints: I) -> A2AClientToolBuilder
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        A2AClientToolBuilder {
            endpoints: endpoints
                .into_iter()
                .map(|endpoint| (endpoint.into(), None))
                .collect(),
            name: None,
            description: None,
        }
    }

    /// The permitted endpoints, sorted.
    pub fn allowed_endpoints(&self) -> Vec<&str> {
        self.endpoints.keys().map(String::as_str).collect()
    }

    /// A fresh remote agent for `endpoint`, or an error naming the permitted ones.
    fn agent_for(&self, endpoint: &str) -> Result<A2AAgent> {
        let client = self.endpoints.get(endpoint).ok_or_else(|| {
            Error::model(format!(
                "Endpoint '{endpoint}' is not in the allowed endpoints list. Permitted endpoints: {}",
                self.allowed_endpoints().join(", ")
            ))
        })?;
        let mut builder = A2AAgent::builder(endpoint);
        if let Some(client) = client.clone() {
            builder = builder.http_client(client);
        }
        Ok(builder.build())
    }
}

/// Builds an [`A2AClientTool`].
#[derive(Clone, Debug)]
#[must_use = "an A2AClientToolBuilder does nothing until built"]
pub struct A2AClientToolBuilder {
    endpoints: BTreeMap<String, Option<reqwest::Client>>,
    name: Option<String>,
    description: Option<String>,
}

impl A2AClientToolBuilder {
    /// Permit `endpoint`, reaching it with `client` (where per-endpoint auth headers go).
    pub fn endpoint_with_client(mut self, endpoint: impl Into<String>, client: reqwest::Client) -> Self {
        self.endpoints.insert(endpoint.into(), Some(client));
        self
    }

    /// Tool name shown to the model (default [`A2A_CLIENT_TOOL_NAME`]).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Tool description shown to the model; replaces the default plus endpoint list.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Build the tool.
    ///
    /// # Errors
    /// Fails when no endpoint was permitted.
    pub fn build(self) -> Result<A2AClientTool> {
        if self.endpoints.is_empty() {
            return Err(Error::model(
                "allowedEndpoints must contain at least one endpoint",
            ));
        }
        let endpoints = self
            .endpoints
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let description = self
            .description
            .unwrap_or_else(|| format!("{DEFAULT_A2A_CLIENT_DESCRIPTION} Permitted endpoints: {endpoints}."));
        let name = self.name.unwrap_or_else(|| A2A_CLIENT_TOOL_NAME.to_owned());
        let spec = ToolSpec::new(&name, &description).with_input_schema(input_schema());
        Ok(A2AClientTool {
            endpoints: self.endpoints,
            spec,
        })
    }
}

/// Verbatim input schema of the TS zod schema.
fn input_schema() -> crate::types::JsonValue {
    json!({
        "type": "object",
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["discover", "send_message"],
                "description": "Action to perform — 'discover' to fetch the agent card, or 'send_message' to send a message"
            },
            "endpoint": {
                "type": "string",
                "description": "Base URL of the target A2A agent. Must be one of the permitted endpoints."
            },
            "message": {
                "type": ["string", "null"],
                "description": "Text to send to the agent. Required when operation is 'send_message'; ignored otherwise."
            }
        },
        "required": ["operation", "endpoint"]
    })
}

#[async_trait]
impl Tool for A2AClientTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        let tool_use_id = ctx.tool_use.tool_use_id.clone();
        let input: A2AClientInput = match ctx.input() {
            Ok(input) => input,
            Err(error) => return Ok(create_error_result(error, &tool_use_id)),
        };
        let agent = match self.agent_for(&input.endpoint) {
            Ok(agent) => agent,
            Err(error) => return Ok(create_error_result(error, &tool_use_id)),
        };

        let outcome = match input.operation {
            A2AClientOperation::Discover => discover(&agent).await,
            A2AClientOperation::SendMessage => match input.message.filter(|m| !m.is_empty()) {
                Some(message) => send_message(&agent, message).await,
                None => Err(Error::model("'message' is required for send_message operation")),
            },
        };

        match outcome {
            Ok(value) => Ok(wrap_in_tool_result(value, &tool_use_id)),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(error) => Ok(create_error_result(error, &tool_use_id)),
        }
    }
}

/// The remote agent's card.
async fn discover(agent: &A2AAgent) -> Result<crate::types::JsonValue> {
    let card = agent.agent_card().await.map_err(|error| {
        Error::model(format!(
            "Failed to discover agent card at '{}': {error}",
            agent.id()
        ))
    })?;
    Ok(serde_json::to_value(card)?)
}

/// Send `message` and return the remote agent's reply as `{ "message": { role, content } }`.
async fn send_message(agent: &A2AAgent, message: String) -> Result<crate::types::JsonValue> {
    let mut stream = Box::pin(agent.stream(message));
    let mut task_state: Option<TaskState> = None;
    let mut status_text = String::new();
    let mut reply = None;

    while let Some(event) = stream.next().await {
        let event = event
            .map_err(|error| Error::model(format!("Failed to send message to '{}': {error}", agent.id())))?;
        match &event {
            A2AStreamEvent::StreamUpdate(update) => {
                if let Some(status) = update.event.status() {
                    task_state = Some(status.state);
                    status_text = status.message.as_ref().map(|m| m.text()).unwrap_or_default();
                }
            }
            A2AStreamEvent::Result(result) => reply = Some(result.result.last_message.clone()),
        }
    }

    if let Some(state) = task_state.filter(|state| *state != TaskState::Completed) {
        let message = format!(
            "Remote agent at '{}' did not complete: task state is '{}'. {status_text}",
            agent.id(),
            state.as_str()
        );
        return Err(Error::model(message.trim_end().to_owned()));
    }

    let reply = reply.ok_or_else(|| {
        Error::model(format!(
            "Failed to send message to '{}': the remote agent produced no result",
            agent.id()
        ))
    })?;
    Ok(json!({ "message": reply }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::AgentCard;
    use crate::a2a::{A2AExecutor, A2AServer, ServerHandle};
    use crate::agent::Agent;
    use crate::testing::{MockMessageModel, MockResponse};
    use crate::tools::tool::test_context;
    use crate::types::{ContentBlock, Message, Role, ToolResultContent, ToolResultStatus};

    async fn serve(reply: &'static str) -> ServerHandle {
        A2AServer::builder("Remote Agent")
            .description("A remote helper")
            .executor(A2AExecutor::with_factory(move |_| {
                Agent::builder()
                    .model(MockMessageModel::shared(vec![MockResponse::message(
                        Message::new(Role::Assistant, vec![ContentBlock::text(reply)]),
                    )]))
                    .build()
                    .unwrap()
            }))
            .build()
            .unwrap()
            .serve()
            .await
            .unwrap()
    }

    async fn call(tool: &A2AClientTool, input: serde_json::Value) -> ToolResultBlock {
        tool.invoke(test_context(A2A_CLIENT_TOOL_NAME, "test-id", input))
            .await
            .unwrap()
    }

    fn json_of(result: &ToolResultBlock) -> serde_json::Value {
        assert_eq!(result.status, ToolResultStatus::Success, "{result:?}");
        match &result.content[0] {
            ToolResultContent::Json(value) => value.clone(),
            other => panic!("expected json content, got {other:?}"),
        }
    }

    fn error_text(result: &ToolResultBlock) -> String {
        assert_eq!(result.status, ToolResultStatus::Error, "{result:?}");
        result
            .content
            .iter()
            .filter_map(|content| match content {
                ToolResultContent::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn fails_when_allowed_endpoints_is_empty() {
        assert!(A2AClientTool::new(Vec::<String>::new()).is_err());
    }

    #[test]
    fn uses_a_custom_name() {
        let tool = A2AClientTool::builder(["http://a.example.com"])
            .name("my_a2a")
            .build()
            .unwrap();
        assert_eq!(tool.name(), "my_a2a");
    }

    #[test]
    fn the_default_description_includes_the_endpoints() {
        let tool = A2AClientTool::new(["http://b.example.com", "http://a.example.com"]).unwrap();
        assert_eq!(
            tool.description(),
            format!("{DEFAULT_A2A_CLIENT_DESCRIPTION} Permitted endpoints: http://a.example.com, http://b.example.com.")
        );
    }

    #[test]
    fn a_custom_description_overrides_the_default() {
        let tool = A2AClientTool::builder(["http://a.example.com"])
            .description("Talk to my agent")
            .build()
            .unwrap();
        assert_eq!(tool.description(), "Talk to my agent");
    }

    #[test]
    fn the_input_schema_matches_the_ts_zod_schema() {
        let tool = A2AClientTool::new(["http://a.example.com"]).unwrap();
        let schema = tool.spec().input_schema.clone().unwrap();
        assert_eq!(
            schema["properties"]["operation"]["enum"],
            json!(["discover", "send_message"])
        );
        assert_eq!(schema["required"], json!(["operation", "endpoint"]));
    }

    #[tokio::test]
    async fn rejects_an_endpoint_not_in_the_allowlist() {
        let tool = A2AClientTool::new(["http://allowed.example.com"]).unwrap();

        let result = call(
            &tool,
            json!({ "operation": "discover", "endpoint": "http://evil.example.com" }),
        )
        .await;

        let text = error_text(&result);
        assert!(text.contains("is not in the allowed endpoints list"), "{text}");
        assert!(text.contains("http://allowed.example.com"), "{text}");
    }

    #[tokio::test]
    async fn discover_returns_the_agent_card() {
        let handle = serve("Hello").await;
        let endpoint = format!("http://{}", handle.local_addr());
        let tool = A2AClientTool::new([endpoint.clone()]).unwrap();

        let result = call(&tool, json!({ "operation": "discover", "endpoint": endpoint })).await;

        let card: AgentCard = serde_json::from_value(json_of(&result)).unwrap();
        assert_eq!(card.name, "Remote Agent");
        assert_eq!(card.description, "A remote helper");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn wraps_a_discovery_failure() {
        let tool = A2AClientTool::new(["http://127.0.0.1:1"]).unwrap();

        let result = call(
            &tool,
            json!({ "operation": "discover", "endpoint": "http://127.0.0.1:1" }),
        )
        .await;

        assert!(
            error_text(&result).contains("Failed to discover agent card at 'http://127.0.0.1:1'"),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn send_message_returns_the_response_message() {
        let handle = serve("Hi from the remote").await;
        let endpoint = format!("http://{}", handle.local_addr());
        let tool = A2AClientTool::new([endpoint.clone()]).unwrap();

        let result = call(
            &tool,
            json!({ "operation": "send_message", "endpoint": endpoint, "message": "hello" }),
        )
        .await;

        let value = json_of(&result);
        assert_eq!(value["message"]["role"], "assistant");
        assert_eq!(value["message"]["content"][0]["text"], "Hi from the remote");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn send_message_requires_a_message() {
        let handle = serve("Hello").await;
        let endpoint = format!("http://{}", handle.local_addr());
        let tool = A2AClientTool::new([endpoint.clone()]).unwrap();

        for message in [None, Some(json!("")), Some(json!(null))] {
            let mut input = json!({ "operation": "send_message", "endpoint": endpoint.clone() });
            if let Some(message) = message {
                input["message"] = message;
            }
            let result = call(&tool, input).await;
            assert!(
                error_text(&result).contains("'message' is required for send_message operation"),
                "{result:?}"
            );
        }
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn reports_a_task_that_did_not_complete() {
        // A model error drives the executor to a failed terminal state through the real server.
        let handle = A2AServer::builder("Failing Agent")
            .executor(A2AExecutor::with_factory(|_| {
                Agent::builder()
                    .model(MockMessageModel::shared(vec![MockResponse::Error(Error::other(
                        "model exploded",
                    ))]))
                    .build()
                    .unwrap()
            }))
            .build()
            .unwrap()
            .serve()
            .await
            .unwrap();
        let endpoint = format!("http://{}", handle.local_addr());
        let tool = A2AClientTool::new([endpoint.clone()]).unwrap();

        let result = call(
            &tool,
            json!({ "operation": "send_message", "endpoint": endpoint, "message": "hi" }),
        )
        .await;

        let text = error_text(&result);
        assert!(
            text.contains("did not complete: task state is 'failed'"),
            "{text}"
        );
        assert!(text.contains("model exploded"), "{text}");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn wraps_a_send_failure() {
        let tool = A2AClientTool::new(["http://127.0.0.1:1"]).unwrap();

        let result = call(
            &tool,
            json!({ "operation": "send_message", "endpoint": "http://127.0.0.1:1", "message": "hi" }),
        )
        .await;

        assert!(
            error_text(&result).contains("Failed to send message to 'http://127.0.0.1:1'"),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn uses_the_client_configured_for_the_endpoint() {
        let handle = serve("Hello").await;
        let endpoint = format!("http://{}", handle.local_addr());
        let client = reqwest::Client::builder()
            .user_agent("strands-test")
            .build()
            .unwrap();
        let tool = A2AClientTool::builder([endpoint.clone()])
            .endpoint_with_client(endpoint.clone(), client)
            .build()
            .unwrap();

        let result = call(
            &tool,
            json!({ "operation": "send_message", "endpoint": endpoint, "message": "hello" }),
        )
        .await;

        assert_eq!(json_of(&result)["message"]["content"][0]["text"], "Hello");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn an_unknown_operation_is_an_input_error() {
        let tool = A2AClientTool::new(["http://a.example.com"]).unwrap();

        let result = call(
            &tool,
            json!({ "operation": "teleport", "endpoint": "http://a.example.com" }),
        )
        .await;

        assert_eq!(result.status, ToolResultStatus::Error);
    }
}
