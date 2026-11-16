#!/usr/bin/env python3
"""Folds .rust-port/staging/<track>/ of a harness track into the shared logs and harness-rs/docs. Usage: hfold.py <track>."""
import os, sys, re
t = sys.argv[1]
d = f'.rust-port/staging/{t}'
def rd(p): return open(p).read() if os.path.exists(p) else ''
def append(path, text):
    if not text.strip(): return
    s = open(path).read().rstrip('\n')
    open(path, 'w').write(s + '\n\n' + text.strip('\n') + '\n')
append('.rust-port/DECISIONS-pending.md', rd(f'{d}/decisions.md'))
append('.rust-port/ts-bugs.md', rd(f'{d}/ts-bugs.md'))
def key(l): return l.split('|')[1].strip()
def clean(r):
    r = r.replace(' (`.rust-port/ts-bugs.md`)', '').replace('`.rust-port/ts-bugs.md`', 'the TS bug log')
    return r
div = [clean(l) for l in rd(f'{d}/divergences.md').splitlines() if l.startswith('|') and not re.match(r'^\|\s*-+', l) and key(l).lower() not in ('area', 'behaviour')]
if div:
    p = 'harness-rs/docs/DIVERGENCES.md'
    lines = open(p).read().rstrip('\n').split('\n')
    seen = set()
    for r in div:
        hits = [i for i, l in enumerate(lines) if l.startswith('|') and key(l) == key(r)]
        if hits and key(r) not in seen: lines[hits[0]] = r; print("replaced divergence:", key(r))
        else:
            last = max(i for i, l in enumerate(lines) if l.startswith('|'))
            lines.insert(last + 1, r)
        seen.add(key(r))
    open(p, 'w').write('\n'.join(lines) + '\n')
rows = [l for l in rd(f'{d}/parity.md').splitlines() if l.startswith('| `')]
if rows:
    p = 'harness-rs/docs/PARITY.md'
    lines = open(p).read().rstrip('\n').split('\n')
    for r in rows:
        hits = [i for i, l in enumerate(lines) if l.startswith('|') and key(l) == key(r)]
        if hits: lines[hits[0]] = r
        else: print('NO ROW FOR', key(r))
    open(p, 'w').write('\n'.join(lines) + '\n')
print('folded', t, 'div', len(div), 'parity', len(rows))
