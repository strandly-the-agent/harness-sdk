| finding | disposition | commit |
|---|---|---|
| correctness-1 | Fixed: sub-agents (`as_tool`, vended `subagent`) join `CancelSignal::abort_only()`, so a parent's deferral and message don't reach the child (P-97); test `agent_as_tool::tests::cancel::a_deferred_parent_cancel_lets_the_wrapped_agent_finish` fails without the fix | 47f8fb0ca |
| correctness-2 | Fixed: the plugin registry resumes at the interrupted plugin and records a name after `init_agent` returns (P-98); test `plugins::plugin::tests::registry::a_dropped_initialize_resumes_at_the_interrupted_plugin` | 47f8fb0ca |
| correctness-3 | Fixed: an abandoned run stays pending until its hooks return (P-98); test `drop_guard::a_dropped_call_retries_the_deferred_hooks_it_did_not_finish` | 47f8fb0ca |
| correctness-4 | Fixed: the deferred hooks run under a cancelled signal the cancel handle targets (P-98, amends P-91); test `drop_guard::the_deferred_hooks_see_a_cancelled_signal_so_background_tasks_do_not_block_the_next_call` | 47f8fb0ca |
| parity-1 | Fixed: ported `collects_interrupts_from_multiple_before_tools_event_hooks` and `handles_before_tool_call_event_interrupt_in_concurrent_mode`; PARITY row now 29/32 with the 3 not-applicable cases | 577385be4 |
| parity-2 | Fixed, matching TS: an empty `Cancel::Message` is no cancel at the four read points (P-99); test `cancel::an_empty_hook_cancel_message_does_not_cancel` | 577385be4 |
| parity-3 | Fixed: `percent` uses the reviewer's `f64` formula; `percent(300, 200_000) == "0.1"` added; P-96 corrected in P-102 | 577385be4 |
| parity-4 | Fixed: the interrupt-run conformance test compares both invocations' streams with `interrupt_run.json` and `interrupt_resume_run.json` (the weather tool now carries the fixture's input schema) | 577385be4 |
| api-1 | Fixed: both configs are exhaustive config structs with the standard `#[expect]`; struct-literal cases added to `tests/api_surface.rs` (P-102) | e5266f8e4 |
| api-2 | Fixed: `Agent::cancel_signal` removed (P-100, amends P-90's name list); crate tests read `agent.local.cancel_signal()` | e5266f8e4 |
| api-3 | Kept the name, recorded the repurposing in the P-15 name map (P-101), as the review allows: the TS class is crate-internal in Rust and the public type is what a user writes at `backgroundTasks` | e5266f8e4 |
| api-4 | Not done in this round (time): the replayed failure is still `Error::Other` with the text only. Proposed shape: keep `Arc<Error>`, replay through a private wrapper whose `source()` is the original, and have `Error::name()` return the original's name | - |
| api-5 | Not done in this round (time): no doc examples added on `ConversationManager`, `SlidingWindowConversationManagerConfig` or `BackgroundTasksConfig` | - |
| api-6 | Fixed: `SlidingWindowConversationManagerConfig` derives `Clone` only (P-102); `SlidingWindowConversationManager::new` carries a `needless_pass_by_value` expectation | e5266f8e4 |
