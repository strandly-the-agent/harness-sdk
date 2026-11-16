import json,sys,collections
def load(p):
    d=collections.OrderedDict()
    for l in open(p):
        i,rest=l.rstrip('\n').split('\t',1)
        d.setdefault(int(i),[]).append(rest)
    return d
ts=load('hts_out.txt'); rs=load('hrs_out.txt')
cases=json.load(open('hcases.json'))
n=0
for i in range(len(cases)):
    a=ts.get(i,[]); b=rs.get(i,[])
    if a==b: continue
    n+=1
    print('CASE',i,json.dumps(cases[i])[:140])
    sa=set(a); sb=set(b)
    for x in a:
        if x not in sb: print('  TS:',x[:int(sys.argv[1])])
    for x in b:
        if x not in sa: print('  RS:',x[:int(sys.argv[1])])
print('diff cases',n,'of',len(cases))
