# h12fixb: dispositions of the phase 12a review findings not reached by h12fixa

| finding | disposition | commit |
|---|---|---|
| correctness-a4 | Fixed: the interpreter step thread gets an explicit 64 MiB stack (`STEP_STACK_BYTES`, `run.rs`); test `deeply_nested_guest_data_does_not_overflow_the_stack` runs the depth-500 list through `str(a)` in the dev profile. The `await echo(x=a)` snippet (host-side conversion of the nested argument) is not covered by the test. | this commit |
