# snapshots + checkpoints (strands-rs)

STATUS: code landed & builds — `Agent::take_snapshot(TakeSnapshotOptions) -> Snapshot`,
`Agent::load_snapshot(&Snapshot) -> Result<()>` (`src/agent/snapshot.rs`),
`Checkpoint`/`CheckpointPosition`/`CheckpointData`/`CheckpointResumeContent`
(`src/agent/checkpoint.rs`), `AgentBuilder::checkpointing(bool)`,
`AgentResult.checkpoint`, `InvokeInput::CheckpointResume`, loop boundaries in
`event_loop.rs` (3 call sites into helper fns).

NEXT: unit tests in both modules, session::agent_snapshot delegation, tests/checkpoint_loop.rs,
gates (fmt/clippy/test), matrix file.

The `interrupts` field is live: `take_snapshot` captures `InterruptState::to_json` and
`load_snapshot` restores it through `InterruptState::from_json` (`null` leaves it untouched), as
`strands-ts/src/agent/snapshot.ts:106-109,164-170` does. Tests:
`agent::snapshot::tests::captures_and_restores_interrupt_state`,
`leaves_interrupt_state_alone_when_the_snapshot_carries_null`,
`interrupt::tests::json_round_trip_preserves_everything_including_the_pending_batch`,
and `tests/interrupts_loop.rs::a_snapshot_of_an_interrupted_agent_resumes_on_a_fresh_instance`
(TS `snapshot.test.ts` "interrupt state round-trip").
