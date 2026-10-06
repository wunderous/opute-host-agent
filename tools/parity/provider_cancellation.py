#!/usr/bin/env python3
"""Native Go/Rust client cancellation with owned sockets and observed body-read progress."""
import argparse
import copy
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
from provider_adapter import ROOT,freeze_binary,sha

class Fixture:
    def __init__(self,stage):
        self.stage=stage;self.records=[];self.eof=[];self.stop=threading.Event();self.started=threading.Event()
        self.socket=socket.socket();self.socket.bind(('127.0.0.1',0));self.socket.listen();self.socket.settimeout(.02)
        self.thread=threading.Thread(target=self.serve,daemon=True);self.thread.start()
    def serve(self):
        while not self.stop.is_set():
            try:client,_=self.socket.accept()
            except socket.timeout:continue
            with client:
                client.settimeout(3);client.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
                raw=b''
                while b'\r\n\r\n' not in raw:raw+=client.recv(4096)
                header,body=raw.split(b'\r\n\r\n',1);lines=header.decode().split('\r\n');headers=dict(line.lower().split(': ',1) for line in lines[1:])
                length=int(headers['content-length'])
                while len(body)<length:body+=client.recv(length-len(body))
                self.records.append({'path':lines[0].split()[1],'headers':headers,'body':body.decode()});self.started.set()
                if self.stage=='body':client.sendall(b'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{')
                try:closed=client.recv(1)==b''
                except socket.timeout:closed=False
                self.eof.append(closed)
    def reset(self):self.records.clear();self.eof.clear();self.started.clear()
    def close(self):self.stop.set();self.thread.join(timeout=4);self.socket.close();assert not self.thread.is_alive()

def launch(command,input_text,env,fixture,source):
    for field in ['trigger','marker']:Path(source[field]).unlink(missing_ok=True)
    process=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,env=env)
    process.stdin.write(input_text);process.stdin.close();process.stdin=None
    deadline=time.monotonic()+4
    if not source['before'] and source.get('timeoutMs')!=0:
        if source['progressStage']=='body':
            while not Path(source['marker']).exists() and process.poll() is None and time.monotonic()<deadline:time.sleep(.001)
            assert Path(source['marker']).exists(),'client never reached a native body read'
        else:assert fixture.started.wait(2),'client never sent a request'
        if 'timeoutMs' not in source:Path(source['trigger']).write_text('cancel\n')
    stdout,stderr=process.communicate(timeout=5);assert process.returncode==0,stderr
    if not source['before'] and source.get('timeoutMs')!=0:
        while not fixture.eof and time.monotonic()<deadline:time.sleep(.001)
        assert fixture.eof==[True],'cancelled request/body did not close its socket'
    return stdout,stderr

def run(go,rust,out):
    go=freeze_binary(go,'go');rust=freeze_binary(rust,'rust');out.mkdir(parents=True,exist_ok=True)
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/provider-core-oracle/main.go',ROOT/'tools/parity/provider-core-oracle/go.mod',ROOT/'tools/parity/provider-core-oracle/go.sum',ROOT/'crates/host-agent/src/provider/adapter.rs',ROOT/'crates/host-agent/src/provider/adapter/envelope.rs',ROOT/'crates/host-agent/src/plan/runner.rs',ROOT/'crates/host-agent/src/goerr.rs',ROOT/'crates/host-agent/src/goerr/printable.rs',ROOT/'Cargo.lock']
    before={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    cases=[('cancel-before','headers',True,None),('expired-before','headers',False,0),('expired-then-cancel','headers',True,0),('cancel-headers','headers',False,None),('deadline-headers','headers',False,500),('cancel-body','body',False,None),('deadline-body','body',False,500)]
    rows=[]
    for name,stage,cancelled,timeout in cases:
        for iteration in range(5):
            folder=out/name/str(iteration);folder.mkdir(parents=True,exist_ok=True);fixture=Fixture(stage)
            try:
                source={'endpoint':f'http://127.0.0.1:{fixture.socket.getsockname()[1]}/mcp','before':cancelled,'progressStage':stage,'marker':str((folder/'progress.marker').resolve()),'trigger':str((folder/'cancel.trigger').resolve())}
                if timeout is not None:source['timeoutMs']=timeout
                input_path=folder/'input.json';input_path.write_text(json.dumps(source,indent=2)+'\n')
                stdout,stderr=launch([str(go)],json.dumps([{'kind':'client-cancel','value':source}]),os.environ,fixture,source)
                reference=json.loads(stdout)[0];capture={'result':reference,'requests':copy.deepcopy(fixture.records),'eof':fixture.eof.copy(),'bodyReadObserved':Path(source['marker']).exists()};(folder/'go.json').write_text(json.dumps(capture,indent=2)+'\n');fixture.reset()
                target=folder/'rust-result.json';env={**os.environ,'PARITY_PROVIDER_CANCEL_INPUT':str(input_path.resolve()),'PARITY_PROVIDER_CANCEL_OUTPUT':str(target.resolve())}
                launch([str(rust),'provider::adapter::tests::cancellation_fixture','--exact','--ignored'],'',env,fixture,source)
                actual=json.loads(target.read_text());capture_rust={'result':actual,'requests':copy.deepcopy(fixture.records),'eof':fixture.eof.copy(),'bodyReadObserved':Path(source['marker']).exists()};(folder/'rust.json').write_text(json.dumps(capture_rust,indent=2)+'\n')
                passed=capture==capture_rust;rows.append({'case':name,'iteration':iteration,'pass':passed})
                if not passed:print('FAIL',name,iteration,reference,actual)
            finally:fixture.close()
    assert before=={str(p.relative_to(ROOT)):sha(p) for p in paths}
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and not git('status','--porcelain')
    report={'scope':'T1 actual native HTTP clients: pre-request cancellation/expiration, header waits and observed partial body reads; exact error/request comparison and socket closure. Go body-read wrapper and Rust cfg(test) progress markers observe stages only; parent/child task waits, TLS/proxy/redirect cancellation and lifecycle remain open','sourceLock':lock,'provenance':before,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'passed':sum(r['pass'] for r in rows),'total':len(rows),'normalization':'none; body-read progress, request bytes/header values, error strings and socket EOF are exact'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name!='index.json'},indent=2)+'\n');print('Native client cancellation:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
