# Divergences from the TypeScript SDK

The Rust SDK follows the TypeScript SDK (`strands-ts/`) in behaviour, text and wire formats. This file lists every user-visible behaviour where the Rust SDK differs, and why. Differences in API shape that come from Rust idiom (builders, `Result`, `snake_case` names, cargo features) are not listed. No difference here changes a format another SDK reads or writes.

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
