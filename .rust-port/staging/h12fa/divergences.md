Rows for `harness-rs/docs/DIVERGENCES.md`.

Replaces the row `Portable config: `subagent.maxDepth``:

| Portable config: `subagent.maxDepth` | Any non-negative safe integer, kept as given | A value above `u32::MAX` is held (and serialized back) as `u32::MAX` | The depth is a `u32`; no delegation chain reaches either bound |
