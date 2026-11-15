//! Model stream data types (TS `models/streaming.ts`).

use serde::{Deserialize, Serialize};

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
    latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    time_to_first_byte_ms: Option<u64>,
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
