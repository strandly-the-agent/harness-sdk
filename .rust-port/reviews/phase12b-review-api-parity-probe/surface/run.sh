#!/bin/bash
export RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:/mnt/workspace/.node/bin:$PATH CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/mnt/workspace/.cache/ct-review12b-api
cd /tmp/review12b_api/surface
cargo test --offline --test surface -- --nocapture --test-threads=1 > /tmp/review12b_api/surface_test.txt 2>&1; echo "surface: $?"
