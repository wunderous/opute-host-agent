#!/usr/bin/env python3
"""Capture a proposed null-input-request refusal; never claim Go parity or owner approval."""
import argparse
import copy
import json
import os
from pathlib import Path
import subprocess
import threading
from provider_adapter import ROOT,Fixture,cases,freeze_binary,sha

def run(go,rust,out):
    go=freeze_binary(go,'go');rust=freeze_binary(rust,'rust');out.mkdir(parents=True,exist_ok=True)
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/provider-core-oracle/main.go',ROOT/'tools/parity/provider-core-oracle/go.mod',ROOT/'tools/parity/provider-core-oracle/go.sum',ROOT/'crates/host-agent/src/provider/adapter.rs',ROOT/'crates/host-agent/src/provider/result.rs',ROOT/'crates/host-agent/src/provider/result/requests.rs',ROOT/'crates/host-agent/src/mcpsdk.rs',ROOT/'crates/host-agent/src/gojson.rs',ROOT/'Cargo.lock']
    before={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True)
        case={**cases()['call-valid'],'callResult':{'inputRequests':{'fixture-request':None}}}
        server=Fixture(case);thread=threading.Thread(target=lambda:server.serve_forever(poll_interval=.01),daemon=True);thread.start()
        try:
            source={'descriptor':{'schema':'opute-provider-plugin.v1','pluginId':'test.provider','version':'v1','capabilities':[{'id':'test.capability','version':1}],'server':{'transport':'streamable_http','endpoint':f'http://127.0.0.1:{server.server_port}/mcp'}},'token':'','action':'call','operation':'test.operation','delegation':'signed-fixture'}
            input_path=folder/'input.json';input_path.write_text(json.dumps(source,indent=2)+'\n')
            reference=subprocess.run([str(go)],input=json.dumps([{'kind':'adapter','value':source}]),text=True,capture_output=True,timeout=15)
            (folder/'go-stdout.txt').write_text(reference.stdout);(folder/'go-panic.txt').write_text(reference.stderr)
            assert reference.returncode==2 and 'nil pointer dereference' in reference.stderr and 'mcp.(*InputRequestMap).UnmarshalJSON' in reference.stderr
            requests=copy.deepcopy(server.records);server.records.clear();server.polls=0
            target=folder/'rust.json';env={**os.environ,'PARITY_PROVIDER_ADAPTER_INPUT':str(input_path.resolve()),'PARITY_PROVIDER_ADAPTER_OUTPUT':str(target.resolve())}
            subprocess.run([str(rust),'provider::adapter::tests::adapter_fixture','--exact','--ignored'],env=env,capture_output=True,check=True,timeout=15)
            actual=json.loads(target.read_text());assert actual=={'error':'nil InputRequest entry'} and requests==server.records
            (folder/'requests.json').write_text(json.dumps({'go':requests,'rust':server.records},indent=2)+'\n')
            rows.append({'iteration':iteration,'goExitStatus':2,'rust':actual,'requestTraceEqual':True})
        finally:server.shutdown();server.server_close();thread.join()
    regression=subprocess.run([str(rust),'provider::result::tests::proposed_null_input_request_refusal_preserves_decoder','--exact'],text=True,capture_output=True,check=True)
    assert '1 passed' in regression.stdout;(out/'rust-regression.txt').write_text(regression.stdout)
    assert before=={str(p.relative_to(ROOT)):sha(p) for p in paths}
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and not git('status','--porcelain')
    report={'scope':'T1 proposed null input-request refusal through owning provider adapter; exact twin request traces and subsequent valid decoder regression','ownerDecision':'pending','status':'proposed-divergence-unverified','rows':rows,'sourceLock':lock,'provenance':before,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'contract':'openspec/changes/reject-null-provider-input-requests/specs/provider-input-refusal/spec.md'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Proposed null-entry refusal: 5 owning adapter observations; owner decision pending')
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve())
