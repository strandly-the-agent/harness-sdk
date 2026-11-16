# Common rules for harness implementer subagents (Phase 12, fourth session)

Read `.rust-port/briefs/run3-common.md` first; everything in it applies (toolchain line, where you work, commits, logs, report, the cut-off after about 110 minutes), with these differences for the harness crate.

## What you build

`strands-harness` (`harness-rs/strands-harness/`), the Rust port of harness-ts at the pin. The SDK crate `strands` is complete and reviewed; you build on its public API and don't change it. If the harness cannot be written without an SDK change (a missing public accessor, for example), make the smallest change in its own commit (`feat(strands): ...`), log a pending decision, and list it in your report.

## Read first

- `.rust-port/PROMPT.md`: section 3; sections 7 and 8.1; sections 9.2-9.7; the Harness paragraphs and "Error and edge-case behaviour" of section 10; section 11 items 36-48; the whole Phase 12 section (`grep -n '^### Phase 12' .rust-port/PROMPT.md`, about 40 lines) and decisions #28-32, #60, #65, #70, #71 in section 6. Long lines: read with `sed -n 'A,Bp'` in ranges of about 40 lines.
- `harness-rs/AGENTS.md`, `strands-rs/AGENTS.md`, `strands-rs/docs/TESTING.md`.
- The TS spec: `/tmp/ts-spec/harness-ts/src` and `/tmp/ts-spec/harness-ts/test` (your brief names your files). Donor Rust (older TS baseline, a different SDK core; reuse logic and tests after re-diffing against the pin, reshape to this SDK's API): `/tmp/donor-strandly/strands-rs/crates/strands-harness/src`.
- The skeleton the lead committed: `harness-rs/strands-harness/src/{lib.rs, error.rs, types.rs, defaults.rs, sandbox.rs}` and the module stubs. `HarnessError` (`error.rs`) is the crate's typed error; add variants you need (other tracks add theirs; keep yours grouped under a comment-free block of your own so merges are simple). `lib.rs` already declares every module; add re-exports for your public names at the place marked for your module.

## Rules that differ from the SDK tracks

- Names: the TS export names re-cased (`makeWebFetch` is `make_web_fetch`, `MakeWebFetchOptions` keeps its name, `Option` of the subagent axes is `ChoiceOption`, #71). Other tracks call your module through these names, so don't invent others; anything TS keeps module-private stays `pub(crate)`.
- Config and option structs: exhaustive, public fields, `Default`, a `bon` builder (#65); JSON-expressible configs derive serde with `rename_all = "camelCase"` only where a TS field is camelCase on the wire (most harness keys are snake_case or single words; follow TS exactly).
- Every built-in tool and all discovery go through the agent's sandbox (`ctx.agent().sandbox()`), never `tokio::fs` or `std::process` on the host (section 11 item 36).
- Tests: port every `it(...)`/`test(...)` case of your spec files, named with the snake_case TS title, module doc comment naming the spec file. Never `set_current_dir`, never mutate process env: use a crate-internal base-directory seam and env-lookup seam (create `pub(crate)` seams in your module if the skeleton has none; say so in the report). No network beyond localhost servers you start; no real sleeps. Unit tests use `strands::testing` mocks.
- Iterate with `cargo check -p strands-harness --all-features --tests` and `cargo test -p strands-harness --all-features --lib <module path>`; before your last commits run `cargo fmt --all` and `GATE_OUT=/mnt/workspace/gates-<track> .rust-port/gates.sh` from your worktree root.
- Logs go to your staging dir: `decisions.md`, `ts-bugs.md`, `divergences.md` (rows for `harness-rs/docs/DIVERGENCES.md`), `parity.md` (full replacement rows for `harness-rs/docs/PARITY.md`, same four columns; count TS cases with `node strands-rs/docs/count-ts-cases.mjs <spec file>`).
- Byte-observable text (prompts, tool descriptions, tool result text, error texts, warnings users see) is copied exactly from TS; add a parity test that reads the TS constant from `/tmp/ts-spec` when present, where the text is long.
- Unused-dependency warnings for crates other tracks will use are expected; don't remove dependencies you didn't add.
