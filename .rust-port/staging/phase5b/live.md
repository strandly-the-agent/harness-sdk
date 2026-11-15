# Live results from Phase 5 item 2 (`AnthropicModel`)

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
