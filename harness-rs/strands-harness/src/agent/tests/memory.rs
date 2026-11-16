//! Ports the `createHarness memory` cases of `harness-ts/test/memory.test.ts`. TS reads the
//! manager's private store list; these cases tell the stores apart by what the agent's manager
//! does: a search reaches the supplied store, and a write fails when no default file store exists.
