# delegation — test matrix (TS → Rust)

TS source: `strands-ts/src/agent/__tests__/agent-delegation.test.ts` (17 its, `AgentDelegation integration`).
Rust: `crates/strands-agents/src/agent/delegation.rs` (`mod tests`, feature `test-support`), run with
`cargo test --all-features -p strands-agents --lib delegation`.

Rows from `/mnt/workspace/survey3/m4b-tests-snapshots-delegation.md` §`agent-delegation.test.ts`, plus the
four rows the brief named (tool spec, handoff forwards messages, result returned, unknown-delegate error).

| TS test | Rust test | Status |
| --- | --- | --- |
| happy path: delegation result becomes the final assistant message, loop exits | `delegation_result_ends_the_turn_as_the_assistant_message` | pass |
| regular (non-delegation) tool runs normally, loop continues | `non_delegation_tool_keeps_looping` | pass |
| `cancels all tools when delegation tool is called alongside other tools, then retries` | `cancels_the_batch_when_a_delegation_tool_is_called_alongside_another_tool` | pass (verbatim cancel text asserted) |
| `cancels all tools when two delegation tools are called simultaneously, then retries` | — | skipped: same code path as the row above (`has_delegation && count > 1`); no extra coverage in Rust |
| `skips delegation when sub-agent returns empty text and loop continues` | `blank_delegation_content_does_not_end_the_turn` | pass |
| auto-registration is a no-op when nothing is registered | `default_plugin_is_a_no_op` | pass |
| `does not trigger delegation when sub-agent fails, loop continues` | — | skipped: needs `AgentAsTool` (error-result plumbing); the guard (`event.error.is_some() \|\| status != Success`) is implemented, untested |
| `does not promote the regular tool result as delegated when retry swaps to a non-delegation tool` | — | skipped: needs `AfterToolCallEvent.retry` + `AgentAsTool`; stale-mark clearing implemented, untested |
| `does not leak delegation state when a later AfterTools hook throws` | — | skipped: needs `AgentAsTool` |
| `child delegation does not leak to the parent agent` (nested) | — | skipped: needs `AgentAsTool` (per-agent state is keyed by `agent.id()`) |
| `does not trigger delegation when parent has a structured output schema` | — | skipped: Rust `AfterToolsEvent` exposes no structured-output flag; **not implemented** (deviation, see report) |
| `emits exactly one assistant MessageAddedEvent with delegated content (no placeholder)` | — | skipped: depends on how the Rust loop materializes `EndTurn::Blocks`; not asserted |
| `throws when delegation tools are present on a stateful model` | — | skipped: no `Model::stateful` equivalent reachable from the plugin in Rust yet; **not implemented** |
| `skips delegation logic for delegation tools added after init — tool runs normally` | — | skipped: stateful-model bypass, same reason |
| `does not enforce single-call constraint for stateful models with runtime-added tools` | — | skipped: stateful-model bypass, same reason |
| streaming: inner agent events surfaced natively in the parent stream (`ExecuteToolStage` middleware) | — | skipped: needs `AgentAsTool` + `ToolStreamUpdateEvent` unwrapping; **not implemented** |
| brief row: `delegate` tool spec verbatim (name/description/schema) | — | **not applicable**: TS has no `delegate` tool (see report §deviations) |
| brief row: handoff forwards messages to the delegate | — | **not applicable**: message forwarding lives in `AgentAsTool`, not in this plugin |
| brief row: delegate result returned to the parent | `delegation_result_ends_the_turn_as_the_assistant_message` | pass (as the final assistant message) |
| brief row: unknown delegate error text | — | **not applicable**: no name→agent registry in TS; unknown tool names are handled by the tool registry |

Counts: 5 Rust tests, 5 pass, 0 fail; 11 TS rows skipped (dependency on `AgentAsTool` / stateful models /
structured-output flag), 4 brief rows not applicable.
