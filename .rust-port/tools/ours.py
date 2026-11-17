#!/usr/bin/env python3
"""Resolves every conflict hunk in the given files by keeping one side. Usage: ours.py ours|theirs <file>..."""
import sys
side = sys.argv[1]
for p in sys.argv[2:]:
    out, state = [], None
    for line in open(p).read().split('\n'):
        if line.startswith('<<<<<<< '): state = 'ours'; continue
        if line.startswith('=======') and state == 'ours': state = 'theirs'; continue
        if line.startswith('>>>>>>> ') and state == 'theirs': state = None; continue
        if state is None or state == side: out.append(line)
    open(p, 'w').write('\n'.join(out))
