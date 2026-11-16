#!/bin/bash
# after cpauto.sh <track>: folds a fix track's staging dir, keeps its dispositions, commits
cd /mnt/workspace/harness-sdk || exit 1
t=$1
python3 /mnt/workspace/hfold.py $t
[ -f .rust-port/staging/$t/dispositions.md ] && git mv .rust-port/staging/$t/dispositions.md .rust-port/handoff/phase12-dispositions-$t.md
[ -f .rust-port/staging/$t/handoff.md ] && git mv .rust-port/staging/$t/handoff.md .rust-port/handoff/$t-handoff.md
git rm -rq .rust-port/staging/$t 2>/dev/null
python3 /mnt/workspace/remap.py .rust-port/handoff/phase12-dispositions-$t.md 2>/dev/null
git add .rust-port harness-rs/docs strands-rs/docs
.rust-port/secret-scan.sh && git commit -q --no-verify -m "chore(rust-port): fold the $t logs into the shared logs" && git log --oneline -1
