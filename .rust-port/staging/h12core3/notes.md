# h12core3 notes (for the next handoff)

- Title check: with the staged rows in `parity.md` applied to `harness-rs/docs/PARITY.md`, `check-test-titles.mjs --parity harness-rs/docs/PARITY.md agent.test builtin-tools` reports `agent.test.ts 59/4/5/32 (row 63/100)` and `builtin-tools.test.ts 11/3/7/1 (row 14/22)`: matched + declared equals the ported count for both. Both still print FAIL only for the cases not ported yet (32 and 1).
- The staged `agent.test.ts` row names two files (`agent/tests.rs`, `builtin_tools/tests.rs`): three delegate cases are tested on `child_options`, which is private to `builtin_tools`.
- Correction to the h12core2 handoff: the default subagent's input schema does offer a `tools` enum (`keeps_subagent_for_sub_delegation_...` in `agent/tests.rs` reads it). So `delegate()` and `child_options_of()` could pass `"tools": [...]` in the tool input and the narrowing cases could run through the real tool; not tried.
- Not counted: `logs info, not a warning, and builds when default web_search hits the default bedrock model`. `web_search_mode_disables_a_default_web_search_on_a_model_without_it_and_says_so` covers the info line only, not the build.
- `lets an explicit Exa selection win over native search`: the tool half is portable with `harness_with_keys`; the "no native tools on the model" half needs the provider model's config accessor.
- `.rust-port/gates.sh` was not run; per commit: module tests, clippy `-D warnings`, `cargo fmt --all --check`.
