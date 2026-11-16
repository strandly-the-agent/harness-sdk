#!/bin/sh
# usage: run.sh <file.rs> [cargo args...]  -> cargo check of the scratch crate with that file as src/lib.rs
export RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:/mnt/workspace/.node/bin:$PATH CARGO_TARGET_DIR=/tmp/ct-review-r10 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2
f="$1"; shift
cd /tmp/review10_api-bar-raiser/scratch && cp "/tmp/review10_api-bar-raiser/$f" src/lib.rs && timeout 1500 cargo check --offline "$@" 2>&1 | grep -v '^\s*Blocking\|^\s*Checking\|^\s*Compiling\|^\s*Locking\|^\s*Adding' | cut -c1-260 | head -60
