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

Not run: `openai_live` (its stateful test changed, but the brief limits live runs to Bedrock and Mantle) and `bedrock_live` (no Bedrock live test changed).
