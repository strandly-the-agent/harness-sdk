# `plugins::vended::goal` — `GoalLoop` (strands-rs)

Branch `agent-tasks/strands-rs-goal`. Port of `strands-ts/src/vended-plugins/goal`
(`plugin.ts`, `judge.ts`, `index.ts`).

Files: `crates/strands-agents/src/plugins/vended/goal.rs` (impl + 12 unit tests),
`crates/strands-agents/tests/goal_loop.rs` (21 agent-loop tests),
`crates/strands-agents/tests/integ_goal.rs` (2 live Bedrock tests); exports in
`plugins/vended/mod.rs`.

Gates (last line each):
```
cargo fmt --all                                                      -> (no output)
cargo clippy -p strands-agents --all-features --tests -- -D warnings -> Finished `dev` profile in 37.26s
cargo test -p strands-agents --all-features --lib goal::             -> test result: ok. 12 passed
cargo test -p strands-agents --all-features --test goal_loop         -> test result: ok. 21 passed
AWS_REGION=us-west-2 cargo test -p strands-agents --all-features --test integ_goal -- --ignored
                                                                     -> test result: ok. 2 passed
```

## API (TS → Rust)

| TS | Rust |
| --- | --- |
| `new GoalLoop({ goal: 'string' })` | `GoalLoop::goal("…")` |
| `new GoalLoop({ goal: async ctx => … })` | `GoalLoop::validator(f)` (async) / `GoalLoop::sync_validator(f)` |
| `{ name, maxAttempts, timeout, preserveContext, judge, resumePromptTemplate }` options object | chained `with_name` / `with_max_attempts` / `with_timeout` / `with_preserve_context` / `with_judge` / `with_resume_prompt_template` |
| `timeout` in ms | `with_timeout(Duration)` |
| `ValidationContext { response, agent }` | `ValidationContext { response: Message, agent: Agent }` |
| validator returns `boolean \| { passed, feedback }` | returns `Result<ValidationOutcome>`; `ValidationOutcome::{passed, failed}` and `From<bool>` |
| validator `throw` | validator `Err(_)` → failing attempt with `Validator error: …` feedback |
| `goalLoop.lastResult(agent)` | `goal.last_result(&agent) -> Option<GoalResult>` |
| `GoalResult { passed, stopReason, attempts }` | same; `GoalStopReason::{Satisfied, MaxAttempts, Timeout}`, `GoalAttempt { attempt, passed, feedback }` |
| `JudgeConfig { model, systemPrompt }` | `JudgeConfig::default().with_model(..).with_system_prompt(..)` |
| `JUDGE_SYSTEM_PROMPT` | `JUDGE_SYSTEM_PROMPT` (byte-identical) |
| `JUDGE_OUTCOME_SCHEMA` (zod) | `judge_outcome_schema()` (schemars, from `JudgeOutcome`) |
| `buildJudgePrompt(goal, transcript)` | `build_judge_prompt(&str, &[Message])` |
| `resumePromptTemplate(feedback?) => string` | `with_resume_prompt_template(|Option<&str>| -> InvokeInput)` |

## Test matrix (TS `it(...)` → Rust test)

`__tests__/plugin.test.ts` / `judge.test.ts`. "unit" = `goal.rs` `mod tests`, "loop" =
`tests/goal_loop.rs`.

| TS test | Rust test | Where | Status |
| --- | --- | --- | --- |
| uses default name and unbounded budgets | `uses_default_name_and_unbounded_budgets` | unit | pass |
| accepts custom name | `accepts_custom_name` | unit | pass |
| throws when goal is not provided | — | — | n/a: `Goal` is an enum argument, so there is no "missing goal" state |
| throws when maxAttempts < 1 | `rejects_max_attempts_below_one` + `max_attempts_below_one_is_rejected_at_attach_time` | unit + loop | pass (rejected by `init_agent`, not the constructor) |
| throws when timeout < 1 | `rejects_zero_timeout` | unit | pass |
| does not warn when maxAttempts is set | — | — | gap: no log-capture harness in the crate (same gap as `skills`) |
| passes on first attempt with no resume | `stops_after_one_attempt_when_the_validator_passes` | loop | pass |
| feeds feedback as user message and re-invokes until passing | `resumes_with_the_feedback_and_counts_the_attempts` | loop | pass |
| hits maxAttempts and surfaces lastResult without throwing | `stops_at_max_attempts_without_passing` | loop | pass |
| treats validator throws as a failed attempt and warns | `a_validator_error_becomes_failing_feedback` | loop | pass (warning itself not asserted) |
| awaits async validator | `an_async_validator_stops_the_run_on_the_wall_clock_budget` | loop | pass (async validator + timeout in one test) |
| exposes the assistant message to the validator | `resumes_with_the_feedback_and_counts_the_attempts` (records every `ctx.response`) | loop | pass |
| honours timeout by terminating before next validate | `an_async_validator_stops_the_run_on_the_wall_clock_budget` | loop | pass |
| lastResult is undefined before first invoke | `last_result_is_none_before_any_run_and_per_agent` | loop | pass |
| lastResult is replaced on each completed run | `a_second_invocation_replaces_the_previous_result` | loop | pass |
| lastResult is undefined after a host throw mid-run | `a_failed_invocation_leaves_no_result_and_does_not_leak_into_the_next_run` | loop | pass |
| lastResult is undefined while a run is mid-flight | `last_result_is_none_while_the_run_is_in_flight` | loop | pass |
| feedback messages accumulate as user-role messages | `resumes_with_the_feedback_and_counts_the_attempts` | loop | pass |
| resumePromptTemplate replaces the default canned English | `the_resume_prompt_can_be_overridden` | loop | pass |
| shares one plugin across agents with independent run state | `one_plugin_instance_serves_several_agents_with_independent_run_state` | loop | pass |
| throws when two GoalLoops are attached to the same agent | `only_one_goal_loop_can_attach_to_an_agent` | loop | pass |
| preserveContext false: agent never sees prior attempts | `preserve_context_false_hides_the_earlier_attempts_from_the_model` | loop | pass |
| preserveContext false: does not roll back appState | `preserve_context_false_does_not_roll_back_the_agent_state` | loop | pass |
| preserves context by default | `preserve_context_true_keeps_the_earlier_attempts_in_the_prompt` | loop | pass |
| clears stale lastResult after a thrown invoke | `a_failed_invocation_leaves_no_result_and_does_not_leak_into_the_next_run` | loop | pass |
| JUDGE_OUTCOME_SCHEMA validates pass/fail shapes | `judge_outcome_schema_describes_pass_fail_shape` | unit | pass |
| buildJudgePrompt embeds goal + role-tagged transcript | `build_judge_prompt_embeds_goal_and_role_tagged_transcript` | unit | pass |
| buildJudgePrompt summarises tool calls and results | `build_judge_prompt_summarises_tool_calls_and_results` | unit | pass |
| buildJudgePrompt truncates very long tool inputs/outputs | `build_judge_prompt_truncates_long_tool_inputs` | unit | pass |
| judges via the host agent model by default | `the_judge_falls_back_to_the_host_model` | loop | pass |
| judges via the host auxModel when no judge model | `the_judge_drives_the_loop_on_the_aux_model` | loop | pass |
| feeds judge feedback back to the host agent | `the_judge_drives_the_loop_on_the_aux_model` | loop | pass |
| passes when the judge replies in prose then retries structured | `a_judge_that_replies_in_prose_is_retried_for_structured_output` | loop | pass |
| builds a fresh judge agent per validation | `the_judge_drives_the_loop_on_the_aux_model` (asserts two judge calls, no prompt leakage) | loop | pass |
| uses judge.model override | `judge_model_overrides_the_aux_model` | loop | pass |
| uses judge.systemPrompt override | `the_judge_system_prompt_can_be_overridden` | loop | pass |

Rust-only coverage (no TS counterpart): `validation_outcome_from_bool_is_the_ts_shorthand`,
`default_resume_prompt_includes_feedback_when_present`, `finish_records_stop_reason_and_pass_flag`,
`last_assistant_message_picks_the_latest_assistant_turn`.

## Divergences

- Config is chained instead of an options object, so invalid values (`max_attempts = 0`, zero
  `timeout`) are rejected by `Plugin::init_agent` rather than the constructor — no panicking
  constructor, same validate-at-use point as `Limits`.
- A validator is a closure in two forms (`validator` async, `sync_validator`); a TS validator
  `throw` maps to `Err`.
- The judge's structured-output schema is derived from `JudgeOutcome` with `schemars` where TS
  uses zod.
- `JUDGE_SYSTEM_PROMPT` and `default_resume_prompt` were compared byte-for-byte against TS.
- The judge prompt truncates on `char` boundaries where TS counts UTF-16 code units (identical
  for ASCII).
- One `GoalLoop` per agent is enforced through a process-global `Weak<AgentInner>` registry
  (TS uses a per-agent symbol).

## Still open

- `tests/integ_goal.rs` (live Bedrock, `#[ignore]`) covers the judge path and a validator retry
  against `us.anthropic.claude-haiku-4-5-20251001-v1:0`; both pass.
- The two log-assertion gaps above need a log-capture harness to close.
