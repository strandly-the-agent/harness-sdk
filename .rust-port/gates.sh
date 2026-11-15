#!/bin/bash
# usage: .rust-port/gates.sh [full]
cd "$(git rev-parse --show-toplevel)" || exit 1
export RUSTUP_HOME="${RUSTUP_HOME:-/mnt/workspace/.rust/rustup}" CARGO_HOME="${CARGO_HOME:-/mnt/workspace/.rust/cargo}"
export PATH="$CARGO_HOME/bin:$PATH"
out="${GATE_OUT:-/tmp}"
fail=0
run(){ name=$1; shift; "$@" > "$out/gate_$name.txt" 2>&1; rc=$?; echo "$name: $rc"; [ $rc -ne 0 ] && fail=1; }
run fmt cargo fmt --all --check
run clippy cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
run clippy_nodef cargo clippy --locked --workspace --all-targets --no-default-features -- -D warnings
run test_all cargo test --locked --workspace --all-features
run test_nodef cargo test --locked --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" run doc cargo doc --locked --workspace --all-features --no-deps
if [ "$1" = "full" ]; then
  run msrv cargo +1.96.0 check --locked --workspace --all-features
  for f in bedrock anthropic openai mcp otel; do
    run feat_$f cargo check --locked --workspace --all-targets --no-default-features --features $f
  done
  run deny cargo deny --locked check
  run conformance bash -c '(cd strands-rs/conformance && npm run generate) && git add -N strands-rs/conformance/fixtures && git diff --exit-code -- strands-rs/conformance/fixtures'
fi
grep -hE '^test result:' "$out/gate_test_all.txt" | awk '{p+=$4; f+=$6; i+=$8} END {print "tests all-features: passed="p" failed="f" ignored="i}'
exit $fail
