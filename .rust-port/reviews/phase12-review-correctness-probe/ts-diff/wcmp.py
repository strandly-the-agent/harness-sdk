import json,sys
cases=json.load(open('/tmp/review12_correctness/web/wcases.json'))
for tr in ['direct','curl']:
    ts={l.split('\t',1)[0]:l.rstrip('\n').split('\t',1)[1] for l in open(f'wts_{tr}.txt')}
    rs={l.split('\t',1)[0]:l.rstrip('\n').split('\t',1)[1] for l in open(f'wrs_{tr}.txt')}
    n=0
    for k in range(len(cases)):
        a=ts.get(str(k)); b=rs.get(str(k))
        if a==b: continue
        n+=1
        print(tr,'CASE',k,json.dumps(cases[k])[:100]); print('  TS:',(a or 'MISSING')[:300]); print('  RS:',(b or 'MISSING')[:300])
    print(tr,'diffs',n,'of',len(cases))
