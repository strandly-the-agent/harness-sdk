# PARITY rows from Phase 5 items 3 and 4 (`OpenAIModel`, Bedrock Mantle)

Rows for `strands-rs/docs/PARITY.md` (same columns).

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/models/openai/__tests__/errors.test.ts` | `strands::models::openai::errors::tests` | 7/7 | the numeric and `null` code cases run through `ApiError::from_status`, which keeps only string codes; one Rust-only test covers the throttling branch |
| `strands-ts/src/models/openai/__tests__/responses.test.ts` | `strands::models::openai::tests::responses` | 72/72 | `uses provided client instance and skips OpenAI constructor` and `does not require API key when client is provided` run through the `Transport::Injected` seam; `resolves stateful once per stream` asserts the outcome (the adapter takes the flag once; there is no `getConfig` spy); `it.each` placeholders are dropped from names; Rust-only tests cover env defaults, client-config precedence, `Debug` redaction, the default-model warning, system prompt blocks, the placement warning, a missing item id, unsupported annotations, transport errors, 5xx/408 not throttled, and aggregation of citations |
| `strands-ts/src/models/openai/__tests__/responses-cache.test.ts` | `strands::models::openai::tests::responses_cache` | 17/17 | `it.each` rows run in one test each |
| vendor transport (`node_modules/openai`) | `strands::models::openai::client::tests` | n/a | local HTTP server: headers, `[DONE]`, retries and backoff, `x-should-retry`, `retry-after`, connection errors, timeout, data `error`, `apiKey` setter per attempt and its error texts, error wording, redaction |
