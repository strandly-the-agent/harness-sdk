//! The `sleep` tool (`strands-ts/src/vended-tools/sleep/{make-sleep.ts,sleep.ts,types.ts}`).
//!
//! Pauses for a bounded, cooperative duration. The wait goes through a [`Sleeper`] — the same
//! injectable sleeper the retry strategies use — so tests can drive it without waiting, and it
//! races [`ToolContext::cancel`] so a cancelled tool call aborts the sleep immediately.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::retry::{Sleeper, TokioSleeper};
use crate::tools::tool::{create_error_result, Tool, ToolContext};
use crate::tools::wrap_in_tool_result;
use crate::types::{ToolResultBlock, ToolSpec};

/// Name of the sleep tool.
pub const SLEEP_TOOL_NAME: &str = "sleep";

/// Default upper bound on `duration`, in seconds (`DEFAULT_MAX_DURATION` in TS).
pub const DEFAULT_MAX_DURATION: f64 = 60.0;

/// Model-facing description with `max_duration` interpolated (`sleepDescription` in TS).
pub fn sleep_description(max_duration: f64) -> String {
    format!(
        "Pauses execution for a specified number of seconds (max {max_duration}). Cooperative and \
         cancellable: the sleep aborts immediately when the agent invocation is cancelled. Rejects \
         negative, NaN, infinite, or non-numeric durations, and durations above {max_duration}."
    )
}

/// Configuration of a [`SleepTool`].
#[derive(Clone)]
#[non_exhaustive]
pub struct SleepConfig {
    /// Tool name reported to the model.
    pub name: String,
    /// Model-facing description; [`SleepConfig::new`] derives it with [`sleep_description`].
    pub description: String,
    /// Upper bound on the `duration` input, in seconds (TS default: 60).
    pub max_duration: f64,
    /// How the tool waits. Default [`TokioSleeper`]; swap it out in tests.
    pub sleeper: Arc<dyn Sleeper>,
}

impl SleepConfig {
    /// Configuration with the given duration cap and the description derived from it.
    pub fn new(max_duration: f64) -> Self {
        Self {
            name: SLEEP_TOOL_NAME.to_owned(),
            description: sleep_description(max_duration),
            max_duration,
            sleeper: Arc::new(TokioSleeper),
        }
    }
}

impl Default for SleepConfig {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DURATION)
    }
}

impl std::fmt::Debug for SleepConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SleepConfig")
            .field("name", &self.name)
            .field("max_duration", &self.max_duration)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
struct SleepInput {
    duration: f64,
}

/// The `sleep` tool: pauses for the requested number of seconds, cancellation-aware.
#[derive(Debug)]
pub struct SleepTool {
    config: SleepConfig,
    spec: ToolSpec,
}

impl Default for SleepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SleepTool {
    /// Sleep tool with the default configuration (60 second cap, real sleeps).
    pub fn new() -> Self {
        Self::with_config(SleepConfig::default()).expect("the default max duration is valid")
    }

    /// Sleep tool with an explicit configuration.
    ///
    /// Errors with [`Error::ToolValidation`] when `max_duration` is not finite and positive, as
    /// the TS factory throws.
    pub fn with_config(config: SleepConfig) -> Result<Self> {
        if !(config.max_duration.is_finite() && config.max_duration > 0.0) {
            return Err(Error::ToolValidation(format!(
                "maxDuration must be a positive, finite number, got {}",
                config.max_duration
            )));
        }
        let spec = sleep_tool_spec(&config);
        Ok(Self { config, spec })
    }

    /// Pause for `duration` seconds, returning the TS success message.
    ///
    /// Rejects a negative, NaN, infinite or oversized `duration` with the TS message.
    pub async fn execute(&self, duration: f64) -> Result<String> {
        self.validate(duration)?;
        self.config.sleeper.sleep(Duration::from_secs_f64(duration)).await;
        Ok(format!("Slept for {duration} seconds"))
    }

    fn validate(&self, duration: f64) -> Result<()> {
        if !duration.is_finite() {
            return Err(Error::ToolValidation(
                "duration must be a finite number".to_owned(),
            ));
        }
        if duration < 0.0 {
            return Err(Error::ToolValidation("duration must be non-negative".to_owned()));
        }
        if duration > self.config.max_duration {
            return Err(Error::ToolValidation(format!(
                "duration exceeds maximum of {} seconds",
                self.config.max_duration
            )));
        }
        Ok(())
    }
}

/// Spec of the sleep tool, with `max_duration` interpolated into the schema and description.
fn sleep_tool_spec(config: &SleepConfig) -> ToolSpec {
    let max_duration = config.max_duration;
    ToolSpec {
        name: config.name.clone(),
        description: config.description.clone(),
        input_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "duration": {
                    "type": "number",
                    "minimum": 0,
                    "maximum": max_duration,
                    "description": format!(
                        "Seconds to pause. Must be finite, non-negative, and no larger than {max_duration}."
                    ),
                },
            },
            "required": ["duration"],
            "additionalProperties": false,
        })),
        output_schema: None,
        annotations: None,
    }
}

#[async_trait]
impl Tool for SleepTool {
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
        let input: SleepInput = ctx.input()?;
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        let slept = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => return Err(Error::Cancelled),
            slept = self.execute(input.duration) => slept,
        };

        match slept {
            Ok(message) => Ok(wrap_in_tool_result(
                serde_json::json!(message),
                &ctx.tool_use.tool_use_id,
            )),
            Err(err) => Ok(create_error_result(err, &ctx.tool_use.tool_use_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool::test_context;
    use crate::types::ToolResultStatus;
    use parking_lot::Mutex;

    /// Sleeper that records what it was asked to wait for and returns immediately.
    #[derive(Debug, Default)]
    struct RecordingSleeper {
        slept: Mutex<Vec<Duration>>,
    }

    #[async_trait]
    impl Sleeper for RecordingSleeper {
        async fn sleep(&self, duration: Duration) {
            self.slept.lock().push(duration);
        }
    }

    fn instant_tool(max_duration: f64) -> (SleepTool, Arc<RecordingSleeper>) {
        let sleeper = Arc::new(RecordingSleeper::default());
        let tool = SleepTool::with_config(SleepConfig {
            sleeper: Arc::clone(&sleeper) as Arc<dyn Sleeper>,
            ..SleepConfig::new(max_duration)
        })
        .unwrap();
        (tool, sleeper)
    }

    fn ctx(input: serde_json::Value) -> ToolContext {
        test_context(SLEEP_TOOL_NAME, "test-id", input)
    }

    async fn error_message(duration: serde_json::Value) -> String {
        let (tool, _) = instant_tool(DEFAULT_MAX_DURATION);
        let result = tool
            .invoke(ctx(serde_json::json!({ "duration": duration })))
            .await;
        match result {
            Ok(result) => {
                assert_eq!(result.status, ToolResultStatus::Error);
                result.text()
            }
            Err(err) => err.to_string(),
        }
    }

    #[tokio::test]
    async fn rejects_negative_durations() {
        assert!(error_message(serde_json::json!(-0.1))
            .await
            .contains("non-negative"));
    }

    #[tokio::test]
    async fn rejects_nan_durations() {
        let (tool, _) = instant_tool(DEFAULT_MAX_DURATION);
        let err = tool.execute(f64::NAN).await.unwrap_err();
        assert!(err.to_string().contains("finite"), "{err}");
    }

    #[tokio::test]
    async fn rejects_positive_infinity() {
        let (tool, _) = instant_tool(DEFAULT_MAX_DURATION);
        let err = tool.execute(f64::INFINITY).await.unwrap_err();
        assert!(err.to_string().contains("finite"), "{err}");
    }

    #[tokio::test]
    async fn rejects_negative_infinity() {
        let (tool, _) = instant_tool(DEFAULT_MAX_DURATION);
        let err = tool.execute(f64::NEG_INFINITY).await.unwrap_err();
        assert!(err.to_string().contains("finite"), "{err}");
    }

    #[tokio::test]
    async fn rejects_non_numeric_input_via_schema() {
        assert!(!error_message(serde_json::json!("one")).await.is_empty());
    }

    #[tokio::test]
    async fn rejects_boolean_input_via_schema() {
        assert!(!error_message(serde_json::json!(true)).await.is_empty());
        assert!(!error_message(serde_json::json!(false)).await.is_empty());
    }

    #[tokio::test]
    async fn rejects_duration_above_default_maximum() {
        assert!(error_message(serde_json::json!(DEFAULT_MAX_DURATION + 1.0))
            .await
            .contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn rejects_duration_above_configured_maximum() {
        let (tool, _) = instant_tool(0.5);
        let err = tool.execute(0.6).await.unwrap_err();
        assert_eq!(err.to_string(), "duration exceeds maximum of 0.5 seconds");
    }

    #[test]
    fn rejects_zero_max_duration() {
        let err = SleepTool::with_config(SleepConfig {
            max_duration: 0.0,
            ..SleepConfig::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("positive"), "{err}");
    }

    #[test]
    fn rejects_negative_max_duration() {
        let err = SleepTool::with_config(SleepConfig {
            max_duration: -1.0,
            ..SleepConfig::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("positive"), "{err}");
    }

    #[test]
    fn rejects_nan_max_duration() {
        let err = SleepTool::with_config(SleepConfig {
            max_duration: f64::NAN,
            ..SleepConfig::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("positive"), "{err}");
    }

    #[test]
    fn rejects_infinity_max_duration() {
        let err = SleepTool::with_config(SleepConfig {
            max_duration: f64::INFINITY,
            ..SleepConfig::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("positive"), "{err}");
    }

    #[tokio::test]
    async fn aborts_immediately_when_cancel_fires_mid_sleep() {
        let tool = SleepTool::new();
        let context = ctx(serde_json::json!({ "duration": 5 }));
        let cancel = context.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });

        let started = std::time::Instant::now();
        let err = tool.invoke(context).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn rejects_immediately_when_the_token_is_already_cancelled() {
        let context = ctx(serde_json::json!({ "duration": 1 }));
        context.cancel.cancel();
        let err = SleepTool::new().invoke(context).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err}");
    }

    #[tokio::test]
    async fn returns_the_expected_message_after_a_short_sleep() {
        let (tool, sleeper) = instant_tool(DEFAULT_MAX_DURATION);
        assert_eq!(tool.execute(0.05).await.unwrap(), "Slept for 0.05 seconds");
        assert_eq!(sleeper.slept.lock().as_slice(), [Duration::from_millis(50)]);
    }

    #[tokio::test]
    async fn accepts_zero_duration() {
        let (tool, _) = instant_tool(DEFAULT_MAX_DURATION);
        assert_eq!(tool.execute(0.0).await.unwrap(), "Slept for 0 seconds");
    }

    #[tokio::test]
    async fn drives_the_sleeper_with_the_requested_duration() {
        let (tool, sleeper) = instant_tool(DEFAULT_MAX_DURATION);
        let result = tool
            .invoke(ctx(serde_json::json!({ "duration": 2 })))
            .await
            .unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(result.text(), "Slept for 2 seconds");
        assert_eq!(sleeper.slept.lock().as_slice(), [Duration::from_secs(2)]);
    }

    #[test]
    fn spec_matches_the_ts_name_description_and_schema() {
        let tool = SleepTool::new();
        assert_eq!(tool.name(), "sleep");
        assert_eq!(
            tool.description(),
            "Pauses execution for a specified number of seconds (max 60). Cooperative and \
             cancellable: the sleep aborts immediately when the agent invocation is cancelled. \
             Rejects negative, NaN, infinite, or non-numeric durations, and durations above 60."
        );
        assert_eq!(
            tool.spec().input_schema,
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "duration": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": 60.0,
                        "description": "Seconds to pause. Must be finite, non-negative, and no larger than 60.",
                    },
                },
                "required": ["duration"],
                "additionalProperties": false,
            }))
        );
    }

    #[test]
    fn a_configured_maximum_is_interpolated_into_the_description() {
        let (tool, _) = instant_tool(5.0);
        assert!(tool.description().contains("(max 5)"), "{}", tool.description());
        assert!(tool.description().contains("above 5."));
    }
}
