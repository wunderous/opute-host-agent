#!/usr/bin/env python3
"""Actual owning Go delegation issuer and typed task/reservation contexts."""
import argparse,base64,copy,gzip,hashlib,hmac,json,os,random,re,subprocess,time,uuid
from pathlib import Path
from provider_adapter import ROOT,freeze_binary,sha

def cases():
    base={'key':b'test-resource-delegation-key'.hex(),'agent':'agent 雪','task':'task','operation':'task.operation','provider':'opaque.provider','generation':'opaque.generation','providerOperation':'opaque.operation','registryTask':'task','status':'working','priorToken':'','cancelled':False,'reservation':{'id':'reservation','agent':'agent 雪','task':'task','operation':'task.operation'}}
    rows=[]
    def add(name,value):rows.append({'case':name,'issuance':value})
    add('valid',copy.deepcopy(base))
    for field in ['agent','task','operation','provider','generation','providerOperation','registryTask','priorToken','status']:
        for i,value in enumerate(['',' ','\t\n','\u0085','\ufeff','other','opaque 雪','control','unmanaged',' task ',' task.operation ',' agent 雪 ','\x00']):
            item=copy.deepcopy(base);item[field]=value;add(field+'-'+str(i),item)
    for field in base['reservation']:
        for i,value in enumerate(['',' ','\t\n','\u0085','\ufeff','other','雪','control','unmanaged',' reservation ','task',' task ','task.operation',' task.operation ','agent 雪',' agent 雪 ']):
            item=copy.deepcopy(base);item['reservation'][field]=value;add('reservation-'+field+'-'+str(i),item)
    for status in ['missing','input_required','completed','cancelled','failed','unknown']:
        item=copy.deepcopy(base);item['status']=status;add('registry-status-'+status,item)
    for i,reservation in enumerate([None,{'id':''},{'id':' '},{'id':'control'},{'id':'unmanaged'}]):
        for prior in ['', '  carried-token  ']:
            item=copy.deepcopy(base);item['reservation']=reservation;item['task']='';item['operation']='';item['status']='missing';item['priorToken']=prior;add('noop-'+str(i)+'-'+prior,item)
    for field in ['provider','generation','providerOperation']:
        item=copy.deepcopy(base);item[field]='  opaque 雪 '+field+'  ';add('opaque-'+field,item)
    for i,space in enumerate([' ','\t','\n','\u0085','\u00a0','\u2000','\u2028','\u3000']):
        item=copy.deepcopy(base)
        for field in ['agent','task','operation']:item[field]=space+item[field]+space
        add('trimmed-context-'+str(i),item)
    for cancelled in [False,True]:
        item=copy.deepcopy(base);item['cancelled']=cancelled;add('context-cancellation-'+str(cancelled),item)
    for owner in [b'',b'a',bytes(range(32))]:
        item=copy.deepcopy(base);item['key']=owner.hex();add('key-'+str(len(owner)),item)
    randomizer=random.Random(701327)
    for i in range(500):
        item=copy.deepcopy(base)
        for _ in range(randomizer.randrange(1,5)):
            field=randomizer.choice(['task','operation','agent','provider','generation','providerOperation','status'])
            item[field]=randomizer.choice(['',' ',base[field],'other','\u0085',' task '])
        item['cancelled']=randomizer.choice([True,False]);add('seeded-'+str(i),item)
    assert len({row['case'] for row in rows})==len(rows)
    return rows

def checked(output,input,started,finished):
    value=copy.deepcopy(output)
    if not value['issued']:return value
    body,mac=value['token'].split('.')
    decoded=lambda part:base64.urlsafe_b64decode(part+'='*((-len(part))%4))
    assert hmac.compare_digest(decoded(mac),hmac.new(bytes.fromhex(input['key']),body.encode(),hashlib.sha256).digest())
    claims=json.loads(decoded(body));identifier=uuid.UUID(claims['ID'])
    assert identifier.version==4 and identifier.variant==uuid.RFC_4122 and str(identifier)==claims['ID']
    assert int(started)+3600<=claims['ExpiresAt']<=int(finished)+3600
    # Only fresh random identity and real-clock expiry are normalized, after
    # their UUID/TTL/signature invariants are independently established.
    if not value['verificationError']:
        assert value['claims']==decoded(body).decode()
        value['claims']=value['claims'].replace('"ID":"'+claims['ID']+'"','"ID":"<uuid-v4>"').replace('"ExpiresAt":'+str(claims['ExpiresAt'])+'}','"ExpiresAt":<one-hour-expiry>}')
    value['token']='<verified-signed-token>'
    return value

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'issuance-go');rust=freeze_binary(rust,'issuance-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock',ROOT/'Cargo.toml',ROOT/'crates/host-agent/Cargo.toml']
    paths+=sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))+sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'.parity/go-src'/path for path in ['internal/hostmcp/resource_delegation.go','internal/resource/service.go','internal/tasks/registry.go','internal/hostagent/service.go','internal/mcphttp/resource_delegation.go']]
    sources={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        git=lambda *args:subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True);inp=folder/'input.json';target=folder/'rust.json';inp.write_text(json.dumps(source)+'\n')
        started=time.time()
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(source),text=True,timeout=120))
        subprocess.run([str(rust),'provider::delegation::tests::issuance_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_ISSUANCE_INPUT':str(inp.resolve()),'PARITY_PROVIDER_ISSUANCE_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120)
        actual=json.loads(target.read_text());finished=time.time();assert len(reference)==len(actual)==len(source)
        interop=[];expected=[]
        for case,value in zip(source,actual):
            if value['issued'] and not value['verificationError']:
                claims=json.loads(value['claims']);interop.append({'delegation':{'key':case['issuance']['key'],'token':value['token'],'active':{claims['ID']:value['token']}}});expected.append({'claims':value['claims'],'error':''})
        cross=json.loads(subprocess.check_output([str(go)],input=json.dumps(interop),text=True,timeout=120));assert cross==expected
        for name,value in [('input',source),('go',reference),('rust',actual),('cross-input',interop),('cross-go',cross)]: (folder/(name+'.json.gz')).write_bytes(gzip.compress(json.dumps(value).encode(),mtime=0))
        (folder/'clock.json').write_text(json.dumps({'started':started,'finished':finished})+'\n');inp.unlink();target.unlink()
        for case,left,right in zip(source,reference,actual):
            passed=checked(left,case['issuance'],started,finished)==checked(right,case['issuance'],started,finished)
            rows.append({'case':case['case'],'iteration':iteration,'pass':passed})
            if not passed:print('FAIL',case['case'],left,right,flush=True)
    assert sources=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    summary={'scope':'Actual owning Go withProviderResourceDelegation and typed context helpers, task registry liveness, crypto interoperability and release. Callback scope, child task ownership, public composition and full M7-M13 E2E remain open','sourceLock':lock,'provenance':sources,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'normalization':'only freshly generated UUID and one-hour expiry after UUID4, real-clock TTL and independent HMAC checks; all other fields/errors/context outcomes exact; native-issued tokens cross-verified by actual Go verifier','rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider issuance:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
