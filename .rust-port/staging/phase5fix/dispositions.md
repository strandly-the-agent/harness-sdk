| finding | disposition | commit |
|---|---|---|
| correctness-1 | Fixed: a Responses stream without `response.completed`/`response.incomplete` fails with `ModelError` "Stream ended without completing a message"; test `fails_a_stream_that_ends_without_a_terminal_response_event`; TS bug and divergence logged (P-208) | 50236240b |
| correctness-2 | Fixed: `MessageSnapshot` returns the compaction message only after `message_stop`, else the vendor error text; test `fails_a_beta_stream_that_ends_without_message_stop` (P-209) | 50236240b |
| correctness-3 | Fixed: the Mantle default chain's credentials are cached and fetched again only within 5 minutes of expiry, never without one; three tests in `default_chain_credentials` (P-210) | 50236240b |
| correctness-4 | Not changed, logged as a divergence: `eventsource-stream` stays the SSE decoder (section 7.5); the Messages API sends a body with every `error` event (P-211) | 50236240b |
| api-1 | Fixed: `bedrock_mantle_config` field removed; `OpenAIModel::with_bedrock_mantle(options, config)` behind `openai` plus `bedrock`; `api_surface` test builds a full `OpenAIModelOptions` literal (P-212, amends P-206) | 123a05dc6 |
| api-2 | Fixed: `strands::models::{AnthropicModel, BedrockModel, OpenAIModel}`, Bedrock types at the crate root, `AnthropicModel::builder()` with a fallible `build()`; the section 13 example is an `api_surface` test and a doctest (P-213) | 123a05dc6 |
| api-3 | Not committed: the change is in `src/agent/`, outside this branch's paths. Verified patch in `.rust-port/staging/phase5fix/api-3-model-source.patch` for the lead (P-218, P-161); `DIVERGENCES.md:29` row goes once it lands | - |
| api-4 | Fixed: `models/http_transport.rs` holds the shared helpers and one retrying `send` with a `RetryPolicy` (provider `retry_after`, log label) and a `TransportError` trait; `replace_if_set` copies removed (P-214, amends P-201). The server-backed client tests stay per provider (they check each client's headers and wording) | 123a05dc6 |
| api-5 | Fixed: `BedrockModelOptions::client_config` (P-215, amends P-151) | 123a05dc6 |
| api-6 | Fixed: `From<BaseModelConfig>` became the private `BedrockModelConfig::from_base` (P-215) | 123a05dc6 |
| api-7 | Not renamed, exception logged: PROMPT.md's Phase 2 item list names `CacheTtl` (and P-29 recorded it), so `CacheTtl`/`BedrockCacheTtl` keep Rust casing (P-216) | - |
| api-8 | Fixed: `aws_config`, `aws_sdk_bedrockruntime`, `aws_credential_types` and `http` re-exported from the provider modules (P-217) | 123a05dc6 |
| api-9 | Fixed: the Mantle errors are documented on `OpenAIModel::with_bedrock_mantle`, the only constructor that raises them | 123a05dc6 |
| api-10 | Fixed: `no_run` examples for Chat Completions and web-search params on `OpenAIModel`, for `with_bedrock_mantle`, for `BedrockModelOptions { region, .. }` and for `AnthropicModel::builder` | 123a05dc6 |
| parity-1 | Fixed: PARITY row for `responses-cache.test.ts` is 20/20 (`count-ts-cases.mjs` prints 20; 20 Rust tests), in `staging/phase5fix/parity.md` | 82fd36bf2 |
| parity-2 | Fixed: `openai::tests::server_side_summary_capability` ports both OpenAI rows (Chat and Responses, default and override); the partial `turns_server_side_summary_off` is gone; PARITY row updated in staging | 82fd36bf2 |
| parity-3 | Fixed: Bedrock region is option, `client_config`, `AWS_REGION`, profile, IMDS (`ImdsRegionProvider` through a future seam), then `us-west-2`; `AWS_DEFAULT_REGION` dropped; divergence row replaced in staging (P-219, amends P-152) | 82fd36bf2 |
| parity-4 | Fixed: `live_mantle_stateful_conversation` and `live_stateful_responses` assert `EndTurn` and empty `agent.messages()` after each turn | 82fd36bf2 |
| parity-5 | Fixed: `live_mantle_cache_hit` runs on `openai.gpt-6.1-sol`, the Mantle id the model info gives `cachePoints`; it passed live (P-220) | 82fd36bf2 |
| parity-6 | Handled by the lead (brief) | - |
