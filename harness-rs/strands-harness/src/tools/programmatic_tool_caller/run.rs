//! Drives one Monty run: interpreter steps on `spawn_blocking`, host functions awaited between them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use monty::{MontyRun, RunProgress};
use monty_types::{
    CallArgs, CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, NameLookupResult,
    ObjectRef, PrintWriter, ResourceLimits, ResourceTracker,
};
use strands::{CancelSignal, JsonValue};
use tokio::sync::Semaphore;
use tokio::time::Instant;

use super::output::Output;

pub(crate) const USER_CODE_FILENAME: &str = "<programmatic_tool_caller>";

/// The limits the Monty VM enforces (TS `LIMITS`). The duration counts interpreter time only: the
/// clock stops while a tool call is in flight.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) max_duration: Duration,
    pub(crate) max_memory: usize,
    pub(crate) max_suspensions: usize,
}

pub(crate) const LIMITS: Limits = Limits {
    max_duration: Duration::from_secs(60),
    max_memory: 256 * 1024 * 1024,
    max_suspensions: 1_000,
};

/// Inner tool calls in flight at once per run (TS `MAX_CONCURRENT_TOOL_CALLS`).
pub(crate) const MAX_CONCURRENT_TOOL_CALLS: usize = 10;

/// A host function: the tool's registry name and its input; `Err` is the guest `RuntimeError` text.
pub(crate) type HostCall =
    Arc<dyn Fn(String, JsonValue) -> BoxFuture<'static, Result<JsonValue, String>> + Send + Sync>;

/// How a run ended.
#[derive(Debug)]
pub(crate) enum RunEnd {
    Complete,
    Cancelled,
    TimedOut,
    Syntax(MontyException),
    Runtime(MontyException),
}

type Step = Result<RunProgress, MontyException>;

/// Runs one interpreter step off the async threads; the VM's own limits bound how long it takes.
async fn step(
    mut output: Output,
    run: impl FnOnce(PrintWriter<'_>) -> Step + Send + 'static,
) -> (Step, Output) {
    let joined = tokio::task::spawn_blocking(move || {
        let progress = run(PrintWriter::Callback(&mut output));
        (progress, output)
    })
    .await;
    joined.unwrap_or_else(|error| {
        (
            Err(MontyException::runtime_error(format!(
                "the interpreter step failed: {error}"
            ))),
            Output::default(),
        )
    })
}

/// Runs `code` with `functions` (guest name to tool name) callable as async functions.
pub(crate) async fn run_code(
    code: String,
    functions: HashMap<String, String>,
    call: HostCall,
    limits: Limits,
    timeout: Option<Duration>,
    cancel: &CancelSignal,
) -> (Output, RunEnd) {
    if cancel.is_cancelled() {
        return (Output::default(), RunEnd::Cancelled);
    }
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    let run = match MontyRun::new(code, USER_CODE_FILENAME, Vec::new(), CompileOptions::default()) {
        Ok(run) => run,
        Err(error) => return (Output::default(), RunEnd::Syntax(error)),
    };
    let tracker = ResourceTracker::new(ResourceLimits {
        max_feed_duration: Some(limits.max_duration),
        max_memory: Some(limits.max_memory),
        max_suspensions: limits.max_suspensions,
        ..ResourceLimits::default()
    });
    let slots = Arc::new(Semaphore::new(MAX_CONCURRENT_TOOL_CALLS));
    let mut pending = FuturesUnordered::new();
    let mut suspensions = 0_usize;
    let (mut progress, mut output) = step(Output::default(), move |print| {
        run.start(Vec::new(), tracker, print)
    })
    .await;
    loop {
        let suspended = match progress {
            Ok(RunProgress::Complete(_)) => return (output, RunEnd::Complete),
            Ok(suspended) => suspended,
            Err(error) => return (output, RunEnd::Runtime(error)),
        };
        if cancel.is_cancelled() {
            return (output, RunEnd::Cancelled);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return (output, RunEnd::TimedOut);
        }
        suspensions += 1;
        let over_budget = (suspensions > limits.max_suspensions).then(|| {
            MontyException::runtime_error(format!(
                "suspension limit exceeded: more than {} host round trips",
                limits.max_suspensions
            ))
        });
        (progress, output) = match (suspended, over_budget) {
            (RunProgress::Complete(_), _) => return (output, RunEnd::Complete),
            (RunProgress::FunctionCall(call), Some(error)) => {
                step(output, move |print| call.abort(error, print)).await
            }
            (RunProgress::NameLookup(lookup), Some(error)) => {
                step(output, move |print| lookup.abort(error, print)).await
            }
            (RunProgress::OsCall(call), Some(error)) => {
                step(output, move |print| call.abort(error, print)).await
            }
            (RunProgress::ResolveFutures(futures), Some(error)) => {
                step(output, move |print| futures.abort(error, print)).await
            }
            (RunProgress::NameLookup(lookup), None) => {
                let answer = match functions.get(&lookup.name) {
                    Some(tool_name) => {
                        NameLookupResult::Value(MontyObject::function(tool_name.clone(), None))
                    }
                    None => NameLookupResult::Undefined,
                };
                step(output, move |print| lookup.resume(answer, print)).await
            }
            (RunProgress::FunctionCall(function), None) => {
                // A call names the function either as the code wrote it or, for a function object
                // a name lookup handed out, by the tool's own name; anything else does not exist.
                let called = &function.function_name;
                let exposed = functions
                    .get(called)
                    .or_else(|| functions.values().find(|tool_name| *tool_name == called))
                    .cloned();
                let Some(name) = exposed else {
                    let answer = ExtFunctionResult::NotFound(called.clone());
                    (progress, output) = step(output, move |print| function.resume(answer, print)).await;
                    continue;
                };
                match keyword_input(&function.args) {
                    Some(input) => {
                        let (call, slots, call_id) =
                            (Arc::clone(&call), Arc::clone(&slots), function.call_id);
                        pending.push(
                            async move {
                                let _slot = slots.acquire().await;
                                (call_id, call(name, input).await)
                            }
                            .boxed(),
                        );
                        step(output, move |print| function.resume_pending(print)).await
                    }
                    None => {
                        let error = MontyException::new(
                            ExcType::TypeError,
                            Some(format!(
                                "{name}() takes keyword arguments only, e.g. {name}(key=value)"
                            )),
                        );
                        step(output, move |print| function.resume(error, print)).await
                    }
                }
            }
            (RunProgress::OsCall(call), None) => {
                // No host operation is implemented: files, the environment and the network don't exist.
                let answer = call.function_call.no_handler_result();
                step(output, move |print| call.resume(answer, print)).await
            }
            (RunProgress::ResolveFutures(futures), None) => {
                let first = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return (output, RunEnd::Cancelled),
                    () = wait_until(deadline) => return (output, RunEnd::TimedOut),
                    first = pending.next() => first,
                };
                let Some(first) = first else {
                    let error = MontyException::runtime_error("awaited a host call that is not in flight");
                    (progress, output) = step(output, move |print| futures.abort(error, print)).await;
                    continue;
                };
                let mut results = vec![first];
                while let Some(Some(next)) = pending.next().now_or_never() {
                    results.push(next);
                }
                let results = results
                    .into_iter()
                    .map(|(call_id, result)| (call_id, host_result(result)))
                    .collect();
                step(output, move |print| futures.resume(results, print)).await
            }
        };
    }
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn host_result(result: Result<JsonValue, String>) -> ExtFunctionResult {
    match result {
        Ok(value) => ExtFunctionResult::Return(to_guest(&value)),
        Err(message) => ExtFunctionResult::Error(MontyException::new(ExcType::RuntimeError, Some(message))),
    }
}

/// The tool input for a call's arguments: the keyword arguments as one object, or `None` when
/// positional arguments were passed.
fn keyword_input(args: &CallArgs) -> Option<JsonValue> {
    if args.args().len() > 0 {
        return None;
    }
    let input = args
        .kwargs()
        .map(|(key, value)| (key_text(&key), from_guest(&value)))
        .collect();
    Some(JsonValue::Object(input))
}

fn key_text(key: &ObjectRef<'_>) -> String {
    key.as_str().map_or_else(|| key.to_string(), str::to_owned)
}

fn to_guest(value: &JsonValue) -> MontyObject {
    match value {
        JsonValue::Null => MontyObject::none(),
        JsonValue::Bool(value) => MontyObject::bool(*value),
        JsonValue::Number(number) => match number.as_i64() {
            Some(int) => MontyObject::int(int),
            None => MontyObject::float(number.as_f64().unwrap_or(f64::NAN)),
        },
        JsonValue::String(text) => MontyObject::string(text.as_str()),
        JsonValue::Array(items) => MontyObject::list(items.iter().map(to_guest)),
        JsonValue::Object(entries) => MontyObject::dict(
            entries
                .iter()
                .map(|(key, value)| (MontyObject::string(key.as_str()), to_guest(value))),
        ),
    }
}

/// A guest value as JSON; a value JSON cannot express becomes its `str()`.
fn from_guest(value: &ObjectRef<'_>) -> JsonValue {
    match value.type_name() {
        "NoneType" => JsonValue::Null,
        "bool" => value.as_bool().map_or(JsonValue::Null, JsonValue::Bool),
        "int" => value
            .as_int()
            .map_or_else(|| JsonValue::String(value.to_string()), JsonValue::from),
        "float" => value
            .as_float()
            .and_then(serde_json::Number::from_f64)
            .map_or(JsonValue::Null, JsonValue::Number),
        "list" | "tuple" => {
            JsonValue::Array(value.items().unwrap_or_default().iter().map(from_guest).collect())
        }
        "dict" => JsonValue::Object(
            value
                .pairs()
                .unwrap_or_default()
                .iter()
                .map(|(key, value)| (key_text(key), from_guest(value)))
                .collect(),
        ),
        _ => JsonValue::String(value.as_str().map_or_else(|| value.to_string(), str::to_owned)),
    }
}
