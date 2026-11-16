# Phase 11 fix dispositions, track p11fixb (API bar-raiser findings and root exports)

| finding | disposition | commit |
|---|---|---|
| api-4 | Fixed as proposed: the `strands-macros` dev-dependency on `strands` is path-only, so cargo strips it from the packaged manifest. `cargo package -p strands-macros --allow-dirty --no-verify --offline` now packages 39 files. `strands` itself still needs `strands-macros 0.1.0` in the registry to build its packaged lockfile (publish `strands-macros` first); `cargo package --list --allow-dirty -p strands` works as the dry check | 213b5db89 |
| api-5 | Fixed as proposed: `LICENSE` and `NOTICE` in both crate directories (`cargo package --list --allow-dirty -p strands` and `-p strands-macros` list both), a README for `strands-macros`, absolute repository links and the same `path` in both README snippets (P-760) | 7e831e0e3 |
| api-11 | Fixed as proposed (P-765) | 543845480 |
| api-7 | Fixed as proposed: `pub use indexmap;` at the crate root, named in the crate docs (P-763); the rule is in `AGENTS.md` (api-14) | f1c618fd7 |
| api-13 | Fixed: the `event.agent` row with links to `LocalAgent` and `ToolAgent`, and the `LifecycleObserver` row | 1fbf96aee |
| api-6 | Fixed as proposed: the crate docs map `LifecycleObserver` to `InterventionHandler::observe_agent`; P-766 corrects P-684 and records why `SnapshotManifest` and `SnapshotLocation` stay crate-private. `phase11-root-exports.md` is a review file and was not edited; the corrected placement is in P-766 | 1fbf96aee |
| api-1 | Fixed with the first option: P-607 applied to `vended_interventions` (flat re-exports dropped); builders are never in a flat set (P-761) | bb5ff6342 |
| api-2 | Fixed as proposed (P-762); the three crate-internal plugins got `pub(crate) const NAME` so one shape holds | 77924cc7b, 06b1f3c78 |
| api-9 | Fixed as proposed (P-764); `HookCallbackOptions`, `ConstantBackoff` and `IntervalTrigger`, which the finding did not list, follow the same rule | ae7f61f7a, 5d3793d97, bfcdaee6e, 943bd35a3, 90740b63e, b66181bf7, 74d60aceb |
| api-12 | Fixed with the first option: `state_mod(vis = "pub")` on the 62 remaining public `bon` builders, and the builder and its state module re-exported in every module-level `pub use` that names the config (P-767). The crate root and the flat `vended_tools`/`vended_plugins` sets are unchanged (section 8.2, P-761) | da923ebf3, 9673290fb |
| api-10 | Fixed as proposed with one generic conversion target, `agent::SharedSource<T>` (P-768) | a5161fb28 |
| api-8 | Fixed as proposed: `BedrockModel::builder()` (`build()` returns the model) and `OpenAIModel::builder()` (`build()` returns `Result`), and the three provider builders have public state modules (P-769) | 2e07b7837 |
| api-3 | Fixed as proposed: public fields, a `bon` builder with a closure setter, `with_name`/`with_trigger`/`with_trigger_fn` dropped. `new(render)` stays as the base of a struct literal because the required `render_content` rules out `Default` (P-770) | c74a2f754, d94411241 |
| root-exports | `LifecycleObserver`: covered by `InterventionHandler::observe_agent`, row added to the crate docs (P-766). `SnapshotManifest`, `SnapshotLocation`: kept crate-private, out of v0.1 with `SnapshotStorage` (P-766). `MemoryStoreConfig`, `RoutingCandidateOptions`, `OffloadStrategyBuilder`: the Rust shape covers each, with TS evidence in P-771 and a row each in the crate docs' TypeScript table | 1fbf96aee, 7d415fa15 |
| api-14 | Fixed as proposed: a "Public API rules" list in `strands-rs/AGENTS.md` with the six rules and the provider-constructor rule (P-772) | 7d415fa15 |

Compile-time checks for api-1, api-2, api-3, api-7, api-8, api-10, api-11 and api-12 are in `strands-rs/strands/tests/api_surface.rs` (5dee7f947).
