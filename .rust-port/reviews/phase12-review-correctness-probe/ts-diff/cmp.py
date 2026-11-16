import json,sys
ts=[l.rstrip('\n').split('\t',1) for l in open('ts_out.txt')]
rs=[l.rstrip('\n').split('\t',1) for l in open('rs_out.txt')]
cases=json.load(open('cases.json'))
d=0
for (i,a),(j,b) in zip(ts,rs):
    if a!=b:
        d+=1
        print('CASE',i,json.dumps(cases[int(i)])[:150]); print('  TS:',a[:int(sys.argv[1])]); print('  RS:',b[:int(sys.argv[1])])
print('diffs',d,'of',len(ts),len(rs))
