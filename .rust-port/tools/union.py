#!/usr/bin/env python3
"""Resolves conflict hunks in a file by keeping both sides (ours, then theirs), dropping duplicate lines."""
import sys, re
for p in sys.argv[1:]:
    out=[]; ours=[]; theirs=[]; state=0
    for l in open(p).read().split('\n'):
        if l.startswith('<<<<<<< '): state=1; ours=[]; theirs=[]; continue
        if l.startswith('||||||| ') and state: state=3; continue
        if l.startswith('=======') and state: state=2; continue
        if l.startswith('>>>>>>> ') and state:
            out+=ours+[x for x in theirs if x not in ours or x.strip()=='' ]; state=0; continue
        if state==1: ours.append(l)
        elif state==2: theirs.append(l)
        elif state==3: pass
        else: out.append(l)
    open(p,'w').write('\n'.join(out))
