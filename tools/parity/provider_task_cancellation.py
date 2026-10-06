#!/usr/bin/env python3
"""Native Go/Rust task waits and independent bounded cancellation RPCs."""
import argparse
import copy
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
from provider_adapter import ROOT, freeze_binary, sha


class Fixture(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, stage, cleanup):
        self.stage = stage
        self.cleanup = cleanup
        self.records = []
        self.eof = []
        self.poll_started = threading.Event()
        self.finished = threading.Event()
        super().__init__(('127.0.0.1', 0), Handler)

    def reset(self):
        self.records.clear()
        self.eof.clear()
        self.poll_started.clear()
        self.finished.clear()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    disable_nagle_algorithm = True

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers['Content-Length']))
        method = json.loads(body)['method']
        self.server.records.append({'path': self.path, 'headers': {k.lower(): v for k, v in self.headers.items()}, 'body': body.decode()})
        if method == 'tools/call':
            result = {'resultType': 'task', 'taskId': 'child-fixture', 'pollIntervalMs': 10000}
            self.reply({'result': result})
        elif method == 'tasks/get':
            self.server.poll_started.set()
            if self.server.stage in ['headers', 'body']:
                self.stall(method, self.server.stage)
            else:
                self.reply({'result': {'status': 'working'}})
        elif method == 'tasks/cancel':
            if self.server.cleanup in ['headers', 'body']:
                self.stall(method, self.server.cleanup)
            elif self.server.cleanup == 'refusal':
                self.reply({'error': {'code': -32001, 'message': 'cleanup refusal'}})
            else:
                self.reply({'result': {}})
        else:
            raise AssertionError(method)

    def reply(self, envelope):
        raw = json.dumps({'jsonrpc': '2.0', 'id': 1, **envelope}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
        self.wfile.flush()

    def stall(self, method, stage):
        if stage == 'body':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', '100')
            self.end_headers()
            self.wfile.write(b'{')
            self.wfile.flush()
        self.connection.settimeout(4)
        try:
            closed = self.connection.recv(1) == b''
        except socket.timeout:
            closed = False
        self.server.eof.append({'method': method, 'closed': closed})
        self.close_connection = True
        if method == 'tasks/cancel':
            self.server.finished.set()


def launch(command, stdin, env, fixture, source):
    for field in ['trigger', 'marker']:
        Path(source[field]).unlink(missing_ok=True)
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
    process.stdin.write(stdin)
    process.stdin.close()
    process.stdin = None
    deadline = time.monotonic() + 5
    try:
        if not source['before'] and source.get('timeoutMs') != 0:
            assert fixture.poll_started.wait(2), 'client never reached tasks/get'
            if fixture.stage != 'headers':
                while not Path(source['marker']).exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(.001)
                assert Path(source['marker']).exists(), 'client never observed task status body'
                if fixture.stage == 'poll':
                    # Require the final unwrapped wait error and exactly one poll below;
                    # the marker alone proves ReadAll returned, not status interpretation.
                    time.sleep(.02)
            if 'timeoutMs' not in source:
                Path(source['trigger']).write_text('cancel\n')
        stdout, stderr = process.communicate(timeout=6)
        assert process.returncode == 0, stderr
        if fixture.stage in ['headers', 'body'] and not source['before']:
            while not any(e['method'] == 'tasks/get' for e in fixture.eof) and time.monotonic() < deadline:
                time.sleep(.001)
        if fixture.cleanup in ['headers', 'body'] and not source['before']:
            assert fixture.finished.wait(2), 'cleanup request never closed its socket'
        return json.loads(stdout)[0] if stdin else None
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()


def run(go, rust, out):
    go = freeze_binary(go, 'go')
    rust = freeze_binary(rust, 'rust')
    out.mkdir(parents=True, exist_ok=True)
    paths = [Path(__file__), ROOT/'tools/parity/provider_adapter.py', ROOT/'tools/parity/provider-core-oracle/main.go', ROOT/'tools/parity/provider-core-oracle/go.mod', ROOT/'tools/parity/provider-core-oracle/go.sum', ROOT/'crates/host-agent/src/plan/runner.rs', ROOT/'Cargo.lock']
    paths += sorted((ROOT/'crates/host-agent/src/provider').rglob('*.rs'))
    paths += [ROOT/'crates/host-agent/src/mcpsdk.rs', ROOT/'crates/host-agent/src/gojson.rs', ROOT/'crates/host-agent/src/gojson/validate.rs', ROOT/'crates/host-agent/src/goerr.rs', ROOT/'crates/host-agent/src/goerr/printable.rs']
    before = {str(p.relative_to(ROOT)): sha(p) for p in paths}
    lock = json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):
            return subprocess.check_output(['git', '-C', str(ROOT/'.parity/go-src'), *args], text=True).strip()
        assert git('rev-parse', 'HEAD') == lock['sourceCommit'] and git('rev-parse', 'HEAD^{tree}') == lock['sourceTree'] and not git('status', '--porcelain')
    pinned()
    cases = [('cancel-before', 'poll', 'ok', True, None), ('expired-before', 'poll', 'ok', False, 0)]
    for stage in ['headers', 'body', 'poll']:
        for mode in ['cancel', 'deadline']:
            cases.append((mode+'-'+stage, stage, 'ok', False, 500 if mode == 'deadline' else None))
    for cleanup in ['refusal', 'headers', 'body']:
        for mode in ['cancel', 'deadline']:
            cases.append((mode+'-cleanup-'+cleanup, 'poll', cleanup, False, 500 if mode == 'deadline' else None))
    rows = []
    for name, stage, cleanup, cancelled, timeout in cases:
        for iteration in range(5):
            folder = out/name/str(iteration)
            folder.mkdir(parents=True, exist_ok=True)
            server = Fixture(stage, cleanup)
            thread = threading.Thread(target=lambda: server.serve_forever(poll_interval=.01), daemon=True)
            thread.start()
            try:
                source = {'endpoint': f'http://127.0.0.1:{server.server_port}/mcp', 'task': True, 'before': cancelled, 'progressStage': 'body-end' if stage == 'poll' else stage, 'progressMethod': 'tasks/get', 'marker': str((folder/'progress.marker').resolve()), 'trigger': str((folder/'cancel.trigger').resolve())}
                if timeout is not None:
                    source['timeoutMs'] = timeout
                inp = folder/'input.json'
                inp.write_text(json.dumps(source, indent=2)+'\n')
                reference = launch([str(go)], json.dumps([{'kind': 'client-cancel', 'value': source}]), os.environ, server, source)
                captures = []
                for result in [reference, None]:
                    if result is None:
                        target = folder/'rust-result.json'
                        env = {**os.environ, 'PARITY_PROVIDER_CANCEL_INPUT': str(inp.resolve()), 'PARITY_PROVIDER_CANCEL_OUTPUT': str(target.resolve())}
                        launch([str(rust), 'provider::adapter::tests::cancellation_fixture', '--exact', '--ignored'], '', env, server, source)
                        result = json.loads(target.read_text())
                    methods = [json.loads(r['body'])['method'] for r in server.records]
                    expected_methods = [] if cancelled or timeout == 0 else ['tools/call', 'tasks/get', 'tasks/cancel']
                    assert methods == expected_methods, (name, methods)
                    expected_eof = ([] if stage == 'poll' else [{'method': 'tasks/get', 'closed': True}]) if expected_methods else []
                    if cleanup in ['headers', 'body'] and expected_methods:
                        expected_eof.append({'method': 'tasks/cancel', 'closed': True})
                    # EOF observations from independent sockets are a set; each method occurs once.
                    eof = sorted(server.eof, key=lambda e: e['method'])
                    assert eof == sorted(expected_eof, key=lambda e: e['method']), (name, eof)
                    if expected_methods and stage == 'poll':
                        cause = 'context deadline exceeded' if timeout is not None else 'context canceled'
                        assert result['error'].startswith('wait for host task child-fixture: '+cause), result
                    capture = {'result': result, 'requests': copy.deepcopy(server.records), 'eof': eof, 'bodyReadObserved': Path(source['marker']).exists()}
                    captures.append(capture)
                    server.reset()
                for side, capture in zip(['go', 'rust'], captures):
                    (folder/(side+'.json')).write_text(json.dumps(capture, indent=2)+'\n')
                passed = captures[0] == captures[1]
                rows.append({'case': name, 'iteration': iteration, 'pass': passed})
                if not passed:
                    print('FAIL', name, iteration, captures[0]['result'], captures[1]['result'], flush=True)
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
    assert before == {str(p.relative_to(ROOT)): sha(p) for p in paths}
    pinned()
    report = {'scope': 'T1 actual native clients: task callback cancellation/deadlines before dispatch, during tasks/get headers and partial body reads, and during polling. Independent cleanup RPC success, refusal and two-second header/body deadline; generation/callback admission and parent-context cause ordering remain open.', 'sourceLock': lock, 'provenance': before, 'goBinarySha256': sha(go), 'rustTestBinarySha256': sha(rust), 'rows': rows, 'passed': sum(r['pass'] for r in rows), 'total': len(rows), 'normalization': 'Only socket EOF observations sorted by method (independent connections). Requests, header values, delegation, task IDs, body-read observations and final errors are exact. No latency mask; two-second cleanup is observed via its native deadline and socket closure.'}
    (out/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)): sha(p) for p in out.rglob('*.json') if p.name != 'index.json'}, indent=2)+'\n')
    print('Native task cancellation:', report['passed'], '/', report['total'])
    return all(r['pass'] for r in rows)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--go', type=Path, required=True)
    p.add_argument('--rust-tests', type=Path, required=True)
    p.add_argument('--out', type=Path, required=True)
    a = p.parse_args()
    raise SystemExit(0 if run(a.go.resolve(), a.rust_tests.resolve(), a.out.resolve()) else 1)
