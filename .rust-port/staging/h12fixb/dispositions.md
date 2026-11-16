# h12fixb: dispositions of the phase 12a review findings not reached by h12fixa

| finding | disposition | commit |
|---|---|---|
| correctness-a4 | Fixed: the interpreter step thread gets an explicit 64 MiB stack (`STEP_STACK_BYTES`, `run.rs`); test `deeply_nested_guest_data_does_not_overflow_the_stack` runs the depth-500 list through `str(a)` in the dev profile. The `await echo(x=a)` snippet (host-side conversion of the nested argument) is not covered by the test. | b23cc5906 |
| parity-a3 | Fixed (P-992): new additive `ToolAgent::call_tool_with_id_prefix`; the harness passes `ptc_`, so an inner call's id is `ptc_<hyphenated uuid>` (v7, not TS's v4). `call_tool` keeps `call_<simple v7>`. Test `inner_calls_carry_a_ptc_tool_use_id`. The SDK's own `types::agent::tests::call_tool` tests were not re-run after the refactor (the harness module tests exercise the path). | 304fa083c |
| parity-a4 | Fixed: `test_support::ts_source` tries the monorepo `harness-ts/src` (through `CARGO_MANIFEST_DIR`) and then `/tmp/ts-spec`; the nine text-parity tests use it. They still skip when neither is present; `prompt.rs` keeps its own equivalent list. All 485 harness lib tests pass. | this commit |
