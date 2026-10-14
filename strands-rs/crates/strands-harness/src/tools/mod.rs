//! Harness-authored tools (`harness-ts/src/tools/`).
//!
//! Thin, self-contained tools that fill gaps in the SDK's vended set.
//!
//! Not ported: `programmatic-tool-caller` (Node-only, depends on a JS runtime inside the tool).

pub mod file_tools;
pub mod subagent;
#[cfg(feature = "http")]
pub mod web_fetch;
pub mod web_search;

pub use file_tools::{edit, make_read, read, write, ReadConfig};
#[cfg(feature = "http")]
pub use web_fetch::{make_web_fetch, FetchedPage, MakeWebFetchOptions, WebFetchTransport};
pub use web_search::{
    exa_web_search, make_exa_web_search, search_tool, SearchBackend, SearchResult, WebSearchError,
};

#[cfg(test)]
pub(crate) mod test_support {
    use strands_agents::agent::InvocationState;
    use strands_agents::test_support::{MockMessageModel, MockResponse};
    use strands_agents::tools::{Tool, ToolContext};
    use strands_agents::types::{JsonValue, Message, ToolResultBlock, ToolResultContent, ToolUse};
    use strands_agents::Agent;
    use tokio_util::sync::CancellationToken;

    /// An agent over a mock model that replies `reply` to every call.
    pub(crate) async fn test_agent(reply: &str) -> Agent {
        Agent::builder()
            .model(MockMessageModel::new(vec![MockResponse::message(
                Message::assistant(reply),
            )]))
            .build()
            .await
            .expect("agent builds")
    }

    pub(crate) async fn context(tool: &dyn Tool, input: JsonValue) -> ToolContext {
        ToolContext::new(
            ToolUse {
                name: tool.name().to_owned(),
                tool_use_id: "test-id".to_owned(),
                input,
            },
            test_agent("mock").await,
            InvocationState::new(),
            CancellationToken::new(),
        )
    }

    /// Invoke `tool` with `input` and return its result block.
    pub(crate) async fn invoke(tool: &dyn Tool, input: JsonValue) -> ToolResultBlock {
        let ctx = context(tool, input).await;
        tool.invoke(ctx).await.expect("tool invocation completes")
    }

    /// The concatenated text of a tool result.
    pub(crate) fn text_of(result: &ToolResultBlock) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                ToolResultContent::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
