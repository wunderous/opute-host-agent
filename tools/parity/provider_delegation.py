#!/usr/bin/env python3
"""Pinned Go signed delegation verifier: raw JSON, signature and active ownership."""
import argparse,base64,copy,gzip,hashlib,hmac,json,os,random,subprocess
from pathlib import Path
from provider_adapter import ROOT,freeze_binary,sha

def cases():
    rows=[]; key=b'test-resource-delegation-key'
    base={'Version':'host-resource-delegation.v1','ID':'permit','ReservationID':'reservation','AgentID':'opaque 雪 agent','TaskID':'task','TaskOperation':'operation','ProviderID':'provider','ProviderGeneration':'generation','ProviderOperation':'provider.operation','ExpiresAt':4102444800}
    def encoded(payload):return base64.urlsafe_b64encode(payload).decode().rstrip('=')
    def signed(payload,owner=key):
        body=encoded(payload);return body+'.'+encoded(hmac.new(owner,body.encode(),hashlib.sha256).digest())
    def add(name,payload=None,token=None,owner=key,active=True,active_id='permit',active_token=None):
        if token is None:token=signed(payload,owner)
        rows.append({'case':name,'delegation':{'key':owner.hex(),'token':token,'active':{active_id:token if active_token is None else active_token} if active else {}}})
    def check(name,claims):add(name,json.dumps(claims,ensure_ascii=False,separators=(',',':')).encode())
    check('valid',base)
    for field in base:
        for i,value in enumerate(['',None,0,1,True,False,[],{},'雪','  ', '\x00','a\nb',-1,0.5,9223372036854775807,9223372036854775808,-9223372036854775809]):
            claims=copy.deepcopy(base);claims[field]=value;check(field+'-'+str(i),claims)
        claims=copy.deepcopy(base);del claims[field];check('absent-'+field,claims)
        for name in dict.fromkeys([field.lower(),field.upper(),field.swapcase(),field.replace('s','ſ').replace('S','ſ')]):
            claims=copy.deepcopy(base);value=claims.pop(field);claims[name]=value;check('case-'+field+'-'+name,claims)
    for i,root in enumerate([None,{},[],False,True,'text',1,1.5]):check('root-'+str(i),root)
    check('expired',{**base,'ExpiresAt':1})
    for extra in [{'unknown':{}},{'nested':{'ID':1}},{'unknown':[None,False,1]}]:check('unknown-'+repr(extra),{**base,**extra})
    prefix=json.dumps(base,ensure_ascii=False,separators=(',',':'))[:-1]
    for field in base:
        for i,value in enumerate(['null','""','false','0','"other"']):
            add('duplicate-'+field+'-'+str(i),(prefix+','+json.dumps(field.lower())+':'+value+'}').encode())
    for literal in ['4102444800.0','4.1024448e9','4102444800e0','-0','0','9223372036854775807','9223372036854775808','-9223372036854775808','-9223372036854775809']:
        add('expires-literal-'+literal,json.dumps(base).replace('4102444800',literal).encode())
    for i,payload in enumerate([b'',b'{',b'{} garbage',b'null null',b'\xef\xbb\xbf{}',b'{"ID":"\xff"}',(prefix+',"Unknown":"\xff"}').encode().replace(b'\xc3\xbf',b'\xff'),prefix.encode()+b',"Unknown":"\\ud800"}']):add('raw-json-'+str(i),payload)
    valid=signed(json.dumps(base).encode())
    for i,token in enumerate(['','.',valid+'.',valid+'.extra',' '+valid,valid+' ',valid.replace('.', '..'),valid[:-1],valid+'=',valid.split('.')[0]+'.',valid.split('.')[0]+'.AA',valid.split('.')[0]+'.'+valid.split('.')[1]+'\n',valid.split('.')[0]+'.'+valid.split('.')[1]+'\r\n',valid.split('.')[0]+'.'+valid.split('.')[1]+'\t']):add('token-'+str(i),token=token)
    add('inactive',token=valid,active=False);add('wrong-active-id',token=valid,active_id='other');add('wrong-active-token',token=valid,active_token='other');add('empty-key',token=valid,owner=b'');add('other-key',token=valid,owner=b'other')
    # Correctly signed malformed payload base64 must reach payload decoding.
    for i,body in enumerate(['','A','AA=','A A','***','AA+','AA/','A\nA','A\r\nA']):
        add('payload-alphabet-'+str(i),token=body+'.'+encoded(hmac.new(key,body.encode(),hashlib.sha256).digest()))
    for i,separator in enumerate(['\n','\r','\r\n']):
        body,mac=valid.split('.')
        body=body[:20]+separator+body[20:]
        add('payload-line-break-'+str(i),token=body+'.'+encoded(hmac.new(key,body.encode(),hashlib.sha256).digest()))
    add('expired-before-inactive',json.dumps({**base,'ExpiresAt':1}).encode(),active=False)
    alphabet='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'
    body,mac=valid.split('.')
    for char in alphabet:add('mac-tail-'+char,token=body+'.'+mac[:-1]+char)
    for char in alphabet:
        alternate=body[:-1]+char
        add('payload-tail-'+char,token=alternate+'.'+encoded(hmac.new(key,alternate.encode(),hashlib.sha256).digest()))
    for owner in [b'a',bytes(range(32)),bytes(range(255))]:add('key-length-'+str(len(owner)),json.dumps(base).encode(),owner=owner)
    randomizer=random.Random(701326)
    for i in range(350):
        claims=copy.deepcopy(base)
        claims[randomizer.choice(list(base))]=randomizer.choice(['',None,False,[],{},1,'雪','\n','opaque value'])
        check('seeded-'+str(i),claims)
    assert len({row['case'] for row in rows})==len(rows)
    return rows

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'delegation-go');rust=freeze_binary(rust,'delegation-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))
    paths+=sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'crates/host-agent/Cargo.toml',ROOT/'Cargo.toml',ROOT/'.parity/go-src/internal/hostmcp/resource_delegation.go',ROOT/'.parity/go-src/internal/capability/capability.go',ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go',ROOT/'.parity/go-src/internal/plan/schema.go']
    sources={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True);inp=folder/'input.json';target=folder/'rust.json'
        inp.write_text(json.dumps(source)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(source),text=True,timeout=120))
        subprocess.run([str(rust),'provider::delegation::tests::delegation_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_DELEGATION_INPUT':str(inp.resolve()),'PARITY_PROVIDER_DELEGATION_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120)
        actual=json.loads(target.read_text()); assert len(reference)==len(actual)==len(source)
        for name,value in [('input',source),('go',reference),('rust',actual)]: (folder/(name+'.json.gz')).write_bytes(gzip.compress(json.dumps(value).encode(),mtime=0))
        inp.unlink();target.unlink()
        for case,left,right in zip(source,reference,actual):
            passed=left==right;rows.append({'case':case['case'],'iteration':iteration,'pass':passed})
            if not passed: print('FAIL',case['case'],left,right,flush=True)
    assert sources=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    summary={'scope':'T0 actual owning Go verifyResourceDelegation, exact error ordering and Go claim encoding; reservation/task issuance, callback scope, child task inheritance and public composition remain unverified by this fixture','sourceLock':lock,'provenance':sources,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'normalization':'none; exact error and encoded delegation bytes','rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider delegations:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
