#!/usr/bin/env python3
"""Owning schema URI/anchor/reference graph differential evidence."""
import argparse
import itertools
import json
from pathlib import Path
import random
from provider_schema_local_uri import run


def cases():
    rows=[]
    def add(schema,base='',action='graph',variants=False):
        rows.append({'case':action+'-'+str(len(rows)), 'schema':schema, 'baseURI':base,
                     'resolverAction':action, 'variants':variants})
    versions=['','https://json-schema.org/draft/2020-12/schema',
              'http://json-schema.org/draft-07/schema#','https://json-schema.org/draft-07/schema#','unknown']
    bases=['','https://e.example/path/root.json','https://e.example/root?query',
           'relative/root','urn:example:root','https:/root','https://u:p@e.example:0/%2f/root',
           'https://e.example/root#frag','https://e.example/%','https://[::1%25eth0]/root']
    identifiers=['', '#', '#a', '#/properties/a', '#a%20b', 'child.json', './child.json',
                 '../child.json', '/child.json', '//other.example/c', 'https://e.example/root',
                 'urn:child:x','https://e.example/root#anchor', '?q', '?', 'a:b',
                 'https:///root', 'https://[::1]/root', '%', 'https://[xyz]/a', 'a%ff', '雪']
    for version,base,identifier in itertools.product(versions,bases,identifiers):
        schema={'$schema':version,'$id':identifier, 'required':['one','one',None]}
        add(schema,base,action='uris');add(schema,base)
    anchors=['','a','a/b','a~b','a b','雪','a\n"','%ff']
    for version,anchor,dynamic in itertools.product(versions,anchors,[False,True]):
        keyword='$dynamicAnchor' if dynamic else '$anchor'
        for ref in ['#'+anchor,'#'+anchor.replace(' ','%20'),'#missing','', '#%ff']:
            add({'$schema':version,'$defs':{'a':{keyword:anchor}},'$ref':ref})
            add({'$schema':version,'$defs':{'a':{keyword:anchor}},'$dynamicRef':ref})
    for version in versions:
        # Duplicate anchors: first registration wins, even when a later one is dynamic.
        for first,second in itertools.product(['$anchor','$dynamicAnchor'],repeat=2):
            add({'$schema':version,'allOf':[{first:'a'},{second:'a'}],
                 '$ref':'#a','$dynamicRef':'#a'})
        # Two distinct bases scope the same named anchor independently.
        for ref in ['a#a','b#a','#a','https://e.example/a#a','https://e.example/b#a']:
            add({'$schema':version,'$id':'https://e.example/root', 'allOf':[
                {'$id':'a','$anchor':'a','$dynamicAnchor':'d'},
                {'$id':'b','$anchor':'a','$dynamicAnchor':'d'}], '$ref':ref})
        for ref in ['#a','https://e.example/root#a','#/allOf/0','b','a','https://e.example/root']:
            add({'$schema':version,'$id':'https://e.example/root','allOf':[
                {'$id':'#a','title':'first'}, {'$id':'#a','title':'second'},
                {'$id':'b','not':{'$ref':ref}}], '$ref':ref})
        # Canonical URI collision is last-write-wins; original base stays bound.
        for ref in ['', '#', 'https://e.example/root', 'https://e.example/base']:
            add({'$schema':version,'$id':'https://e.example/root','allOf':[
                {'$id':'https://e.example/root'}, {'$id':'https://e.example/root'}],'$ref':ref},'https://e.example/base')
        # Draft-7 ignores malformed IDs only in nonempty $ref objects.
        for identifier in identifiers:
            for ref in ['#','#/not','https://remote.example/x']:
                add({'$schema':version,'$id':identifier,'$ref':ref,'not':{}})
    # Nil-success targets, malformed paths, typed scalar targets, raw bytes,
    # and percent-decoded non-UTF8 fragments at each cursor type.
    rich={'$defs':{'a/b~c':{},'':False,'雪':True}, 'allOf':[{},False,True],
          'type':['object','string'], 'required':['a'], 'dependentRequired':{'':['a']},
          '$vocabulary':None, 'default':{'a':1}, 'enum':[{'a':True}], 'not':{}, 'title':'hi',
          'minimum':1,'deprecated':False}
    pointers=['','/not','/if','/type','/type/0','/type/+0','/type/01','/title', '/title/next',
              '/minimum','/minimum/next','/required/0','/$defs/a~1b~0c','/$defs/', '/$defs/雪',
              '/allOf/0','/allOf/-1','/allOf/9223372036854775808','/default/0','/enum/0/a',
              '/dependentRequired//0','/unknown', '//', '/allOf/00','/allOf/0/if']
    for pointer in pointers:
        for keyword in ['$ref','$dynamicRef']:
            add({**rich,keyword:'#'+pointer})
    for byte in range(256):
        escaped='%'+format(byte,'02x')
        for prefix in ['', '/', '/$defs/', '/allOf/', '/type/', '/required/',
                       '/dependentRequired/', '/default/', '/title/', '/enum/']:
            add({**rich,'$ref':'#'+prefix+escaped})
    for fragment in ['#%C3%A9','#%ff%fe','#/%c0%80','#/%E2%98%83','#%','#/allOf/%ff',
                     '#/%2f%ff','#/dependentRequired/%ff','##a','#?a','#a?b']:
        add({**rich,'$ref':fragment})
    for version in versions:
        for schema in [{'$defs':{},'definitions':{},'$id':'%'},
                       {'$vocabulary':{},'$id':'%'}, {'$id':'%','$ref':'%'},
                       {'allOf':[None], '$id':'%'}, {'$ref':'%', '$dynamicRef':'#missing'},
                       {'$ref':'#missing','$dynamicRef':'%'}, {'title':False}, {'minItems':1.5}]:
            add({'$schema':version,**schema})
    for field in ['$defs','properties','dependentSchemas']:
        add({field:{'a':None,'b':None}},variants=True)
    rng=random.Random(0xACE7013)
    # Coherent trees mix scope changes, duplicate anchors/IDs, references and
    # explicit false schemas. No pattern compilation or loader is stubbed.
    for _ in range(1000):
        root={'$schema':rng.choice(versions)}
        if rng.randrange(2):root['$id']=rng.choice(identifiers)
        children=[]
        for i in range(rng.randrange(1,5)):
            child={'title':str(i)}
            for key,pool in [('$id',identifiers),('$anchor',anchors),('$dynamicAnchor',anchors),
                             ('$ref',['','#','#a','#/not','https://e.example/root']),
                             ('$dynamicRef',['','#','#a','#/not','https://e.example/root'])]:
                if rng.randrange(3)==0:child[key]=rng.choice(pool)
            if rng.randrange(2):child['not']={'$anchor':rng.choice(anchors)}
            children.append(child)
        root['allOf']=children
        if rng.randrange(2):root['$ref']=rng.choice(['#','#a','#/allOf/0','https://e.example/root'])
        add(root,rng.choice(bases[:7]))
    # JSON-representable form of upstream TestResolveURIs plus local ref cycles.
    owner_tree={'$id':'http://b.com','items':{'$id':'/foo.json'},
        'contains':{'$id':'/bar.json','$anchor':'a','$dynamicAnchor':'da',
            'items':{'$anchor':'b','items':{'$id':'?items','$anchor':'c'}}}}
    for base in ['', 'http://a.com']:
        add(owner_tree,base,action='uris');add(owner_tree,base)
        for ref in ['http://b.com/bar.json#a','http://b.com/bar.json#b',
                    'http://b.com/bar.json?items#c','http://b.com/bar.json#da']:
            add({**owner_tree,'$ref':ref,'$dynamicRef':ref},base)
    for version,child_version in itertools.product(versions,repeat=2):
        add({'$schema':version,'$defs':{'a':{'$schema':child_version,'$id':'#a'},
            'b':{'$schema':child_version,'$anchor':'a'}},'$ref':'#a'})
        add({'$schema':version,'$id':'https://e.example/root',
            '$defs':{'a':{'$id':'a','$ref':'b'},'b':{'$id':'b','$ref':'a'}},'$ref':'a'})
    for default in [None,False,42,{'wrong':['type']}]:
        add({'type':'string','default':default,'$ref':'#/not','not':{'$ref':'#'}})
    assert len({r['case'] for r in rows})==len(rows)
    return rows


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True)
    p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    args=p.parse_args()
    scope='T0 actual private Go resolveURIs graph and public Schema.Resolve for regexp-free owning registry defaults (no loader, no ValidateDefaults) vs native URI/anchor/lexical/dynamic-reference graph. Full regexp compilation and registry admission wiring remain open.'
    raise SystemExit(0 if run(args.go.resolve(),args.rust_tests.resolve(),args.out.resolve(),'local',scenario_factory=cases,scope=scope,extra_sources=[Path(__file__)],normalization="Exact complete errors and full actual Go graph projections (draft, URI registrations, typed schema paths, base/URI identity, anchors and dynamic flags, lexical/dynamic targets, and required sets). No URI canonicalization, error rewriting or UTF-8 replacement. Three multi-fault structural-map refusals must equal an entire observed owning Go outcome from 128 reference runs; invented refusal rejected.") else 1)
