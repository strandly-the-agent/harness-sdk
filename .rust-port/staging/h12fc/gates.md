# Gate steps run, track h12fc (tip 305a39dcc plus staging; target dir `/tmp/ct-h12fc`)

All with `--locked`; every step exited 0.

| step | result |
|---|---|
| `cargo fmt --all --check` | 0 |
| `cargo clippy -p strands --all-targets --all-features -- -D warnings` | 0 |
| `cargo clippy -p strands --all-targets --no-default-features -- -D warnings` | 0 |
| `cargo clippy -p strands-harness --all-targets --all-features -- -D warnings` | 0 |
| `cargo clippy -p strands-harness --all-targets --no-default-features -- -D warnings` | 0 |
| `cargo test -p strands --all-features` (whole crate) | 4596 passed, 0 failed, 67 ignored (the live tests, not run) |
| `cargo test -p strands --no-default-features` (whole crate) | 3317 passed, 0 failed |
| `cargo test -p strands-harness --all-features` | 567 passed, 0 failed, 16 ignored (live and cross-SDK, not run) |
| `cargo test -p strands-harness --no-default-features` | 433 passed, 0 failed |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p strands --all-features --no-deps` | 0 |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p strands-harness --all-features --no-deps` | 0 |
| `check-test-titles.mjs --parity harness-rs/docs/PARITY.md` and the SDK default | no FAIL |

Not run: `gates.sh full` (MSRV, per-feature checks, `cargo deny`, conformance), live tests.

Commit `09704296d` on its own has one `dead_code` warning (`EnvironmentContext::yielding` is first used by `ceda52f5c`); it compiles and its tests pass.
