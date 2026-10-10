#!/usr/bin/env python3
"""Build an isolated overlay against the clean pinned Go activation owner."""
import hashlib
import json
import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
module = ROOT / 'tools/parity/provider-activation-oracle'
source = ROOT / '.parity/go-src'
lock = json.loads((ROOT / 'baseline/source-lock.json').read_text())
git = lambda *args: subprocess.check_output(['git', '-C', str(source), *args], text=True).strip()
assert git('rev-parse', 'HEAD') == lock['sourceCommit']
assert git('rev-parse', 'HEAD^{tree}') == lock['sourceTree']
assert not git('status', '--porcelain')
virtual = source / 'internal/hostmcp/parity_activation_bridge.go'
assert not virtual.exists()
overlay = ROOT / '.parity/m7/provider-activation-overlay.json'
overlay.parent.mkdir(parents=True, exist_ok=True)
overlay.write_text(json.dumps({'Replace': {str(virtual): str(module / 'bridge.go.txt')}}) + '\n')
go = str(ROOT / '.parity/setup/go/bin/go')
env = {**os.environ, 'GOTOOLCHAIN': lock['goToolchain']}
assert subprocess.check_output([go, 'mod', 'verify'], cwd=module, env=env, text=True).strip() == 'all modules verified'
binary = ROOT / '.parity/bin/provider-activation-oracle'
subprocess.run([go, 'build', '-mod=readonly', '-overlay=' + str(overlay), '-o', str(binary), '.'], cwd=module, env=env, check=True)
assert not virtual.exists() and not git('status', '--porcelain')
print('Pinned activation oracle:', hashlib.sha256(binary.read_bytes()).hexdigest())
