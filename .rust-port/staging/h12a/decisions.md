## P-1050 (pending): `Agent::interventions()` lists the agent's intervention handlers

TS has no public accessor; its harness tests read the private `_interventionRegistry.handlers`. Rust adds `Agent::interventions() -> impl ExactSizeIterator<Item = &Arc<dyn InterventionHandler>>`, the same shape as `Agent::tools()`, in registration order. Handlers rather than names only: a caller can compare an instance with `Arc::ptr_eq` (TS `toEqual([handler])`) and read `name()`. Alternative: names only (`impl Iterator<Item = &str>`), which cannot show that a handler instance was passed through. Used by `registers_no_interventions_by_default`, `wires_an_interventions_preset`, `passes_an_intervention_handler_instance_through` and `registers_the_inherited_interventions_gate_on_the_built_child` in the harness.

## P-1051 (pending): harness tests read the provider model's config through `Seams::on_resolve_model`

TS reads `agent.model.getConfig()`. In Rust the built model is an `Arc<dyn Model>`, and `Model::get_config()` returns only `BaseModelConfig` (no cache config, native tools or request fields); downcasting is ruled out. `Seams` gains `on_resolve_model: Option<Arc<dyn Fn(ResolveModelOptions) + Send + Sync>>` (`None` in `create_harness`), called with the options `create_harness` resolves the `model` option with. A test records them and derives the typed provider config with `models::plan` and `models::build::*_options`, the functions `resolve_model` builds the model from (now `pub(crate)`), and checks the derived `model_id` against `agent.model().get_config().model_id`. The case therefore covers what `create_harness` asks for and what that request builds, not the built object's own fields. Alternatives: an observer inside `models.rs` handing out each built model's `config()` (needs a feature-gated enum that is dead code outside tests); asserting on the provider's request through a transport seam (one transport double per provider). Any `Seams { .. }` literal needs the third field.

## P-1052 (pending): `logging::warned` (test-only) for once-per-process warnings

`warn_once` dedupes per process and Rust has no `resetWarnOnce` between tests, so a case cannot require that its own call logged a constant message (the Exa notice, the caching-on-an-instance notice). `#[cfg(test)] logging::warned(message)` reports whether the message has been logged in this process; the cases assert that after their build. Alternative: a test-only reset, which races with tests running in parallel.

## P-1053 (pending): delegate cases build the real child and run a stand-in

TS calls the `subagent` tool's private `builder` and inspects the child. Rust runs the real tool with a `BuildAgent` that builds the real child from the options the tool derived, keeps it for the assertions, and returns a mock-model harness agent for the tool to run, so a child holding a provider model never sends a request. Narrowing goes through the tool's own `tools` input.

## P-1054 (pending): the `builds %s from its factory` cases assert effects, not factory arguments

TS spies on `makeShell`, `makeProgrammaticToolCaller` and `makeSubagent` through module mocks. Rust cannot spy on a function, so each row checks where the argument shows: the shell tool's description; `builtin_tools::programmatic_tool_caller_options` (the config-to-options mapping, extracted from `build_builtin_tools` unchanged) for `allowedTools` and `timeout` (seconds to `Duration`, `null` to unbounded); and for `subagent.maxDepth` the `subagentDepth` the real tool stores on the agent it runs (`maxDepth: 1` leaves 0, no config leaves 1 of the default 2).
