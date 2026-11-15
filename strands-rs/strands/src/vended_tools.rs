//! Tools that ship with the SDK (TS `vended-tools/`).
//!
//! The file and shell tools route through the agent's [`Sandbox`], or
//! through one bound when the tool is made.

pub mod bash;
pub mod edit;
mod paths;
pub mod read;
pub mod shell;
pub mod write;

pub use bash::{BashOutput, BashSessionError, BashTimeoutError, bash};
pub use edit::{DEFAULT_EDIT_DESCRIPTION, MakeEditOptions, edit, make_edit};
pub use read::{DEFAULT_READ_DESCRIPTION, MakeReadOptions, make_read, read};
pub use shell::{
    MakeShellOptions, SANDBOX_SHELL_DESCRIPTION, ShellExecutionError, ShellOutput, ShellTimeoutError,
    make_shell,
};
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
