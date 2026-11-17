#!/usr/bin/env python3
"""Assembles harness-rs/docs/reviews/phase-12.md from the dispositions files under .rust-port/handoff/."""
import re, glob, os
def table(path):
    out = {}
    for l in open(path).read().split('\n'):
        if not l.startswith('|'): continue
        cells = [c.strip() for c in l.strip().strip('|').split('|')]
        if len(cells) < 3 or cells[0] in ('finding', '') or set(cells[0]) <= set('-'): continue
        k = cells[0]
        m = re.match(r'((?:correctness|api|parity)(?:-[ab])?-?[a-z]?\d+)', k)
        key = k if ('(' in k and '-b-' in k) else (m.group(1) if m else k)
        out[key] = (k, ' \\| '.join(cells[1:-1]), cells[-1])
    return out
first = table('.rust-port/handoff/phase12a-dispositions.md')
rows = {}
for f in sorted(glob.glob('.rust-port/handoff/phase12-dispositions-*.md')):
    if f.endswith('-verify.md') or f.endswith('-lead.md'): continue
    rows.update(table(f))
lead = table('.rust-port/handoff/phase12-dispositions-lead.md') if os.path.exists('.rust-port/handoff/phase12-dispositions-lead.md') else {}
rows.update(lead)
verify = table('.rust-port/handoff/phase12-dispositions-verify.md') if os.path.exists('.rust-port/handoff/phase12-dispositions-verify.md') else {}
def order(keys, prefixes):
    def k(x):
        m = re.match(r'([a-z]+(?:-[ab])?)-?a?(\d+)', x)
        return (prefixes.index(m.group(1)) if m and m.group(1) in prefixes else 99, int(m.group(2)) if m else 0, x)
    return sorted(keys, key=k)
def clean(t):
    t = re.sub(r' \((?:owned by|track) `h12[a-z0-9]+`\)', '', t)
    t = re.sub(r', owned by `h12[a-z0-9]+`', '', t)
    return t
def emit(d, keys):
    out = ['| finding | disposition | commit |', '|---|---|---|']
    for k in keys:
        name, disp, commit = d[k]
        out.append(f'| {name} | {clean(disp)} | {commit} |')
    return '\n'.join(out)
for k in list(rows):
    if re.match(r'(correctness|api|parity)-b-\d+', k):
        verify.setdefault(k, rows.pop(k))
want = [f'correctness-{i}' for i in range(1, 10)] + [f'api-{i}' for i in range(1, 22)] + [f'parity-{i}' for i in range(1, 15)]
missing = [w for w in want if w not in rows]
extra = [k for k in rows if k not in want and '(whole crate' not in k and not k.startswith('`cargo') and not k.startswith('`node') and not k.startswith('`RUSTDOC') and k not in ('step',) and not k.startswith('live:')]
doc = f"""# Phase 12 review: the harness

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed the `strands-harness` crate and the SDK changes made for it. The review ran in three parts: an early review of the leaf modules (models, tools, plugins, interventions, telemetry, portable config) at `087a6f158`, before `create_harness` was written; the full gate over the whole crate, at `c2c6d557d` for the correctness reviewer and the API bar-raiser and at `24242240e` for the parity validator, whose own run of the full gate script was green at that commit; and a verification pass over the fix diff. Every finding has a disposition below. Finding ids are the reviewers' own; `P-n` names an entry in the run's decision log.

## Early review of the leaf modules

{emit(first, order(first.keys(), ['correctness', 'parity']))}

## Full gate

{emit(rows, [w for w in want if w in rows])}

### Fixed alongside the findings

{emit(rows, extra) if extra else 'None.'}
"""
if verify:
    doc += f"""
## Verification pass

{emit(verify, order(verify.keys(), ['correctness-b', 'api-b', 'parity-b']))}
"""
doc += """
The verification pass reviewed the fix diff with two reviewers (correctness; API bar-raiser and parity validator), and the second ran the full gate script at `6dc02c342` with every step at 0. The fixes for its own findings were not reviewed again; each has a regression test. After the last fixes the full gate script passed at `80209759f` (all steps, 5256 tests with all features, 84 ignored live tests), and the live sweeps of both crates were rerun at that commit (`live.md` in this folder and in `strands-rs/docs/reviews/`).
"""
os.makedirs('harness-rs/docs/reviews', exist_ok=True)
open('harness-rs/docs/reviews/phase-12.md', 'w').write(doc)
print('missing', missing); print('extra', extra)
