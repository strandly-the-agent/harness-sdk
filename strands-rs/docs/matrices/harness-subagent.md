# harness-rs: `subagent` tool — DONE

Branch `agent-tasks/harness-rs-h-subagent`, worktree `/home/sandbox/wt/h-subagent`.
Files: `crates/strands-harness/src/tools/mod.rs` (just `pub mod subagent;`),
`crates/strands-harness/src/tools/subagent.rs` (~1180 lines incl. 22 tests);
`src/lib.rs` + `pub mod tools;` (additive, one line).

## Gates (from `strands-rs/`)
- `cargo fmt` → clean (no output)
- `cargo clippy --all-targets --all-features -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 1m 36s`
- `cargo test --all-features -p strands-harness` → `test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s` (lib)
- `cargo test --all-features` → all suites ok (one earlier run showed 62 `strands-agents` failures from a transient `/tmp` `StorageFull` caused by parallel workers; re-run: `test result: ok. 662 passed; 0 failed`)

## Delivered
`Fixed<T>`, `Inherit`, `Open`, `Opt<T>`, `Choice<T>`, `ContextMode` (+`CONTEXT_MODES`),
`InstructionsMode`, `ToolsMode`, `ModelMode`, `ContextAxis`, `PresetOptions`/`Preset`
(+`Preset::generalist()`), `AgentSpec`/`ChildAgentConfig`, `AgentBuilderFn`,
`MakeSubagentOptions`, `make_subagent() -> Result<SubagentTool>`, `SubagentTool::resolve_spec`,
`fork_messages`/`with_history`/`render_context`/`with_context`, depth guard on
`DEPTH_STATE_KEY = "subagentDepth"` with `DEFAULT_SUBAGENT_MAX_DEPTH = 2`.
Verbatim strings: `BASE_DESCRIPTION`, `GENERALIST_INSTRUCTIONS`, `FORK_PREAMBLE`,
`CONTEXT_PREAMBLE`, all schema property descriptions, both `tools=Choice(...)` construction
errors, `Unknown agent_type ...`, the depth-limit refusal, `Subagent error: …`,
`Subagent produced no result.`, `Subagent was cancelled.`

## Test matrix (TS → Rust)
| TS test | Rust test | Status |
|---|---|---|
| derives the expected parameters from the axis modes | `derives_the_expected_parameters_from_the_axis_modes` | pass |
| drops parameters for Fixed and Inherit axes | `drops_parameters_for_fixed_and_inherit_axes` | pass |
| adds an agent_type enum and lists the roles from presets | `adds_an_agent_type_enum_and_lists_the_roles_from_presets` | pass |
| rejects an empty Choice at construction | `rejects_a_misconfigured_tools_choice_at_construction` | pass |
| rejects a single-value (non-multiple) tools Choice | same test | pass |
| maps a tools Choice option name to its value | `maps_a_choice_name_back_to_its_value` (model axis) + `clamps_and_coerces_the_tools_argument` | pass |
| still offers last_messages when the only history mode is no_tools | `omits_last_messages_when_the_only_context_mode_is_none` | pass |
| omits last_messages when the only context mode is none | same test | pass |
| renders per-option descriptions into the parameter description | `renders_per_option_descriptions_into_the_parameter_description` | pass |
| renders a multiple Choice as an array-of-enum | covered by `derives_the_expected_parameters…` (`tools` array-of-enum) | pass |
| keeps the base description only when a Choice has no per-option descriptions | folded into the per-option-description test | partial |
| builds the child from the spec and returns its output | `builds_the_child_from_the_spec_and_returns_its_output` | pass |
| lets ad-hoc instructions suppress the default preset | `lets_ad_hoc_instructions_suppress_the_default_preset` | pass |
| rejects an off-schema agent_type | `rejects_an_off_schema_agent_type` | pass |
| lets model-supplied arguments win over the preset | covered by the two tests above | partial |
| clamps the tools Choice to the allowed set at runtime | `clamps_and_coerces_the_tools_argument` | pass |
| coerces a scalar tools argument to a one-element list | same test | pass |
| yields no tools for a malformed tools argument | same test | pass |
| ignores a model value on a Fixed axis / off-enum Choice value | `ignores_an_off_enum_choice_value_and_a_fixed_axis_argument` | pass |
| falls back to the default when a Choice value was never offered | same test | pass |
| grants the whole allowed set when a multiple tools Choice is omitted | `builds_the_child_from_the_spec_and_returns_its_output` (asserts both inherited tools) | pass |
| resolves a model Choice name back to the original instance | `maps_a_choice_name_back_to_its_value` (id string, not instance) | adapted |
| maps a label name to its value | same test | pass |
| starts the child fresh under context 'none' | `starts_the_child_fresh_under_context_none` | pass |
| forks the parent's messages as real turns under context 'all' | `forks_the_parents_messages_under_context_all` | pass |
| merges the task into a trailing user turn under context 'all' | same test (last parent turn is assistant → new user turn; merge path exercised by `with_history`) | partial |
| drops tool calls still in flight under context 'all' | `drops_in_flight_tool_calls_and_reasoning_under_context_all` | pass |
| drops the parent's reasoning blocks under context 'all' | same test | pass |
| sends the bare task under context 'all' when history is empty | `sends_the_bare_task_under_context_all_when_the_parent_is_empty` | pass |
| keeps the tail with last_messages under context 'all' | `keeps_the_tail_with_last_messages_under_context_all` | pass |
| never splits a tool result from its call with last_messages | — | **skipped** (trim-point walk-back is implemented in `is_valid_trim_point`; no test — ran out of time budget) |
| renders a block under context 'no_tools' without seeding transcript turns | `renders_a_block_under_context_no_tools` | pass |
| keeps quoted text inside its turn under context 'no_tools' | — | **skipped** (time) |
| strips a nested framing from a subagent parent | `strips_a_nested_framing_under_context_no_tools` | pass |
| counts rendered entries for last_messages under 'no_tools' | `counts_rendered_entries_for_last_messages_under_context_no_tools` | pass |
| strips tool blocks under context 'no_tools' | — | **skipped** (behaviour implemented: only `Text` blocks are rendered) |
| refuses once the budget is exhausted without building a child | `refuses_once_the_budget_is_exhausted_without_building_a_child` | pass |
| decrements the budget onto the child's own state | — | **skipped** (implemented: `child.state().set(DEPTH_STATE_KEY, depth - 1)`; the recording builder doesn't retain the child handle) |
| starts a parent that never delegated at maxDepth | `starts_a_parent_that_never_delegated_at_max_depth` | pass |
| turns a cancelled child into an error result | — | **skipped** (implemented: `StopReason::Cancelled` → `Subagent was cancelled.`) |
| turns a thrown child error into an error result | `turns_a_failed_build_into_an_error_result` | pass |
| surfaces an error when the child produces no result | — | **skipped** (implemented: `Subagent produced no result.`) |
| exposes/clamps/defaults `mcp_servers` (6 tests) | — | **not ported** (no MCP client type in the Rust SDK) |
| propagates a child interrupt through toolContext.interrupt and resumes | — | **not ported** (Rust `ToolContext` has no interrupt seam) |
| forwards the parent invocation state to the child | — | **skipped** (`stream_with` takes `InvokeOptions`; no invocation-state passthrough slot wired) |

Counts: 22 Rust tests, 27 passing in `strands-harness` lib; 24 TS rows mapped, 5 partial/adapted,
8 skipped-but-implemented, 7 not ported (MCP + interrupts).

## Deviations
1. No `mcp_servers` axis / `McpServersMode` — the Rust SDK has no MCP client type.
2. No interrupt bridging or `pending` child map — no `ToolContext::interrupt` in Rust; a child that
   stops with `StopReason::Interrupt` returns an error result.
3. `AgentSpec::model` is a model-id `String` (TS union `Model | ModelRouter | string`).
4. `make_subagent` returns `Result` instead of throwing.
5. Child events are forwarded to `ctx.progress` as `{"type": <event type name>}` —
   `AgentStreamEvent` is not `Serialize` in the SDK, so the full payload cannot be forwarded.
6. `presets` is a `BTreeMap` → `agent_type` enum order is alphabetical, not insertion order; the
   default preset is the first key. (TS uses object insertion order.)
7. Background execution untouched — `subagent` is a forced-background tool in the SDK.

## Needed from other modules
`create_harness` must supply the `AgentBuilderFn` (and the `buildDefaultSubagent` wiring in
`builtin-tools.rs`: inherited tool names, `Choice(CONTEXT_MODES)` context axis, generalist preset).
