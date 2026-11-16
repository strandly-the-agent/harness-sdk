#!/bin/bash
# continues a cherry-pick after conflicts were resolved and staged
cd /mnt/workspace/harness-sdk || exit 1
git -c core.editor=true cherry-pick --continue >> /tmp/cp_cont.log 2>&1; rc=$?
echo "continue rc=$rc"; git status --short | grep -E '^(UU|AA|DU|UD|AU|UA)'; git log --oneline -1
