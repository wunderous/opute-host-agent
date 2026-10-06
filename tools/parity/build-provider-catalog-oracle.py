#!/usr/bin/env python3
"""Build a bridge overlay calling pinned unexported catalog projection functions."""
import json
import os
from pathlib import Path
import subprocess
ROOT = Path(__file__).resolve().parents[2]
lock = json.loads((ROOT/'baseline/source-lock.json').read_text())
source = ROOT/'.parity/go-src'
assert subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip() == lock['sourceCommit']
assert subprocess.check_output(['git','-C',str(source),'status','--porcelain'],text=True).strip() == ''
virtual = source/'internal/hostmcp/parity_catalog_bridge.go'
assert not virtual.exists()
bridge = ROOT/'tools/parity/provider-catalog-oracle/bridge.go.txt'
overlay = ROOT/'.parity/m7/provider-catalog-overlay.json'
overlay.parent.mkdir(parents=True,exist_ok=True)
overlay.write_text(json.dumps({'Replace':{str(virtual):str(bridge)}},indent=2)+'\n')
subprocess.run([str(ROOT/'.parity/setup/go/bin/go'),'build','-mod=readonly','-overlay='+str(overlay),'-o',str(ROOT/'.parity/bin/provider-catalog-oracle'),'.'],cwd=ROOT/'tools/parity/provider-catalog-oracle',env={**os.environ,'GOTOOLCHAIN':lock['goToolchain']},check=True)
assert not virtual.exists()
assert subprocess.check_output(['git','-C',str(source),'status','--porcelain'],text=True).strip() == ''
