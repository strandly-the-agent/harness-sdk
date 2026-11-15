# Pending decisions

Format: **P-N. Title.** Context. Options. Choice. TS reference (file:line). Files.

**P-1. Commit with `--no-verify`.** The husky pre-commit hook installed by `npm ci` builds and tests strands-ts on every commit, and this branch changes no TypeScript. Options: run the hook on every commit, or skip it. Choice: `git commit --no-verify` for every commit on this branch. TS reference: `.husky/pre-commit`. Files: none.

**P-2. `monty` held at 1.5 by the MSRV-aware resolver.** `monty` 1.6.0 declares `rust-version = 1.98`; 1.5.0 declares 1.96, matching decision #59. Options: raise the MSRV to 1.98, or stay on 1.96 and let resolver 3 pick the newest compatible `monty`. Choice: MSRV stays 1.96; the manifest says `monty = "1.5"` and `Cargo.lock` resolves 1.5.0. Raising the MSRV is a later minor-release call. TS reference: none. Files: `Cargo.toml`, `Cargo.lock`.

**P-3. cargo-deny bans: targeted duplicate denial.** The AWS SDK, cedar-policy and monty trees bring two or three versions of leaf utility crates (rand, phf, sha2, digest, getrandom, syn, thiserror, base64, hashbrown, ...), which no feature choice removes. Options: deny all duplicates with a long skip list, or warn on duplicates and deny them for the crates where two copies mean two stacks. Choice: `multiple-versions = "warn"`, plus `deny-multiple-versions` for tokio, hyper, http, h2, reqwest, rustls, rustls-webpki, opentelemetry, opentelemetry_sdk, cedar-policy-core, serde_json and schemars. TS reference: none. Files: `deny.toml`.

**P-4. cargo-deny license and advisory exceptions from `monty`.** `monty` (required by the harness `programmatic_tool_caller`, #29) pulls `minicbor`/`minicbor-serde` (BlueOak-1.0.0, a permissive OSI-approved license) and `postcard -> heapless 0.7 -> atomic-polyfill` (RUSTSEC-2023-0089, unmaintained, no safe upgrade, not a vulnerability). Choice: allow BlueOak-1.0.0 and ignore exactly RUSTSEC-2023-0089 with a reason. Files: `deny.toml`.

**P-5. `reqwest`, `htmd`, `cedar-policy` and `process-wrap` are non-optional `strands` dependencies.** The vended `http_request` and `web_fetch` tools, `CedarAuthorization` and the local sandbox are in v0.1, and the design names only five features (`bedrock`, `anthropic`, `openai`, `mcp`, `otel`). Choice: these are always compiled. Files: `strands-rs/strands/Cargo.toml`.

**P-6. Dependency major versions.** `syn` 3 (darling 0.24 requires it), `reqwest` 0.13 (the version `rmcp` 3.10, `jsonschema` 0.61 and `opentelemetry-otlp` 0.33 use, so one copy resolves), `base64` 0.23, `jsonschema` with default features off (no HTTP or file `$ref` resolution), `opentelemetry-otlp` with `http-proto` and the async `reqwest-client` (no blocking client, no tonic). Files: `Cargo.toml`.

**P-7. Clippy lint set.** `clippy::all` plus the denies listed in `strands-rs/AGENTS.md` ("Selected lints") and a selective pedantic set rather than `clippy::pedantic` wholesale (`must_use_candidate` and `module_name_repetitions` are noisy for a TS-named API). `print_stdout`/`print_stderr` are denied; the printer and examples opt out with scoped `#[expect]`. Files: `Cargo.toml`, `clippy.toml`.

**P-8. Run paths under `/mnt/workspace`.** The sandbox loses `/tmp` across restarts, so the spec export, donor clone and reference clone live in `/mnt/workspace/{ts-spec, donor-strandly, ref-witcherish}` with `/tmp/ts-spec`, `/tmp/donor-strandly` and `/tmp/ref-witcherish` as symlinks. Toolchains live in `/mnt/workspace/.rust`, Node 22.23.4 in `/mnt/workspace/.node`. Files: `.rust-port/STATE.md`, `.rust-port/gates.sh`.

**P-9. Branch persistence by push.** This environment is ephemeral between sessions and its instructions are to persist work by pushing a branch, which section 3 item 1 allows. Choice: push only `feat/rust-sdk` to the remote `fork` (`strandly-the-agent/harness-sdk`) as `feat/rust-sdk-run-20261115`; never force-push, never push anything else, never open a PR. Files: none.

**P-10. PARITY scope calls from Appendix B.** `storage/search/qmd` is out of scope (SQLite BM25 through `@tobilu/qmd`; `KeywordSearchStrategy` is the v0.1 strategy). `vended-memory-stores/test-memory-store` is out of scope (`TestMemoryStore` is not in the section 5.1 list; `FileMemoryStore` is the v0.1 local store). `logging/__tests__/logger` is in scope for the structured log format only. Harness rows live in `harness-rs/docs/PARITY.md`. Files: `strands-rs/docs/PARITY.md`, `harness-rs/docs/PARITY.md`.

**P-11. `extern crate self as strands` lands with the macros.** A placeholder `lib.rs` with it trips `unused_extern_crates` under `-D warnings`. It is added in Phase 3 with `#[tool]`, when vended tools use the macro inside the crate. Files: `strands-rs/strands/src/lib.rs`.

**P-12. No Rust complexity adapter for `pr-metrics` in v0.1.** Rust isn't scored by `.github/scripts/pr-metrics` at the pin. A `rust-code-analysis-cli` adapter would fit its "parse, never execute" rule, but it adds a new CI binary that can't be validated here. Choice: skip it; listed in follow-ups. Files: none.

**P-13. CI workflow files staged in `.rust-port/ci-workflows/`.** The token that persists this branch (P-9) is refused by GitHub for any commit touching `.github/workflows/` (no `workflow` scope). Options: drop the workflows, or keep them on the branch outside `.github/`. Choice: the five new workflows and the updated `ci.yml` live in `.rust-port/ci-workflows/`, validated with PyYAML and actionlint 1.7.12 but never run; moving them into `.github/workflows/` is a follow-up for someone with that scope. Files: `.rust-port/ci-workflows/*`.
