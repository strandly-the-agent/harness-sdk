# snapshots + checkpoints (strands-rs)

STATUS: code landed & builds — `Agent::take_snapshot(TakeSnapshotOptions) -> Snapshot`,
`Agent::load_snapshot(&Snapshot) -> Result<()>` (`src/agent/snapshot.rs`),
`Checkpoint`/`CheckpointPosition`/`CheckpointData`/`CheckpointResumeContent`
(`src/agent/checkpoint.rs`), `AgentBuilder::checkpointing(bool)`,
`AgentResult.checkpoint`, `InvokeInput::CheckpointResume`, loop boundaries in
`event_loop.rs` (3 call sites into helper fns).

NEXT: unit tests in both modules, session::agent_snapshot delegation, tests/checkpoint_loop.rs,
gates (fmt/clippy/test), matrix file.
