#!/usr/bin/env python3
"""T0 owning Go generation service mounts vs native ownership layer."""
import argparse
import copy
import gzip
import json
import os
from pathlib import Path
import subprocess
from provider_adapter import ROOT, freeze_binary, sha


def cases():
    rows = []
    def service(id='base', family='test.base', dependencies=()):
        return {'id': id, 'capabilityId': family, 'version': 1, 'operations': [],
                'dependencies': [{'id': family, 'version': 1} for family in dependencies]}
    def mount(services, provider='test.provider', generation='g1', adapter='a'):
        return {'action': 'mount', 'manifest': {'provider': {'id': provider, 'version': 'v1'},
                'services': services}, 'generation': generation, 'adapter': adapter}
    def connection(id='a'):
        return {'action': 'adapter', 'adapter': id}
    def restore(provider='test.provider', generation='g1', state='candidate'):
        return {'action': 'restore', 'restore': {'ID': generation, 'Provider': {'id': provider, 'version': 'v1'},
                'ManifestHash': 'hash', 'Endpoint': 'http://127.0.0.1:1', 'State': state}}
    def lookup(service='base', provider='test.provider', generation='g1'):
        return {'action': 'lookup', 'service': service, 'provider': provider, 'generation': generation}
    def add(name, steps):
        rows.append({'case': name, 'mounts': {'steps': copy.deepcopy(steps)+[{'action': 'close'}, {'action': 'close'}]}})
    stack = [service(), service('consumer', 'test.consumer', ['test.base'])]
    for services, label in [([], 'empty'), (None, 'nil'), ([service()], 'one'), (stack, 'stack'),
                            (list(reversed(stack)), 'reversed'), ([service(), service('bad', 'test.bad', ['missing'])], 'missing-late'),
                            ([service('bad', 'test.bad', ['missing']), service()], 'missing-first'),
                            ([service(), service()], 'duplicate-service'),
                            ([service(), service('alias', 'test.base'), service('consumer', 'test.consumer', ['test.base'])], 'ambiguous')]:
        add(label, [connection(), mount(services), lookup(), lookup('consumer')])
    for generation in ['', ' ', '\t\n', ' g1 ', 'g1', 'opaque@id', 'opaque/雪']:
        add('generation-'+repr(generation), [connection(), mount(stack, generation=generation),
            lookup(generation=generation), lookup('consumer', generation=generation),
            {'action': 'unmount', 'provider': 'test.provider', 'generation': generation}, lookup(generation=generation)])
    add('nil-adapter', [mount(stack), lookup()])
    add('nil-adapter-blank-generation', [mount(stack, generation='')])
    for dependencies in [[], ['test.base'], [' test.base '], ['test.base', 'test.base'], ['test.base', 'missing'], ['missing', 'test.base'], [''], ['\n'], ['TEST.BASE']]:
        add('dependencies-'+repr(dependencies), [connection(), mount([service(), service('consumer', 'test.consumer', dependencies)]), lookup('consumer')])
    for ids in [['test.provider/base@g1'], ['test.provider/consumer@g1'],
                ['test.provider/base@g1', 'test.provider/consumer@g1'],
                ['test.provider/consumer@g1', 'test.provider/base@g1'], ['missing'], [],
                ['test.provider/base@g1', 'test.provider/base@g1']]:
        add('dispose-'+repr(ids), [connection(), mount(stack), {'action': 'ids', 'ids': ids}, lookup('consumer'),
                                 {'action': 'ids', 'ids': ids}, lookup()])
    add('failed-remount-preserves-owner', [connection(), mount(stack), connection('new'), mount(stack, adapter='new'), lookup('consumer')])
    for state in ['candidate', 'ready', 'active', 'draining', 'stopped', 'failed', 'unknown']:
        add('external-dependency-'+state, [restore(state=state), connection(), mount([service()]), connection('other'),
            mount([service('consumer', 'test.consumer', ['test.base'])], 'other.provider', 'other-1', 'other'),
            lookup('consumer', 'other.provider', 'other-1')])
    replacement = [restore(), connection(), mount(stack), {'action': 'ready', 'generation': 'g1'},
                   {'action': 'activate', 'generation': 'g1'}, restore(generation='g2'), connection('two'),
                   mount(stack, generation='g2', adapter='two')]
    add('replacement-active-is-ambiguous', replacement+[lookup('consumer', generation='g2')])
    # A candidate with no dependency can coexist; after activation the old
    # mounted generation is excluded from dependency injection.
    swap = [restore(), connection(), mount([service()]), {'action': 'ready', 'generation': 'g1'},
            {'action': 'activate', 'generation': 'g1'}, restore(generation='g2'), connection('two'),
            mount([service()], generation='g2', adapter='two'), {'action': 'ready', 'generation': 'g2'},
            {'action': 'activate', 'generation': 'g2'}]
    add('draining-excluded', swap+[connection('other'), mount([service('consumer', 'test.consumer', ['test.base'])], 'other.provider', 'other-1', 'other'), lookup('consumer', 'other.provider', 'other-1'),
        {'action': 'unmount', 'provider': 'test.provider', 'generation': 'g1'}, lookup(generation='g2')])
    for provider, families in [('', ['test.base']), (' test.provider ', [' test.base ']),
                               ('other.provider', ['test.base']), ('unknown', ['test.consumer']),
                               ('other.provider', []), ('other.provider', ['']), ('other.provider', ['missing'])]:
        add('displace-'+repr((provider, families)), swap+[{'action': 'displace', 'provider': provider, 'families': families}, lookup(), lookup(generation='g2')])
    for provider in [' test.provider ', '', 'test.provider/', '雪/provider', 'test/provider@opaque']:
        for name in [' base ', '', 'slash/service', 'service@opaque', '雪']:
            add('opaque-'+repr((provider, name)), [connection(), mount([service(name)], provider), lookup(name, provider),
                {'action': 'unmount', 'provider': provider, 'generation': 'g1'}, lookup(name, provider)])
    add('opaque-key-collision', [connection(), mount([service('s@g')], generation='h'), connection('two'),
                                mount([service('s')], generation='g@h', adapter='two'), lookup('s@g', generation='h')])
    def invoke(candidate=False, provider='test.provider', generation='g1', name='probe', service_id='base'):
        return {'action':'invoke','provider':provider,'generation':generation,'service':service_id,
                'candidate':candidate,'operation':{'id':name,'version':1}}
    for state in ['candidate','ready','active','draining','stopped','failed','unknown']:
        for candidate in [False,True]:
            for attached in [False,True]:
                setup=[restore(state=state),connection()]
                if attached:
                    setup += [mount([service()]), {'action':'candidateAdapter','generation':'g1','adapter':'a'}]
                add('binding-'+repr((state,candidate,attached)), setup+[invoke(candidate),invoke(candidate,provider='other.provider'),invoke(candidate,generation='missing')])
    for generation in ['', ' ', ' g1 ', '雪/@generation']:
        add('binding-opaque-'+repr(generation),[restore(generation=generation),connection(),
            mount([service()],generation=generation),{'action':'candidateAdapter','generation':generation,'adapter':'a'},
            invoke(True,generation=generation),invoke(False,generation=generation)])
    add('binding-superseded',swap+[invoke(),invoke(generation='g2'),invoke(True),invoke(True,generation='g2'),
        {'action':'candidateAdapter','generation':'g1','adapter':'a'},invoke(True),
        {'action':'unmount','provider':'test.provider','generation':'g1'},invoke(True),invoke(generation='g2'),
        {'action':'unmount','provider':'test.provider','generation':'g2'},invoke(generation='g2')])
    add('binding-absent-session',[connection(),{'action':'candidateAdapter','generation':'g1','adapter':'a'},invoke(True),invoke()])
    add('binding-nil-adapter',[restore(),{'action':'candidateAdapter','generation':'g1','adapter':'missing'},invoke(True)])
    assert len({row['case'] for row in rows}) == len(rows)
    return rows


def run(go, rust, out):
    out.mkdir(parents=True, exist_ok=True)
    go, rust = freeze_binary(go, 'mounts-go'), freeze_binary(rust, 'mounts-rust')
    paths = [Path(__file__), ROOT/'tools/parity/provider_adapter.py', ROOT/'tools/parity/build-provider-catalog-oracle.py',
             ROOT/'baseline/source-lock.json', ROOT/'Cargo.lock', ROOT/'crates/host-agent/src/provider.rs',
             ROOT/'crates/host-agent/src/provider/mounts.rs', ROOT/'crates/host-agent/src/provider/binding.rs', ROOT/'crates/host-agent/src/provider/candidates.rs', ROOT/'crates/host-agent/src/catalog.rs', ROOT/'crates/host-agent/src/provider/adapter.rs',
             ROOT/'crates/host-agent/src/provider/definition.rs', ROOT/'crates/host-agent/src/goerr.rs',
             ROOT/'crates/host-agent/src/goerr/printable.rs',
             ROOT/'crates/host-agent/src/gojson.rs', ROOT/'.parity/go-src/internal/hostmcp/provider_service_plugin.go', ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go', ROOT/'.parity/go-src/internal/capability/adapters.go', ROOT/'.parity/go-src/internal/tools/dispatch.go',
             ROOT/'.parity/go-src/internal/cordis/context.go', ROOT/'.parity/go-src/internal/cordis/generation.go',
             ROOT/'.parity/go-src/internal/cordis/mcp/adapter.go']
    paths += sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths += sorted((ROOT/'.parity/go-src/contracts/provider').glob('*.go'))
    for package in ['strconv', 'unicode']:
        paths += sorted((ROOT/'.parity/setup/go/src'/package).glob('*.go'))
    sources = {str(p): sha(p) for p in paths if p.is_file()}
    lock = json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):
            return subprocess.check_output(['git', '-C', str(ROOT/'.parity/go-src'), *args], text=True).strip()
        assert git('rev-parse', 'HEAD') == lock['sourceCommit']
        assert git('rev-parse', 'HEAD^{tree}') == lock['sourceTree']
        assert not git('status', '--porcelain')
    pinned()
    source, rows = cases(), []
    for iteration in range(5):
        folder = out/str(iteration); folder.mkdir(parents=True, exist_ok=True)
        inp, target = folder/'input.json', folder/'rust.json'
        inp.write_text(json.dumps(source)+'\n')
        reference = json.loads(subprocess.check_output([str(go)], input=json.dumps(source), text=True, timeout=60))
        subprocess.run([str(rust), 'provider::mounts::tests::mounts_fixture', '--exact', '--ignored'],
                       env={**os.environ, 'PARITY_PROVIDER_MOUNTS_INPUT': str(inp.resolve()),
                            'PARITY_PROVIDER_MOUNTS_OUTPUT': str(target.resolve())},
                       capture_output=True, check=True, timeout=60)
        actual = json.loads(target.read_text())
        assert len(reference) == len(actual) == len(source)
        (folder/'input.json.gz').write_bytes(gzip.compress(inp.read_bytes(), mtime=0)); inp.unlink(); target.unlink()
        for side, observed in [('go', reference), ('rust', actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(observed).encode(), mtime=0))
            for row in observed:
                assert row['observations'][-1]['plugins'] == row['observations'][-1]['services'] == []
                assert row['observations'][-1] == row['observations'][-2]
        for case, left, right in zip(source, reference, actual):
            passed = left == right
            rows.append({'case': case['case'], 'iteration': iteration, 'pass': passed})
            if not passed:
                print('FAIL', case['case'], [(i, l, r) for i, (l, r) in enumerate(zip(left['observations'], right['observations'])) if l != r], flush=True)
    assert sources == {str(p): sha(p) for p in paths if p.is_file()}; pinned()
    report = {'scope': 'T0 generation service mount ownership vs actual Go mount, dependency, lookup, displace and Cordis disposal functions, plus actual Go provider capability Invoke session/adapter refusal and tool-error-vs-call-error behavior. Invocation uses unreserved context (owning delegation is a no-op); signed delegation and full error envelopes are not validated here. Unconnected adapters probe owning closed-state admission without network effects. No provider process, public activation, callback or full M7 E2E claim.',
              'sourceLock': lock, 'provenance': sources, 'goBinarySha256': sha(go), 'rustTestBinarySha256': sha(rust),
              'normalization': 'none; complete observations including immutable inject keys, mount order, service keys and adapter refusal states are exact',
              'rows': rows, 'passed': sum(row['pass'] for row in rows), 'total': len(rows)}
    (out/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)): sha(p) for p in out.rglob('*') if p.is_file() and p.name != 'index.json'}, indent=2)+'\n')
    print('Provider service mounts:', report['passed'], '/', report['total'])
    return all(row['pass'] for row in rows)

if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--go', type=Path, required=True); p.add_argument('--rust-tests', type=Path, required=True); p.add_argument('--out', type=Path, required=True)
    a = p.parse_args()
    raise SystemExit(0 if run(a.go.resolve(), a.rust_tests.resolve(), a.out.resolve()) else 1)
