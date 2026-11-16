#!/bin/sh
# usage: runu.sh <file.rs> [cargo subcommand and args, default "check"]
export RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:/mnt/workspace/.node/bin:$PATH CARGO_TARGET_DIR=/tmp/ct-review-r10 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2
f="$1"; shift
[ $# -eq 0 ] && set -- check
cd /tmp/review10_api-bar-raiser/usage && cp "/tmp/review10_api-bar-raiser/$f" src/lib.rs && sub="$1"; shift; timeout 1500 cargo "$sub" --offline "$@" 2>&1 | grep -v '^\s*Blocking\|^\s*Checking\|^\s*Compiling\|^\s*Locking\|^\s*Adding' | cut -c1-300 | head -80
