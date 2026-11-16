Read `.rust-port/briefs/phase12-review-common.md` first (at `/mnt/workspace/review-h12/.rust-port/briefs/` if present there, otherwise `/mnt/workspace/harness-sdk/.rust-port/briefs/`) and follow it.

Review worktree: `/mnt/workspace/review-h12`. Report file: `/mnt/workspace/reviews/phase12-review-api-bar-raiser.md`. Finding prefix `api`. Scratch dir `/tmp/review12_api/`.

Your role: API bar-raiser for the public surface of `strands-harness` (start from `harness-rs/strands-harness/src/lib.rs` and `cargo doc -p strands-harness --all-features --no-deps`; list the public items with `cargo doc` output or `grep -n 'pub ' src`), compared with the harness-ts public surface (`/tmp/ts-spec/harness-ts/src/index.ts` and what it re-exports; `internal.ts` is TS-internal). Check:
- names against TS, re-cased per PROMPT.md section 7.1 (a name changes only on a Rust prelude clash: `Option` becomes `ChoiceOption`; any other invented or changed name needs a pending-decision entry: check `.rust-port/DECISIONS-pending.md`);
- `HarnessAgentOptions`: every TS field present (compare field by field with `HarnessAgentOptions` in `harness-ts/src/agent.ts` and `types.ts`), the `AgentConfig` passthrough, struct and builder form, one enum per polymorphic option with an explicit `Auto` where TS distinguishes unset from explicit;
- the `#[non_exhaustive]` policy of decision #65 and section 7.5 (config and option structs exhaustive with public fields, `Default` and a builder; enums, events, errors, results and wire structs `#[non_exhaustive]` with constructors);
- `HarnessError`: typed variants where callers branch, source chains, no stringly errors, the TS error texts;
- public versus internal surface (TS-internal things must be `pub(crate)` or `#[doc(hidden)]`), crate-root re-exports, no glob re-exports, no `Any`/`TypeId`/downcasting required of users, no tokio types in public signatures;
- idiom, rustdoc completeness on every public item, doctest coverage (every doc example compiles; `no_run` only when it needs network or credentials), the crate README and `harness-rs/AGENTS.md` matching the code, the developer-experience examples of PROMPT.md section 13 that involve the harness;
- semver hazards, cargo features (`strands-harness` forwards the provider features; `publish = false`; docs.rs metadata), dependencies (`monty` only in the harness; nothing unused);
- the public SDK additions made for the harness in `strands-rs/strands/src` (names, docs, whether each is the narrowest API for the job, section 7.4).
