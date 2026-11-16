# Phase 4 review: interrupts, cancellation, lifecycle, plugins and middleware

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed `743a1880d..f5e53f220`, excluding the later-phase paths that landed in the same range (providers, MCP, sandbox, vended tools, storage, sessions). The parity validator ran `gates.sh full` at `f5e53f220`, which passed. Findings marked open are not fixed yet; the phase stays open until they are.

| finding | disposition | commit |
|---|---|---|
| correctness-1 | Fixed: sub-agents (`as_tool`, vended `subagent`) join `CancelSignal::abort_only()`, so a parent's deferral and message don't reach the child (P-97); test `agent_as_tool::tests::cancel::a_deferred_parent_cancel_lets_the_wrapped_agent_finish` fails without the fix | c2a701fbd |
| correctness-2 | Fixed: the plugin registry resumes at the interrupted plugin and records a name after `init_agent` returns (P-98); test `plugins::plugin::tests::registry::a_dropped_initialize_resumes_at_the_interrupted_plugin` | c2a701fbd |
| correctness-3 | Fixed: an abandoned run stays pending until its hooks return (P-98); test `drop_guard::a_dropped_call_retries_the_deferred_hooks_it_did_not_finish` | c2a701fbd |
| correctness-4 | Fixed: the deferred hooks run under a cancelled signal the cancel handle targets (P-98, amends P-91); test `drop_guard::the_deferred_hooks_see_a_cancelled_signal_so_background_tasks_do_not_block_the_next_call` | c2a701fbd |
| parity-1 | Fixed: ported `collects_interrupts_from_multiple_before_tools_event_hooks` and `handles_before_tool_call_event_interrupt_in_concurrent_mode`; PARITY row now 29/32 with the 3 not-applicable cases | 17d13485f |
| parity-2 | Fixed, matching TS: an empty `Cancel::Message` is no cancel at the four read points (P-99); test `cancel::an_empty_hook_cancel_message_does_not_cancel` | 17d13485f |
| parity-3 | Fixed: `percent` uses the reviewer's `f64` formula; `percent(300, 200_000) == "0.1"` added; P-96 corrected in P-102 | 17d13485f |
| parity-4 | Fixed: the interrupt-run conformance test compares both invocations' streams with `interrupt_run.json` and `interrupt_resume_run.json` (the weather tool now carries the fixture's input schema) | 17d13485f |
| api-1 | Fixed: both configs are exhaustive config structs with the standard `#[expect]`; struct-literal cases added to `tests/api_surface.rs` (P-102) | 08c1d879d |
| api-2 | Fixed: `Agent::cancel_signal` removed (P-100, amends P-90's name list); crate tests read `agent.local.cancel_signal()` | 08c1d879d |
| api-3 | Kept the name, recorded the repurposing in the P-15 name map (P-101), as the review allows: the TS class is crate-internal in Rust and the public type is what a user writes at `backgroundTasks` | 08c1d879d |
| api-4 | Fixed: the registry keeps the failure as `Arc<Error>` and every attempt, the first included, returns it through a private wrapper: same text, same `Error::name()`, the original as `source()`. Test `plugins::plugin::tests::registry::every_initialization_attempt_reports_the_same_error_name_text_and_source` (P-340) | a5bca5b46 |
| api-5 | Fixed: doc examples on `ConversationManager` (the TS `Last10MessagesManager` example), `SlidingWindowConversationManagerConfig` and `BackgroundTasksConfig` | a5bca5b46 |
| api-6 | Fixed: `SlidingWindowConversationManagerConfig` derives `Clone` only (P-102); `SlidingWindowConversationManager::new` carries a `needless_pass_by_value` expectation | 08c1d879d |

After the last fixes, `.rust-port/gates.sh full` passed at `a5bca5b46` (all steps, 3455 tests with all features).
