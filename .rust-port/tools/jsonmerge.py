#!/usr/bin/env python3
"""3-way merge of strands-rs/docs/test-title-map.json during a cherry-pick conflict (stages 1/2/3)."""
import json, subprocess, sys
p='strands-rs/docs/test-title-map.json'
def stage(n):
    try: return json.loads(subprocess.check_output(['git','show',f':{n}:{p}']).decode())
    except Exception: return {}
base, ours, theirs = stage(1), stage(2), stage(3)
out = {}
for spec in list(ours.keys()) + [k for k in theirs if k not in ours]:
    b, o, t = base.get(spec), ours.get(spec), theirs.get(spec)
    if o is None:
        if b is not None and t == b: continue  # ours deleted it
        out[spec] = t; continue
    if t is None:
        if b is not None and o != b: out[spec] = o  # theirs deleted, ours changed: keep ours
        elif b is None: out[spec] = o
        continue
    if not isinstance(o, dict) or not isinstance(t, dict):
        out[spec] = t if o == b else o; continue
    b = b if isinstance(b, dict) else {}
    m = {}
    for title in list(t.keys()) + [k for k in o if k not in t]:
        bv, ov, tv = b.get(title), o.get(title), t.get(title)
        if title in o and title in t:
            m[title] = tv if ov == bv else ov
            if ov != bv and tv != bv and ov != tv: print('BOTH CHANGED', spec, '|', title, file=sys.stderr)
        elif title in o:
            if title in b and ov == bv: continue  # theirs deleted
            m[title] = ov
        else:
            if title in b and tv == bv: continue  # ours deleted
            m[title] = tv
    out[spec] = m
json.dump(out, open(p,'w'), indent=2, ensure_ascii=False); open(p,'a').write('\n')
print('merged', len(out), 'specs')
