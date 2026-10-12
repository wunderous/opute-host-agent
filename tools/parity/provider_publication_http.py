#!/usr/bin/env python3
"""Owning Go provider publication metadata, retirement and catalog restore."""
import argparse,copy,gzip,json,os,random,subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from provider_adapter import ROOT,freeze_binary,sha

def manifest(provider='opaque.provider',family='test.family',name='test.probe',schema=None):
    return {'provider':{'id':provider,'version':'v1'},'services':[{'id':'service','capabilityId':family,'operations':[{'id':name,'version':1,'effect':'read','inputSchema':{'type':'object'} if schema is None else schema,'outputSchema':{'type':'object'}}]}]}

def cases():
    from provider_publication import cases as original
    prefixes=['empty-', 'replace-retained-', 'replace-provider-', 'displacement-', 'displacement-schema-', 'empty-services-', 'restore-active-', 'restore-candidate-', 'restore-failed-']
    rows = [row for row in original() if any(row['case'].startswith(prefix) for prefix in prefixes)]
    steps=[{'action':'register','manifest':manifest(),'generation':'g1'},
           {'action':'register','manifest':manifest('other.provider',name='other.probe',schema={'type':'bad-type'}),'generation':'g2'},
           {'action':'retire','provider':'missing.provider','generation':'missing-generation'}]
    for mode in [False,True]:rows.append({'case':'noop-retire-stale-wire-'+str(mode),'publication':{'standalone':mode,'steps':copy.deepcopy(steps)}})
    return rows

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'publication-go');rust=freeze_binary(rust,'publication-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/provider_publication.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))
    paths+=sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'crates/host-agent/Cargo.toml',ROOT/'Cargo.toml',ROOT/'.parity/go-src/internal/hostmcp/server.go',ROOT/'.parity/go-src/internal/capability/capability.go',ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go',ROOT/'.parity/go-src/internal/plan/schema.go']
    paths+=[ROOT/'.parity/go-src'/path for path in ['internal/hostmcp/recipe_run.go','internal/catalog/registry.go']]
    sources={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    def batch_run(job):
        iteration,batch,start=job
        chunk=source[start:start+12];capture=out/str(iteration)/str(batch);capture.mkdir(parents=True,exist_ok=True)
        inp=capture/'input.json';target=capture/'rust.json';inp.write_text(json.dumps(chunk)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(chunk),text=True,timeout=120))
        subprocess.run([str(rust),'provider::publication::tests::publication_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_PUBLICATION_HTTP':'1','PARITY_PROVIDER_PUBLICATION_INPUT':str(inp.resolve()),'PARITY_PROVIDER_PUBLICATION_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120)
        actual=json.loads(target.read_text());assert len(reference)==len(actual)==len(chunk)
        for name,value in [('input',chunk),('go',reference),('rust',actual)]:
            with gzip.open(capture/(name+'.json.gz'),'wt',compresslevel=1) as file:json.dump(value,file)
        inp.unlink();target.unlink();results=[]
        for case,left,right in zip(chunk,reference,actual):
            passed=left==right;results.append({'case':case['case'],'iteration':iteration,'batch':batch,'pass':passed})
            if not passed:
                from parity.canon import diff
                print('FAIL',case['case'],diff(left,right)[:5],flush=True)
        print('Publication batch',iteration,batch,'completed',flush=True)
        return results
    jobs=[(iteration,batch,start) for iteration in range(5) for batch,start in enumerate(range(0,len(source),12))]
    # Each process owns its loopback SDK connection, native registry, and capture
    # directory. No runtime lease or provider generation is shared across jobs.
    with ThreadPoolExecutor(max_workers=8) as workers:
        for results in workers.map(batch_run,jobs):rows.extend(results)
    assert sources=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    summary={'scope':'Owning publication, retirement and restore with actual native authenticated HTTP tools/list after every transition, compared against Go SDK ListTools. Provider invocation and public lifecycle commands remain open.','sourceLock':lock,'provenance':sources,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'normalization':'none; exact error and encoded publication bytes','rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider publications:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
