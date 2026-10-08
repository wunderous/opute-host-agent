#!/usr/bin/env python3
"""Pinned Go regexp grammar evidence and unwaived resource-limit diagnostics."""
import argparse
import gzip
import itertools
import json
import os
from pathlib import Path
import random
import string
import subprocess
from provider_adapter import ROOT,freeze_binary,sha
from provider_schema_decoder import equal
from generate_regexp_names import source


def grammar(facts):
    rows=[]
    def add(pattern):rows.append({'case':'grammar-'+str(len(rows)),'regexp':pattern})
    for pattern in ['', '\\','(',')','[',']','[]','[^]','[a-]','[z-a]','[a-\\d]',
        'a**','a++','a*?*','(?i)','(?i:a)','(?-i)','(?i-:a)','(?P<0>a)',
        '(?<>a)','(?P<','(?=a)','a{0}','a{1001}','a{01}','a{1,01}','a{2,1}',
        '(a{1000}){2}','(a{1000}){0}','a*(?i)*','\\Q(.*\\E*','\\x{d800}']:
        add(pattern)
    for c in string.printable+'雪😀\u0085\u2028\u2029':
        for template in ['\\{}','[\\{}]','(?{})','(?{}:x)','(?P<{}>a)','[{}-x]','[x-{}]']:
            add(template.format(c))
    for name in facts['candidates']+['sc=Latin','IsLatin','gc=Ll','space','Lowercase Letter',
                                    'l_at-in','^L','^^L','','Signwriting','Olditalic']:
        for variant in [name,name.lower(),name.upper(),name.replace('_','-'),name.replace('_',' ')]:
            for template in ['\\p{{{}}}','\\P{{{}}}','[\\p{{{}}}]','(?i:\\p{{{}}})','\\p{}']:
                add(template.format(variant))
    for a,b in itertools.product(['','0','00','1','01','2','1000','1001','999999999','-1'],repeat=2):
        for template in ['a{{{},{}}}','a{{{},{}}}?','{{{},{}}}','(a{{2}}){{{},{}}}']:
            add(template.format(a,b))
    for escape in ['\\x','\\x0','\\x0g','\\x{','\\x{}','\\x{110000}', '\\x{FFFFFFFF}',
        '\\x{10ffff}','\\x{dfff}','\\p{','\\p{}','\\p{Any}','\\P{^ASCII}',
        '\\1','\\01','\\777','\\8','\\c','\\C','\\b','\\B','\\A','\\z','\\Q','\\E']:
        add(escape);add('['+escape+']');add('a'+escape+'?')
    for name in ['alnum','alpha','ascii','blank','cntrl','digit','graph','lower','print',
                 'punct','space','upper','word','xdigit','Alnum','bad','']:
        for prefix in ['', '^']:
            add('[[:'+prefix+name+':]]');add('[a-[:'+prefix+name+':]]')
    rng=random.Random(20261007)
    for _ in range(20000):
        add(''.join(rng.choice('ab0129*+?{}()[]|^$\\:Pi<>,=-QExnp雪') for _ in range(rng.randrange(45))))
    for n in [998,999,1000,1001,1100,2000]:
        add('('*n+'a'+')'*n);add('(?:'*n+'a'+')'*n);add('('*n);add('(a|'*n+'b'+')'*n)
    return rows


def limits():
    rows=[]
    def add(name,pattern):rows.append({'case':name,'regexp':pattern})
    for n in [990,995,996,997,998,999,1000,1001,1100]:
        for body in ['a','[a]b','[aa]b','a|b','a|a','abc|abd','(a|a)','|','a|ab','[Aa]b','(?i:[a])b']:
            add('depth-'+str(n)+'-'+body,'('*n+body+')'*n)
    for n in [1000,3000,10000,27000,30000]:add('runes-'+str(n),'\\pL'*n)
    for n in [3000,4000]:
        for body in ['a{1000}','(?:a|a){1000}','(?:ab|ac){1000}','[a]{1000}','(?:a|ab){1000}']:
            add('size-'+str(n)+'-'+body,body*n)
    return rows


def run(go,rust,out,kind):
    out.mkdir(parents=True,exist_ok=True)
    generator=ROOT/'tools/parity/go-regexp-data/main.go'
    facts=json.loads(subprocess.check_output([str(ROOT/'.parity/setup/go/bin/go'),'run',str(generator)],
        env={**os.environ,'GOTOOLCHAIN':'go1.25.4'},text=True,timeout=60))
    assert facts['goVersion']=='go1.25.4' and facts['unicodeVersion']=='15.0.0'
    assert source(facts)==(ROOT/'crates/host-agent/src/catalog/schema/regexp_names.rs').read_text()
    (out/'unicode-facts.json').write_text(json.dumps(facts,indent=2)+'\n')
    go=freeze_binary(go,'regexp-go');rust=freeze_binary(rust,'regexp-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/provider_schema_decoder.py',
        generator,ROOT/'tools/parity/generate_regexp_names.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'Cargo.lock',ROOT/'licenses/Go-BSD.txt',
        ROOT/'crates/host-agent/src/catalog/schema.rs',ROOT/'crates/host-agent/src/catalog/schema/regexp.rs',
        ROOT/'crates/host-agent/src/catalog/schema/regexp_names.rs',ROOT/'baseline/source-lock.json']
    paths+=list((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    for package in ['regexp','regexp/syntax','unicode']:
        paths+=list((ROOT/'.parity/setup/go/src'/package).glob('*.go'))
    hashes={str(p):sha(p) for p in paths}
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree']
        assert not git('status','--porcelain')
    pinned();rows=grammar(facts) if kind=='grammar' else limits();comparisons=[]
    for iteration in range(5 if kind=='grammar' else 1):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True)
        inp=folder/'input.json';target=folder/'rust.json';inp.write_text(json.dumps(rows,indent=2)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(rows),text=True,timeout=60))
        subprocess.run([str(rust),'catalog::schema::tests::regexp_fixture','--exact','--ignored'],
            env={**os.environ,'PARITY_SCHEMA_REGEXP_INPUT':str(inp.resolve()),'PARITY_SCHEMA_REGEXP_OUTPUT':str(target.resolve())},
            capture_output=True,check=True,timeout=60)
        actual=json.loads(target.read_text());target.unlink();assert len(rows)==len(reference)==len(actual)
        for side,data in [('go',reference),('rust',actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(data).encode(),mtime=0))
        for row,left,right in zip(rows,reference,actual):
            passed=equal(left,right);comparisons.append({'case':row['case'],'iteration':iteration,'pass':passed})
            if not passed:print('FAIL',row['case'],left['error'].split(': ')[1] if left['error'] else 'accepted',right['error'].split(': ')[1] if right['error'] else 'accepted',flush=True)
    assert hashes=={str(p):sha(p) for p in paths};pinned()
    report={'scope':('T0 actual Go regexp.Compile vs native private grammar parser; complete error/acceptance comparisons for scoped syntax corpus. AST factoring, Unicode class expansion and full resource-limit equivalence remain open; not promoted into registry compiler.' if kind=='grammar' else 'T0 unwaived compiler-resource diagnostics against actual Go regexp.Compile. Failing rows remain an explicit implementation gap; not a parity pass or waived refusal.'),
        'sourceLock':lock,'provenance':hashes,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),
        'cases':len(rows),'iterations':5 if kind=='grammar' else 1,'total':len(comparisons),'passed':sum(r['pass'] for r in comparisons),
        'failed':sum(not r['pass'] for r in comparisons),'rows':comparisons,
        'normalization':'Exact complete errors or empty success; no diagnostic replacement, scope-dependent waiver, AST normalization or forced acceptance. Unicode names generated from pinned Go tables and filtered through actual Go regexp.Compile; runtime uses static native name facts and no Go fallback.'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print(kind,report['passed'],'/',report['total'],'failed',report['failed']);return report['failed']==0


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True)
    p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    p.add_argument('--kind',choices=['grammar','limits'],required=True);a=p.parse_args()
    raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve(),a.kind) else 1)
