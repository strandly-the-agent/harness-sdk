# strands-rs ergonomics (ADOPT-ARRON items 1–11)

Branch `agent-tasks/strands-rs-ergonomics`, worktree `/home/sandbox/wt/ergonomics`.
604 tests green (546 lib + 33 + 24 + 1 doc; 23 live-Bedrock integ ignored without creds).

| # | Item | Status | Where |
|---|------|--------|-------|
| 1 | `pub mod prelude` | done | `src/lib.rs` (inline module; `AgentBuilder` re-exported from `agent`, `agent::builder` is a private mod) |
| 2 | root `tool` re-export removed, fn → `function_tool` | done | `src/tools/function_tool.rs`, `src/tools/mod.rs`, `tests/agent_loop{,_extra}.rs`; `FunctionTool::builder` unchanged |
| 3 | `BedrockModel::default_model()` / `with_model_id()` | done | `src/models/bedrock/mod.rs`; test asserts resolved config shape via `resolve_config(BedrockModelBuilder…config)` — no AWS calls (the ctors themselves are only exercised by the live integ suite) |
| 4 | `AgentResult::text()` | done | `src/agent/result.rs`; `Display` left as-is (structured output first, then text + reasoning — TS `toString()` parity). `text()` is `last_message.text()`, i.e. text blocks **concatenated** (no separator, matching `Message::text()`) |
| 5 | `ModelEventStream<'a>` | done | `src/models/model.rs`, used in `Model::stream` + the `Arc<M>` impl, re-exported from `models` |
| 6 | `Model::aggregate` | done | provided method folding `stream_aggregated`; test `aggregate_returns_the_final_aggregated_result` with a `TestModel` over the existing `text_message` helper |
| 7 | `Error::model_with_source` | done | `src/error.rs` |
| 8 | missing-model build error text | done | `agent/builder.rs` `NO_MODEL_MESSAGE`; **no test asserted the old text** (grepped `src`, `tests`, docs) so nothing to update |
| 9 | `#[non_exhaustive]` + `Default` | **partial, deliberate** | see deviation below |
| 10 | AGENTS.md sections | done | "Adding a model provider" (12 lines), "Known deviations from a literal port" (5 lines) |
| 11 | `examples/weather_agent.rs` | done | `FunctionTool::builder(..).typed_handler(..)` + `BedrockModel::default_model().await?`, 18 code lines, `use strands_agents::prelude::*` |

## Deviation on item 9

`#[non_exhaustive]` on `StreamOptions` / `CountTokensOptions` **breaks** the pattern the item wants
documented: RFC 2008 forbids functional-record-update outside the defining crate, so
`StreamOptions { system_prompt, ..Default::default() }` fails with `E0639` — it broke
`tests/integ_bedrock.rs` at 3 sites immediately. Resolution:

- `StreamOptions`, `CountTokensOptions`: already `Default`; left **exhaustive** and documented
  (`/// Construct with ..Default::default() so added fields stay non-breaking`). That combination
  actually delivers the intent; `#[non_exhaustive]` would not.
- `StreamAggregatedResult`: `#[non_exhaustive]` added (SDK-produced, no external construction
  site). **No `Default`** — `message: Message` and `stop_reason: StopReason` have no meaningful
  default and don't implement `Default`; deriving it would mean inventing semantics.

No construction sites needed `..Default::default()` fixes (all in-crate; `tests/` already used it).

## Files touched

`src/lib.rs` (prelude + dropped `tool` from the `tools` re-export only), `src/tools/{function_tool,mod}.rs`,
`src/models/{model,mod}.rs`, `src/models/bedrock/mod.rs`, `src/agent/{result,builder}.rs`, `src/error.rs`,
`tests/agent_loop.rs`, `tests/agent_loop_extra.rs`, `AGENTS.md`, `examples/weather_agent.rs`.
Untouched, per task: `examples/tools.rs`, `README.md`, `docs/PORTING.md`.

## Gates (from `strands-rs/`, verbatim last lines)

```
cargo fmt                                                   (no output)
cargo clippy --all-targets --all-features -- -D warnings    Finished `dev` profile [unoptimized] target(s) in 0.10s
cargo test --all-features                                   test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.52s
cargo build --examples --all-features                       Finished `dev` profile [unoptimized] target(s) in 0.10s
cargo doc --no-deps --all-features                          Generated /home/sandbox/cargo-target/doc/strands_agents/index.html
```
