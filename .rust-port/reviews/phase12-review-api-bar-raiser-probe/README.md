Scratch crates for the Phase 12 API bar-raiser review. Each file under `tests/` is one target: run it alone with
`cargo check --offline --test <name>` or `cargo test --offline --test <name> -- --nocapture` (toolchain exports from the common brief).

Expected to fail to compile (the errors are the evidence): `dx` (api-1), `names` (api-1, api-2, api-3), `forms2`, `axes` (api-19).
Expected to fail at run time: `debug_secret` (api-7), `plugins`, `env_plugin` (api-15); `probe2`: `feature_message` (api-18).
Expected to pass: `dx_with_path_variant`, `literals`, `forms`, `runtime`, `roundtrip` (api-11 prints the nested-deserialize result).
