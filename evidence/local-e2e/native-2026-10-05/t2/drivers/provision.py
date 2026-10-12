import json, pathlib, secrets, subprocess
root=pathlib.Path(__file__).resolve().parent
ssh=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10','-o','StrictHostKeyChecking=yes','-o',f'UserKnownHostsFile={root}/known_hosts','-i',str(root/'id_ed25519'),'-p','32022','ubuntu@127.0.0.1']
configs={}
for name, binary, port in [('goref','go-reference',3014),('rustref','opute-host-agent',3015)]:
    env={'OPUTE_AGENT_MODE':'standalone','OPUTE_REMOTE_AGENT_ID':f'parity-native-t2-{name}-opaque','OPUTE_HOST_AGENT_INSTANCE':name,'OPUTE_SHARED_HOST_OWNER_INSTANCE':'goref','OPUTE_LOCAL_HOST_AGENT_INSTANCE_ID':name,'OPUTE_INCUS_OWNERSHIP_MODE':'enforce','OPUTE_STANDALONE_ALLOW_MUTATIONS':'true','OPUTE_STANDALONE_STATE_DIR':f'/opt/parity/state-{name}','OPUTE_HOST_AGENT_INSTANCE_ROOT':f'/opt/parity/instance-{name}','OPUTE_HOST_RESOURCE_LOCK_DIR':f'/opt/parity/locks-{name}','HOST_MCP_PORT':str(port),'HOST_MCP_BIND_HOST':'127.0.0.1','MCP_AUTH_TOKEN':secrets.token_urlsafe(40),'OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES':str(6<<30),'OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES':'4'}
    unit=f'[Unit]\nDescription=Disposable parity {name} Host Agent\nAfter=network-online.target\n\n[Service]\nType=simple\nEnvironmentFile=/opt/parity/{name}.env\nExecStart=/home/ubuntu/{binary} serve --env-file /opt/parity/{name}.env\nSlice=opute-host-agent-protected.slice\nKillMode=control-group\nRestart=no\n'
    configs[name]={'env':env,'unit':unit}
code='''import json,pathlib,subprocess,sys\nc=json.load(sys.stdin)\np=pathlib.Path('/opt/parity');p.mkdir(exist_ok=True)\nfor name,v in c.items():\n e=p/(name+'.env');e.write_text(''.join(k+'='+x+'\\n' for k,x in v['env'].items()));e.chmod(0o600)\n pathlib.Path('/etc/systemd/system/opute-host-agent@'+name+'.service').write_text(v['unit'])\nsubprocess.run(['systemctl','daemon-reload'],check=True)\nsubprocess.run(['systemctl','start','opute-host-agent@goref.service','opute-host-agent@rustref.service'],check=True)\n'''
subprocess.run(ssh+['sudo -n python3 -c '+__import__('shlex').quote(code)],input=json.dumps(configs),text=True,check=True)
print('Go and Rust isolated services provisioned inside dedicated VM')
