"""Recompute the M8d (oci domain) validation receipt from the raw captures.

  python3 evidence/m8d/verify.py

Reproduces evidence/m8d/validation.json: the committed head and a clean tree
before and after the round, the binaries the manifest records, the logs of
the standard checks and the owning Go tests (oci, the process primitives,
core), every T1 summary's provenance, both T2 suites re-derived from their
receipts (t2_verify) over the base, nested-Incus, k3s and OCI goldens, the Go live-test oracle run in the
twins, the secret sweep (the oci domain has no write-only field), the inventory gap
ownership, and every milestone gate report rerun and compared byte for byte.
Failures are recorded as failures; nothing is relabelled.
"""
import hashlib, json, re, subprocess, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import t2, t2_verify  # noqa: E402

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
log = lambda name: (HERE / 'checks' / f'{name}.log').read_text()
lock = load(ROOT / 'baseline/source-lock.json')
manifest = load(ROOT / 'parity-manifest.json')
go, rust = ROOT / '.parity/bin/go-reference', ROOT / 'target/release/opute-host-agent'
EMPTY_TREE = hashlib.sha256(b'').hexdigest()

head = (HERE / 'checks/head').read_text().strip()
assert subprocess.run(['git', '-C', str(ROOT), 'merge-base', '--is-ancestor', head, 'HEAD']).returncode == 0
for when in ('before', 'after'):
    assert (HERE / f'checks/tree.{when}.sha256').read_text().split()[0] == EMPTY_TREE, when
for name in ('binaries', 'binaries.after'):
    pinned = dict(reversed(line.split()) for line in (HERE / f'checks/{name}.sha256').read_text().splitlines())
    assert pinned[str(rust)] == sha(rust) == manifest['rustCandidate']['binarySha256'], name
    assert pinned[str(go)] == sha(go) == manifest['goReference']['binarySha256'], name
exits = dict(line.split(' exit=') for line in (HERE / 'checks/exits.log').read_text().splitlines())

# Standard checks.
assert exits['fmt'] == '0' and not log('fmt').strip()
assert exits['clippy'] == '0' and 'warning' not in log('clippy')
tests = [tuple(map(int, m)) for m in re.findall(r'test result: ok\. (\d+) passed; (\d+) failed', log('workspace'))]
assert exits['workspace'] == '0' and tests and 'FAILED' not in log('workspace')
assert exits['harness'] == '0' and log('harness').rstrip().endswith('OK')
assert exits['spec'] == '0'
spec = re.search(r'Totals: (\d+) passed, (\d+) failed', log('spec'))
assert spec and spec.group(2) == '0'
assert exits['catalog-source'] == '0' and exits['go-reference'] == '0' and exits['release'] == '0'
go_owner = log('go-owner-oci')
for name in ('go-owner-oci',):
    assert exits[name] == '0' and '--- FAIL' not in log(name) and 'FAIL\t' not in log(name), name
assert exits['go-owner-core'] == '0' and 'FAIL' not in log('go-owner-core')
go_owner_passes = len(re.findall(r'^\s*--- PASS', go_owner, re.M))


def differential(name, repeat, right):
    summary = load(HERE / name / 'summary.json')
    prov = summary['provenance']
    assert not prov['changedDuringRun'] and prov['repeat'] == repeat
    assert prov['sourceCommit'] == lock['sourceCommit'] and prov['sourceTree'] == lock['sourceTree']
    assert prov['left']['binarySha256'] == sha(go) and prov['right']['binarySha256'] == sha(right)
    assert sorted(summary['items']) == sorted(item['id'] for item in manifest['items'])
    return {status: sum(1 for item in summary['items'].values() if item['status'] == status)
            for status in sorted({item['status'] for item in summary['items'].values()})}


# T2: every scenario, both suites, re-derived from receipts.
problems = []
t2_status = t2_verify.check(ROOT, manifest, problems.append)
assert not problems, problems[:5]
t2_suites = {suite: dict(sorted({s: row[suite] for s, row in t2_status.items()}.items()))
             for suite in ('go-vs-go', 'go-vs-rust')}
golden = load(HERE / 't2-golden.json')
assert golden['goldenSha256'] == manifest['t2Evidence']['golden']['goldenSha256']
incus_golden = load(HERE / 't2-golden-incus.json')
assert incus_golden['goldenSha256'] == manifest['t2Evidence']['incusGolden']['goldenSha256']
assert incus_golden['baseGoldenSha256'] == golden['goldenSha256']
k3s_golden = load(HERE / 't2-golden-k3s.json')
assert k3s_golden['goldenSha256'] == manifest['t2Evidence']['k3sGolden']['goldenSha256']
assert k3s_golden['incusGoldenSha256'] == incus_golden['goldenSha256']
oci_golden = load(HERE / 't2-golden-oci.json')
assert oci_golden['goldenSha256'] == manifest['t2Evidence']['ociGolden']['goldenSha256']
assert oci_golden['k3sGoldenSha256'] == k3s_golden['goldenSha256']
assert oci_golden['facts']['debSha256'] == manifest['t2Evidence']['ociGolden']['debSha256']

# Go oracle in the sandbox: the live Incus tests, per binary, in twins.
oracle_t2 = load(HERE / 'oracles-t2.json')
assert exits['oracles-t2'] == '0'
assert oracle_t2['goldens'] == {'base': golden['goldenSha256'], 'incus': incus_golden['goldenSha256']}
oracle_rows = {r['binary']: {'status': r['status'], 'passed': r['passed'], 'failed': r['failed'],
                             'skipped': r['skipped']} for r in oracle_t2['results']}
assert oracle_rows['go']['status'] == oracle_rows['rust']['status'] == 'PASS'
assert not oracle_rows['negative-control']['passed']

caught = lambda name: len(re.findall(r'\(ok\)$', log(name), re.M))
contracts = {}
for suite in ('oauth-issuance', 'standalone-read-only-gate', 'reject-without-audit-writes',
              'refuse-before-operation-record', 'evidence-redaction'):
    line = log(f'contract-{suite}').strip().splitlines()[-1]
    passed, total = map(int, re.match(r'(\d+)/(\d+) contract scenarios pass', line).groups())
    contracts[suite] = {'passed': passed, 'total': total}

m5 = {name: {k: v for k, v in load(HERE / 'm5' / name / 'summary.json').items() if not isinstance(v, (dict, list))}
      for name in ('crash', 'cross-read', 'migration', 'secrets', 'unknown-projection', 'shape')}
assert m5['secrets']['fieldCount'] == 16
# The earlier domains' write-only fields stay among the sweep's, in both modes.
import gzip  # noqa: E402
swept = json.loads(gzip.decompress((HERE / 'm5/secrets/outcomes.json.gz').read_bytes()))['cases']
for mode in ('standalone', 'platform'):
    fields = {(c['tool'], tuple(c['path'])) for c in swept[mode]}
    assert {('put_k8s_secret', ('data',)), ('install_cluster_agent', ('bridgeToken',))} <= fields, mode


sdk = load(HERE / 'oauth-sdk-client/receipt.json')
assert sdk['pass'] and sdk['rustBinarySha256'] == sha(rust)

seam = load(ROOT / 'evidence/seam/m8d/report.json')
canary = load(ROOT / 'evidence/seam/m8d/canary/result.json')
assert canary['red'] and exits['seam'] == '0'

# Gate reports: rerun each and require the committed report byte for byte.
gates = {}
with tempfile.TemporaryDirectory() as tmp:
    for gate in ('m0', 'm1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm8a', 'm8b', 'm8c', 'm8d', 'cutover'):
        out = Path(tmp) / f'{gate}.json'
        code = subprocess.run([sys.executable, '-m', 'parity', 'verify', '--gate', gate, '--report', str(out)],
                              cwd=ROOT / 'tools/parity', capture_output=True).returncode
        assert out.read_bytes() == (HERE / f'verify-{gate}.json').read_bytes(), gate
        report = load(out)
        gates[gate] = {'pass': code == 0, 'failures': report['gate'].get('failures', []),
                       'problems': len(report['problems'])}

gaps = load(ROOT / 'baseline/inventory/gaps.json')
assert gaps['unownedCount'] == 0
owned_gaps = {owner: sorted(f"{g['file']}:{g['item']}" for g in gaps['items'] if g.get('owner') == owner)
              for owner in ('host', 'incus', 'kubernetes', 'cluster', 'oci')}
assert not [g for g in gaps['items'] if g.get('milestone') in ('M8b', 'M8c', 'M8d')], 'M8b-M8d gaps remain'
CLUSTER_ENV = ('OPUTE_K3S_VM', 'OPUTE_K3S_CONTAINER', 'OPUTE_DOGFOOD_GATEWAY_VM', 'OPUTE_PLATFORM_URL',
               'OPUTE_PLATFORM_PUBLIC_URL', 'OPUTE_PLATFORM_GUEST_HOST')
env_status = {i['name']: i['status'] for i in load(ROOT / 'baseline/inventory/env.json')['items']}
assert all(env_status[k] == 'covered' for k in CLUSTER_ENV), {k: env_status[k] for k in CLUSTER_ENV}
t2_inventory = {i['tool']: i['status'] for i in load(ROOT / 'baseline/inventory/t2.json')['items']
                if i['owner'] == 'oci'}
effects = {i['command']: i['tier'] for i in load(ROOT / 'baseline/inventory/effects.json')['items']}
# The M8d gaps: the image tools run for real in the OCI twins.
assert effects['podman'] == effects['buildah'] == 'T2-sandbox', (effects['podman'], effects['buildah'])
assert set(t2_inventory.values()) == {'covered'}, t2_inventory

receipt = {
    'schemaVersion': 'opute-native-validation.v1',
    'milestone': 'M8d',
    'head': head,
    'binaries': {'go': sha(go), 'rust': sha(rust)},
    'workspaceTests': {'passed': sum(p for p, _ in tests), 'failed': 0},
    'goOwnerPasses': go_owner_passes,
    'spec': {'passed': int(spec.group(1)), 'failed': 0},
    'goVsGo': differential('go-vs-go', 20, go),
    'goVsRust': differential('go-vs-rust', 5, rust),
    't2': {'golden': golden['goldenSha256'], 'incusGolden': incus_golden['goldenSha256'],
           'k3sGolden': k3s_golden['goldenSha256'], 'ociGolden': oci_golden['goldenSha256'],
           'ociPackages': oci_golden['facts']['packages'], 'suites': t2_suites},
    'clusterEnv': {k: env_status[k] for k in CLUSTER_ENV},
    't2Inventory': dict(sorted(t2_inventory.items())),
    'goOracleT2': oracle_rows,
    'canaries': {'go': caught('canaries'), 'rust': caught('rust-canaries')},
    'contracts': contracts,
    'm5': m5,
    'seam': {'gaps': seam.get('gaps'), 'canaryRed': canary['red']},
    'gates': gates,
    'inventory': {'gaps': gaps['count'], 'byMilestone': gaps['byMilestone'], 'owned': owned_gaps},
    'effectTiers': dict(sorted(effects.items())),
}
(HERE / 'validation.json').write_text(json.dumps(receipt, indent=2, sort_keys=True) + '\n')
print(json.dumps({k: receipt[k] for k in ('goVsGo', 'goVsRust')}), {g: v['pass'] for g, v in gates.items()})
