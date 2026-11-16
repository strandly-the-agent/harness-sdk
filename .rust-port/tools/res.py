#!/usr/bin/env python3
"""res.py <file> <n> <replacement-file|-->: replaces the n-th (1-based) conflict block with text from stdin."""
import sys
p=sys.argv[1]; n=int(sys.argv[2]); rep=sys.stdin.read()
lines=open(p).read().split('\n'); out=[]; i=0; k=0
while i<len(lines):
    if lines[i].startswith('<<<<<<< '):
        k+=1; j=i
        while not lines[j].startswith('>>>>>>> '): j+=1
        if k==n:
            out+=rep.rstrip('\n').split('\n') if rep.strip() else []
            i=j+1; continue
    out.append(lines[i]); i+=1
open(p,'w').write('\n'.join(out))
