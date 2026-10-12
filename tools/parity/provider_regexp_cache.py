#!/usr/bin/env python3
"""Owning Go lazy regexp size/height caches and allocation-reuse boundaries."""
import argparse
from pathlib import Path
from provider_schema_regexp import run


def cases(_facts):
    rows=[]
    def add(name,pattern):rows.append({'case':name,'regexp':pattern})
    for length in [1,2,3,5,8]:
        prefix='abcdefgh'[:length]
        bodies=[f'(?:{prefix}d|{prefix}e|{prefix}f)',
                f'({prefix}d|{prefix}e|{prefix}f)',
                f'(?i:{prefix}d|{prefix}e|{prefix}f)',
                f'(?:{prefix}[d]|{prefix}[e]|{prefix}[f])',
                f'(?:{prefix}d|{prefix}d|{prefix}d)',
                f'(?:{prefix}d|{prefix}e|{prefix}f|{prefix}g)',
                f'(?:{prefix}d|{prefix}e|{prefix}f|z)',
                f'(?:{prefix}d|{prefix}e|{prefix}f|)',
                f'(?:{prefix}d|{prefix}e|{prefix}f)z',
                f'z(?:{prefix}d|{prefix}e|{prefix}f)']
        count=3355443//(1000*(length+1))
        for i,body in enumerate(bodies):
            for delta in [-1,0,1]:
                n=count+delta
                add(f'prefix-{length}-body-{i}-count-{n}',('(?:'+body+'){1000}')*n)
    # Actual Go can leave the size cache inactive for very long strings when
    # the allocator count times the global repeat product stays small.
    for n in [3355442,3355443,3355444]:add('uncached-literal-'+str(n),'a'*n)
    for n in [998,999,1000,1001]:
        for body in ['abc|abd|abe','[a]b[c]','ab|ab|ab','a{2}b|a{2}c','a||',
                     '(?i:a)|A','(?:a|b)(?:c|d)','(?:abcd|abce|abcf){1000}']:
            add('cached-depth-'+str(n)+'-'+body,'('*n+body+')'*n)
    assert len({r['case'] for r in rows})==len(rows)
    return rows


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--go',type=Path,required=True)
    p.add_argument('--rust-tests',type=Path,required=True)
    p.add_argument('--out',type=Path,required=True)
    a=p.parse_args()
    raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve(),
        'cache',scenario_factory=cases,extra_sources=[Path(__file__)]) else 1)
