# Phase 2 review: model abstraction, streaming and retry

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed `0e4b20905..ef33e6992`. All gates passed at review time.

| Finding | Disposition | Commit |
|---|---|---|
| Correctness 1: a deeply nested model tool input overflows the stack and aborts the process | Fixed: saved-data and model JSON is capped at 500 nesting levels (measured safe for parse, clone, compare, serialize and drop on a 2 MiB stack in debug builds); deeper input is a clean parse error, and tool input over the cap is a `ModelError` | `92d333e0a`, `85dcc7a53` |
| Correctness 2: blank-text checks use Rust `trim`, which differs from JS `trim` on U+FEFF and U+0085 | Fixed: JS-compatible trim | `92d333e0a` |
| Correctness 3: the default `model_info` ignores an overridden `model_id()` | Fixed | `92d333e0a` |
| Correctness 4: the test provider calls the user's generator while holding its lock | Fixed | `7b51876eb` |
| Correctness 5: learned structured-output support is per provider, while TS keeps it per process | Kept per provider (no process-global mutable state beyond warn-once sets); recorded as a divergence | `85dcc7a53` |
| API 1: providers outside the crate can't build on the default model info | Fixed: `base_model_info` and `merge_model_info` are public and documented | `92d333e0a` |
| API 2: `async_trait` isn't re-exported, so async trait methods can't be implemented with only `strands` as a dependency | Fixed: `strands::async_trait` re-export and doc example | `c8729653a`, `92d333e0a` |
| API 3: `update_config` can't clear a field | Documented (`None` keeps the current value); recorded as a divergence | `92d333e0a` |
| API 4: `ModelRetryContext` accepts attempt 0 | Fixed: `NonZeroU32` | `45cc642c2` |
| Parity 1: a TS aggregation bug (stale tool use after an unparseable tool input) was copied | Fixed in Rust; recorded as a divergence and a TS bug | `92d333e0a`, `85dcc7a53` |
| Parity 2: structured-output mechanism logged with Rust `Debug` names | Fixed: TS lowercase names | `6f69d7ad5` |
