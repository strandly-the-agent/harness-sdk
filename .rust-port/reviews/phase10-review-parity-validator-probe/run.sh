#!/bin/bash
# usage: run.sh <out-file> <cargo args...>   (env passed through)
export RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:/mnt/workspace/.node/bin:$PATH CARGO_TARGET_DIR=/tmp/ct-review-r10 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2
cd /mnt/workspace/reviews/phase10-review-parity-validator-probe
out=$1; shift
cargo "$@" > "$out" 2>&1
echo "exit=$?" >> "$out"
