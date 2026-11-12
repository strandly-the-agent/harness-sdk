#!/bin/bash
# usage: .rust-port/commit.sh "<conventional message>" <paths...>
# Stages the paths, runs the secret scan, and commits and pushes only if the scan is clean.
set -e
msg=$1; shift
cd "$(git rev-parse --show-toplevel)"
git add -- "$@"
if ! .rust-port/scan.sh; then
  git reset -q -- "$@"
  echo "commit.sh: scan hit, paths unstaged, nothing committed or pushed" >&2
  exit 1
fi
git commit -q --no-verify -m "$msg"
git push -q fork feat/rust-sdk 2>&1 | grep -v '^remote' || true
git log --oneline -1
