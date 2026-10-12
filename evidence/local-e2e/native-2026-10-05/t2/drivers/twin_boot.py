import fcntl, hashlib, json, pathlib, socket, subprocess, sys, time, os
root=pathlib.Path(__file__).resolve().parent
name, port_text=sys.argv[1:]; assert name in ['go','rust'];port=int(port_text)
assert port in [32022,32023]
lease=(root/(name+'.lease')).open('a+')
fcntl.flock(lease,fcntl.LOCK_EX|fcntl.LOCK_NB)
golden=root/'golden.qcow2'
record=json.loads((root/'runtime.json').read_text())
assert record.get('exitCode')==0,'golden VM must have shut down cleanly'
disk=root/(name+'.qcow2');assert not disk.exists(),'never silently reset an existing runtime disk'
subprocess.run(['qemu-img','create','-f','qcow2','-F','qcow2','-b',str(golden),str(disk)],check=True)
with socket.socket() as probe: probe.bind(('127.0.0.1',port))
argv=['qemu-system-x86_64','-enable-kvm','-cpu','host','-smp','4','-m','6144','-drive',f'file={disk},if=virtio,format=qcow2','-netdev',f'user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:22','-device','virtio-net-pci,netdev=net0','-display','none','-serial',f'file:{root}/{name}-serial.log','-monitor','none']
p=subprocess.Popen(argv,stdout=(root/(name+'-qemu.log')).open('w'),stderr=subprocess.STDOUT)
r={'name':name,'qemuPid':p.pid,'supervisorPid':os.getpid(),'exclusiveLease':str(root/(name+'.lease')),'sshPort':port,'argv':argv,'goldenSha256':hashlib.file_digest(golden.open('rb'),'sha256').hexdigest(),'imageSha256':record['imageSha256'],'createdUnix':time.time(),'scope':'independent disposable QEMU clone, no shared runtime effects'}
(root/(name+'-runtime.json')).write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r),flush=True)
try: code=p.wait()
finally:
 if p.poll() is None: p.terminate();p.wait(timeout=30)
 r.update(stoppedUnix=time.time(),exitCode=p.returncode);(root/(name+'-runtime.json')).write_text(json.dumps(r,indent=2)+'\n')
raise SystemExit(code)
