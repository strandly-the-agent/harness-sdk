# Divergences from the TypeScript harness

This file lists every user-visible behaviour difference between the Rust harness (`strands-harness`) and the TypeScript harness, with the reason for it. The TypeScript harness (`harness-ts/`) is the specification, so a difference is recorded here only when Rust can't or shouldn't match it, for example where the language rules out the TypeScript mechanism. Differences in API shape that follow Rust idiom (builders, `Result`, `snake_case` names) aren't listed. A fix never changes a cross-SDK wire or persisted format: data written by either SDK keeps loading in the other.

| Area | TypeScript | Rust | Reason |
|---|---|---|---|
