# Divergences from Phase 5 items 3 and 4 (`OpenAIModel`, Bedrock Mantle)

Rows for `strands-rs/docs/DIVERGENCES.md` (same columns).

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| OpenAI connection and timeout failures | The SDK's `APIConnectionError`/`APIConnectionTimeoutError` reach the agent untouched, so `DefaultModelRetryStrategy` doesn't retry them | `ModelNetworkError` (after the transport's own two retries), which `DefaultModelRetryStrategy` retries | Transport failures are `ModelNetworkError` across providers |
| Pre-built OpenAI clients | `OpenAIModelOptions.client` takes any `OpenAI` client instance | No `client` option; the model always builds its own client (base URL, key, headers, retries and timeout configurable through `OpenAIClientConfig`) | The option takes a vendor SDK object |
| OpenAI client options | `clientConfig` takes every vendor `ClientOptions` field (`fetch`, `logger`, `defaultQuery`, `workloadIdentity`, `provider`, ...) | `OpenAIClientConfig` has `base_url`, `api_key`, `organization`, `project`, `default_headers`, `max_retries`, `timeout` | The others configure the JS SDK itself |
