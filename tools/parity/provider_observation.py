#!/usr/bin/env python3
"""Owning provider output-validation hook, observation encoding and Go regexp execution."""
import argparse,copy,gzip,json,os,random,subprocess
from pathlib import Path
from provider_adapter import ROOT,freeze_binary,sha

def cases():
    rows=[]
    def add(name,schema=None,result=None):
        rows.append({'case':name,'observation':{'manifest':{'provider':{'id':'opaque.provider'},'services':[{'id':'service','operations':[{'id':'opaque.operation','version':7,'outputSchema':schema or {}}]}]},'result':result}})
    def check(name,schema,value): add(name,schema,{'structuredContent':value})
    values=[None,False,True,0,1,1.5,1e-9,1e20,9007199254740993,'','snow 雪','${opaque}',[],[1],{}, {'a':1}]
    for type in ['', 'object','array','string','integer','number','boolean','null','unknown',['string','number'],['number',None,'boolean'],[]]:
        for i,value in enumerate(values):check('type-'+repr(type)+'-'+str(i),{'type':type},value)
    for schema in [{'enum':['a','b']},{'enum':[]},{'enum':[1,2]},{'enum':['a',1]},{'const':'a'},{'const':False},{'const':{'a':1}},{'const':[1]},{'const':None},{'const':1e20},
        {'type':'object','required':['a']},{'type':'object','required':['雪\n"']},{'type':'object','required':[1]},
        {'type':'object','properties':{'a':{'type':'string'}},'additionalProperties':False},
        {'type':'array','minItems':2},{'type':'array','minItems':1e-9},{'type':'array','items':{'type':'integer'}},
        {'type':'string','minLength':4},{'type':'string','minLength':1e-9},{'type':'string','minLength':1e20},
        {'type':'integer','minimum':1.5},{'type':'boolean','minimum':1},{'type':'unknown','minimum':1},
        {'minimum':1}, {'type':'object','properties':{'a':{'const':'text'}}}]:
        for i,value in enumerate(values):check('schema-'+repr(schema)+'-'+str(i),schema,value)
    for value in values:
        add('tool-error-'+repr(value),{'type':'object','required':['required']},{'structuredContent':value,'isError':True,'content':[{'type':'text','text':'typed provider refusal'}]})
    add('nil-result',{'type':'object'});add('empty-result',{'type':'object'},{});add('nil-structured',{}, {'structuredContent':None})
    add('metadata-not-inferred',{}, {'structuredContent':{'uri':'opute://opaque/resource'},'content':[{'type':'resource_link','uri':'opute://opaque/resource','name':'resource'}],'_meta':{'fixture':'opaque'}})
    patterns=['','a','^a$','a|b','(?:ab|ac)*','(?i)k','(?i)ſ','(?i)Σ','(?i)ß','[a-z]','(?i)[a-z]','[[:alpha:]]','[[:^alpha:]]',r'\d+',r'\w+',r'\s+',r'\b',r'\B',r'\ba\b',r'\A.+\z','^a$', '(?m)^a$', '(?s).','.',r'\Qa.b\E',r'\Q(x)*',r'\p{Greek}',r'\p{Han}',r'\p{Kawi}',r'\p{Assigned}',r'\P{L}',r'\p{L}',r'\p{LC}',r'\x{D800}',r'[^\x{0}-\x{10ffff}]',r'[\x{d7ff}-\x{e001}]','a{0}','a{2,4}','(ab|c)+','(?P<name>a)','(?<name>a)',r'\077','(?U)a+','(?-i:a)',r'\C','[','(',')','*','a{1001}',r'\p{NoSuchClass}','(?=a)',r'\1',r'[z-a]']
    texts=['','a','b','abc','aa','aaaa','a\n','\na\n','\n','é','雪','𑼀','K','k','K','ſ','S','σ','ς','Σ','ß','SS','a.b','(x)*','_','1',' ','\t','\x00','\x3f','\ud7ff','\ue000','${ref}']
    for pattern in dict.fromkeys(patterns):
        for index,text in enumerate(texts):check('pattern-'+repr(pattern)+'-'+str(index),{'type':'string','pattern':pattern},text)
    randomizer=random.Random(701325)
    atoms=['a','b','雪','.','[ab]','[^a]',r'\w',r'\W',r'\s',r'\d',r'\p{Greek}',r'\p{Han}',r'\Q(a)\E',r'\b',r'\B','^','$','(?:)','(?i:k)','(?m:^a$)','(?s:.)']
    generated=set()
    for _ in range(1000):
        children=[randomizer.choice(atoms) for _ in range(randomizer.randrange(1,5))]
        pattern=randomizer.choice(['','(?i)','(?m)'])+'(?:'+randomizer.choice(['','|']).join(children)+')'+randomizer.choice(['','*','+','?','{0}','{0,2}','{1,3}'])
        generated.add(pattern)
    for i,pattern in enumerate(sorted(generated)):
        for j,text in enumerate(['','a','b','ab','aaab','kK','σ雪','é_a','a\n','(a)','\t','${ref}']):
            check('seeded-pattern-'+str(i)+'-'+str(j),{'type':'string','pattern':pattern},text)
    for index,pattern in enumerate(['a'*5000, '('*900+'a'+')'*900, 'a{1000}', '(a{1000}){1000}', '(?:a|ab|abc|abcd|abcde)', '(?:[ab]|[bc]|[cd])*']):
        for j,text in enumerate(['','a','ab','aaaa','a'*1000]):
            check('resource-pattern-'+str(index)+'-'+str(j),{'type':'string','pattern':pattern},text)
    assert len({row['case'] for row in rows})==len(rows)
    return rows

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'observation-go');rust=freeze_binary(rust,'observation-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))
    paths+=sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'.parity/go-src/internal/capability/adapters.go',ROOT/'.parity/go-src/internal/capability/capability.go',ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go',ROOT/'.parity/go-src/internal/plan/schema.go']
    sources={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True);inp=folder/'input.json';target=folder/'rust.json'
        inp.write_text(json.dumps(source)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(source),text=True,timeout=120))
        subprocess.run([str(rust),'provider::observation::tests::observation_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_OBSERVATION_INPUT':str(inp.resolve()),'PARITY_PROVIDER_OBSERVATION_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120)
        actual=json.loads(target.read_text()); assert len(reference)==len(actual)==len(source)
        for name,value in [('input',source),('go',reference),('rust',actual)]: (folder/(name+'.json.gz')).write_bytes(gzip.compress(json.dumps(value).encode(),mtime=0))
        inp.unlink();target.unlink()
        for case,left,right in zip(source,reference,actual):
            passed=left==right;rows.append({'case':case['case'],'iteration':iteration,'pass':passed})
            if not passed: print('FAIL',case['case'],left,right,flush=True)
    assert sources=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    summary={'scope':'T0 actual owning Go capability ValidateResult and exact PassThroughObservation bytes on valid SDK-decoded JSON results; provider input requests, callback delegation, public composition and non-wire programmatic non-JSON Go values remain outside this fixture','sourceLock':lock,'provenance':sources,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'normalization':'none; exact error and encoded observation bytes','rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider observations:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
