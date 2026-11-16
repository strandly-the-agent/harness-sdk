//! The `programmatic_tool_caller` tool (TS `tools/programmatic-tool-caller.ts`): lets the agent
//! orchestrate its other tools with Python code.
//!
//! The model calls the tool with a `code` string; the code runs in a Monty sandbox with every
//! other registered tool exposed as an `async` function taking keyword arguments. Only text the
//! code passes to `print()` is returned to the model.
//!
//! Monty's VM implements no host operations (no filesystem, environment, network or process
//! access), and its duration and recursion limits stop the code hard. The only way out of the
//! sandbox is the tool functions handed in here. The interpreter runs in this process, on the
//! blocking thread pool.
//!
//! Inner tool calls go through `ToolAgent::call_tool`, so `interventions` gate them: a denied call
//! raises a catchable `RuntimeError` in the code, and a call that needs interactive approval is
//! refused the same way, since a direct call cannot prompt.

mod output;
mod run;
#[cfg(test)]
mod spike;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use serde::Deserialize;
use strands::tools::FunctionTool;
use strands::types::agent::ToolCallOutcome;
use strands::types::messages::{ToolResultBlock, ToolResultContent};
use strands::types::tools::ToolResultStatus;
use strands::{JsonValue, ToolContext, ToolError};

use self::output::{MAX_OUTPUT_CHARS, cap};
use self::run::{HostCall, LIMITS, Limits, RunEnd, run_code};

/// Wall-clock ceiling for a run, tool calls included.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(900_000);

const CANCELLED: &str = "programmatic_tool_caller was cancelled; no further tool calls are made.";

/// The name of every live caller made by this module; none is ever exposed to another's code.
/// The `Weak` dies with the caller's handler, so a dropped caller stops reserving its name.
static INSTANCES: LazyLock<Mutex<Instances>> = LazyLock::new(Mutex::default);

type Instances = Vec<(String, Weak<()>)>;

/// The description of the default `programmatic_tool_caller` tool.
pub const DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION: &str = "Execute Python code that orchestrates the agent's other tools, each exposed as an async function \
taking keyword arguments -- always `await` them, e.g. `result = await read(path=\"/etc/hosts\")`. \
The code runs in an async context, so `await` and `asyncio.gather(...)` work without boilerplate. \
A tool result that is structured content or JSON text (an object or array) arrives parsed, as a \
dict/list; anything else arrives as a string. A tool that fails raises RuntimeError, which you can \
catch. \
Only text sent to `print()` is returned to you: a tool's return value stays in the code's local \
scope unless you print it. A tool whose name is not a valid Python identifier (for example \
`fetch-url` or `ns.fetch`) is also available with those characters replaced by underscores \
(`fetch_url`, `ns_fetch`). The code runs in a sandboxed Python subset: `asyncio`, `json`, `re`, \
`math`, `datetime`, `collections`, `itertools`, `functools`, `dataclasses`, `typing`, `base64` \
are importable; filesystem, network, and subprocess access do not exist -- use the agent's tools \
for those. Generators (`yield`), class inheritance, `match`, and `del` are not supported. Use this \
to chain, loop over, filter, or parallelize tool calls in a single turn instead of one tool call \
per model round-trip, keeping large intermediate results out of the conversation.";

/// Options for [`make_programmatic_tool_caller`].
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct MakeProgrammaticToolCallerOptions {
    /// Registry names of the tools to expose to the code. Defaults to every other registered
    /// tool. No programmatic tool caller is ever exposed, even if named here.
    pub allowed_tools: Option<Vec<String>>,
    /// Tool name. Defaults to `programmatic_tool_caller`.
    pub name: Option<String>,
    /// Tool description shown to the model.
    pub description: Option<String>,
    /// Wall-clock ceiling for a run, tool calls included; `Some(None)` disables it. Defaults to
    /// 900 seconds. Guest compute time is bounded separately by the sandbox.
    pub timeout: Option<Option<Duration>>,
}

/// What one inner tool call came to.
#[derive(Debug)]
pub(crate) enum InnerCall {
    /// The tool's result, after the per-tool pipeline.
    Result(ToolResultBlock),
    /// A hook asked for an approval nobody has answered; the text is its reason.
    NeedsApproval(String),
    /// The call was cancelled before the tool ran.
    Cancelled,
}

/// Runs one inner call of the tool `name` for the caller's own `ToolContext`, through the agent's
/// per-tool pipeline. `Err` is the text of a failure before or around the tool.
pub(crate) type InnerCaller = Arc<
    dyn Fn(ToolContext, String, JsonValue) -> BoxFuture<'static, Result<InnerCall, String>> + Send + Sync,
>;

/// The inner caller of the tools this module makes: `ToolAgent::call_tool`, which runs the call
/// the way the executor runs a model-issued one, so `interventions` gate it.
fn gated_inner_caller() -> InnerCaller {
    Arc::new(|parent: ToolContext, name: String, input: JsonValue| {
        async move {
            // A copy, so an inner call cannot change the parent's invocation state.
            let state = parent.invocation_state().detached();
            match parent.agent().call_tool(&name, input, state).await {
                Ok(ToolCallOutcome::Result(result)) => Ok(InnerCall::Result(result)),
                Ok(ToolCallOutcome::Interrupt(interrupt)) => {
                    Ok(InnerCall::NeedsApproval(match interrupt.reason() {
                        Some(JsonValue::String(reason)) => reason.clone(),
                        Some(reason) => reason.to_string(),
                        None => interrupt.name().to_owned(),
                    }))
                }
                Ok(ToolCallOutcome::Cancelled) => Ok(InnerCall::Cancelled),
                // TS `gateInnerCall` throws the refusal, so it gets the `Failed to execute` prefix.
                Ok(ToolCallOutcome::HookCancelled(result)) => Err(extract_text(result.content())),
                Ok(other) => Err(format!("unexpected outcome {other:?}")),
                Err(error) => Err(error.to_string()),
            }
        }
        .boxed()
    })
}

/// Creates a programmatic-tool-caller tool: it runs agent-authored Python in a Monty sandbox in
/// which the agent's other tools are `async` functions, and returns only `print()` output.
#[must_use]
pub fn make_programmatic_tool_caller(options: MakeProgrammaticToolCallerOptions) -> FunctionTool {
    make_programmatic_tool_caller_with(options, gated_inner_caller(), LIMITS)
}

/// The default programmatic tool caller, which exposes every other registered tool.
#[must_use]
pub fn programmatic_tool_caller() -> FunctionTool {
    make_programmatic_tool_caller(MakeProgrammaticToolCallerOptions::default())
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    code: String,
}

pub(crate) fn make_programmatic_tool_caller_with(
    options: MakeProgrammaticToolCallerOptions,
    inner: InnerCaller,
    limits: Limits,
) -> FunctionTool {
    let MakeProgrammaticToolCallerOptions {
        allowed_tools,
        name,
        description,
        timeout,
    } = options;
    let name = name.unwrap_or_else(|| "programmatic_tool_caller".to_owned());
    let description = description.unwrap_or_else(|| DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION.to_owned());
    let timeout = timeout.unwrap_or(Some(DEFAULT_TIMEOUT));
    let alive = Arc::new(());
    {
        let mut instances = INSTANCES.lock().unwrap_or_else(PoisonError::into_inner);
        instances.retain(|(_, alive)| alive.strong_count() > 0);
        instances.push((name.clone(), Arc::downgrade(&alive)));
    }
    let handler = move |input: Input, context: ToolContext| {
        let _alive = &alive;
        let (allowed_tools, inner) = (allowed_tools.clone(), Arc::clone(&inner));
        async move {
            let available = resolve_available_tools(&context, allowed_tools.as_deref());
            let functions = build_external_lookup(&available);
            let call = host_call(inner, context.clone());
            let (output, end) = run_code(
                input.code,
                functions,
                call,
                limits,
                timeout,
                context.cancel_signal(),
            )
            .await;
            let error = match end {
                RunEnd::Complete => {
                    let text = output.text();
                    return Ok(if text.is_empty() {
                        "(no output)".to_owned()
                    } else {
                        text
                    });
                }
                RunEnd::Cancelled => "Execution cancelled.".to_owned(),
                RunEnd::TimedOut => output.with_error(&format!(
                    "Execution error: timed out after {} seconds.",
                    timeout.unwrap_or_default().as_secs_f64()
                )),
                RunEnd::Syntax(error) => cap(&format!("Syntax error:\n{error}"), MAX_OUTPUT_CHARS),
                RunEnd::Runtime(error) => output.with_error(&format!("Execution error:\n{error}")),
            };
            Err(ToolError::msg(error))
        }
    };
    FunctionTool::typed(name, description, handler).with_input_schema(serde_json::json!({
        "type": "object",
        "properties": {
            "code": {
                "type": "string",
                "description": "Python code to execute. Use `await tool_name(...)` to call tools.",
            },
        },
        "required": ["code"],
        "additionalProperties": false,
    }))
}

/// The host function behind every tool function: one inner call, with its result as the value the
/// code receives or the text of the `RuntimeError` it raises.
fn host_call(inner: InnerCaller, parent: ToolContext) -> HostCall {
    Arc::new(move |name: String, input: JsonValue| {
        let (inner, parent) = (Arc::clone(&inner), parent.clone());
        async move {
            // A cancelled run makes no further real tool calls behind the model's back.
            if parent.cancel_signal().is_cancelled() {
                return Err(CANCELLED.to_owned());
            }
            let refusal = match inner(parent, name.clone(), input).await {
                Ok(InnerCall::Result(result)) => {
                    return if *result.status() == ToolResultStatus::Error {
                        let text = extract_text(result.content());
                        let text = if text.is_empty() { "Unknown error" } else { &text };
                        Err(format!("Tool '{name}' error: {text}"))
                    } else {
                        Ok(unwrap_result(&result))
                    };
                }
                Ok(InnerCall::NeedsApproval(reason)) => format!(
                    "Tool '{name}' needs approval ({reason}), which programmatic_tool_caller cannot prompt for; \
                     call it as a normal tool call instead."
                ),
                Ok(InnerCall::Cancelled) => CANCELLED.to_owned(),
                Err(message) => message,
            };
            Err(format!("Failed to execute tool '{name}': {refusal}"))
        }
        .boxed()
    })
}

fn extract_text(content: &[ToolResultContent]) -> String {
    let parts: Vec<String> = content
        .iter()
        .filter_map(|block| match block {
            ToolResultContent::Text(text) => Some(text.text().to_owned()),
            ToolResultContent::Json(json) => Some(json.json().to_string()),
            _ => None,
        })
        .collect();
    parts.join("\n")
}

/// The value the code receives for a tool result: a lone `json` block, or a lone text block that
/// is a JSON object or array, as data; otherwise the text blocks joined.
fn unwrap_result(result: &ToolResultBlock) -> JsonValue {
    let content = result.content();
    if let [ToolResultContent::Json(json)] = content {
        return json.json().clone();
    }
    let text = extract_text(content);
    if content.len() == 1
        && (text.starts_with('{') || text.starts_with('['))
        && let Ok(parsed) = serde_json::from_str(&text)
    {
        return parsed;
    }
    JsonValue::String(text)
}

/// The tools to expose to the code: every registered tool that is not a programmatic tool caller.
fn resolve_available_tools(context: &ToolContext, allowed_tools: Option<&[String]>) -> Vec<String> {
    let instances = INSTANCES.lock().unwrap_or_else(PoisonError::into_inner);
    context
        .agent()
        .tool_registry()
        .list()
        .map(|tool| tool.name().to_owned())
        .filter(|name| {
            !instances
                .iter()
                .any(|(caller, alive)| caller == name && alive.strong_count() > 0)
        })
        .filter(|name| allowed_tools.is_none_or(|allowed| allowed.contains(name)))
        .collect()
}

fn is_py_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// The functions handed to the sandbox, as guest name to tool name: one per tool, plus an alias
/// for each tool whose name is not a valid identifier, with every non-identifier character
/// replaced by `_`. An alias that would shadow a tool name or another tool's alias is dropped.
fn build_external_lookup(available: &[String]) -> HashMap<String, String> {
    let mut lookup: HashMap<String, String> = available
        .iter()
        .map(|name| (name.clone(), name.clone()))
        .collect();
    let mut aliases = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    let sorted: BTreeSet<&String> = available.iter().collect();
    for name in sorted {
        if is_py_identifier(name) {
            continue;
        }
        let alias: String = name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        if !is_py_identifier(&alias) || available.contains(&alias) {
            continue;
        }
        if aliases.contains_key(&alias) {
            ambiguous.insert(alias);
            continue;
        }
        aliases.insert(alias, name.clone());
    }
    for alias in ambiguous {
        aliases.remove(&alias);
        tracing::warn!("programmatic_tool_caller: multiple tools normalize to '{alias}', no alias injected");
    }
    lookup.extend(aliases);
    lookup
}
