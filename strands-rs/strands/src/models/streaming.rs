//! Model stream event types, `Usage` and `Metrics` (TS `models/streaming.ts`).
//!
//! Counters are `u64`, so they serialize without a fractional part like TS integers do.

use serde::{Deserialize, Serialize};

/// Token usage of a model call (TS `Usage`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Total tokens.
    pub total_tokens: u64,
    /// Input tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Input tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
}

impl Usage {
    /// Creates usage from the input, output and total token counts.
    #[must_use]
    pub fn new(input_tokens: u64, output_tokens: u64, total_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            ..Self::default()
        }
    }

    /// Sets the cache-read input tokens.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    /// Sets the cache-write input tokens.
    #[must_use]
    pub fn with_cache_write_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_write_input_tokens = Some(tokens);
        self
    }

    /// Adds `source` into `self` with saturating arithmetic (TS `accumulateUsage`). A cache counter
    /// is only touched when `source` has it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by stream aggregation and agent metrics")
    )]
    pub(crate) fn accumulate(&mut self, source: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(source.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(source.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(source.total_tokens);
        if let Some(read) = source.cache_read_input_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0).saturating_add(read));
        }
        if let Some(write) = source.cache_write_input_tokens {
            self.cache_write_input_tokens =
                Some(self.cache_write_input_tokens.unwrap_or(0).saturating_add(write));
        }
    }

    /// The prompt size including cache tokens, under either provider convention (TS
    /// `totalPromptTokens`): when input plus output already equals the total, the cache tokens sit
    /// inside `input_tokens`; otherwise they are added to it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the conversation and context managers")
    )]
    pub(crate) fn total_prompt_tokens(&self) -> u64 {
        if self.input_tokens.saturating_add(self.output_tokens) == self.total_tokens {
            return self.input_tokens;
        }
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens.unwrap_or(0))
            .saturating_add(self.cache_write_input_tokens.unwrap_or(0))
    }
}

/// Performance metrics of a model call (TS `Metrics`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    /// Latency in milliseconds.
    pub latency_ms: u64,
    /// Time to the first byte in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_byte_ms: Option<u64>,
}

impl Metrics {
    /// Creates metrics from a latency in milliseconds.
    #[must_use]
    pub fn new(latency_ms: u64) -> Self {
        Self {
            latency_ms,
            time_to_first_byte_ms: None,
        }
    }

    /// Sets the time to the first byte in milliseconds.
    #[must_use]
    pub fn with_time_to_first_byte_ms(mut self, time_to_first_byte_ms: u64) -> Self {
        self.time_to_first_byte_ms = Some(time_to_first_byte_ms);
        self
    }
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/models/__tests__/streaming.test.ts

    use serde_json::json;

    use super::*;

    #[test]
    fn counts_cache_tokens_on_disjoint_providers_where_they_add_to_input_tokens() {
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        assert_eq!(usage.total_prompt_tokens(), 5858);
    }

    #[test]
    fn does_not_double_count_cache_tokens_on_subset_providers_where_they_sit_inside_input_tokens() {
        let usage = Usage::new(12936, 10, 12946).with_cache_read_input_tokens(6457);
        assert_eq!(usage.total_prompt_tokens(), 12936);
    }

    #[test]
    fn counts_the_cache_read_from_an_anthropic_direct_payload_whose_total_tokens_includes_the_cache() {
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        assert_eq!(usage.total_prompt_tokens(), 5858);
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

    #[test]
    fn accumulate_adds_counters_and_only_touches_cache_counters_the_source_has() {
        let mut total = Usage::default();
        total.accumulate(&Usage::new(1, 2, 3));
        assert_eq!(total, Usage::new(1, 2, 3));
        total.accumulate(
            &Usage::new(3, 3, 6)
                .with_cache_read_input_tokens(2)
                .with_cache_write_input_tokens(5),
        );
        assert_eq!(
            serde_json::to_value(total).unwrap(),
            json!({ "inputTokens": 4, "outputTokens": 5, "totalTokens": 9, "cacheReadInputTokens": 2, "cacheWriteInputTokens": 5 })
        );
    }

    #[test]
    fn accumulate_saturates() {
        let mut total = Usage::new(u64::MAX, 0, u64::MAX).with_cache_read_input_tokens(u64::MAX);
        total.accumulate(&Usage::new(1, 1, 1).with_cache_read_input_tokens(1));
        assert_eq!(
            total,
            Usage::new(u64::MAX, 1, u64::MAX).with_cache_read_input_tokens(u64::MAX)
        );
    }

    #[test]
    fn usage_and_metrics_write_integers_and_omit_unset_fields() {
        assert_eq!(
            serde_json::to_string(&Usage::new(9_007_199_254_740_991, 0, 9_007_199_254_740_991)).unwrap(),
            r#"{"inputTokens":9007199254740991,"outputTokens":0,"totalTokens":9007199254740991}"#
        );
        assert_eq!(
            serde_json::to_string(&Metrics::new(0)).unwrap(),
            r#"{"latencyMs":0}"#
        );
        assert_eq!(
            serde_json::to_string(&Metrics::new(1234).with_time_to_first_byte_ms(56)).unwrap(),
            r#"{"latencyMs":1234,"timeToFirstByteMs":56}"#
        );
    }
}
