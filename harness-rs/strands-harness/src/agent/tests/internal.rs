//! Ports `harness-ts/test/internal.test.ts`. TS ships the factory plumbing on a second entry
//! point, `@strands-agents/harness/internal`, for the TypeScript CLI. Rust has no such entry
//! point: the plumbing is `pub(crate)`, and the crate root is the whole public surface.
