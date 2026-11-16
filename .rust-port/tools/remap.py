#!/usr/bin/env python3
"""Rewrites short commit hashes in a file that are not ancestors of HEAD to the HEAD commit with the same subject. Usage: remap.py <file>..."""
import re, subprocess, sys
def sh(*a):
    r = subprocess.run(a, capture_output=True, text=True); return r.stdout.strip() if r.returncode == 0 else None
head_subjects = {}
for line in sh('git', 'log', '--format=%h\t%s', '7296caba7..HEAD').splitlines():
    h, s = line.split('\t', 1); head_subjects.setdefault(s, h)
for path in sys.argv[1:]:
    text = open(path).read(); changed = {}
    for h in set(re.findall(r'\b[0-9a-f]{9}\b', text)):
        full = sh('git', 'rev-parse', '--verify', '-q', h + '^{commit}')
        if not full: continue
        if subprocess.run(['git', 'merge-base', '--is-ancestor', full, 'HEAD']).returncode == 0: continue
        subj = sh('git', 'log', '-1', '--format=%s', full)
        if subj in head_subjects: changed[h] = head_subjects[subj]
        else: print('NO MATCH', path, h, subj)
    for a, b in changed.items(): text = text.replace(a, b)
    open(path, 'w').write(text); print(path, 'remapped', len(changed))
