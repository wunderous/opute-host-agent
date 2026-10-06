#!/usr/bin/env python3
"""T1 loopback provider discovery/registration comparison with actual Go adapter."""
import argparse
import copy
import hashlib
import http.server
import json
import os
import shutil
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
    result={'content':[],'structuredContent':{'value':'fixture'}}
    rows['call-valid']={**valid,'action':'call','callResult':result,'arguments':{'value':'input'},'delegation':' signed-fixture '}
    rows['call-closed']={**valid,'action':'closed','callResult':result}
    rows['call-unregistered']={**valid,'action':'call','operation':'unknown.operation','callResult':result}
    rows['sync-valid']={**valid,'action':'sync','callResult':result}
    for i,value in enumerate([{'resultType':'task'},{'taskId':'inner'},{'taskId':None},{'taskId':False},['task'],'primitive']):rows[f'sync-structured-{i}']={**valid,'action':'sync','callResult':{'content':[],'structuredContent':value}}
    manifest={'schema':'opute-provider-install-manifest.v1','provider':{'id':'test.provider','version':'v1'},'provides':[{'id':'test.capability','version':1}],'validation':{'capability':'test.capability','operation':'test.operation'},'services':[{'id':'test.service','capabilityId':'test.capability','version':1,'operations':[]}]}
    rows['manifest-valid']={**valid,'action':'manifest','callResult':{'content':[],'structuredContent':manifest}}
    for i,value in enumerate([{}, {'content':[],'isError':True,'structuredContent':manifest},{'content':[],'structuredContent':None},{'content':[],'structuredContent':'wrong-type'},{'content':[],'structuredContent':{**manifest,'provider':{'id':'wrong','version':'v1'}}},{'content':[],'structuredContent':{**manifest,'services':[]}}]):rows[f'manifest-invalid-{i}']={**valid,'action':'manifest','callResult':value}
    task={'resultType':'task','taskId':'child-fixture','pollIntervalMs':1}
    rows['task-completed']={**valid,'action':'call','callResult':task,'taskStatuses':[{'status':'working'},{'status':' completed ','result':result}],'delegation':' signed-fixture '}
    for status in ['failed','cancelled','input_required']:
        rows['task-'+status]={**valid,'action':'call','callResult':task,'taskStatuses':[{'status':status,'statusMessage':' fixture refusal '}],'delegation':' signed-fixture '}
    rows['task-no-result']={**valid,'action':'call','callResult':task,'taskStatuses':[{'status':'completed'}]}
    rows['task-poll-error']={**valid,'action':'call','callResult':task,'taskGetError':{'code':-32000,'message':'poll refusal'},'delegation':' signed-fixture '}
    rows['task-cancel-error']={**valid,'action':'call','callResult':task,'taskGetError':{'code':-32000,'message':'poll refusal'},'cancelError':{'code':-32001,'message':'cancel refusal'}}
    sdk=[{}, {'resultType':'complete','inputRequests':{}}, {'resultType':'input_required','inputRequests':{'request-id':{'method':'elicitation/create','params':{'message':'confirm'}}}}, {'inputRequests':{'request-id':{'method':'roots/list','params':{}}}}, {'inputRequests':{'request-id':{'method':'sampling/createMessage','params':{'messages':[{'role':'user','content':[{'type':'text','text':'hello'}]}]}}}}]
    for kind in ['text','image','audio','resource','resource_link','tool_use','tool_result']:
        sdk.extend([{'content':[{'type':kind}]},{'content':[{'type':kind,'unknown':'ignored','annotations':{'audience':[None],'priority':.5}}]}])
    sdk.extend([{'content':[None]},{'content':[{'type':'unknown'}]},{'content':[{'type':'text','data':'%%%'}]},{'content':[{'type':'text','text':42}]},{'content':[{'type':'resource_link','size':9223372036854775807}]},{'content':[{'type':'image','data':[0,255]}]},{'content':[{'type':'tool_result','content':[{'type':'tool_use'}]}]},{'inputRequests':False},{'inputRequests':{'request-id':{'method':'unsupported','params':{}}}},{'inputRequests':{'request-id':{'method':'sampling/createMessage','params':{'messages':[{'role':False,'content':{'type':'text'}}]}}}}])
    for i,value in enumerate(sdk):
        rows[f'sdk-call-{i}']={**valid,'action':'call','callResult':value,'delegation':' signed-fixture '}
        if i in [0,1,2,19,20,21,22,27,28]:rows[f'sdk-task-{i}']={**valid,'action':'call','callResult':task,'taskStatuses':[{'status':'completed','result':value}],'delegation':' signed-fixture '}
    return rows

class Fixture(http.server.ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self,case):
        self.case=case;self.records=[];self.polls=0
        super().__init__(('127.0.0.1',0),Handler)
class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    disable_nagle_algorithm=True  # Match Go net/http provider servers; no latency normalization.
    def log_message(self,*args):pass
    def do_POST(self):
        body=self.rfile.read(int(self.headers['Content-Length']))
        payload=json.loads(body)
        self.server.records.append({'path':self.path,'headers':{k.lower():v for k,v in self.headers.items()},'body':body.decode()})
        key={'server/discover':'discover','tools/list':'list','tools/call':'call','tasks/get':'taskGet','tasks/cancel':'cancel'}[payload['method']]
        case=self.server.case
        status=case.get(key+'Status',200)
        if status>=400:response=case[key+'Body'].encode()
        else:
            envelope={'jsonrpc':'2.0','id':1}
            if key+'Error' in case:envelope['error']=case[key+'Error']
            else:
                if key=='taskGet':
                    statuses=case['taskStatuses'];envelope['result']=statuses[min(self.server.polls,len(statuses)-1)];self.server.polls+=1
                elif key=='cancel':envelope['result']={}
                else:envelope['result']=case[{'discover':'discover','list':'listed','call':'callResult'}[key]]
            response=json.dumps(envelope).encode()
        self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(response)));self.end_headers();self.wfile.write(response)

def sha(path):return hashlib.file_digest(path.open('rb'),'sha256').hexdigest()
def freeze_binary(binary,kind):
    digest=sha(binary);snapshot=ROOT/'.parity/bin'/f'provider-adapter-{kind}-{digest}'
    if not snapshot.exists():
        shutil.copyfile(binary,snapshot);snapshot.chmod(0o755)
    assert sha(snapshot)==digest, 'binary changed while snapshotting'
    return snapshot

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);rows=[]
    go=freeze_binary(go,"go");rust=freeze_binary(rust,"rust")
    provenance_paths=[Path(__file__),ROOT/"crates/host-agent/src/provider/adapter.rs",ROOT/"Cargo.lock",ROOT/"tools/parity/provider-core-oracle/main.go",ROOT/"tools/parity/provider-core-oracle/go.mod",ROOT/"tools/parity/provider-core-oracle/go.sum",ROOT/"crates/host-agent/src/provider/result.rs",ROOT/"crates/host-agent/src/provider/result/requests.rs",ROOT/"crates/host-agent/src/mcpsdk.rs",ROOT/"crates/host-agent/src/gojson.rs"]
    provenance_before={str(p.relative_to(ROOT)):sha(p) for p in provenance_paths}
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    for name,case in cases().items():
        for iteration in range(5):
            folder=out/name/str(iteration);folder.mkdir(parents=True,exist_ok=True)
            server=Fixture(case);thread=threading.Thread(target=lambda:server.serve_forever(poll_interval=0.01),daemon=True);thread.start()
            try:
                source={'descriptor':{'schema':'opute-provider-plugin.v1','pluginId':'test.provider','version':'v1','capabilities':[{'id':'test.capability','version':1}],'server':{'transport':'streamable_http','endpoint':f'http://127.0.0.1:{server.server_port}/mcp'}},'token':case.get('token',''),'name':case.get('name',''),'version':case.get('version',''),'action':case.get('action',''),'operation':case.get('operation','test.operation'),'arguments':case.get('arguments'),'delegation':case.get('delegation','')}
                input_path=folder/'input.json';input_path.write_text(json.dumps(source,indent=2)+'\n')
                reference=json.loads(subprocess.check_output([str(go)],input=json.dumps([{'kind':'adapter','value':source}]),text=True,timeout=15))[0]
                captured_go=copy.deepcopy(server.records);server.records.clear();server.polls=0
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
    assert provenance_before=={str(p.relative_to(ROOT)):sha(p) for p in provenance_paths}, 'source changed during oracle run'
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    report={'scope':'T1 loopback modern discovery, registered core calls, sync-only guards, typed manifest retrieval and flat-task polling/cancellation RPCs; null input-request refusal (proposed divergence), TLS, proxy, malformed envelopes, transport failures, cancelled/deadline differential waits, lifecycle and M7 E2E remain open','provenance':provenance_before,'binarySnapshots':{'go':str(go.relative_to(ROOT)),'rust':str(rust.relative_to(ROOT))},'rows':rows,'total':len(rows),'passed':sum(r['pass'] for r in rows),'normalization':'HTTP header names are lowercased (case-insensitive HTTP semantics); ToolNames are projected as a sorted set (Go map iteration is unspecified). Body bytes and header values remain exact; the same exclusively bound OS-assigned port serves both implementations.','sourceLock':lock,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'driverSha256':sha(Path(__file__)),'rustSourceSha256':sha(ROOT/'crates/host-agent/src/provider/adapter.rs'),'cargoLockSha256':sha(ROOT/'Cargo.lock'),'goOracleSourceSha256':sha(ROOT/'tools/parity/provider-core-oracle/main.go'),'goOracleModSha256':sha(ROOT/'tools/parity/provider-core-oracle/go.mod'),'goOracleSumSha256':sha(ROOT/'tools/parity/provider-core-oracle/go.sum')}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    index={str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name!='index.json'};(out/'index.json').write_text(json.dumps(index,indent=2)+'\n')
    print('Provider adapter core flows:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
