#!/usr/bin/env python3
"""Reproduce pinned Go regexp Unicode admission-name facts in native Rust."""
import json
import os
from pathlib import Path
import subprocess
ROOT=Path(__file__).resolve().parents[2]


def canonical(name):
    result='';first=True
    for c in name:
        if c in '_ -':continue
        c=c.upper() if first and 'a'<=c<='z' else c.lower() if not first and 'A'<=c<='Z' else c
        first=False;result+=c
    return result


def source(facts):
    assert facts['goVersion']=='go1.25.4' and facts['unicodeVersion']=='15.0.0'
    assert all(name.isascii() for name in facts['names'])
    lines=['// Copyright 2009 The Go Authors. All rights reserved.',
        '// SPDX-License-Identifier: BSD-3-Clause; see licenses/Go-BSD.txt.',
        '// Generated Go 1.25.4 Unicode 15.0.0 accepted category/script/alias names.',
        '// tools/parity/generate_regexp_names.py; facts filtered by actual Go regexp.Compile.',
        '#[rustfmt::skip]','pub(super) const NAMES: &[&str] = &[']
    lines+=['    '+json.dumps(name)+',' for name in sorted({canonical(n) for n in facts['names']})]
    return '\n'.join(lines+['];'])+'\n'


def data_source(facts):
    classes={canonical(k):v for k,v in facts['classes'].items()}
    data={'classes':classes,'posix':facts['posix'],'folds':facts['folds']}
    return json.dumps(data,sort_keys=True,separators=(',',':'))+'\n'


if __name__=='__main__':
    facts=json.loads(subprocess.check_output([str(ROOT/'.parity/setup/go/bin/go'),'run',
        str(ROOT/'tools/parity/go-regexp-data/main.go')],text=True,
        env={**os.environ,'GOTOOLCHAIN':'go1.25.4'},timeout=60))
    (ROOT/'crates/host-agent/src/catalog/schema/regexp_names.rs').write_text(source(facts))
    (ROOT/'crates/host-agent/src/catalog/schema/regexp_data.json').write_text(data_source(facts))
