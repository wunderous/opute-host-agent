#!/usr/bin/env python3
"""Server-owned provider status, validation and immutable capability factories."""
import argparse,copy,gzip,json,os,subprocess,threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from provider_adapter import ROOT,Fixture,freeze_binary,sha
from provider_publication import manifest

def cases():
    rows=[]
    def add(name,steps):
        for mode in [False,True]:rows.append({'case':name+'-'+str(mode),'runtime':{'standalone':mode,'steps':copy.deepcopy(steps)}})
    def status(provider='opaque.provider',**args):return {'action':'status','arguments':{'provider':provider,**args}}
    def validate(provider='opaque.provider',**args):return {'action':'validate','arguments':{'provider':provider,**args}}
    def declaration(validation='test.probe'):
        value=manifest(family='opute.capability.test.v1');value.update(schema='opute-provider-install-manifest.v1',provides=[{'id':'opute.capability.test.v1','version':1}],validation={'capability':'opute.capability.test.v1','operation':validation});value['services'][0]['version']=1
        return value
    def generation(id='g1',state='active'):
        return {'action':'restore','generation':{'ID':id,'Provider':{'id':'opaque.provider','version':'v1'},'ManifestHash':'hash-'+id,'Endpoint':'fixture://'+id,'State':state,'CreatedAt':'2026-10-01T00:00:00Z','ActiveAt':'2026-10-01T00:00:01Z','CatalogRevision':'baseline-revision'}}
    def setup(id='g1',value=None):
        return [generation(id),{'action':'candidate','id':id,'manifest':declaration() if value is None else value},
                {'action':'connect','id':id,'descriptor':{'schema':'opute-provider-plugin.v1','pluginId':'opaque.provider','version':'v1','capabilities':[{'id':'opute.capability.test.v1','version':1}],'server':{'transport':'streamable_http','endpoint':'fixture://'+id}}},
                {'action':'activate','metadata':{'activate':True,'providerGenerationId':id}}]
    def handle(id='h1'):return {'action':'handle','id':id,'operation':'test.probe'}
    def invoke(id='h1'):return {'action':'invoke','id':id,'arguments':{'fixture':'owned'}}
    for provider in ['', ' ', 'opaque.provider',' opaque.provider ','other.provider','OPAQUE.PROVIDER']:
        add('absent-'+repr(provider),[status(provider),validate(provider)])
    add('unconnected',[generation(),status(expectedEndpoint='fixture://g1'),validate(),status(expectedEndpoint='http://different.invalid')])
    add('connected',setup()+[status(expectedEndpoint='fixture://g1'),validate(),validate(operation='test.alt'),validate(operation='missing.operation'),handle(),invoke(),{'action':'unmount','provider':'opaque.provider','id':'g1'},status(),validate(),invoke()])
    add('missing-validation',setup(value=declaration(''))+[status(),validate(),validate(operation='test.probe')])
    add('explicit-whitespace',setup()+[validate(operation=' test.alt '),validate(operation=' '),status(' opaque.provider ',expectedEndpoint=' fixture://g1 ')])
    add('retained-generation',setup()+[handle(),invoke(),{'action':'fail','id':'g1'}]+setup('g2')+[status(expectedEndpoint='fixture://g2'),validate(),invoke(),handle('h2'),invoke('h2')])
    add('failed-mounted-generation',setup()+[handle(),{'action':'fail','id':'g1'},status(),validate(),invoke()])
    return rows

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'runtime-go');rust=freeze_binary(rust,'runtime-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/provider_publication.py',ROOT/'tools/parity/build-provider-activation-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock',ROOT/'Cargo.toml',ROOT/'crates/host-agent/Cargo.toml']
    paths+=sorted((ROOT/'tools/parity/provider-activation-oracle').iterdir())+sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))+sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    sources={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        git=lambda *args:subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();corpus=cases();rows=[]
    def batch_run(job):
        iteration,batch,start=job;chunk=copy.deepcopy(corpus[start:start+6]);folder=out/str(iteration)/str(batch);folder.mkdir(parents=True,exist_ok=True)
        fixture=Fixture({'discover':{'supportedVersions':['2026-07-28']},'listed':{'tools':[{'name':'opute.provider.get_install_manifest'},{'name':'test.probe'},{'name':'test.alt'}]},'callResult':{'content':[],'structuredContent':{'ready':True}}})
        thread=threading.Thread(target=fixture.serve_forever,daemon=True);thread.start()
        endpoint='http://127.0.0.1:'+str(fixture.server_port)
        chunk=json.loads(json.dumps(chunk).replace('fixture://g1',endpoint+'/g1/mcp').replace('fixture://g2',endpoint+'/g2/mcp'))
        inp=folder/'input.json';target=folder/'rust.json';inp.write_text(json.dumps(chunk)+'\n')
        try:
            left=json.loads(subprocess.check_output([str(go)],input=json.dumps(chunk),text=True,timeout=180));go_requests=list(fixture.records);fixture.records.clear()
            subprocess.run([str(rust),'provider::runtime::tests::runtime_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_RUNTIME_INPUT':str(inp.resolve()),'PARITY_PROVIDER_RUNTIME_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=180)
            right=json.loads(target.read_text());rust_requests=list(fixture.records)
        finally:
            fixture.shutdown();fixture.server_close();thread.join(timeout=2);assert not thread.is_alive()
        assert len(chunk)==len(left)==len(right)
        for name,value in [('input',chunk),('go',left),('rust',right)]:
            with gzip.open(folder/(name+'.json.gz'),'wt',compresslevel=1) as file:json.dump(value,file)
        (folder/'requests.json').write_text(json.dumps({'go':go_requests,'rust':rust_requests},indent=2)+'\n');inp.unlink();target.unlink()
        def requests(values):return [{'path':r['path'],'payload':json.loads(r['body']),'headers':{k:v for k,v in r['headers'].items() if k in ['mcp-protocol-version','mcp-client-name','mcp-client-version','authorization','opute-resource-delegation']}} for r in values]
        assert requests(go_requests)==requests(rust_requests),'provider request mismatch'
        results=[]
        for case,a,b in zip(chunk,left,right):
            passed=a==b;results.append({'case':case['case'],'iteration':iteration,'batch':batch,'pass':passed})
            if not passed:
                from parity.canon import diff
                print('FAIL',case['case'],diff(a,b)[:6],flush=True)
        print('Runtime batch',iteration,batch,'completed',flush=True);return results
    with ThreadPoolExecutor(max_workers=8) as workers:
        for result in workers.map(batch_run,[(i,b,s) for i in range(5) for b,s in enumerate(range(0,len(corpus),6))]):rows.extend(result)
    pinned();assert sources=={str(p.relative_to(ROOT)):sha(p) for p in paths}
    summary={'scope':'Actual server status/validation through native authenticated HTTP and Go SDK lifecycle admission. Trusted setup activates pre-restored active generations; immutable capability factories invoke connected adapters directly. Public provider installation, ordinary provider capability dispatch, restart and full M7-M13 E2E remain open.','sourceLock':lock,'provenance':sources,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'passed':sum(r['pass'] for r in rows),'total':len(rows),'normalization':'none; exact parsed tool results and typed provider request payloads/protocol headers'}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider runtime:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
