//! Model stream data types (TS `models/streaming.ts`).

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::types::json::JsonValue;

/// Token usage for a model call (TS `Usage`).
///
/// Counters are `u64` (JSON integers never gain `.0`) and accumulate with saturating arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_write_input_tokens: Option<u64>,
}

impl Usage {
    /// Usage with these input, output and total token counts.
    pub fn new(input_tokens: u64, output_tokens: u64, total_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            ..Self::default()
        }
    }

    /// Sets the tokens read from the prompt cache.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    /// Sets the tokens written to the prompt cache.
    #[must_use]
    pub fn with_cache_write_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_write_input_tokens = Some(tokens);
        self
    }

    /// Input tokens.
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens
    }

    /// Output tokens.
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens
    }

    /// Total tokens.
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    /// Tokens read from the prompt cache.
    pub fn cache_read_input_tokens(&self) -> Option<u64> {
        self.cache_read_input_tokens
    }

    /// Tokens written to the prompt cache.
    pub fn cache_write_input_tokens(&self) -> Option<u64> {
        self.cache_write_input_tokens
    }

    /// Adds `other` into `self` (TS `accumulateUsage`); cache counters appear once either side has them.
    pub fn accumulate(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        if let Some(tokens) = other.cache_read_input_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0).saturating_add(tokens));
        }
        if let Some(tokens) = other.cache_write_input_tokens {
            self.cache_write_input_tokens =
                Some(self.cache_write_input_tokens.unwrap_or(0).saturating_add(tokens));
        }
    }

    /// The prompt size in tokens, including cached tokens (TS `totalPromptTokens`).
    ///
    /// Providers that count cache tokens inside `inputTokens` report `input + output == total`;
    /// for them the input count is already the full prompt. Otherwise cache tokens are added.
    pub fn total_prompt_tokens(&self) -> u64 {
        if self.input_tokens.saturating_add(self.output_tokens) == self.total_tokens {
            return self.input_tokens;
        }
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens.unwrap_or(0))
            .saturating_add(self.cache_write_input_tokens.unwrap_or(0))
    }
}

/// Performance metrics for a model call (TS `Metrics`), in whole milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Metrics {
    #[serde(deserialize_with = "whole_ms")]
    latency_ms: u64,
    #[serde(
        default,
        deserialize_with = "whole_ms_opt",
        skip_serializing_if = "Option::is_none"
    )]
    time_to_first_byte_ms: Option<u64>,
}

/// Reads milliseconds written as an integer or, by other writers, as a fraction (rounded).
fn whole_ms<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = JsonValue::deserialize(deserializer)?;
    if let Some(ms) = value.as_u64() {
        return Ok(ms);
    }
    match value.as_f64() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked finite and non-negative"
        )]
        Some(ms) if ms.is_finite() && ms >= 0.0 => Ok(ms.round() as u64),
        _ => Err(D::Error::custom(format!("invalid milliseconds {value}"))),
    }
}

fn whole_ms_opt<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    whole_ms(deserializer).map(Some)
}

impl Metrics {
    /// Metrics with this latency.
    pub fn new(latency_ms: u64) -> Self {
        Self {
            latency_ms,
            time_to_first_byte_ms: None,
        }
    }

    /// Sets the time to the first byte of the response.
    #[must_use]
    pub fn with_time_to_first_byte_ms(mut self, time_to_first_byte_ms: u64) -> Self {
        self.time_to_first_byte_ms = Some(time_to_first_byte_ms);
        self
    }

    /// End-to-end latency in milliseconds.
    pub fn latency_ms(&self) -> u64 {
        self.latency_ms
    }

    /// Time to the first response byte in milliseconds.
    pub fn time_to_first_byte_ms(&self) -> Option<u64> {
        self.time_to_first_byte_ms
    }
}

/// Ported from strands-ts/src/models/__tests__/streaming.test.ts (the `Usage` parts).
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    mod total_prompt_tokens {
        use super::*;

        #[test]
        fn counts_cache_tokens_on_disjoint_providers_where_they_add_to_input_tokens() {
            assert_eq!(
                Usage::new(10, 4, 5862)
                    .with_cache_read_input_tokens(5848)
                    .total_prompt_tokens(),
                5858
            );
        }

        #[test]
        fn does_not_double_count_cache_tokens_on_subset_providers_where_they_sit_inside_input_tokens() {
            assert_eq!(
                Usage::new(12936, 10, 12946)
                    .with_cache_read_input_tokens(6457)
                    .total_prompt_tokens(),
                12936
            );
        }

        #[test]
        fn counts_the_cache_read_from_an_anthropic_direct_payload_whose_total_tokens_includes_the_cache() {
            assert_eq!(
                Usage::new(10, 4, 5862)
                    .with_cache_read_input_tokens(5848)
                    .total_prompt_tokens(),
                5858
            );
        }

        #[test]
        fn adds_cache_reads_and_writes_on_disjoint_providers() {
            let usage = Usage::new(10, 5, 100)
                .with_cache_read_input_tokens(60)
                .with_cache_write_input_tokens(25);
            assert_eq!(usage.total_prompt_tokens(), 95);
        }

        #[test]
        fn collapses_to_input_tokens_when_there_are_no_cache_tokens() {
            assert_eq!(Usage::new(100, 50, 150).total_prompt_tokens(), 100);
        }
    }

    #[test]
    fn accumulate_adds_counters_and_starts_cache_counters_when_seen() {
        let mut total = Usage::default();
        total.accumulate(&Usage::new(10, 5, 15));
        assert_eq!(total.cache_read_input_tokens(), None);
        total.accumulate(
            &Usage::new(20, 10, 30)
                .with_cache_read_input_tokens(8)
                .with_cache_write_input_tokens(2),
        );
        assert_eq!(
            serde_json::to_value(total).unwrap(),
            json!({"inputTokens": 30, "outputTokens": 15, "totalTokens": 45, "cacheReadInputTokens": 8, "cacheWriteInputTokens": 2})
        );
    }

    #[test]
    fn accumulate_saturates_instead_of_overflowing() {
        let mut total = Usage::new(u64::MAX, 1, u64::MAX - 1).with_cache_read_input_tokens(u64::MAX);
        total.accumulate(&Usage::new(1, 1, 5).with_cache_read_input_tokens(1));
        assert_eq!(
            (total.input_tokens(), total.output_tokens(), total.total_tokens()),
            (u64::MAX, 2, u64::MAX)
        );
        assert_eq!(total.cache_read_input_tokens(), Some(u64::MAX));
        assert_eq!(total.total_prompt_tokens(), u64::MAX);
    }

    #[test]
    fn fractional_milliseconds_from_other_writers_load_rounded() {
        let metrics: Metrics =
            serde_json::from_value(json!({"latencyMs": 812.6, "timeToFirstByteMs": 95})).unwrap();
        assert_eq!(
            (metrics.latency_ms(), metrics.time_to_first_byte_ms()),
            (813, Some(95))
        );
        assert!(serde_json::from_value::<Metrics>(json!({"latencyMs": -1})).is_err());
    }

    #[test]
    fn counters_serialize_as_integers() {
        let usage = Usage::new(0, 4_294_967_296, 9_007_199_254_740_991);
        assert_eq!(
            serde_json::to_string(&usage).unwrap(),
            r#"{"inputTokens":0,"outputTokens":4294967296,"totalTokens":9007199254740991}"#
        );
        let metrics = Metrics::new(812).with_time_to_first_byte_ms(95);
        assert_eq!(
            serde_json::to_string(&metrics).unwrap(),
            r#"{"latencyMs":812,"timeToFirstByteMs":95}"#
        );
    }
}
