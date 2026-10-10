#!/usr/bin/env python3
"""Build a bridge overlay calling pinned unexported catalog projection functions."""
import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
ROOT = Path(__file__).resolve().parents[2]
lock = json.loads((ROOT/'baseline/source-lock.json').read_text())
module = ROOT/'tools/parity/provider-catalog-oracle'
go = str(ROOT/'.parity/setup/go/bin/go')
go_env = {**os.environ,'GOTOOLCHAIN':lock['goToolchain']}
verification = subprocess.check_output([go,'mod','verify'],cwd=module,env=go_env,text=True).strip()
assert verification == 'all modules verified'
source = ROOT/'.parity/go-src'
assert subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip() == lock['sourceCommit']
assert subprocess.check_output(['git','-C',str(source),'status','--porcelain'],text=True).strip() == ''
virtual = source/'internal/hostmcp/parity_catalog_bridge.go'
assert not virtual.exists()
bridge = ROOT/'tools/parity/provider-catalog-oracle/bridge.go.txt'
overlay = ROOT/'.parity/m7/provider-catalog-overlay.json'
overlay.parent.mkdir(parents=True,exist_ok=True)
schema_virtual = source/'internal/catalog/parity_schema_bridge.go'
assert not schema_virtual.exists()
schema_bridge = ROOT/'tools/parity/provider-catalog-oracle/schema-bridge.go.txt'
library = Path('/home/houman/go/pkg/mod/github.com/google/jsonschema-go@v0.4.3')
def inventory(folder):
    return {str(p.relative_to(folder)):hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(folder.rglob('*')) if p.is_file()}
library_inventory = inventory(library)
library_digest = hashlib.sha256(json.dumps(library_inventory,sort_keys=True).encode()).hexdigest()
# Go forbids overlays inside GOMODCACHE. Use a byte-identical private module
# snapshot and a build-only alternate module file; never modify the cache or
# the pinned checkout. The virtual bridge is the only extra source file.
library_snapshot = ROOT/'.parity/m7'/('schema-reference-'+library_digest)
if not library_snapshot.exists(): shutil.copytree(library,library_snapshot)
assert inventory(library_snapshot) == library_inventory
resolver_virtual = library_snapshot/'jsonschema/parity_resolver_bridge.go'
assert not resolver_virtual.exists()
resolver_bridge = ROOT/'tools/parity/provider-catalog-oracle/resolver-bridge.go.txt'
mounts_virtual = source/'internal/hostmcp/parity_mounts_bridge.go'
assert not mounts_virtual.exists()
mounts_bridge = module/'mounts-bridge.go.txt'
overlay.write_text(json.dumps({'Replace':{str(virtual):str(bridge),str(schema_virtual):str(schema_bridge),str(resolver_virtual):str(resolver_bridge),str(mounts_virtual):str(mounts_bridge)}},indent=2)+'\n')
alternate = ROOT/'.parity/m7/provider-catalog-oracle.mod'
alternate.write_text((module/'go.mod').read_text().replace('../../../.parity/go-src',str(source))+
                     '\nreplace github.com/google/jsonschema-go => '+str(library_snapshot)+'\n')
alternate.with_suffix('.sum').write_bytes((module/'go.sum').read_bytes())
subprocess.run([go,'build','-mod=readonly','-modfile='+str(alternate),'-overlay='+str(overlay),'-o',str(ROOT/'.parity/bin/provider-catalog-oracle'),'.'],cwd=module,env=go_env,check=True)
assert not virtual.exists()
assert subprocess.check_output(['git','-C',str(source),'status','--porcelain'],text=True).strip() == ''

assert not schema_virtual.exists()
assert not mounts_virtual.exists()
assert not resolver_virtual.exists()
assert inventory(library_snapshot) == library_inventory == inventory(library)
(ROOT/'.parity/m7/schema-reference-source.json').write_text(json.dumps({
    'moduleVersion':'v0.4.3','snapshot':str(library_snapshot),'source':str(library),
    'files':library_inventory,'inventoryDigest':library_digest,
    'moduleCacheVerification':verification},indent=2)+'\n')
