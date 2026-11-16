//! Ports the `createHarness` cases of `harness-ts/test/plugins/todos.test.ts` and
//! `harness-ts/test/plugins/environment.test.ts`. TS reads the agent's plugin list; an `Agent`
//! here has none to read, so these cases look at what each plugin adds: the `todo_write` tool
//! and the `<environment>` block of the first model request.
