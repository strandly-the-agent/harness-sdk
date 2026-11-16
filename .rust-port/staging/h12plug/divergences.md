# Divergence rows, track h12plug (rows for `harness-rs/docs/DIVERGENCES.md`)

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| `EnvironmentContext`: `AGENTS.md` cut at the 16,000 cap | `text.slice(0, 16000)` cuts at a UTF-16 code unit, so a cut inside a surrogate pair leaves a lone surrogate | The cap is counted in UTF-16 code units too, but a character the cut would split is dropped whole | A Rust `String` cannot hold a lone surrogate; text outside the BMP at the cut is the only case that differs, by one code unit |
