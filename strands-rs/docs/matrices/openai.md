# openai — Pass A report

STATUS: **Pass A landed and green.** OpenAI provider (Responses + Chat Completions + Bedrock
Mantle) behind feature `openai` (default ON), plus a shared SSE parser. Commit `0bd9f5e31` on
`agent-tasks/strands-rs-openai`.

## Files added
- `crates/strands-agents/src/models/sse.rs` (113) — incremental SSE parser (`SseParser`,
  `SseEvent`), unconditional (no deps), 4 tests. **Merge note:** if the anthropic worker adds its
  own parser, keep one of them; this one is `push(&str) -> Vec<SseEvent>` + `finish()`.
- `crates/strands-agents/src/models/openai/mod.rs` (306) — `OpenAIApi`, `OpenAIModelConfig`,
  `OpenAIModel::{new, with_transport, format_request}`, `OpenAIRequest`, `OpenAITransport`
  (injectable), `ReqwestTransport`, `classify_openai_error`, `impl Model`.
- `.../openai/responses.rs` (423) — `format_request` + `ResponsesStream` state machine.
- `.../openai/chat.rs` (357) — `format_request` + `ChatStream` state machine + `map_finish_reason`.
- `.../openai/mantle.rs` (161) — base-path prefixes, anchored region validation, base URL, 5 tests.
- `.../openai/tests.rs` (495) — 14 adapter/error/config tests with an injected static transport.
- `crates/strands-agents/tests/integ_openai.rs` (44) — 2 live tests
  `#[ignore = "live OpenAI (OPENAI_API_KEY)"]`, skipping (not failing) when the key is unset.
- Wiring: `models/mod.rs` exports, `lib.rs` re-exports `OpenAIApi/OpenAIModel/OpenAIModelConfig`,
  feature `openai = ["dep:reqwest"]` added to `default`; workspace `reqwest` gained `stream`.

## Test counts
23 new tests (`cargo test openai`), all passing; 2 live tests ignored by default.

## Gates (verbatim last lines)
- `cargo fmt` — clean, no output.
- `cargo clippy --all-targets --all-features -- -D warnings` — no output (clean).
- `cargo test --all-features -p strands-agents` — `test result: ok. 723 passed; 0 failed; ...`
  (lib) and ok for every other target; 22+2+2 ignored are the pre-existing/live ones.
- `cargo build --no-default-features -p strands-agents` —
  `Finished \`dev\` profile [unoptimized] target(s) in 45.22s`.

## Matrix (TS → Rust), Pass A
| TS group | Rust | state |
| --- | --- | --- |
| `responses.test.ts` request formatting (model/input/stream/store, instructions, temperature/max_output_tokens/top_p, tools+strict, tool_choice, builtin tools survive) | `formats_responses_request_with_all_options`, `responses_strict_tools_and_builtin_tools_survive`, `responses_input_maps_tool_use_and_result_items` | pass |
| `responses.test.ts` stream mapping (text/reasoning switchContent, usage incl. cached>0 only, function-call replay at finalize, incomplete→maxTokens, response.failed throws) | `responses_stream_maps_text_reasoning_and_usage`, `responses_stream_replays_function_calls_at_the_end`, `responses_incomplete_sets_max_tokens_stop_reason`, `responses_failed_event_errors` | pass |
| `chat.test.ts` API request formatting (all options, system message, tools, tool_choice, n>1, tool spec validation) | `formats_chat_request_with_all_options`, `chat_rejects_tool_spec_without_description_and_n_above_one` | pass |
| `chat.test.ts` basic streaming + tool calling + buffered usage | `chat_stream_maps_text_tool_calls_and_buffered_usage` | pass |
| `chat.test.ts` stop reasons (known + unknown w/ camelCase fallback) | `maps_openai_stop_reasons`, `handles_unknown_stop_reasons_with_warning` | pass |
| `errors.test.ts` `classifyOpenAIError` (throttle first, overflow list, passthrough) | `classifies_throttling_before_context_overflow`, `classifies_context_overflow`, `passes_other_errors_through` | pass |
| `mantle.test.ts` base-path by model family, base URL, region resolution + validation | `resolves_openai_base_path_for_known_prefixes`, `falls_back_to_v1_base_path`, `builds_base_url_from_region_and_model`, `rejects_invalid_regions`, `accepts_multi_segment_regions`, `mantle_config_derives_base_url` | pass |
| validation: empty messages; `stateful` only for responses | `rejects_empty_messages`, `stateful_only_for_responses_api` | pass |
| SSE framing (no TS equivalent — TS uses the `openai` npm client) | `sse::tests` ×4 | pass (added) |
| `updateConfig` / `getConfig` groups | — | skipped: no `update_config` surface on the Rust `Model` trait |
| cacheConfig → `prompt_cache_key` / `prompt_cache_retention` group (11 tests) | — | **Pass B** |
| systemPrompt block arrays, cache points, guard-content filtering (7+3 tests) | — | **Pass B** (currently `SystemPrompt::text()` join) |
| media blocks: image/document/`image_url` base64 + tool-result media hoisting (~8 tests) | — | **Pass B** (dropped with a warn today) |
| cancellation mid-stream (both adapters) | — | **Pass B** (token is checked between events only, via `take_while`) |
| stateful `previous_response_id` round-trip + `responseId` capture | — | **Pass B** (needs a model-state slot; `store` is already sent) |
| citations annotation mapping | implemented in `responses.rs`, untested | **Pass B** (add row test) |
| live integ (`test/integ/models/openai/`) | `tests/integ_openai.rs` ×2 | ignored (needs `OPENAI_API_KEY`) |

## Deviations / gaps
- **Transport, not the `openai` SDK.** TS delegates HTTP+SSE to the npm client; Rust hand-rolls it
  behind `OpenAITransport` so tests are offline. `ReqwestTransport` classifies non-2xx bodies via
  `classify_openai_error`; no retry/backoff (the SDK's `retry` module owns that).
- **Mantle token minting is a documented gap**: no Rust port of `@aws/bedrock-token-generator`, so
  `mantle::mint_bearer_token` requires `AWS_BEARER_TOKEN_BEDROCK` (or an explicit `api_key`) and
  errors with that instruction otherwise. The TS mint-per-request behaviour is not reproduced.
- `params` managed-key handling: managed fields are stripped + `warn_once`'d (TS warns at config
  time and lets managed fields overwrite); net request shape is identical.
- Reasoning blocks in history are dropped with the TS warning; non-text assistant history content
  likewise.
- Assistant text in Responses input is joined with `\n` into one `output_text` item (per spec).

NEXT (Pass B): cacheConfig/prompt-cache keys; system-prompt block arrays + guard-content filtering;
media blocks (image/document, tool-result media); mid-stream cancellation; stateful
`previous_response_id` + `responseId` capture; citations row test; reconcile `sse.rs` with the
anthropic worker's parser if one lands.
