# PARITY rows from Phase 5 item 2 (`AnthropicModel`)

Rows for `strands-rs/docs/PARITY.md` (same columns).

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/models/__tests__/anthropic.test.ts` | `strands::models::anthropic::tests` | 183/186 | `disables caching for an unknown strategy` (1): `CacheStrategy` is a closed enum, so the TS runtime check has nothing to reject; `on @anthropic-ai/sdk 0.x` (2): those cases run only against the previous SDK major, whose API version Rust doesn't send (see DIVERGENCES). The `it.each` forms that are JS-only (`Headers` instances, header tuples, `null`/`undefined` header values) fold into one `HeaderMap` case. The pause-turn continuation test expects the accumulated text block too (P-185). Test names drop the `it.each` placeholders (`$name`, `%s`); two Rust-only tests sit beside the ported ones (`skips_s3_location_images`, `the_model_and_its_options_are_send_and_sync`) |
| `strands-ts/src/models/__tests__/cache-portability.test.ts` | `strands::models::anthropic::tests::cache_portability` | 0/1 (Anthropic half ported) | the case compares Bedrock, Anthropic and Vercel requests; the Anthropic half is ported, the Bedrock half lands with `BedrockModel`, Vercel is out of v0.1 |
| `strands-ts/src/models/__tests__/server-side-summary-capability.test.ts` | unchanged | unchanged | no Anthropic case: the provider list in `models without server-side summary` excludes `AnthropicModel`, which keeps the capability |

Rust-only transport tests (vendor SDK behaviour, no TS spec): `strands::models::anthropic::client::tests` (retry statuses, `x-should-retry`, `retry-after`, backoff, connection errors, the headers timeout, SSE `error` events, count tokens, the beta final message, error wording, key redaction, and end-to-end model calls against a local server).
