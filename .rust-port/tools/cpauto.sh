#!/bin/bash
# cherry-picks a track; auto-merges test-title-map.json conflicts; stops at any other conflict
cd /mnt/workspace/harness-sdk || exit 1
t=$1
if [ -n "$t" ]; then b=$(git merge-base HEAD wip/$t); git cherry-pick --allow-empty $b..wip/$t > /tmp/cp_$t.log 2>&1; fi
while true; do
  u=$(git diff --name-only --diff-filter=U)
  [ -z "$u" ] && { [ -f .git/CHERRY_PICK_HEAD ] && git -c core.editor=true cherry-pick --continue >> /tmp/cp_cont.log 2>&1; u=$(git diff --name-only --diff-filter=U); [ -z "$u" ] && [ ! -f .git/CHERRY_PICK_HEAD ] && [ ! -d .git/sequencer ] && break; }
  if [ "$u" = "strands-rs/docs/test-title-map.json" ]; then python3 /mnt/workspace/jsonmerge.py && git add strands-rs/docs/test-title-map.json && git -c core.editor=true cherry-pick --continue >> /tmp/cp_cont.log 2>&1; continue; fi
  [ -n "$u" ] && { echo "CONFLICT in:"; echo "$u"; break; }
done
git log --oneline -1; git status --short | grep -v '^??' | head -5
