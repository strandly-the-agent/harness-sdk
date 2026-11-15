# Live test results

## Bedrock (`strands/tests/bedrock_live.rs`)

Run: 2026-11-15, `cargo test -p strands --all-features --test bedrock_live -- --ignored live_`, default AWS credential chain, region `us-west-2`, model `us.anthropic.claude-haiku-4-5-20251001-v1:0` (used by the TS integration tests). Run twice (the second with `--nocapture` to confirm no test skipped); both 6 passed, 0 failed.

| Test | Result | Checks |
|---|---|---|
| `live_text_with_the_converse_api` | pass | `stream: false` (Converse): stop reason `endTurn`, a non-empty text delta, input and output tokens > 0 |
| `live_streaming_events_and_max_tokens` | pass | ConverseStream: first event `messageStart`, text deltas, `endTurn`; `max_tokens: 5` gives `maxTokens` |
| `live_tool_round_trip_through_an_agent` | pass | Agent with a `#[tool]`: `get_weather` called with `city` Seattle, tool result `success` containing the tool's output, final stop `endTurn` |
| `live_structured_output` | pass | `structured_output_schema::<Person>()` parses `{ name: "Ada", age: 36 }` |
| `live_cache_hit_with_tools_and_messages` | pass | `cacheConfig { strategy: auto }` with a tool spec and a large unique message: first call `cacheWriteInputTokens > 0`, second identical call `cacheReadInputTokens > 0` (tools and message cache points) |
| `live_invalid_model_id_fails_with_a_model_error` | pass | an unknown model id fails with a `ModelError`-family error |

Not covered live: API-key (bearer) authentication (no Bedrock API key in this environment), guardrails (need a guardrail resource; creating one is not allowed), audio/video/document media.

For `strands-rs/docs/reviews/live.md`. Command (2026-11-15):

```bash
cargo test -p strands --all-features --test anthropic_live -- --ignored live_ --nocapture
```

`ANTHROPIC_API_KEY` was not set in this environment, so every test printed `skipping: ANTHROPIC_API_KEY is not set` and returned (reported as `ok`, exit 0). No request reached the API; nothing below has been verified live yet.

| Test (`strands/tests/anthropic_live.rs`) | Checks | Model | Result |
|---|---|---|---|
| `live_text` | agent invoke ends with `endTurn` and a non-empty text block | `claude-haiku-4-5` | skipped (no key) |
| `live_streaming` | `stream` starts with a message start, has text deltas, `maxTokens` stop at `max_tokens: 20`, output tokens <= 20 | `claude-haiku-4-5` | skipped (no key) |
| `live_tool_round_trip` | the model calls `get_weather` with `city` containing Lisbon, the tool result holds the tool output, the turn ends with `endTurn` | `claude-haiku-4-5` | skipped (no key) |
| `live_structured_output` | `structured_output::<Person>()` parses with `age == 30` | `claude-haiku-4-5` | skipped (no key) |
| `live_cache_hit` | a system prompt cache point over ~8k tokens: the first call reports cache writes, the second cache reads | `claude-haiku-4-5` | skipped (no key) |

The TS checklist (`strands-ts/test/integ/models/anthropic.test.ts`) also covers image input, thinking blocks, the `web_search` server tool with citations, injected context with preserved thinking, the pinned `anthropic-version` with `temperature`, and native token counting; those are not yet live tests here and stay on the checklist for the live-test item.

## OpenAI and Bedrock Mantle

```bash
cargo test -p strands --all-features --no-fail-fast --test openai_live --test mantle_live -- --ignored live_ --nocapture --test-threads=2
```

The output was scanned for key patterns and credential values before reading (clean).

## OpenAI (`strands/tests/openai_live.rs`)

`OPENAI_API_KEY` was not set, so all 8 tests printed `skipping: OPENAI_API_KEY is not set` and returned (reported `ok`). No request reached the API; these are unverified live. Tests: `live_text_responses`, `live_text_chat`, `live_streaming`, `live_tool_round_trip_responses`, `live_tool_round_trip_chat`, `live_structured_output`, `live_stateful_responses`, `live_cache_hit` (model `gpt-5.4-mini`).

## Bedrock Mantle (`strands/tests/mantle_live.rs`)

Model `openai.gpt-oss-120b`, region `us-east-1` (as in `strands-ts/test/integ/models/openai/mantle.test.node.ts`), default AWS credential chain, bearer token minted by the Rust SigV4 port.

| Test | Checks | Result |
|---|---|---|
| `live_mantle_text_chat` | Chat Completions agent invoke ends `endTurn`, reply contains `4` | pass |
| `live_mantle_text_responses` | same on the Responses API | pass |
| `live_mantle_streaming` | first event message start, text deltas, usage with input and output tokens | pass |
| `live_mantle_tool_round_trip_chat` | `get_weather` called with `city` containing Lisbon, result contains `21 degrees`, `endTurn` | pass |
| `live_mantle_tool_round_trip_responses` | same on the Responses API | pass |
| `live_mantle_structured_output` | `Person { name: "John..", age: 30 }` through the structured output tool | pass |
| `live_mantle_stateful_conversation` | `stateful: true`, second turn answers with the name from the first (`previous_response_id` chaining) | pass |
| `live_mantle_reasoning_multi_turn` | `params.reasoning.effort = low`, two turns, second ends `endTurn` | pass |
| `live_mantle_cache_hit` | second identical request (`cache_config` set, session key `strands-rust-live-cache`) reports `cacheReadInputTokens > 0` | fail: the second call's usage was `{"inputTokens":3086,"outputTokens":123,"totalTokens":3209}`, with no `cached_tokens` reported (tried on Chat Completions and on Responses). `openai.gpt-oss-120b` has no `cachePoints` capability in the model-info table, so no checkpoints are sent; the endpoint reported no implicit cache read for this model. The test is kept |

## MCP

No live model tests were added; the MCP tests run against local servers (in-process `rmcp` servers and a Node stdio fixture), which need no credentials.

Stdio fixture run: 2026-11-15, `STRANDS_TEST_NODE=<node> STRANDS_TEST_NODE_MODULES=<repo node_modules> cargo test -p strands --features mcp --test mcp_stdio`: 5 passed, 0 failed.

## Bedrock Mantle after the Phase 5 review fixes

Mantle live run on the fix branch (2026-11-15), `cargo test -p strands --all-features --test mantle_live -- --ignored live_ --test-threads=2 --nocapture`, region `us-east-1`, default AWS credential chain; no test printed a skip message. Output scanned with the section 9.5 patterns: clean.

| Test | Model | Result |
|---|---|---|
| `live_mantle_text_chat` | `openai.gpt-oss-120b` | pass |
| `live_mantle_text_responses` | `openai.gpt-oss-120b` | pass |
| `live_mantle_streaming` | `openai.gpt-oss-120b` | pass |
| `live_mantle_tool_round_trip_chat` | `openai.gpt-oss-120b` | pass |
| `live_mantle_tool_round_trip_responses` | `openai.gpt-oss-120b` | pass |
| `live_mantle_structured_output` | `openai.gpt-oss-120b` | pass |
| `live_mantle_stateful_conversation` | `openai.gpt-oss-120b` | pass (now also asserts `endTurn` and empty local history per turn) |
| `live_mantle_reasoning_multi_turn` | `openai.gpt-oss-120b` | pass |
| `live_mantle_cache_hit` | `openai.gpt-6.1-sol` | pass (second call reports `cacheReadInputTokens > 0`) |

Not rerun after the fixes: `openai_live` (no `OPENAI_API_KEY` in this environment) and `bedrock_live` (no Bedrock live test changed).
