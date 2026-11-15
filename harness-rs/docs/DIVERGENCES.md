# Divergences from the TypeScript harness

The Rust harness follows `harness-ts/` in options, defaults, prompts, tool text and persisted layout. This file lists every user-visible behaviour where the Rust harness differs, and why. Differences in API shape that come from Rust idiom are not listed, and SDK-level differences are in [strands-rs/docs/DIVERGENCES.md](../../strands-rs/docs/DIVERGENCES.md). No difference here changes a format another SDK reads or writes.

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
