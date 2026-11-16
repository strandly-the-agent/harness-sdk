# h12fixa: dispositions of the phase 12a review findings

| finding | disposition | commit |
|---|---|---|
| correctness-a1 | Fixed: interpreter steps run on a detached `std::thread` with the result over a `oneshot`; test `a_cancelled_busy_loop_does_not_hold_the_runtime_open` times the runtime drop. `cancels_an_in_flight_run_and_makes_no_further_inner_tool_calls` moved to the real clock: tokio's paused clock auto-advances while a step runs off the blocking pool. | 657334e3b |
| correctness-a2 | Fixed: when `execute` ends without an outcome, `call_tool` reads the queued `TaskMessage::Failed` and returns its error; test `types::agent::tests::call_tool::a_failing_execute_tool_middleware_surfaces_its_error`. | cdc991866 |
| parity-a1 | Fixed: new `ToolCallOutcome::HookCancelled(ToolResultBlock)` (P-990); the harness maps it to the `Failed to execute tool '<name>': <message>` refusal; `deny_blocks_the_inner_call_and_the_tool_never_runs` asserts the exact TS text. | the commit after cdc991866 (`fix(harness): a hook-cancelled inner call ...`) |
| parity-a2 | Not reached | |
| correctness-a3 | Not reached | |
| correctness-a4 | Not reached | |
| correctness-a5 | Not reached | |
| correctness-a6 | Not reached | |
| correctness-a7 | Not reached | |
| correctness-a8 | Not reached | |
| parity-a3 | Not reached | |
| parity-a4 | Not reached | |
| parity-a5 | Not reached | |
| parity-a6 | Not reached | |
| parity-a7 | Not reached | |
| parity-a8 | Not reached | |
