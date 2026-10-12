#!/usr/bin/env python3
"""Actual owning Go callback scope and independent durable-child resource owner."""
import argparse,base64,copy,gzip,hashlib,hmac,json,os,random,subprocess
from pathlib import Path
from provider_adapter import ROOT,freeze_binary,sha

def cases():
    key=b'test-resource-delegation-key'
    claims={'Version':'host-resource-delegation.v1','ID':'permit','ReservationID':'reservation','AgentID':'agent 雪','TaskID':'task','TaskOperation':'task.operation','ProviderID':'opaque.provider','ProviderGeneration':'opaque.generation','ProviderOperation':'opaque.operation','ExpiresAt':4102444800}
    base={'key':key.hex(),'agent':'agent 雪','registryTask':'task','status':'working','tool':'get_host_info','hostTools':['get_host_info'],'implementations':{},'cancelled':False,'childCancelled':False,'task':'original-task','operation':'original-operation','reservation':None}
    rows=[]
    def add(name,input,claim=claims):
        item=copy.deepcopy(input)
        if 'meta' not in item:
            body=base64.urlsafe_b64encode(json.dumps(claim,ensure_ascii=False,separators=(',',':')).encode()).decode().rstrip('=')
            token=body+'.'+base64.urlsafe_b64encode(hmac.new(key,body.encode(),hashlib.sha256).digest()).decode().rstrip('=')
            item['meta']={'io.opute/host-resource-delegation':token};item['active']={claim.get('ID',''):token}
        item.setdefault('active',{})
        rows.append({'case':name,'callback':item})
    add('valid',base)
    lifecycle=['validate_host_plan','run_host_plan','get_host_plan_run','validate_host_local_recipe','run_host_local_recipe','validate_runtime_recipe','run_runtime_recipe','get_runtime_recipe_run','validate_tunnel_recipe','run_tunnel_recipe','get_tunnel_run','opute.provider.install','opute.provider.validate','opute.provider.status','opute.provider.reload','opute.provider.teardown','get_capability_catalog','open_assistant_session']
    for tool in lifecycle+['get_host_info','unknown',' get_host_info ','GET_HOST_INFO','get_host_info\n','provider.call','雪']:
        for implementation in [None,'provider:opaque',' provider:opaque ','provider','Provider:opaque','host-agent:system','']:
            for registered in [False,True]:
                item=copy.deepcopy(base);item['tool']=tool;item['hostTools']=[tool] if registered else [];item['implementations']={tool:implementation};add('scope-'+repr((tool,implementation,registered)),item)
    for field in ['AgentID','TaskID','TaskOperation','ReservationID','ProviderID','ProviderGeneration','ProviderOperation','ID','Version']:
        for i,value in enumerate(['',' ','other','agent 雪','task','task.operation','control','unmanaged',' task ',' task.operation ','\u0085']):
            claim={**claims,field:value};add('claim-'+field+'-'+str(i),base,claim)
    for expiry in [0,1,4102444800,9223372036854775807,9223372036854775807-62135596800-1,9223372036854775807-62135596800,9223372036854775807-62135596800+1]:add('expiry-'+str(expiry),base,{**claims,'ExpiresAt':expiry})
    for status in ['missing','input_required','failed','cancelled','completed','unknown','working']:
        for agent in ['agent 雪',' agent 雪 ','other','']:
            item=copy.deepcopy(base);item['status']=status;item['agent']=agent;add('live-'+status+'-'+agent,item)
    for i,value in enumerate([None,False,True,0,1,[],{},'', ' ', 'bad.signature']):
        item=copy.deepcopy(base);item['meta']={'io.opute/host-resource-delegation':value};add('metadata-'+str(i),item)
    for meta in [None,{}, {'other':'wrong'}, {'IO.OPUTE/HOST-RESOURCE-DELEGATION':'wrong'}]:
        item=copy.deepcopy(base);item['meta']=meta;add('missing-metadata-'+repr(meta),item)
    for cancelled in [False,True]:
        for child in [False,True]:
            item=copy.deepcopy(base);item['cancelled']=cancelled;item['childCancelled']=child;add('independent-child-'+repr((cancelled,child)),item)
    parent={'id':'parent','agent':'agent 雪','task':'original-task','operation':'original-operation','expiresAt':4102444800}
    for field in parent:
        for i,value in enumerate(([None,0,1,-1,-9223372036854775808,4102444800] if field=='expiresAt' else ['', ' ',parent[field],'other','control','unmanaged'])):
            item=copy.deepcopy(base);item['meta']={};item['reservation']={**parent,field:value};item['registryTask']='original-task';add('existing-owner-'+field+'-'+str(i),item)
    for i,space in enumerate([' ', '\t\n','\u0085','\u00a0','\u2000','\u3000']):
        item=copy.deepcopy(rows[0]['callback']);item['meta']['io.opute/host-resource-delegation']=space+item['meta']['io.opute/host-resource-delegation']+space;add('metadata-trim-'+str(i),item)
    randomizer=random.Random(701328)
    for i in range(350):
        item=copy.deepcopy(base);item['status']=randomizer.choice(['missing','working','completed']);item['agent']=randomizer.choice(['agent 雪','other']);item['cancelled']=randomizer.choice([False,True]);item['childCancelled']=randomizer.choice([False,True]);item['tool']=randomizer.choice(['get_host_info','run_host_plan','unknown']);item['hostTools']=randomizer.choice([['get_host_info'],['get_host_info','run_host_plan'],[]]);add('seeded-'+str(i),item)
    assert len({row['case'] for row in rows})==len(rows)
    return rows

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'callback-go');rust=freeze_binary(rust,'callback-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))
    paths+=sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'crates/host-agent/Cargo.toml',ROOT/'Cargo.toml',ROOT/'.parity/go-src/internal/hostmcp/resource_delegation.go',ROOT/'.parity/go-src/internal/capability/capability.go',ROOT/'.parity/setup/go/src/time/time.go',ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go',ROOT/'.parity/go-src/internal/plan/schema.go']
    paths+=[ROOT/'.parity/go-src'/path for path in ['internal/hostmcp/server.go','internal/resource/service.go','internal/tasks/registry.go','internal/hostagent/service.go','internal/mcphttp/resource_delegation.go']]
    sources={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True);inp=folder/'input.json';target=folder/'rust.json'
        inp.write_text(json.dumps(source)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(source),text=True,timeout=120))
        subprocess.run([str(rust),'provider::delegation::tests::callback_scope_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_CALLBACK_INPUT':str(inp.resolve()),'PARITY_PROVIDER_CALLBACK_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120)
        actual=json.loads(target.read_text()); assert len(reference)==len(actual)==len(source)
        for name,value in [('input',source),('go',reference),('rust',actual)]: (folder/(name+'.json.gz')).write_bytes(gzip.compress(json.dumps(value).encode(),mtime=0))
        inp.unlink();target.unlink()
        for case,left,right in zip(source,reference,actual):
            passed=left==right;rows.append({'case':case['case'],'iteration':iteration,'pass':passed})
            if not passed: print('FAIL',case['case'],left,right,flush=True)
    assert sources=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    summary={'scope':'Actual owning Go contextForProviderCallback, providerCallbackTaskOwnerFromContext and asyncTaskExecutionContext; exact scope, liveness, reservation ownership, expiry, context and child isolation outcomes. Public authenticated dispatch and admission composition remain open','sourceLock':lock,'provenance':sources,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'normalization':'none; exact error and encoded callback bytes','rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider callbacks:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
