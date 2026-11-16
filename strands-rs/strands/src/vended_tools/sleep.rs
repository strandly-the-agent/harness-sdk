//! The cooperative sleep tool (TS `vended-tools/sleep/`).

use std::time::Duration;

use serde::Deserialize;

use crate::JsonValue;
use crate::error::{Error, Result};
use crate::retry::model_retry_strategy::{Sleeper, tokio_sleeper};
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};

use super::shell::seconds_json;
use super::{js_number, schema_tool};

/// The default maximum sleep (TS `DEFAULT_MAX_DURATION`, 60 seconds).
pub const DEFAULT_MAX_DURATION: Duration = Duration::from_secs(60);

/// The default description, for the 60-second maximum (TS `SLEEP_DESCRIPTION`).
pub const SLEEP_DESCRIPTION: &str = "Pauses execution for a specified number of seconds (max 60). Cooperative and cancellable: the sleep aborts immediately when the agent invocation is cancelled. Rejects negative, NaN, infinite, or non-numeric durations, and durations above 60.";

/// The model-facing description for a maximum (TS `sleepDescription`).
pub(crate) fn sleep_description(max_duration: Duration) -> String {
    let max = js_number(max_duration.as_secs_f64());
    format!(
        "Pauses execution for a specified number of seconds (max {max}). Cooperative and cancellable: the sleep \
         aborts immediately when the agent invocation is cancelled. Rejects negative, NaN, infinite, or \
         non-numeric durations, and durations above {max}."
    )
}

/// Options for [`make_sleep`] (TS `MakeSleepOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct MakeSleepOptions {
    /// The longest sleep the model may request (default [`DEFAULT_MAX_DURATION`]).
    pub max_duration: Option<Duration>,
    /// The tool name (default `sleep`).
    pub name: Option<String>,
    /// The description (default [`SLEEP_DESCRIPTION`], with the configured maximum in place of 60).
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct SleepInput {
    duration: f64,
}

fn sleep_input_schema(max: Duration) -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "duration": {
                "type": "number",
                "minimum": 0,
                "maximum": seconds_json(max),
                "description": format!(
                    "Seconds to pause. Must be finite, non-negative, and no larger than {}.",
                    js_number(max.as_secs_f64())
                ),
            },
        },
        "required": ["duration"],
        "additionalProperties": false,
    })
}

/// A sleep tool (TS `makeSleep`). The sleep ends early, with an error, when the invocation is
/// cancelled.
///
/// # Errors
///
/// `max_duration` is zero.
pub fn make_sleep(options: MakeSleepOptions) -> Result<FunctionTool> {
    make_sleep_with(options, tokio_sleeper())
}

/// The default sleep tool (TS `sleep`).
pub fn sleep() -> FunctionTool {
    build(
        "sleep".to_owned(),
        sleep_description(DEFAULT_MAX_DURATION),
        DEFAULT_MAX_DURATION,
        tokio_sleeper(),
    )
}

/// [`make_sleep`] waiting on `sleeper`.
pub(crate) fn make_sleep_with(options: MakeSleepOptions, sleeper: Sleeper) -> Result<FunctionTool> {
    let max = options.max_duration.unwrap_or(DEFAULT_MAX_DURATION);
    if max.is_zero() {
        return Err(Error::other(format!(
            "maxDuration must be a positive, finite number, got {}",
            js_number(max.as_secs_f64())
        )));
    }
    let name = options.name.unwrap_or_else(|| "sleep".to_owned());
    let description = options.description.unwrap_or_else(|| sleep_description(max));
    Ok(build(name, description, max, sleeper))
}

fn build(name: String, description: String, max: Duration, sleeper: Sleeper) -> FunctionTool {
    let max_secs = max.as_secs_f64();
    let schema = sleep_input_schema(max);
    let precheck = move |input: &JsonValue| match input.get("duration").and_then(JsonValue::as_f64) {
        Some(duration) if duration < 0.0 => Err(ToolError::msg(
            "Invalid input at /duration: duration must be non-negative",
        )),
        Some(duration) if duration > max_secs => Err(ToolError::msg(format!(
            "Invalid input at /duration: duration exceeds maximum of {} seconds",
            js_number(max_secs)
        ))),
        _ => Ok(()),
    };
    schema_tool(
        name,
        description,
        schema.clone(),
        InputValidator::new(&schema),
        precheck,
        move |input: SleepInput, context: ToolContext| {
            let sleeper = sleeper.clone();
            async move {
                let signal = context.cancel_signal().clone();
                if signal.is_cancelled() {
                    return Err(abort_error(signal.reason(), "Sleep cancelled before it started"));
                }
                let delay = Duration::try_from_secs_f64(input.duration).map_err(ToolError::from)?;
                tokio::select! {
                    () = sleeper(delay) => Ok(format!("Slept for {} seconds", js_number(input.duration))),
                    () = signal.cancelled() => Err(abort_error(signal.reason(), "Sleep cancelled")),
                }
            }
        },
    )
}

/// The cancel reason, else TS's `AbortError` message.
fn abort_error(reason: Option<String>, fallback: &str) -> ToolError {
    ToolError::msg(reason.unwrap_or_else(|| fallback.to_owned()))
}

#[cfg(test)]
mod tests;
