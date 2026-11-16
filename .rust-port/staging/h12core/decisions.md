**P-950. `resolve_builtin_tools` is infallible; the TS input-shape errors are unrepresentable.** `BuiltinToolsOption` is typed (`Names(Vec<BuiltinToolName>)` or `Config(BuiltinToolsConfig)` with `Setting<C>` fields), so an unknown name, an unknown config key, a config on `write`/`edit`, a non-boolean value or `'*'`, and a non-list/non-mapping value cannot reach `resolve_builtin_tools`. The JSON path rejects them in `config/normalize.rs`. `ResolvedBuiltinTools` is a struct with one typed field per built-in; `builtin_tool_config(&resolved.<field>)` returns `Some(C::default())` when on without a config and `None` when off. All of it is `pub(crate)` (TS exports these only from `internal.ts`).

**P-951. `HarnessError::NoWebSearch { target }` and `HarnessError::FeatureDisabled { option, feature }` added.** `NoWebSearch` carries the TS text. `FeatureDisabled` has no TS counterpart: `web_search: 'exa'` in a build without the `mcp` feature is an error from `web_search_mode` instead of silently selecting no tool (see also P-961).

**P-952. `BuildAgent` and `BuiltinTools` shapes.** `BuildAgent = Arc<dyn Fn(HarnessAgentOptions) -> BoxFuture<'static, Result<Agent>> + Send + Sync>`; `build_builtin_tools(build_agent, &parent, &resolved, &env) -> Result<IndexMap<BuiltinToolName, Arc<dyn Tool>>>` (the `web_search` entry, the Exa tool, exists only with the `mcp` feature; `select_builtin_tools` skips a name with no entry). The default subagent's builder is the pure `child_options(..) -> Result<HarnessAgentOptions>` followed by `build_agent`, so the child-option rules are unit-tested without running a delegate.

**P-953. The unnamed-MCP-client warning names the Rust field.** TS: "has no applicationName ... Set applicationName to name it."; Rust writes `application_name` (the `McpClientOptions` field), since the TS spelling names nothing a Rust caller can set. The unnamed sentinel is `strands::mcp::DEFAULT_CLIENT_NAME`.

**P-954. A programmatic-tool-caller `timeout` that is not a valid `Duration` (negative, NaN, overflow) becomes zero** (`Duration::try_from_secs_f64(..).unwrap_or_default()`); TS multiplies by 1000 unchecked.

**P-955. `create_harness_with(options, Seams) -> BoxFuture<'static, Result<Agent>>` is the crate-internal entry; `create_harness` calls it with the process seams.** `Seams { base_dir: Option<PathBuf>, env: EnvLookup }`: with `base_dir: Some(..)` the relative default paths (`./.agent/sessions`, `./.agent/skills`, `./.agent/memory`, and a relative `session.dir`) are joined to it; `None` leaves them as TS writes them. It is boxed, not an `async fn`, because the default subagent's builder calls back into it and an `async fn` makes the `Send` proof cyclic. `create_harness`'s future is asserted `Send` in `agent/tests.rs`.

**P-956. `check_passthrough()` runs first in `create_harness`,** before telemetry setup, so a harness-owned key on `agent_config` fails before any side effect.

**P-957. `CachingOption::Auto` is the "not passed" case.** `Auto` resolves the model with `caching: true, caching_explicit: false` (warns on a provider without caching); `On` is `true, true` (errors there); `Off` is `false, true`. TS's explicit `'auto'` (which raises like `true`) has no Rust spelling because the option type has no "unset" state.

**P-958. Plugins are recognised by name, not by type.** `dyn Plugin` has no downcast, so "an `AgentSkills` is already in `plugins`" is `plugin.name() == AgentSkills::NAME`, and a built-in plugin is skipped when a consumer plugin has the same `name()` as a default-configured `Todos`/`EnvironmentContext`. A consumer `Todos` built with a custom name is therefore not recognised, and a second default one is added. PENDING: an `as_any` on `Plugin` would restore the TS rule; that is an SDK change.

**P-959. Session storage for any `session.dir`: `LocalFileStorage::new(<parent of dir>).namespace(<last component>)`,** which for the default is exactly `LocalFileStorage::new("./.agent").namespace("sessions")` (#70); a dir with no last component falls back to un-namespaced `LocalFileStorage::new(dir)`. The stash is `LocalFileStorage::new(<dir>/<id>)`. A test invokes an agent and checks that `<dir>/<id>/` is the only entry under the session dir.

**P-960. The random session id is 8 hex digits from `std::collections::hash_map::RandomState`** (no `uuid`/`fastrand` dependency added to the harness crate).

**P-961. `mcp_servers` without the `mcp` feature is `HarnessError::FeatureDisabled { option: "mcp_servers", feature: "mcp" }`;** `web_search: 'exa'` uses the same variant (P-951). `HarnessError::EmptySessionId` and `HarnessError::ToolNameCollision { name, sources, remedy }` carry the TS texts.

**P-962. `resolve_interventions` is called with `ask: None`.** `create_harness` has no option to pass an ask callback; if the CLI needs one, it must come through a new seam or option.

**P-963. Stale `expect(dead_code)` markers removed** from `interventions.rs`, `memory.rs`, `models.rs` (two), `telemetry.rs`, `tools/shell.rs`, `tools/mod.rs` and `agent.rs` (`check_passthrough`); `config.rs` `builtin_tool_config_keys` keeps a `cfg_attr(not(test), expect(dead_code))` because only tests read it (P-950).
