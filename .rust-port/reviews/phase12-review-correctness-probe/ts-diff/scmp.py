import json,sys,collections
def load(p):
    d=collections.OrderedDict()
    for l in open(p):
        i,rest=l.rstrip('\n').split('\t',1)
        d.setdefault(i,[]).append(rest)
    return d
ts=load('sts_out.txt'); rs=load('srs_out.txt')
c=json.load(open('scases.json'))
n=0
for k in ts:
    a=ts.get(k,[]); b=rs.get(k,[])
    if a==b: continue
    n+=1
    o,i=k.split('.')
    print('CASE',k,json.dumps(c['options'][int(o)]),json.dumps(c['inputs'][int(i)])[:140])
    sa=set(a); sb=set(b)
    for x in a:
        if x not in sb: print('  TS:',x[:int(sys.argv[1])])
    for x in b:
        if x not in sa: print('  RS:',x[:int(sys.argv[1])])
print('diff cases',n,'of',len(ts),len(rs))
