# Divergences from Phase 5 item 2 (`AnthropicModel`)

Rows for `strands-rs/docs/DIVERGENCES.md` (same columns).

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| Anthropic connection and timeout failures | The SDK's `APIConnectionError`/`APIConnectionTimeoutError` reach the agent wrapped in `ModelError`, so `DefaultModelRetryStrategy` doesn't retry them | `ModelNetworkError` (after the transport's own two retries), which `DefaultModelRetryStrategy` retries | Transport failures are `ModelNetworkError` across providers |
| Pre-built Anthropic clients | `AnthropicModelOptions.client` takes any Messages API client, such as `AnthropicBedrock` | No `client` option; `AnthropicModel` always talks to the Anthropic Messages API (base URL configurable) | The option takes a vendor SDK object; Bedrock-hosted Claude goes through `BedrockModel` |
| Anthropic `anthropic-version` default | Follows the installed `@anthropic-ai/sdk` major: `2023-06-01` on 0.x, `2026-10-21` from 1.0 | Always `2026-10-21` unless a header pins one | Rust reproduces the 1.x SDK transport |
| Anthropic error texts that name options | `'apiKey' option`, `the default for @anthropic-ai/sdk <version>`, `clientConfig.defaultHeaders['anthropic-version']` | `'api_key' option`, `the AnthropicModel default`, `client_config.default_headers['anthropic-version']`; the rest of each text is the same | The messages name the Rust options |
| Anthropic client environment | The SDK also reads `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_LOG`, `ANTHROPIC_WEBHOOK_SIGNING_KEY` and its credential profiles | Only `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` and `ANTHROPIC_CUSTOM_HEADERS` | Strands requires an API key; the others configure SDK features Rust doesn't have |
| Anthropic request headers | The SDK sends `User-Agent: Anthropic/JS <version>` and `x-stainless-*` platform and retry headers | Neither | They describe the JS SDK and its runtime |
| `default_headers` forms | `defaultHeaders` accepts records, tuples and `Headers`, with `null`/`undefined` values left out | One `http::HeaderMap`; names are case-insensitive and multi-valued | Rust-native header type |
