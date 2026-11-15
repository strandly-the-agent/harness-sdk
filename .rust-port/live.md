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
