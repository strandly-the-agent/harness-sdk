# Common brief for every subagent of this run

- Repo: `~/work/harness-sdk` (absolute: `/home/sandbox/work/harness-sdk`), branch `feat/rust-sdk`, already checked out. Never switch branches, never rebase or rewrite pushed history.
- The full run brief is `.rust-port/PROMPT.md`. It is authoritative. Read it in full before starting (it is about 156 KB; read it in chunks).
- State and progress: `.rust-port/STATE.md`, `.rust-port/PLAN-TRACKER.md`, `.rust-port/DECISIONS-pending.md` (check the highest P number before adding one), `.rust-port/ts-bugs.md`.
- TS spec at the pin: `/tmp/ts-spec/{strands-ts,harness-ts,shared}`. Donor: `/tmp/donor-strandly/strands-rs/` (git worktree at `06ae0d6db`). The TheWitcherish reference repo is unavailable (clone returned "Repository not found").
- Toolchain: run `. ~/.cargo/env` first in every shell. Stable 1.99.1 plus 1.96.0 (MSRV). `cargo deny` is installed.
- Disk is small (about 5 GB free). Always build with `CARGO_TARGET_DIR=$HOME/target CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only` (`.rust-port/gates.sh` sets these). Never create another target dir. Check `df -h /` before big installs; never commit `node_modules`, `dist`, `target`.
- 2 CPUs: builds are slow. Redirect all build and test output to `/tmp/*.txt` and read it with `tail`/`grep`.
- Commits: always `.rust-port/commit.sh "<conventional message>" <explicit paths...>`. It stages the paths, runs `.rust-port/scan.sh`, and commits with `--no-verify` and pushes only if the scan is clean (P-42). Never call `git commit` or `git push` directly (the sandbox is ephemeral; the fork push is how the branch survives, P-2). Push only this branch, only to `fork`. No PRs, issues, comments or posts anywhere.
- Credentials: AWS works for Bedrock invoke via the default chain; `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` are missing. Never print or read secrets; check with `test -n`.
- Hard constraints in section 3 of PROMPT.md apply in full (no edits under strands-ts/, harness-ts/, strands-py/, harness-py/, strands-cli/, site/; no AWS resource changes; no publish).
- Integration tests: one binary per crate (`tests/main.rs` declaring modules under `tests/`), never one binary per file; each test binary links the whole crate and the disk can't hold dozens (P-19). Prefer in-crate `#[cfg(test)]` unit tests. If `df -h /` shows under 1 GB free, run `cargo clean -p strands -p strands-harness -p strands-macros` before continuing and say so in your report.
- `.github/workflows/` can't be pushed with this token (P-16). CI changes go to `.rust-port/workflows/` (see its README); never try another credential.
