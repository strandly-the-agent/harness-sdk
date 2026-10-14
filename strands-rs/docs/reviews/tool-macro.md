# `#[tool]` proc-macro for strands-rs

Branch `agent-tasks/strands-rs-tool-macro`, worktree `/home/sandbox/wt/tool-macro`.

## What landed

- **New crate `crates/strands-agents-macros`** (`[lib] proc-macro = true`, added to workspace
  `members`; `syn`/`quote`/`proc-macro2` added to `[workspace.dependencies]`). Single
  `#[proc_macro_attribute] pub fn tool`, ported from
  `/home/sandbox/wt/arron/strands-rs/strands-macros/src/lib.rs` and adapted to our `Tool` trait.
- **Wiring** in `crates/strands-agents`: feature `macros = ["dep:strands-agents-macros"]`, added to
  `default`; `#[cfg(feature = "macros")] pub use strands_agents_macros::tool;` (coexists with the
  existing `tools::tool` builder fn — different namespaces); `#[doc(hidden)] pub mod __private`
  re-exporting `async_trait`, `schemars`, `serde`, `serde_json`.
- **Tests** `crates/strands-agents/tests/tool_macro.rs` (`#![cfg(feature = "macros")]`), 10 tests.
- **Docs/examples**: `examples/tools.rs` now uses `#[tool]`; the builder form moved to
  `examples/tools_builder.rs`; README snippet + feature list updated; `#[tool]` row added to
  `docs/PORTING.md`.

## Generated-code shape

For `/// Current weather for a city.` + `#[tool] async fn weather(city: String, days: Option<u32>) -> String`:

```rust
#[doc(hidden)]
#[derive(::strands_agents::__private::serde::Deserialize,
         ::strands_agents::__private::schemars::JsonSchema)]
#[serde(crate = "::strands_agents::__private::serde")]
#[schemars(crate = "::strands_agents::__private::schemars")]
struct WeatherToolArgs { city: String, days: Option<u32> }   // ToolContext params omitted

pub struct WeatherTool { spec: ::strands_agents::types::ToolSpec }   // + Clone, Debug, Default

impl WeatherTool {
    pub fn new() -> Self {                     // spec built once here
        Self { spec: ToolSpec::new("weather", "Current weather for a city.")
            .with_input_schema(::strands_agents::tools::input_schema_for::<WeatherToolArgs>()) }
    }
}

#[::strands_agents::__private::async_trait]
impl ::strands_agents::tools::Tool for WeatherTool {
    fn name(&self) -> &str { &self.spec.name }
    fn description(&self) -> &str { &self.spec.description }
    fn spec(&self) -> &ToolSpec { &self.spec }
    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        // serde_json::from_value(ctx.tool_use.input) -> Err => create_error_result
        // call user fn (ToolContext param gets `ctx`); Result-returning fn: Err => create_error_result
        // serialize + wrap_in_tool_result(value, &tool_use_id)   <- same helper as FunctionTool
    }
}
```

All generated paths are absolute `::strands_agents::...` / `::core::...`; nothing requires the user
to import `serde`, `schemars`, or `async_trait`. `wrap_in_tool_result` and `input_schema_for` were
already `pub` in `function_tool.rs`, so no visibility changes were needed — the `#[tool]` and
`FunctionTool` paths produce byte-identical result blocks for the same returned value.

Macro-level diagnostics: `self`-taking fns and non-identifier parameter patterns are a
`compile_error!` instead of a panic. Doc-comment extraction stops at an `# Arguments` /
`Args:` / `Arguments:` section, as in the reference.

## Tests (10, all pass)

| Test | Asserts |
| --- | --- |
| `derives_name_and_description_from_the_function` | `name()`, `description()` from doc comment, `spec().name` |
| `derives_the_input_schema_from_the_parameters` | `type: object`, `properties.city.type == string`, `required == ["city"]` |
| `optional_parameters_are_not_required` | `Option<u32>` param absent from `required` |
| `wraps_the_return_value_in_a_success_result` | valid input ⇒ `status: success`, wrapped text |
| `invalid_input_produces_an_error_result` | `{"city": 42}` ⇒ `status: error`, `Error: ` prefix |
| `err_from_the_function_produces_an_error_result` | `Result::Err` ⇒ `Error: division by zero` |
| `ok_from_the_function_produces_a_success_result` | `Result::Ok` ⇒ success |
| `supports_tools_without_parameters` | no-param tool: object schema, no `required`, invokes |
| `passes_the_tool_context_through_and_hides_it_from_the_schema` | `ToolContext` param excluded from schema, `tool_use_id` reaches the fn body |
| `a_generated_tool_runs_in_the_agent_loop` | `MockMessageModel` round trip: tool-use ⇒ success tool result ⇒ final text |

Suite total: 611 tests pass (543 lib + 33 + 24 + 10 new + 1 doc; 23 Bedrock integ ignored),
up from 601.

## Gates (from `strands-rs/`, last line verbatim)

```
cargo fmt                                                   -> (no output)
cargo clippy --all-targets --all-features -- -D warnings    -> Finished `dev` profile [unoptimized] target(s) in 10.80s
cargo test --all-features                                   -> test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
cargo build --examples --all-features                       -> Finished `dev` profile [unoptimized] target(s) in 3.46s
cargo build --no-default-features                           -> Finished `dev` profile [unoptimized] target(s) in 11.96s
cargo doc --no-deps --all-features                          -> Generated /home/sandbox/cargo-target/doc/strands_agents/index.html and 1 other file
```

(The `cargo test` last line is the final test binary's summary; all eight summaries were `ok`,
0 failed.)

## Not done / deviations

- Input is **deserialized, not jsonschema-validated** (per task spec), so a type mismatch surfaces
  as a serde message rather than `FunctionTool::typed_handler`'s pretty validation issue list.
- Error results from `#[tool]` carry no `.with_error(err)` payload (the user error type is only
  `Display`, not our `Error`); `FunctionTool` attaches one.
- No `trybuild`/compile-fail tests for the macro diagnostics.
- The reference's name-based context skipping (`context`/`tool_context`/`agent` param names) was
  dropped in favour of type-based `ToolContext` detection.
