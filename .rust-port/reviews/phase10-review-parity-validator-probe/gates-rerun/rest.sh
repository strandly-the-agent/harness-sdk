#!/bin/bash
# Remaining gates of .rust-port/gates.sh full, with the target dir on /mnt/workspace because / is full.
cd /mnt/workspace/review-r10 || exit 1
export RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:/mnt/workspace/.node/bin:$PATH
export CARGO_TARGET_DIR=/mnt/workspace/.tmp-review10-pv/target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2
export TMPDIR=/mnt/workspace/.tmp-review10-pv/tmp; mkdir -p "$TMPDIR"
out=/mnt/workspace/.tmp-review10-pv/gates
space_ok(){ avail=$(df --output=avail -k /mnt/workspace | tail -1); [ "$avail" -gt 2500000 ]; }
run(){ name=$1; shift; if ! space_ok; then echo "$name: skipped (low disk)"; return; fi; "$@" > "$out/gate_$name.txt" 2>&1; echo "$name: $?"; }
run msrv cargo +1.96.0 check --locked --workspace --all-features
for f in bedrock anthropic openai mcp otel; do
  run feat_$f cargo check --locked --workspace --all-targets --no-default-features --features $f
done
run deny cargo deny --locked check
run conformance bash -c '(cd strands-rs/conformance && npm run generate) && git add -N strands-rs/conformance/fixtures && git diff --exit-code -- strands-rs/conformance/fixtures'
echo done
