#!/usr/bin/env python3
"""T1 loopback provider discovery/registration comparison with actual Go adapter."""
import argparse
import copy
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import threading

ROOT=Path(__file__).resolve().parents[2]
MANIFEST='opute.provider.get_install_manifest'

def cases():
    valid={'discover':{'supportedVersions':['2026-07-28']},'listed':{'tools':[{'name':MANIFEST},{'name':'test.operation'}]}}
    rows={'valid':valid,'explicit-client':{**valid,'name':' fixture-client ','version':' test-version ','token':'fixture-bearer'},'blank-client':{**valid,'name':' ','version':' ','token':' '}}
    for i,versions in enumerate([[],['2025-11-25'],['2025-11-25','2026-07-28'],'2026-07-28',[20260728],[None],None]):rows[f'versions-{i}']={**valid,'discover':{'supportedVersions':versions}}
    rows['discover-empty']={**valid,'discover':{}}
    for i,tools in enumerate([[],None,{},[{}],[None],[{'name':42}],[{'name':' '}],[{'name':MANIFEST}]*2,[{'name':'test.operation'}],[{'name':MANIFEST},{'name':' test.operation '}]]):rows[f'tools-{i}']={**valid,'listed':{'tools':tools}}
    rows['missing-tools']={**valid,'listed':{}}
    rows['discover-http-403']={**valid,'discoverStatus':403,'discoverBody':' forbidden \n'}
    rows['list-http-503']={**valid,'listStatus':503,'listBody':' unavailable \n'}
    rows['discover-rpc-error']={**valid,'discoverError':{'code':-32000,'message':'fixture refusal'}}
    rows['list-rpc-error']={**valid,'listError':{'code':-32001,'message':'fixture list refusal'}}
    return rows

class Fixture(http.server.ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self,case):
        self.case=case;self.records=[]
        super().__init__(('127.0.0.1',0),Handler)
class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args):pass
    def do_POST(self):
        body=self.rfile.read(int(self.headers['Content-Length']))
        payload=json.loads(body)
        self.server.records.append({'path':self.path,'headers':{k.lower():v for k,v in self.headers.items()},'body':body.decode()})
        key='discover' if payload['method']=='server/discover' else 'list'
        case=self.server.case
        status=case.get(key+'Status',200)
        if status>=400:response=case[key+'Body'].encode()
        else:
            envelope={'jsonrpc':'2.0','id':1}
            if key+'Error' in case:envelope['error']=case[key+'Error']
            else:envelope['result']=case['discover' if key=='discover' else 'listed']
            response=json.dumps(envelope).encode()
        self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(response)));self.end_headers();self.wfile.write(response)

def sha(path):return hashlib.file_digest(path.open('rb'),'sha256').hexdigest()
def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);rows=[]
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    for name,case in cases().items():
        for iteration in range(5):
            folder=out/name/str(iteration);folder.mkdir(parents=True,exist_ok=True)
            server=Fixture(case);thread=threading.Thread(target=lambda:server.serve_forever(poll_interval=0.01),daemon=True);thread.start()
            try:
                source={'descriptor':{'schema':'opute-provider-plugin.v1','pluginId':'test.provider','version':'v1','capabilities':[{'id':'test.capability','version':1}],'server':{'transport':'streamable_http','endpoint':f'http://127.0.0.1:{server.server_port}/mcp'}},'token':case.get('token',''),'name':case.get('name',''),'version':case.get('version','')}
                input_path=folder/'input.json';input_path.write_text(json.dumps(source,indent=2)+'\n')
                reference=json.loads(subprocess.check_output([str(go)],input=json.dumps([{'kind':'adapter','value':source}]),text=True,timeout=15))[0]
                captured_go=copy.deepcopy(server.records);server.records.clear()
                (folder/'go.json').write_text(json.dumps({'result':reference,'requests':captured_go},indent=2)+'\n')
                result_path=folder/'rust-result.json'
                env={**os.environ,'PARITY_PROVIDER_ADAPTER_INPUT':str(input_path.resolve()),'PARITY_PROVIDER_ADAPTER_OUTPUT':str(result_path.resolve())}
                subprocess.run([str(rust),'provider::adapter::tests::adapter_fixture','--exact','--ignored'],env=env,capture_output=True,check=True,timeout=15)
                actual=json.loads(result_path.read_text());captured_rust=copy.deepcopy(server.records)
                (folder/'rust.json').write_text(json.dumps({'result':actual,'requests':captured_rust},indent=2)+'\n')
                passed=reference==actual and captured_go==captured_rust
                rows.append({'case':name,'iteration':iteration,'pass':passed})
                if not passed:print('FAIL',name,iteration,'result',reference==actual,'requests',captured_go==captured_rust)
            finally:server.shutdown();server.server_close();thread.join()
    report={'scope':'T1 loopback modern discovery/registration, HTTP status/RPC refusals and exact request bytes/headers; no tool calls, TLS, proxy, transport failures, cancellation, lifecycle or M7 E2E claim','rows':rows,'total':len(rows),'passed':sum(r['pass'] for r in rows),'normalization':'HTTP header names are lowercased (case-insensitive HTTP semantics); ToolNames are projected as a sorted set (Go map iteration is unspecified). Body bytes and header values remain exact; the same exclusively bound OS-assigned port serves both implementations.','sourceLock':lock,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'driverSha256':sha(Path(__file__)),'rustSourceSha256':sha(ROOT/'crates/host-agent/src/provider/adapter.rs'),'cargoLockSha256':sha(ROOT/'Cargo.lock'),'goOracleSourceSha256':sha(ROOT/'tools/parity/provider-core-oracle/main.go'),'goOracleModSha256':sha(ROOT/'tools/parity/provider-core-oracle/go.mod'),'goOracleSumSha256':sha(ROOT/'tools/parity/provider-core-oracle/go.sum')}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    index={str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name!='index.json'};(out/'index.json').write_text(json.dumps(index,indent=2)+'\n')
    print('Provider adapter discovery:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
