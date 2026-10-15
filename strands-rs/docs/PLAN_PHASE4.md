# Phase 4 plan — closing the feature matrix

Integration branch: `agent-tasks/strands-rs` (fork `strandly-the-agent/harness-sdk`). Each
workstream lives on `agent-tasks/strands-rs-<module>`, branched from the integration branch, and is
merged back after review. [FEATURE_MATRIX.md](FEATURE_MATRIX.md) rows below flip from ❌ to ✅ as
modules land.

## Workstreams

| Branch suffix     | Scope                                                                    | TS spec                                                                                       | Python counterpart                                      |
| ----------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| `injection`       | Message/context injection (`injection/`) + Printer callback handler      | `strands-ts/src/injection/`, `strands-ts/src/agent/printer.ts`                                | `strands-py/src/strands/injection`, `handlers/`         |
| `memory-stores`   | `BedrockKnowledgeBaseMemoryStore`, `TestMemoryStore`                     | `strands-ts/src/vended-memory-stores/{bedrock-knowledge-base,test-memory-store}`              | `strands-py/src/strands/vended_memory_stores`           |
| `s3`              | `S3Storage` (storage) + `S3SessionStorage` (session)                     | `strands-ts/src/storage/s3-storage.ts`, `strands-ts/src/session/s3-storage.ts`                | `strands-py/src/strands/storage`, `session`             |
| `sandboxes`       | `DockerSandbox`, `SshSandbox`                                            | `strands-ts/src/sandbox/{docker,ssh,base,posix-shell,stream-process}.ts`                      | `strands-py/src/strands/sandbox`                        |
| `cedar`           | Cedar policy intervention                                                | `strands-ts/src/vended-interventions/cedar/`                                                  | `strands-py/src/strands/vended_interventions/cedar`     |
| `steering`        | Steering (LLM handler, tool ledger / context providers)                  | `strands-ts/src/vended-interventions/steering/`                                               | `strands-py/src/strands/vended_plugins/steering`        |
| `goal`            | Goal plugin (goal loop + judge)                                          | `strands-ts/src/vended-plugins/goal/`                                                         | `strands-py/src/strands/vended_plugins/goal`            |
| `vended-tools`    | SDK vended tools: `file_editor`, `http_request`, `sleep`, `handoff_to_user`, `mcp_router` | `strands-ts/src/vended-tools/{file-editor,http-request,sleep,handoff-to-user,mcp-router}` | `strands-py/src/strands/vended_tools`             |

Out of scope this phase: `python_repl`, `notebook` (need a Python runtime), `a2a_client`,
multi-agent, bidi, decisions.

## Rules every workstream follows

1. **TS is the behavioural spec; Python is the tie-breaker.** Where TS and Python differ, prefer TS
   and note the choice in the module doc comment. Port behaviour and tests faithfully; port the API
   faithfully *unless* Rust idiom says otherwise — then follow the shapes already in this crate
   (see [DESIGN.md](DESIGN.md) "Core shapes" and "Conventions") and record the divergence in
   DESIGN.md.
2. **Consistency with the existing crate beats consistency with TS.** Before designing, read the
   nearest existing sibling (e.g. `FileMemoryStore` for a memory store, `LocalFileStorage` for a
   storage, `LocalSandbox` for a sandbox, `HumanInTheLoop` for an intervention, `ContextInjector`
   / `AgentSkills` for a plugin, `ShellTool` for a vended tool) and mirror its constructor /
   config / error / feature-flag pattern.
3. **AWS-backed modules** (`aws-sdk-s3`, `aws-sdk-bedrockagentruntime`, …) sit behind a cargo
   feature (`s3`, `bedrock-kb`, …) in the pattern `bedrock` already uses; unit tests use
   constructed SDK types or `aws-smithy-mocks` / `StaticReplayClient`; live tests are
   `#[ignore = "live …"]` in `crates/strands-agents/tests/integ_<module>.rs` and skip with a
   message when the env they need is unset.
4. **Every TS `it(...)` in scope gets a Rust test with the same intent**, snake_case name, next to
   the code (`#[cfg(test)] mod tests`). Cross-module behaviour (agent loop + the new module) gets a
   test in `crates/strands-agents/tests/<module>_loop.rs` using `MockMessageModel`.
5. **Gates** (run from `strands-rs/`): `cargo fmt --all -- --check`,
   `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`,
   `cargo build --no-default-features -p strands-agents`, `cargo doc --no-deps --all-features`.
6. Write a per-module traceability matrix at `docs/matrices/<module>.md` (TS test → Rust test;
   TS API → Rust API; divergences) and flip the FEATURE_MATRIX row(s).
7. Commit early and often; push the branch to `origin` (the fork) after every green gate run.

## Live test environment (this sandbox)

- Bedrock Converse works (`AWS_REGION=us-west-2`), e.g.
  `us.anthropic.claude-haiku-4-5-20251001-v1:0`.
- S3: bucket `strandly-managed-strands-rs-e2e` (us-west-2) exists; put/get/list/delete objects
  work. Bucket create works for `strandly-managed-*` names; bucket tagging/delete do not.
- Bedrock Knowledge Bases: none readable; creation needs the pre-made `*_managed_kb_role` and a
  vector store — attempt it, otherwise the live test skips on missing `STRANDS_TEST_KB_ID`.
- Docker is not installed; `ssh` client is, `sshd` is not (try `dnf install openssh-server`).
