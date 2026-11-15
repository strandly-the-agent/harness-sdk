#!/bin/bash
# usage: .rust-port/commit.sh "<conventional message>" <paths...>
# Stages explicit paths, runs the secret scan, commits with --no-verify (P-1), pushes the branch to the fork (P-2).
set -e
cd "$(git rev-parse --show-toplevel)"
# Serialize commits from parallel agents.
exec 9>/tmp/rust-port-commit.lock; flock 9
msg=$1; shift
git add -- "$@"
.rust-port/scan.sh || { git reset -q -- "$@"; exit 1; }
git commit --no-verify -q -m "$msg"
git push -q fork HEAD:refs/heads/feat/rust-sdk-fresh 2>&1 | tail -1 || echo "push failed; retry later"
git log -1 --oneline
