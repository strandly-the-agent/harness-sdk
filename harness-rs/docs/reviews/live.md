# Live test results

Results of the harness live tests (`harness-rs/strands-harness/tests/live_e2e.rs`) and of the cross-SDK session check (`harness-rs/strands-harness/tests/cross_sdk/`), from 2026-11-16. No secrets, account ids or response bodies are recorded here.

## How the sweep was run

```bash
STRANDS_TEST_NODE=<node binary> \
STRANDS_TEST_NODE_MODULES=<a node_modules with @modelcontextprotocol/sdk and zod> \
STRANDS_CROSS_SDK_HARNESS_TS=<a harness-ts checkout with its dependencies installed> \
  cargo test -p strands-harness --all-features -- --ignored live_ --nocapture > /tmp/h12live_run.txt 2>&1; echo "EXIT: $?"
```

AWS credentials came from the default chain and were used for Bedrock and Bedrock Mantle model calls only; the tests create, change and delete no AWS resource. `AWS_REGION` was `us-west-2`. `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` were not set. Every output file was scanned for key patterns and environment values before it was read: all clean.

| Run | Commit | `live_` tests | Ran and passed | Skipped (key missing) | Failed | Exit |
|---|---|---|---|---|---|---|
| 1 | `19461dc70` | 13 | 11 | 2 | 0 | 0 |
| 2 | `19461dc70` | 13 | 11 | 2 | 0 | 0 |

A skipped test returns early and the test harness prints `ok` for it. The 2 skips are the `skipping:` lines (`--nocapture`): `skipping: ANTHROPIC_API_KEY is not set` and `skipping: OPENAI_API_KEY is not set`. No request was sent to Anthropic or OpenAI. Without the three variables above, the MCP flow and the cross-SDK tests skip with a message too.

Every test runs in a temporary directory: the session directory, the memory directory and every path given to the model are absolute paths inside it, the MCP fixture is copied into it, and the TypeScript process of the cross-SDK check has it as its working directory. Assertions read the message trace and side effects (tool attempted or succeeded, parsed input, tool-result content, files on disk, stop reason, usage), as the TS helpers `toolAttempted`, `toolSucceeded` and `toolResultContains` do.

## Per provider: `tests/live_e2e.rs`

TS spec: `harness-ts/test/integration/e2e.test.ts` (helpers and the testing system prompt of `harness-ts/test/integration/harness.ts`).

One flow runs per provider. A first agent (`builtin_tools: [write]`, the MCP server `harness-ts/test/tools/echo-mcp-server.mjs` under the name `fixture`, `interventions: "ask"`, a session with a fixed id) is asked to write a file and then call `fixture_echo`. Checks: the first result stops with `interrupt`, its approval prompt starts with `Approve "write"` and carries the file path, the file does not exist yet and the tool use is not in the history; after answering every interrupt with `yes` the turn ends `endTurn` with at least two interrupts answered, input and output tokens above zero, `write` succeeded with the expected `path` input and the file on disk holds the marker, `fixture_echo` succeeded with input `text: pong-77` and result `echo:pong-77`, and the session folder exists. A second `create_harness` with the same session id (`builtin_tools: [read]`) restores a history equal to the first agent's, and a turn that does not repeat the file path ends `endTurn` with a `read` call on that path whose result holds the marker; the restored messages are an unchanged prefix of the new history.

| Test | Model | Region | Run 1 | Run 2 |
|---|---|---|---|---|
| `live_bedrock_tool_call_mcp_tool_interrupt_resume_and_session_resume` | `bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0`, effort `off` | `us-west-2` | pass | pass |
| `live_bedrock_mantle_tool_call_mcp_tool_interrupt_resume_and_session_resume` | `bedrock-mantle/openai.gpt-oss-120b`, effort `low` | `us-west-2` | pass | pass |
| `live_anthropic_tool_call_mcp_tool_interrupt_resume_and_session_resume` | `anthropic/claude-haiku-4-5`, effort `off` | - | skipped: `ANTHROPIC_API_KEY is not set` | skipped |
| `live_openai_tool_call_mcp_tool_interrupt_resume_and_session_resume` | `openai/gpt-5.4-mini`, effort `off` | - | skipped: `OPENAI_API_KEY is not set` | skipped |

The Bedrock Mantle test runs at effort `low`. With effort `off` the harness sends `reasoning.effort: "none"` on Bedrock Mantle, as the TypeScript harness does, and `openai.gpt-oss-120b` answered every such request (3 of 3 from Rust, 1 of 1 from the TypeScript harness, against 3 of 3 and 1 of 1 passing at `low`) with

```
The server had an error while processing your request. Sorry about that!
```

(error code `server_error`). One earlier run of the Mantle test at `low` failed on capacity and passed when repeated:

```
503 Exceeded on-demand capacity. Ensure your workload does not double faster than once per 30 minutes.
```

## Bedrock only: the rest of the TS integration checklist

Model `bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0`, effort `off`, region `us-west-2`.

| Test | TS case | Run 1 | Run 2 | Checks |
|---|---|---|---|---|
| `live_bedrock_uses_the_shell_tool` | `e2e.test.ts`: uses the shell tool | pass | pass | `shell` succeeded, its result holds the echoed marker, the file the command wrote in the temporary directory holds it |
| `live_bedrock_uses_the_file_and_web_tools_the_todos_plugin_and_the_subagent_in_one_turn` | `e2e.test.ts`: uses the file/web tools, the todos plugin, and the subagent in one turn; uses the todo_write tool from the todos plugin | pass | pass | `read`, `write`, `edit`, `web_fetch`, `todo_write` and `subagent` each succeeded; the `read` result holds `4271`; the written file holds `4271` and the edited file `omega`; the subagent's sum `333` is in the conversation; `endTurn` |
| `live_bedrock_blocks_a_denied_tool_call_via_interventions` | `e2e.test.ts`: blocks a denied tool call via interventions | pass | pass | the interrupt is answered `false`: `write` attempted, not succeeded, its result starts `CONFIRMATION_FAILED: Approve`, no file; `endTurn` |
| `live_bedrock_persists_a_session_with_a_minted_id` | `e2e.test.ts`: persists a session with a minted id | pass | pass | one folder under the session directory, named by 8 hex digits, holding `snapshot_latest.json` |
| `live_bedrock_persists_memory_across_agents` | `e2e.test.ts`: persists memory across agents (fresh conversation, no session) | pass | pass | after `flush`, a `.md` file in the memory directory holds the fact; a second agent sharing only that directory has the fact in its conversation; `endTurn` |
| `live_bedrock_cache_reads_grow_across_turns` | `caching.test.ts`: reads grow across turns | pass | pass | cumulative `cache_read_input_tokens` is above zero after three turns and grows on every turn |
| `live_exa_web_search_returns_parsed_results` | `web-search.test.ts`: returns parsed results | pass | pass | the public `exa_web_search` tool, keyless: a `Sources:` list whose entries each have a title and an `http` URL (skips when the keyless rate limit is hit) |

The TS cases `allows an approved tool call via interventions` and `persists sessions across agents` are covered by the per-provider flow.

## Cross-SDK session check: `tests/cross_sdk/`

`main.rs` holds the Rust side and drives `session.mts`, the TypeScript side, which loads `createHarness` from the harness-ts checkout and runs in the temporary directory, so both SDKs use that directory's `./.agent/sessions`. Both sides use `bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0` (effort `off`, tools `write` and `read`, no plugins, skills and memory off), region `us-west-2`. The harness-ts checkout was a scratch copy of the pinned sources with the pinned `strands-ts` packed and installed into it as version `1.999.0`.

```bash
# In a scratch copy of strands-ts and harness-ts at the pin:
(cd strands-ts && npm version 1.999.0 --no-git-tag-version && npm pack --pack-destination <dir>)
(cd harness-ts && npm install && npm install --no-save <dir>/strands-agents-sdk-1.999.0.tgz)

STRANDS_TEST_NODE=<node binary> STRANDS_CROSS_SDK_HARNESS_TS=<scratch>/harness-ts \
  cargo test -p strands-harness --all-features --test cross_sdk -- --ignored live_cross_sdk --nocapture
```

The TypeScript side is run by the tests as

```bash
node <harness-ts>/node_modules/tsx/dist/cli.mjs tests/cross_sdk/session.mts <session id> \
  [--expect <messages.json>] [--prompt <text>] [--dump <messages.json>]
```

| Test | Direction | Run 1 | Run 2 | Checks |
|---|---|---|---|---|
| `live_cross_sdk_rust_resumes_a_session_written_by_typescript` | TypeScript writes, Rust resumes | pass | pass | TypeScript runs a turn with a `write` call and dumps its history; Rust restores a history equal to it, runs a turn that reads the file named only in the restored history (`read` result holds the marker, the restored messages are an unchanged prefix); TypeScript then restores a history equal to the one Rust ended with |
| `live_cross_sdk_typescript_resumes_a_session_written_by_rust` | Rust writes, TypeScript resumes | pass | pass | the mirror image: Rust writes, TypeScript restores an equal history and runs the reading turn, Rust restores a history equal to the one TypeScript ended with |

Histories are compared as JSON values of the messages in their wire shape (`Message.toJSON()` in TypeScript, the `serde` form in Rust). No format difference was found in either direction.

## Test bugs fixed before the sweeps

These were errors in the new tests, found on their first runs and corrected; none was a harness defect.

- The per-provider flow expected the gated `write` tool use in the history while the agent was paused; the SDK appends the assistant message together with the tool results, after the tools ran, as the TypeScript SDK does.
- The denied-call test looked for `denied` in the tool result; a rejected confirmation reads `CONFIRMATION_FAILED: <prompt>`.
- The Exa test split a result line at the first ` — `; a title can hold one, so it now splits at the last.

No harness defect was found, and nothing under `src/` was changed.

## Not covered

- `e2e.test.ts`, `injects the environment context (project AGENTS.md)`: the environment plugin reads the process's working directory, and the tests neither change it nor run inside the repository. The plugin is also left out of the six-tool and caching tests for that reason, where TS enables it.
- Anthropic and OpenAI (keys missing): the two tests are kept and skip.
- The cross-SDK check covers a completed turn with a tool call. It does not cover a session saved while an interrupt is pending, or the context manager's stash files.
