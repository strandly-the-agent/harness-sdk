//! Tools that ship with the SDK (TS `vended-tools/`).
//!
//! The file and shell tools route through the agent's [`Sandbox`], or
//! through one bound when the tool is made.

pub mod bash;
pub mod edit;
pub mod handoff_to_user;
pub mod http_request;
#[cfg(feature = "mcp")]
pub mod mcp_router;
pub mod notebook;
mod paths;
pub mod python_repl;
pub mod read;
pub mod shell;
pub mod sleep;
pub mod stop;
pub mod subagent;
pub mod web_fetch;
pub mod write;

#[doc(inline)]
pub use bash::{BashOutput, BashSessionError, BashTimeoutError, bash};
#[doc(inline)]
pub use edit::{DEFAULT_EDIT_DESCRIPTION, MakeEditOptions, edit, make_edit};
#[doc(inline)]
pub use handoff_to_user::{
    HANDOFF_INTERRUPT_NAME, MakeHandoffToUserOptions, handoff_to_user, make_handoff_to_user,
};
#[doc(inline)]
pub use http_request::{HttpRequestOutput, http_request};
#[doc(inline)]
#[cfg(feature = "mcp")]
pub use mcp_router::{MCP_ROUTER_DESCRIPTION, MakeMcpRouterOptions, McpRouterToolError, make_mcp_router};
#[doc(inline)]
pub use notebook::{MakeNotebookOptions, make_notebook, notebook};
#[doc(inline)]
pub use python_repl::{
    MakePythonReplOptions, PYTHON_REPL_DESCRIPTION, PythonReplError, PythonReplOutput, make_python_repl,
    python_repl,
};
#[doc(inline)]
pub use read::{DEFAULT_READ_DESCRIPTION, MakeReadOptions, make_read, read};
#[doc(inline)]
pub use shell::{
    MakeShellOptions, SANDBOX_SHELL_DESCRIPTION, ShellExecutionError, ShellOutput, ShellTimeoutError,
    make_shell,
};
#[doc(inline)]
pub use sleep::{DEFAULT_MAX_DURATION, MakeSleepOptions, SLEEP_DESCRIPTION, make_sleep, sleep};
#[doc(inline)]
pub use stop::{
    DEFAULT_MAX_STOP_MESSAGE_LENGTH, DEFAULT_STOP_DESCRIPTION, DEFAULT_STOP_MESSAGE, MakeStopOptions,
    make_stop, stop,
};
#[doc(inline)]
pub use subagent::{
    DEFAULT_SUBAGENT_DESCRIPTION, DEFAULT_SUBAGENT_MAX_DEPTH, GENERALIST_DESCRIPTION,
    GENERALIST_INSTRUCTIONS, MakeSubagentOptions, SubagentTool, generalist, make_subagent, subagent,
};
#[doc(inline)]
pub use web_fetch::{MakeWebFetchOptions, WebFetchMode, make_web_fetch, web_fetch};
#[doc(inline)]
pub use write::{DEFAULT_WRITE_DESCRIPTION, MakeWriteOptions, make_write, write};

use std::future::Future;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::JsonValue;
use crate::sandbox::Sandbox;
use crate::tools::function_tool::{FunctionTool, InputValidator};
use crate::tools::{ToolContext, ToolError};

/// A function tool with a fixed JSON schema: `precheck` runs on the raw input (for the TS
/// validation texts the schema can't carry), then the schema validates it and `handler` gets the
/// parsed input.
pub(crate) fn schema_tool<I, P, F, Fut, R>(
    name: String,
    description: String,
    schema: JsonValue,
    validator: InputValidator,
    precheck: P,
    handler: F,
) -> FunctionTool
where
    I: DeserializeOwned + Send + 'static,
    P: Fn(&JsonValue) -> Result<(), ToolError> + Send + Sync + 'static,
    F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<R, ToolError>> + Send + 'static,
    R: Serialize,
{
    let handler = Arc::new(handler);
    FunctionTool::new(
        name,
        description,
        move |input: JsonValue, context: ToolContext| {
            let parsed = precheck(&input).and_then(|()| validator.parse::<I>(input));
            let handler = Arc::clone(&handler);
            async move { handler(parsed?, context).await }
        },
    )
    .with_input_schema(schema)
}

/// No extra input checks.
pub(crate) fn no_precheck(_: &JsonValue) -> Result<(), ToolError> {
    Ok(())
}

/// The bound sandbox, else the agent's (TS `boundSandbox ?? context.agent.sandbox`).
pub(crate) fn resolve_sandbox(
    bound: Option<&Arc<dyn Sandbox>>,
    context: &ToolContext,
) -> Result<Arc<dyn Sandbox>, ToolError> {
    match bound {
        Some(sandbox) => Ok(Arc::clone(sandbox)),
        None => Ok(context.agent().sandbox()?),
    }
}

/// A JS number as `String(n)` prints it, for messages that echo a TS number.
pub(crate) fn js_number(value: f64) -> String {
    crate::types::json::to_js_string(&serde_json::Number::from_f64(value).unwrap_or_else(|| 0.into()))
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
pub(crate) mod test_support;
