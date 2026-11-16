#!/bin/bash
# usage: cp.sh <track>  (cherry-picks merge-base..wip/<track> onto the current branch)
cd /mnt/workspace/harness-sdk || exit 1
t=$1; b=$(git merge-base HEAD wip/$t)
git cherry-pick --allow-empty $b..wip/$t > /tmp/cp_$t.log 2>&1; rc=$?
echo "cherry-pick $t rc=$rc"; git status --short | grep -E '^(UU|AA|DU|UD|AU|UA)' ; git log --oneline -1
