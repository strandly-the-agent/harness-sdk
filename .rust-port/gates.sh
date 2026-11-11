#!/bin/bash
# usage: .rust-port/gates.sh [full]
cd "$(git rev-parse --show-toplevel)" || exit 1
. "$HOME/.cargo/env" 2>/dev/null
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/target}" CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_INCREMENTAL=0
fail=0
run(){ name=$1; shift; "$@" > /tmp/gate_$name.txt 2>&1; rc=$?; echo "$name: $rc"; [ $rc -ne 0 ] && fail=1; }
run fmt cargo fmt --all --check
run clippy cargo clippy --workspace --all-targets --all-features -- -D warnings
run clippy_nodef cargo clippy --workspace --all-targets --no-default-features -- -D warnings
run test_all cargo test --workspace --all-features
run test_nodef cargo test --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" run doc cargo doc --workspace --all-features --no-deps
if [ "$1" = "full" ]; then
  RUSTFLAGS="-D warnings" CARGO_TARGET_DIR=/tmp/msrv-target run msrv cargo +1.96.0 check --workspace --all-features --locked
  rm -rf /tmp/msrv-target
  run deny cargo deny check
fi
grep -hE '^test result:' /tmp/gate_test_all.txt | awk '{p+=$4; f+=$6; i+=$8} END {print "tests all-features: passed="p" failed="f" ignored="i}'
exit $fail
