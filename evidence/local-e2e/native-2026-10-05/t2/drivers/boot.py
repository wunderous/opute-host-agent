import fcntl, hashlib, json, os, pathlib, socket, subprocess, time
root = pathlib.Path(__file__).resolve().parent
lease = (root / 'runtime.lease').open('a+')
fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
image = root / 'noble-server-cloudimg-amd64.img'
subprocess.run(['gpgv', '--keyring', '/usr/share/keyrings/ubuntu-cloudimage-keyring.gpg', str(root/'SHA256SUMS.gpg'), str(root/'SHA256SUMS')], check=True)
expected = next(line.split()[0] for line in (root/'SHA256SUMS').read_text().splitlines() if line.split()[-1].lstrip('*') == image.name)
actual = hashlib.file_digest(image.open('rb'), 'sha256').hexdigest()
assert actual == expected, (actual, expected)
key = root / 'id_ed25519'
if not key.exists():
    subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(key)], check=True)
user = '#cloud-config\nhostname: opute-parity-t2\nssh_pwauth: false\nusers:\n  - name: ubuntu\n    groups: [sudo]\n    sudo: ALL=(ALL) NOPASSWD:ALL\n    shell: /bin/bash\n    ssh_authorized_keys:\n      - ' + key.with_suffix('.pub').read_text().strip() + '\n'
(root/'user-data').write_text(user)
(root/'meta-data').write_text('instance-id: opute-parity-native-t2-20261005\nlocal-hostname: opute-parity-t2\n')
subprocess.run(['cloud-localds', str(root/'seed.img'), str(root/'user-data'), str(root/'meta-data')], check=True)
disk = root/'golden.qcow2'
assert not disk.exists(), 'existing VM disk requires explicit reuse, never reset'
subprocess.run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', str(image), str(disk), '32G'], check=True)
port = 32022
with socket.socket() as probe:
    probe.bind(('127.0.0.1', port))
argv = ['qemu-system-x86_64', '-enable-kvm', '-cpu', 'host', '-smp', '4', '-m', '6144', '-drive', f'file={disk},if=virtio,format=qcow2', '-drive', f'file={root}/seed.img,if=virtio,format=raw', '-netdev', f'user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:22', '-device', 'virtio-net-pci,netdev=net0', '-display', 'none', '-serial', f'file:{root}/serial.log', '-monitor', 'none']
process = subprocess.Popen(argv, stdout=(root/'qemu.log').open('w'), stderr=subprocess.STDOUT)
record = {'scope': 'dedicated disposable QEMU VM, no shared Incus/Kubernetes/tunnel state', 'owner': 'local M1-M6 validation goal', 'supervisorPid': os.getpid(), 'qemuPid': process.pid, 'exclusiveLease': str(root/'runtime.lease'), 'sshPort': port, 'imageSha256': actual, 'imageSource': 'https://cloud-images.ubuntu.com/noble/20260926/noble-server-cloudimg-amd64.img', 'createdUnix': time.time(), 'argv': argv}
(root/'runtime.json').write_text(json.dumps(record, indent=2)+'\n')
print(json.dumps(record), flush=True)
try:
    code = process.wait()
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=30)
    record['stoppedUnix'] = time.time()
    record['exitCode'] = process.returncode
    (root/'runtime.json').write_text(json.dumps(record, indent=2)+'\n')
raise SystemExit(code)
