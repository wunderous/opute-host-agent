#!/usr/bin/env python3
"""Capture owning Go activation reference cases; no Rust parity claim."""
import argparse
import copy
import gzip
import json
import subprocess
from pathlib import Path

from provider_adapter import ROOT, freeze_binary, sha
from provider_publication import manifest


def cases():
    generation = {'ID': 'g1', 'Provider': {'id': 'opaque.provider', 'version': 'v1'},
                  'ManifestHash': 'hash', 'Endpoint': 'http://127.0.0.1:1',
                  'State': 'candidate', 'CreatedAt': '2026-10-01T00:00:00Z'}
    start = [{'action': 'restore', 'generation': generation},
             {'action': 'candidate', 'id': 'g1', 'manifest': manifest()}]
    activate = {'action': 'activate', 'metadata': {'activate': True, 'providerGenerationId': 'g1'}}
    scripts = {
        'noop': [{'action': 'activate', 'metadata': {}}],
        'active-no-adapter': start + [activate, activate],
        'admission-denial': start + [{'action': 'admission', 'denial': 'fixture denied'}, activate],
        'ready-persistence-failure': start + [{'action': 'closeState'}, activate],
    }
    return [{'case': name + '-' + str(mode), 'activation': {'standalone': mode, 'steps': copy.deepcopy(steps)}}
            for mode in [False, True] for name, steps in scripts.items()]


def verify(inputs, outputs):
    assert len(inputs) == len(outputs) == 8
    for case, output in zip(inputs, outputs):
        steps = output['observations']
        assert len(steps) == len(case['activation']['steps'])
        last = steps[-1]
        events = [event['name'] for event in last['events']]
        if case['case'].startswith('noop-'):
            assert not last['error'] and not events and not last['generations']
        elif case['case'].startswith('active-no-adapter-'):
            assert not last['error'] and last['generations']['g1']['State'] == 'active'
            assert events == ['provider.ready', 'provider.admission', 'provider.activated']
            assert last['candidates'] == {} and last['durable'][0]['Status'] == 'active'
            assert steps[-2] == steps[-1], 'already-active retry changes state'
        elif case['case'].startswith('admission-denial-'):
            assert last['error'] == 'provider "opaque.provider" generation "g1" was denied admission: fixture denied'
            assert events == ['provider.ready', 'provider.admission']
            assert last['generations']['g1']['State'] == 'failed' and not last['active']
            # The outer recipe cleanup is not run by this composition fixture.
            assert last['durable'][0]['Status'] == 'ready'
        else:
            assert last['error'] == 'persist provider generation readiness: sql: database is closed'
            assert not events and last['generations']['g1']['State'] == 'ready'
            assert last['durableError'] == 'sql: database is closed'


def run(go, out):
    out.mkdir(parents=True, exist_ok=True)
    binary = freeze_binary(go, 'activation-go')
    lock = json.loads((ROOT / 'baseline/source-lock.json').read_text())
    source = ROOT / '.parity/go-src'
    git = lambda *args: subprocess.check_output(['git', '-C', str(source), *args], text=True).strip()
    assert git('rev-parse', 'HEAD') == lock['sourceCommit'] and git('rev-parse', 'HEAD^{tree}') == lock['sourceTree']
    assert not git('status', '--porcelain')
    paths = [Path(__file__), ROOT / 'tools/parity/provider_publication.py', ROOT / 'tools/parity/provider_adapter.py',
             ROOT / 'tools/parity/build-provider-activation-oracle.py', ROOT / 'baseline/source-lock.json']
    paths += sorted((ROOT / 'tools/parity/provider-activation-oracle').iterdir())
    paths += [source / path for path in ['internal/hostmcp/recipe_run.go', 'internal/hostmcp/server.go',
                                         'internal/hostmcp/provider_events.go', 'internal/hostmcp/provider_install.go',
                                         'internal/hostmcp/provider_service_plugin.go', 'internal/state/store.go']]
    provenance = {str(p.relative_to(ROOT)): sha(p) for p in paths}
    inputs = cases()
    outputs = json.loads(subprocess.check_output([str(binary)], input=json.dumps(inputs), text=True, timeout=120))
    verify(inputs, outputs)
    for name, value in [('input', inputs), ('go', outputs)]:
        with gzip.open(out / (name + '.json.gz'), 'wt') as file:
            json.dump(value, file)
    assert provenance == {str(p.relative_to(ROOT)): sha(p) for p in paths}
    assert not git('status', '--porcelain')
    summary = {'scope': 'Owning Go activation composition reference only, two modes, independent durable state and fiber-owned admission listener. No adapters, native implementation comparison, public invocation, recipe cleanup or process E2E claim.',
               'sourceLock': lock, 'provenance': provenance, 'goBinarySha256': sha(binary),
               'cases': [row['case'] for row in inputs], 'referenceChecks': len(inputs)}
    (out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    (out / 'index.json').write_text(json.dumps({p.name: sha(p) for p in out.iterdir() if p.is_file() and p.name != 'index.json'}, indent=2) + '\n')
    print('Owning Go activation reference checks:', len(inputs), '(no Rust parity claim)')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--go', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    run(args.go.resolve(), args.out.resolve())
