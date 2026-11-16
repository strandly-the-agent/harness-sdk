# Common rules for implementer subagents (third session, 2026-11-16)

You are implementing one track of the Strands Rust SDK port (`strands-rs`). Work autonomously; the owner is unavailable and nobody answers questions.

## Read first

- `.rust-port/PROMPT.md` sections 3, 7, 8, 9.2-9.7, 10 and 11, then the section 12 phase section and section 6 decisions your brief names. The file has very long lines; read it with `sed -n 'A,Bp' .rust-port/PROMPT.md` in ranges of about 40 lines (find headings with `grep -n '^##' .rust-port/PROMPT.md`).
- `strands-rs/AGENTS.md` and `strands-rs/docs/TESTING.md`.
- `grep` `.rust-port/DECISIONS-pending.md` (215 KB; never read it whole) for decisions touching your area.
- The existing Rust code you build on (Phases 0-5, 8, 9 and most of 6 are done: agent loop, hooks, plugins, middleware, tools and macros, providers, storage, sessions, sandbox, vended tools, MCP, the experimental context manager). Reuse it; never duplicate it.

## Where you work

- Only inside your git worktree (named in your brief), on its local branch. Never switch branches, never push, never touch `/mnt/workspace/harness-sdk` (the lead's checkout) or another `/mnt/workspace/wt-*` worktree. The lead integrates your commits by cherry-picking, so commit small and often, and keep edits to shared files (`agent/agent.rs`, `agent/config.rs`, `lib.rs`, `Cargo.toml`) minimal and in their own commit where practical; list every edit outside your paths in your report.
- Hard constraints (PROMPT.md section 3) apply: never open or comment on PRs or issues, never publish, never create or modify AWS resources, never edit `strands-ts/`, `harness-ts/`, `strands-py/`, `harness-py/`, `strands-cli/` or `site/`, public names only, no secrets in files or output, no severity labels, no time estimates.

## Toolchain

Put this at the start of every shell command (with your track's `CARGO_TARGET_DIR` from the brief):

```bash
export RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:/mnt/workspace/.node/bin:$PATH CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=<from your brief>
```

- The host has 4 cores and 16 GB shared by several agents building at once. Iterate with `cargo check -p strands --all-features --tests` and targeted tests (`cargo test -p strands --all-features --lib <module path>` or `--test <file>`); run the whole gate script only before your last commits: `GATE_OUT=/mnt/workspace/gates-<track> .rust-port/gates.sh` from your worktree root (prints `name: rc` lines; outputs in `$GATE_OUT/gate_*.txt`). Run `cargo fmt --all` first. Never weaken a lint or skip a test to get green.
- Redirect long output to files under `/tmp` and read the tail or grep it. Tool output over about 6 KB is truncated, so read files in slices.
- Never kill processes by name (other agents run cargo); only PIDs you started. Never build into another target dir. If `df -h /tmp /mnt/workspace` shows under 3 GB free on the volume holding your target dir, delete `$CARGO_TARGET_DIR/debug/build/strands*` directories older than 15 minutes.
- The TS spec at the pin is in `/tmp/ts-spec/strands-ts/src` (tests in `__tests__`). Donor Rust (older baseline; reuse freely but re-diff against the pin first): `/tmp/donor-strandly/strands-rs/crates/` (search with `rg`). Node and the built TS SDK are available read-only in `/mnt/workspace/harness-sdk/node_modules` and `/mnt/workspace/harness-sdk/strands-ts/dist` for generating zod schemas or checking a TS behaviour with `npx tsx` (write scratch scripts under `/tmp`).

## Tests

Port every applicable TS `it(...)` case, named with the snake_case TS title; the test module doc comment names the source spec file. No real sleeps (virtual time or an injected sleeper), no network beyond localhost servers you start, never `set_current_dir`, never mutate process env (use the crate's env-lookup seams), wire shapes asserted with `serde_json::json!`, exact TS error texts.

## Logs

Don't edit the shared logs (`.rust-port/DECISIONS-pending.md`, `.rust-port/ts-bugs.md`, `.rust-port/PLAN-TRACKER.md`, `.rust-port/STATE.md`, `strands-rs/docs/PARITY.md`, `strands-rs/docs/DIVERGENCES.md`). Write your entries, in the same formats, to `.rust-port/staging/<track>/{decisions,ts-bugs,divergences,parity}.md` (create only the ones you need). Parity rows use the exact first-column text of the existing `PARITY.md` row for the TS spec file (`grep` for it), with columns `TS spec | Rust tests | ported/total | gaps (with reason)`; count TS cases with `node strands-rs/docs/count-ts-cases.mjs <spec file>` if it accepts a path (read the script). Number decisions from the P number your brief gives. Write staging entries as you go, not at the end.

## Commits

Conventional commits (`feat(strands): ...`, `test(strands): ...`, `fix(strands): ...`, `chore(rust-port): ...`), `git add` explicit paths, then `.rust-port/secret-scan.sh` must print `scan: clean`, then `git commit --no-verify`. You are cut off without warning after about 110 minutes, and uncommitted work is lost: commit after every step that compiles (a step that compiles with its tests passing is enough; the full gate is for the final commits), in the order your brief gives.

## Report (concise, under 400 words)

What was built (modules, key public types), test counts, cases ported per TS spec file and gaps with reasons, P numbers used, TS bugs logged, edits outside your paths, commit hashes in order, and anything left undone (be exact: the lead plans the next run from this).
