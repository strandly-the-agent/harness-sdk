# strands-cli-worker

The Strands CLI's Rust runtime. The TypeScript CLI (`strands --runtime rust`) spawns this binary
and drives a `strands-harness` agent through the worker protocol in
[`../../docs/CLI_WORKER_PROTOCOL.md`](../../docs/CLI_WORKER_PROTOCOL.md): one JSON command per
line on stdin, one JSON message per line on fd 3; stdout/stderr are diagnostics only
(`RUST_LOG` controls them).

```bash
cargo build --release -p strands-cli-worker
export STRANDS_RUST_WORKER=$PWD/target/release/strands-cli-worker
strands --runtime rust
```

`--stdout` sends messages to stdout instead of fd 3, for poking at it by hand:

```bash
printf '%s\n' '{"type":"init","overrides":{"session":false}}' '{"type":"prompt","prompt":"hi"}' '{"type":"close"}' \
  | strands-cli-worker --stdout
```

The crate is a library plus a thin binary: `strands_cli_worker::run` is transport-agnostic and
takes an `AgentFactory`, which is how `tests/worker.rs` runs the whole protocol against
`MockMessageModel` over in-memory pipes.
