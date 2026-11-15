# Live results from Phase 5 items 3 and 4 (`OpenAIModel`, Bedrock Mantle)

For `strands-rs/docs/reviews/live.md`. Command (2026-11-15):

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
