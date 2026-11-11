//! Model streaming types (TS `models/streaming.ts`).

use serde::{Deserialize, Serialize};

/// Token usage of a model call (TS `Usage`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Usage {
    /// Input tokens, as the provider counts them (cached tokens may or may not be included).
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// All tokens processed, including cached input tokens.
    pub total_tokens: u64,
    /// Input tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Input tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
}

impl Usage {
    /// Usage with the three required counters.
    pub fn new(input_tokens: u64, output_tokens: u64, total_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
        }
    }

    /// Sets the cache-read token count.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    /// Sets the cache-write token count.
    #[must_use]
    pub fn with_cache_write_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_write_input_tokens = Some(tokens);
        self
    }

    /// Adds `other` into `self`, saturating (TS `accumulateUsage`).
    ///
    /// A cache counter is set only once some accumulated usage reports it.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop and telemetry"))]
    pub(crate) fn accumulate(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        if let Some(read) = other.cache_read_input_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0).saturating_add(read));
        }
        if let Some(write) = other.cache_write_input_tokens {
            self.cache_write_input_tokens =
                Some(self.cache_write_input_tokens.unwrap_or(0).saturating_add(write));
        }
    }

    /// Prompt tokens including cache reads and writes, counted once (TS `totalPromptTokens`).
    ///
    /// Providers whose `input_tokens` already include cached tokens report
    /// `input + output == total`; for them `input_tokens` is the answer. Otherwise the cache
    /// counters are added.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop and telemetry"))]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Metrics {
    /// Latency in milliseconds.
    pub latency_ms: u64,
    /// Milliseconds from sending the request to the first content chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_byte_ms: Option<u64>,
}

impl Metrics {
    /// Metrics with a latency.
    pub fn new(latency_ms: u64) -> Self {
        Self {
            latency_ms,
            time_to_first_byte_ms: None,
        }
    }

    /// Sets the time to first byte.
    #[must_use]
    pub fn with_time_to_first_byte_ms(mut self, ms: u64) -> Self {
        self.time_to_first_byte_ms = Some(ms);
        self
    }
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/models/__tests__/streaming.test.ts (`totalPromptTokens`).
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

    #[test]
    fn accumulate_adds_counters_and_sets_cache_counters_once_reported() {
        let mut total = Usage::default();
        total.accumulate(&Usage::new(1, 2, 3));
        assert_eq!(total, Usage::new(1, 2, 3));
        total.accumulate(&Usage::new(1, 1, 2).with_cache_read_input_tokens(4));
        assert_eq!(total, Usage::new(2, 3, 5).with_cache_read_input_tokens(4));
        assert_eq!(total.cache_write_input_tokens, None);
    }

    #[test]
    fn accumulate_saturates() {
        let mut total = Usage::new(u64::MAX, 0, u64::MAX).with_cache_write_input_tokens(u64::MAX);
        total.accumulate(&Usage::new(1, 1, 1).with_cache_write_input_tokens(1));
        assert_eq!(
            total,
            Usage::new(u64::MAX, 1, u64::MAX).with_cache_write_input_tokens(u64::MAX)
        );
    }

    #[test]
    fn usage_and_metrics_serialize_with_ts_keys() {
        let usage = Usage::new(1, 2, 3)
            .with_cache_read_input_tokens(4)
            .with_cache_write_input_tokens(5);
        assert_eq!(
            serde_json::to_string(&usage).unwrap(),
            r#"{"inputTokens":1,"outputTokens":2,"totalTokens":3,"cacheReadInputTokens":4,"cacheWriteInputTokens":5}"#
        );
        let metrics = Metrics::new(125).with_time_to_first_byte_ms(7);
        assert_eq!(
            serde_json::to_string(&metrics).unwrap(),
            r#"{"latencyMs":125,"timeToFirstByteMs":7}"#
        );
    }
}
