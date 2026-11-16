#!/usr/bin/env python3
"""Folds .rust-port/staging/<track>/ into the shared logs. Usage: fold.py <track> (from the repo root)."""
import os, sys, re
t = sys.argv[1]
d = f'.rust-port/staging/{t}'
def rd(p):
    return open(p).read() if os.path.exists(p) else ''
def append(path, text):
    if not text.strip(): return
    s = open(path).read().rstrip('\n')
    open(path, 'w').write(s + '\n\n' + text.strip('\n') + '\n')
append('.rust-port/DECISIONS-pending.md', rd(f'{d}/decisions.md'))
append('.rust-port/ts-bugs.md', rd(f'{d}/ts-bugs.md'))
div = [l for l in rd(f'{d}/divergences.md').splitlines() if l.startswith('|') and not re.match(r'^\|\s*-+', l) and not l.lower().startswith('| area') and not l.lower().startswith('| behaviour')]
if div:
    p = 'strands-rs/docs/DIVERGENCES.md'
    lines = open(p).read().rstrip('\n').split('\n')
    for r in div:
        k = r.split('|')[1].strip()
        hits = [i for i, l in enumerate(lines) if l.startswith('|') and l.split('|')[1].strip() == k]
        if hits:
            lines[hits[0]] = r; print('replaced divergence:', k)
        else:
            last = max(i for i, l in enumerate(lines) if l.startswith('|'))
            lines.insert(last+1, r)
    open(p, 'w').write('\n'.join(lines) + '\n')
rows = [l for l in rd(f'{d}/parity.md').splitlines() if l.startswith('| `')]
if rows:
    p = 'strands-rs/docs/PARITY.md'
    lines = open(p).read().rstrip('\n').split('\n')
    def key(l): return l.split('|')[1].strip()
    out_idx = next(i for i, l in enumerate(lines) if l.startswith('## Out of scope'))
    for r in rows:
        k = key(r)
        hits = [i for i, l in enumerate(lines) if l.startswith('|') and key(l) == k]
        if hits:
            i = hits[0]
            if r.count('|') <= 3:  # two-column rows (out of scope, live checklist) stay in place
                lines[i] = r
            elif i > out_idx: # moves from out-of-scope/live to in-scope
                del lines[i]
                ins = max(j for j, l in enumerate(lines[:out_idx]) if l.startswith('|')) + 1
                lines.insert(ins, r); out_idx += 1
                print('moved in scope:', k)
            else:
                lines[i] = r
        else:
            ins = max(j for j, l in enumerate(lines[:out_idx]) if l.startswith('|')) + 1
            lines.insert(ins, r); out_idx += 1
            print('new row:', k)
    # keep the in-scope table sorted by first column
    start = next(i for i, l in enumerate(lines) if l.startswith('## In scope'))
    tbl = [i for i in range(start, out_idx) if lines[i].startswith('| `')]
    if tbl:
        body = sorted((lines[i] for i in tbl), key=key)
        for i, l in zip(tbl, body): lines[i] = l
    open(p, 'w').write('\n'.join(lines) + '\n')
print('folded', t, 'div', len(div), 'parity', len(rows))
