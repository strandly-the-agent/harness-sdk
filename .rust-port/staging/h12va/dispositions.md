# Dispositions of track h12va (Phase 12 verification pass, harness side)

| finding | disposition | commit |
|---|---|---|
| correctness-b-1 | Fixed. Reproduced: with the slot emptied by the shutdown, `telemetry::tests::a_shutdown_after_a_harness_created_after_a_shutdown_returns_ok` fails with `Sdk(Other(AlreadyShutdown))`. The slot now has three states (`Unset`, `Configured`, `ShutDown`); `shutdown_telemetry` leaves it `ShutDown`, so a later setup does not call the SDK's `setup_tracer` again and a later `shutdown_telemetry` returns `Ok(())`. A shutdown before any exporter was wired leaves the slot `Unset` (setup can still run). The rustdoc says a harness created after the call is not traced. Tests: `a_shutdown_after_a_harness_created_after_a_shutdown_returns_ok` (create, shutdown, create, shutdown over one shared provider, as the SDK slot hands out), `setup_after_a_shutdown_does_not_call_setup_tracer_again`, and the process-wide case in `the_process_wide_setup_warns_once_and_shutdown_takes_the_provider`. P-1210. | c528a1df5 |
| api-b-5 | Same defect as correctness-b-1; fixed by the same commit. | c528a1df5 |
