#!/bin/bash
# rebuilds the toolchain and spec copies; logs to /mnt/workspace/setup-env.log
export RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo
export PATH=$CARGO_HOME/bin:/mnt/workspace/.node/bin:$PATH
REPO=/mnt/workspace/harness-sdk
set -x
if [ ! -x $CARGO_HOME/bin/rustup ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable --component rustfmt,clippy
fi
rustup toolchain install 1.96.0 --profile minimal
rustc --version; cargo --version
# spec export and donor
if [ ! -d /mnt/workspace/ts-spec/strands-ts ]; then
  mkdir -p /mnt/workspace/ts-spec && git -C $REPO archive 7296caba7 strands-ts harness-ts shared AGENTS.md | tar -x -C /mnt/workspace/ts-spec
fi
if [ ! -d /mnt/workspace/donor-strandly/.git ]; then
  git clone -q https://github.com/strandly-the-agent/harness-sdk.git /mnt/workspace/donor-strandly && git -C /mnt/workspace/donor-strandly checkout -q 06ae0d6db6529aab0331f8bd05b5ca97ad6e251c
fi
if [ ! -d /mnt/workspace/ref-witcherish/.git ]; then
  git clone -q https://github.com/TheWitcherish/strands-harness-rs.git /mnt/workspace/ref-witcherish && git -C /mnt/workspace/ref-witcherish checkout -q 30e99ea || echo "ref-witcherish unavailable"
fi
ln -sfn /mnt/workspace/ts-spec /tmp/ts-spec; ln -sfn /mnt/workspace/donor-strandly /tmp/donor-strandly; [ -d /mnt/workspace/ref-witcherish ] && ln -sfn /mnt/workspace/ref-witcherish /tmp/ref-witcherish
# node
want=$(cat $REPO/.node-version 2>/dev/null); echo "node want=$want have=$(node --version 2>/dev/null)"
if [ ! -x /mnt/workspace/.node/bin/node ]; then
  if command -v node >/dev/null && [ "$(node --version)" = "v${want#v}" ]; then :; else
    arch=$(uname -m); [ $arch = aarch64 ] && arch=arm64; [ $arch = x86_64 ] && arch=x64
    v=${want#v}; mkdir -p /mnt/workspace/.node && curl -sSfL https://nodejs.org/dist/v$v/node-v$v-linux-$arch.tar.xz | tar -xJ -C /mnt/workspace/.node --strip-components=1
  fi
fi
node --version
(cd $REPO && npm ci >/tmp/npm-ci.log 2>&1; echo "npm ci rc=$?"; npm run build >/tmp/npm-build.log 2>&1; echo "npm build rc=$?")
# cargo-deny (binary install if possible)
if ! command -v cargo-deny >/dev/null; then
  cargo install cargo-deny --locked --version 0.20.2 >/tmp/cargo-deny-install.log 2>&1; echo "cargo-deny rc=$?"
fi
cargo deny --version
echo SETUP-DONE
