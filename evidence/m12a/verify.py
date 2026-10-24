"""Recompute the M12a (M12 without Platform) validation receipt from the raw captures.

  python3 evidence/m12a/verify.py

Reproduces evidence/m12a/validation.json: the M10 receipt for this round, and
the cutover rehearsals re-derived from their receipts (rehearsal_verify:
forward, restart and rollback on state copies, the Go control against Rust,
and the stop drills). It covers the committed head and a clean tree
before and after the round, the binaries and release sets the manifest
records (the Rust candidate is the x64 release artifact) and both release
sets' binaries rebuilt byte for byte, the logs of the standard checks and the
owning Go tests, every T1 summary's provenance, both T2 suites re-derived
from their receipts (t2_verify), the Go live-test oracle run in the twins,
the secret sweep, the distribution round re-derived from its receipts
(dist_verify: artifacts, log lines, Ubuntu and Debian clean machines, Go ->
Rust -> Go, the npm launcher and the site), the inventory gap ownership, the
seam stage, and every milestone gate report rerun and compared byte for byte.
Failures are recorded as failures; nothing is relabelled.
"""
import hashlib, json, re, subprocess, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import dist_verify, rehearsal_verify, t2, t2_verify  # noqa: E402

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
log = lambda name: (HERE / 'checks' / f'{name}.log').read_text()
lock = load(ROOT / 'baseline/source-lock.json')
manifest = load(ROOT / 'parity-manifest.json')
go, rust = ROOT / '.parity/bin/go-reference', ROOT / 'dist/host-agent-linux-x64'
go_release = ROOT / '.parity/releases/go'
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

# Release sets: as pinned before and after the round, and rebuilt.
releases = manifest['distEvidence']['releases']
for name in ('releases', 'releases.after'):
    pinned = dict(reversed(line.split()) for line in (HERE / f'checks/{name}.sha256').read_text().splitlines())
    assert pinned['dist/SHA256SUMS'] == sha(ROOT / 'dist/SHA256SUMS') == releases['rust']['sha256sums'], name
    assert pinned[f'{go_release}/SHA256SUMS'] == sha(go_release / 'SHA256SUMS') == releases['go']['sha256sums'], name
for name in ('rust-release', 'go-release', 'rust-rebuild', 'go-rebuild'):
    assert exits[name] == '0' and 'FAILED' not in log(name), name
rebuilt = {name: sorted(re.findall(r'^(\S+): OK$', log(name), re.M)) for name in ('rust-rebuild', 'go-rebuild')}
REPRODUCED = sorted(['host-agent-linux-x64', 'opute-provider-k3s-linux-x64', 'opute-provider-cloudflare-linux-x64',
                     'opute-provider-tailscale-linux-x64'])
assert rebuilt == {'rust-rebuild': REPRODUCED, 'go-rebuild': REPRODUCED}, rebuilt

# Standard checks.
assert exits['fmt'] == '0' and not log('fmt').strip()
assert exits['clippy'] == '0' and 'warning' not in log('clippy')
tests = [tuple(map(int, m)) for m in re.findall(r'test result: ok\. (\d+) passed; (\d+) failed', log('workspace'))]
assert exits['workspace'] == '0' and tests and 'FAILED' not in log('workspace')
assert exits['harness'] == '0' and log('harness').rstrip().endswith('OK')
assert exits['spec'] == '0'
spec = re.search(r'Totals: (\d+) passed, (\d+) failed', log('spec'))
assert spec and spec.group(2) == '0'
assert exits['catalog-source'] == '0' and exits['go-reference'] == '0'
assert exits['go-owner-core'] == '0' and 'FAIL' not in log('go-owner-core')
go_owner_packages = len(re.findall(r'^ok\s', log('go-owner-core'), re.M))


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
postgres_golden = load(HERE / 't2-golden-postgres.json')
pinned = manifest['t2Evidence']['postgresGolden']
assert postgres_golden['goldenSha256'] == pinned['goldenSha256']
assert postgres_golden['k3sGoldenSha256'] == k3s_golden['goldenSha256'] == pinned['k3sGoldenSha256']
assert postgres_golden['chart'] == pinned['chart'] and postgres_golden['images'] == pinned['images']
assert all(entry['exit'] == 0 for entry in postgres_golden['recipeLog'])

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
assert m5['secrets']['fieldCount'] == 16 and m5['secrets']['d24FieldCount'] == 8
# The earlier domains' write-only fields stay among the sweep's, in both
# modes, and D24 adds the relay token, top-level and nested, on Rust.
import gzip  # noqa: E402
swept = json.loads(gzip.decompress((HERE / 'm5/secrets/outcomes.json.gz').read_bytes()))
D24 = {('release_postgresql_service_relay', ('relayToken',))} | {
    (tool, ('localRelay', 'relayToken')) for tool in
    ('get_postgresql_service_status', 'reconcile_postgresql_service', 'remove_postgresql_service')}
for mode in ('standalone', 'platform'):
    fields = {(c['tool'], tuple(c['path'])) for c in swept['cases'][mode]}
    assert {('put_k8s_secret', ('data',)), ('install_cluster_agent', ('bridgeToken',))} <= fields, mode
    assert {(c['tool'], tuple(c['path'])) for c in swept['d24Cases'][mode]} == D24, mode


# M12a: the rehearsals and stop drills, re-derived from their receipts.
assert exits['rehearsal'] == '0'
problems = []
rehearsal_status = rehearsal_verify.check(ROOT, manifest, lock, problems.append)
assert not problems, problems
rehearsal_results = load(HERE / 'rehearsal/results.json')
rehearsal_summary = load(HERE / 'rehearsal/summary.json')
assert rehearsal_summary['pass'] is True and rehearsal_summary['provenance']['repeat'] == 3
rehearsals = {
    'status': rehearsal_status,
    'repeat': rehearsal_summary['provenance']['repeat'],
    'noIncusOnPath': rehearsal_summary['provenance']['noIncusOnPath'],
    'declared': [d['drill'] + ':' + d['decision'] for d in manifest['rehearsalEvidence']['declared']],
}

# M10: the distribution round, re-derived from its receipts.
assert exits['dist'] == '0'
problems = []
dist_status = dist_verify.check(ROOT, manifest, lock, problems.append)
assert not problems, problems
dist_results = load(HERE / 'dist/results.json')
dist_summary = load(HERE / 'dist/summary.json')
debian_golden = load(HERE / 't2-golden-debian.json')
assert debian_golden['goldenSha256'] == manifest['distEvidence']['goldens']['debian'] == dist_summary['goldens']['debian']
assert dist_summary['goldens']['ubuntu'] == golden['goldenSha256']
distribution = {
    'status': dist_status,
    'artifacts': dist_results['artifacts']['checks'],
    'logCases': len(dist_results['logs']['cases']),
    'machines': {d: {'comparisons': {k: len(v) for k, v in dist_results[f'machines-{d}']['comparisons'].items()},
                     'identity': dist_results[f'machines-{d}']['identity'],
                     'masksApplied': dist_results[f'machines-{d}']['masksApplied'],
                     'expectations': dist_results[f'machines-{d}']['expectations']}
                 for d in manifest['distEvidence']['distros']},
    'npm': {'checks': len(dist_results['npm']['checks']),
            'failed': sorted(k for k, v in dist_results['npm']['checks'].items() if not v),
            'diffs': {k: len(v) for k, v in dist_results['npm']['diffs'].items()}},
    'site': dist_results['site']['checks'],
    'launcherTarballSha256': dist_summary['launcherTarballSha256'],
}

sdk = load(HERE / 'oauth-sdk-client/receipt.json')
assert sdk['pass'] and sdk['rustBinarySha256'] == sha(rust)

seam = load(ROOT / 'evidence/seam/m12a/report.json')
canary = load(ROOT / 'evidence/seam/m12a/canary/result.json')
assert canary['red'] and exits['seam'] == '0'
assert seam['gaps'] == [] and seam['problems'] == [], (seam['gaps'], seam['problems'])

# Gate reports: rerun each and require the committed report byte for byte.
gates = {}
with tempfile.TemporaryDirectory() as tmp:
    for gate in ('m0', 'm1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm8a', 'm8b', 'm8c', 'm8d', 'm8e', 'm8f', 'm10', 'm12a', 'cutover'):
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
              for owner in sorted({g.get('owner') for g in gaps['items']})}
# What remains after M12a: the M10 gaps (Windows artifacts, arm64 and WSL2
# installs, the three WSL tools' T2 effects), the M11 dependency (platform),
# the writer lock Go does not have (core), the soak, the M13 evidence and the
# 0.2.3 publish decision (release).
assert owned_gaps == {
    'core': ['cutover.json:state-directory writer lock'],
    'host': ['t2.json:compact_wsl_disk', 't2.json:shutdown_wsl', 't2.json:terminate_wsl_distribution'],
    'platform': ['cutover.json:M11 Platform-driven journey', 'cutover.json:M11 client first-success',
                 'cutover.json:M11 staging validation identity'],
    'release': ['cutover.json:72-hour soak', 'cutover.json:M13 cutover evidence',
                'cutover.json:stable 0.2.3 publish decision',
                'distribution.json:WSL2 install', 'distribution.json:host-agent-windows-x64.exe',
                'distribution.json:host-agent-windows-x64.exe.gz', 'distribution.json:host-agent-windows-x64.gz',
                'distribution.json:linux-arm64 clean-machine install'],
}, owned_gaps
cutover_items = {i['name']: i['status'] for i in load(ROOT / 'baseline/inventory/cutover.json')['items']}
dist_items = {i['name']: i['status'] for i in load(ROOT / 'baseline/inventory/distribution.json')['items']}
assert [n for n, s in dist_items.items() if s == 'covered'] and \
    all(s == 'covered' for n, s in dist_items.items() if 'windows' not in n and 'install' not in n), dist_items

receipt = {
    'schemaVersion': 'opute-native-validation.v1',
    'milestone': 'M12a',
    'head': head,
    'binaries': {'go': sha(go), 'rust': sha(rust)},
    'releases': {'go': releases['go']['sha256sums'], 'rust': releases['rust']['sha256sums'], 'rebuilt': rebuilt},
    'workspaceTests': {'passed': sum(p for p, _ in tests), 'failed': 0},
    'goOwnerPackages': go_owner_packages,
    'spec': {'passed': int(spec.group(1)), 'failed': 0},
    'goVsGo': differential('go-vs-go', 3, go),
    'goVsRust': differential('go-vs-rust', 3, rust),
    't2': {'golden': golden['goldenSha256'], 'incusGolden': incus_golden['goldenSha256'],
           'k3sGolden': k3s_golden['goldenSha256'], 'ociGolden': oci_golden['goldenSha256'],
           'ociPackages': oci_golden['facts']['packages'], 'postgresGolden': postgres_golden['goldenSha256'],
           'postgresChart': postgres_golden['chart'], 'postgresImages': postgres_golden['images'],
           'debianGolden': debian_golden['goldenSha256'], 'suites': t2_suites},
    'distribution': distribution,
    'distributionInventory': dist_items,
    'rehearsals': rehearsals,
    'cutoverInventory': cutover_items,
    'goOracleT2': oracle_rows,
    'canaries': {'go': caught('canaries'), 'rust': caught('rust-canaries')},
    'contracts': contracts,
    'm5': m5,
    'seam': {'gaps': seam.get('gaps'), 'canaryRed': canary['red']},
    'gates': gates,
    'inventory': {'gaps': gaps['count'], 'byMilestone': gaps['byMilestone'], 'owned': owned_gaps},
}
(HERE / 'validation.json').write_text(json.dumps(receipt, indent=2, sort_keys=True) + '\n')
print(json.dumps({k: receipt[k] for k in ('goVsGo', 'goVsRust')}), {g: v['pass'] for g, v in gates.items()})
