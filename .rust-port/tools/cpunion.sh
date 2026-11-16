#!/bin/bash
# continues a cherry-pick, resolving conflicts in the listed "keep both sides" files with union.py and Cargo.lock with ours;
# stops at any other conflict
cd /mnt/workspace/harness-sdk || exit 1
t=$1
if [ -n "$t" ]; then b=$(git merge-base HEAD wip/$t); git cherry-pick --allow-empty $b..wip/$t > /tmp/cp_$t.log 2>&1; fi
while true; do
  u=$(git diff --name-only --diff-filter=U)
  if [ -z "$u" ]; then
    if [ -f .git/CHERRY_PICK_HEAD ] || [ -d .git/sequencer ]; then git -c core.editor=true cherry-pick --continue >> /tmp/cp_cont.log 2>&1 || { git diff --name-only --diff-filter=U | grep -q . || { git -c core.editor=true commit --allow-empty --no-verify -q -C CHERRY_PICK_HEAD 2>/dev/null; git cherry-pick --continue >> /tmp/cp_cont.log 2>&1; }; }; u=$(git diff --name-only --diff-filter=U); fi
    [ -z "$u" ] && [ ! -f .git/CHERRY_PICK_HEAD ] && [ ! -d .git/sequencer ] && break
  fi
  other=""
  for f in $u; do
    case "$f" in
      Cargo.lock) git checkout --ours Cargo.lock && git add Cargo.lock; echo "lock: ours";;
      harness-rs/strands-harness/src/lib.rs|harness-rs/strands-harness/src/error.rs|harness-rs/strands-harness/src/tools/mod.rs|harness-rs/strands-harness/src/plugins/mod.rs|harness-rs/strands-harness/Cargo.toml|Cargo.toml)
        python3 /mnt/workspace/union.py "$f" && git add "$f"; echo "union: $f";;
      strands-rs/docs/test-title-map.json) python3 /mnt/workspace/jsonmerge.py && git add "$f";;
      *) other="$other $f";;
    esac
  done
  [ -n "$other" ] && { echo "CONFLICT in:$other"; break; }
done
git log --oneline -1; git status --short | grep -v '^??' | head -5
