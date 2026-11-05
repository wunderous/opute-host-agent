#!/usr/bin/env python3
"""The E2E dev VM: one qemu/KVM VM, run as the user, no root on the host.

  devvm.py lease acquire --holder NAME [--hours H] | lease release --holder NAME | lease show
  devvm.py build --rust-release DIR      build once, record the baseline, snapshot
  devvm.py start | stop | status
  devvm.py restore [--check]             discard every change since the snapshot
  devvm.py ssh -- COMMAND...             run a command in the VM as root
  devvm.py state                         the VM's state document (guest/state.py)

Every command except `lease` refuses to run without the dev-vm lease, which
the T2 twins share (no twin starts while it is held).

Layout under .parity/e2e/devvm (never committed):
  base.qcow2   overlay of the pinned golden-incus, provisioned once, then
               read-only: the snapshot. The golden itself is never written.
  run.qcow2    overlay of base.qcow2 the VM runs on; restore recreates it.
  build.json   what the build did (recipe, hashes, timings), no secrets.
  baseline.json  the guest state at the snapshot.
  token        the agent's test bearer token (0600). It is only ever in
               this file and the guest's /etc/opute/host-agent.env.
"""
from __future__ import annotations

import argparse
import json
import os
import secrets
import shlex
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import e2elib  # noqa: E402
from e2elib import DEV, T2, sha256_file  # noqa: E402

HERE = Path(__file__).resolve().parent
GUEST = HERE / "guest"
SNAPSHOT_PY = e2elib.REPO_ROOT / "tools/parity/t2guest/snapshot.py"
GOLDEN = T2 / "golden-incus.qcow2"
GOLDEN_RECORD = T2 / "golden-incus.json"
# The published Go v0.2.4 GitHub release assets, pinned by the SHA256SUMS hash
# in baseline/source-lock.json (e2e-1 found the build used a stale v0.2.3 set).
GO_RELEASE = e2elib.PARITY / "releases/go-published-v0.2.4"
SOURCE_LOCK = e2elib.REPO_ROOT / "baseline/source-lock.json"
GO_DEPLOY = e2elib.PARITY / "go-src/deploy"
DISTRIBUTION = e2elib.REPO_ROOT / "baseline/inventory/distribution.json"
VCPUS, MEMORY_MB, DISK = 8, 16384, "100G"
SSH_PORT, MCP_PORT = 42222, 43014
AGENT_ID = "e2e-devvm-agent"
UNIT = "opute-bootstrap-mcp.service"
VERSION = "0.2.4"
POOL, BRIDGE, PROJECT = "opute-e2e", "opute-e2e0", "opute-e2e"
HOST_KEY_ALIAS = "opute-parity-t2-golden"

# Run as root in the VM, in order; each command is logged with its exit
# status. None carries a secret: the env file is copied in, never echoed.
PROVISION = [
    "incus admin waitready --timeout=180",
    # The 100 GiB overlay: grow the root partition and filesystem into it.
    "dev=$(findmnt -no SOURCE /) && disk=/dev/$(lsblk -no PKNAME \"$dev\") && "
    "part=$(cat /sys/class/block/$(basename \"$dev\")/partition) && "
    "(growpart \"$disk\" \"$part\" || true) && resize2fs \"$dev\"",
    f"incus storage create {POOL} btrfs size=60GiB",
    f"incus network create {BRIDGE} ipv4.address=10.0.120.1/24 ipv4.nat=true ipv6.address=none",
    f"incus project create {PROJECT} -c features.images=false -c features.profiles=true "
    "-c features.storage.volumes=true -c features.networks=false",
    f"incus profile device add default root disk path=/ pool={POOL} --project {PROJECT}",
    f"incus profile device add default eth0 nic network={BRIDGE} name=eth0 --project {PROJECT}",
    # The release sets: Rust is installed; Go is present only as the
    # migration source for e2e-1 journey 8 and e2e-4.
    "cd /opt/opute-e2e/releases/rust/v0.2.4 && sha256sum -c --quiet SHA256SUMS",
    "cd /opt/opute-e2e/releases/go/v0.2.4 && sha256sum -c --quiet SHA256SUMS",
    # The production-style system install (Go's deploy files, unchanged):
    # the opute user, the release binary, the system unit, the sudoers file,
    # and the identity and token in an env file through a unit drop-in.
    "id opute >/dev/null 2>&1 || useradd -m -s /bin/bash opute",
    "usermod -aG incus-admin opute",
    "install -d -o opute -g opute -m 0755 /home/opute/.local /home/opute/.local/share /home/opute/.local/share/opute",
    "gzip -dc /opt/opute-e2e/releases/rust/v0.2.4/host-agent-linux-x64.gz > /tmp/opute-host-agent && "
    "install -o opute -g opute -m 0755 /tmp/opute-host-agent /home/opute/.local/share/opute/opute-host-agent && "
    "rm /tmp/opute-host-agent",
    f"install -m 0644 /opt/opute-e2e/deploy/opute-bootstrap-mcp.service /etc/systemd/system/{UNIT}",
    "install -m 0440 /opt/opute-e2e/deploy/opute-host-agent-sudoers /etc/sudoers.d/opute-host-agent && "
    "visudo -cf /etc/sudoers.d/opute-host-agent",
    "install -d -m 0750 -g opute /etc/opute && install -m 0640 -g opute /opt/opute-e2e/host-agent.env "
    "/etc/opute/host-agent.env && rm /opt/opute-e2e/host-agent.env",
    f"install -d /etc/systemd/system/{UNIT}.d && printf '[Service]\\nEnvironmentFile=/etc/opute/host-agent.env\\n' "
    f"> /etc/systemd/system/{UNIT}.d/e2e.conf",
    f"systemctl daemon-reload && systemctl enable --now {UNIT}",
    "for i in $(seq 60); do curl -fsS http://127.0.0.1:3014/health >/dev/null && exit 0; sleep 1; done; exit 1",
    "sync",
]


def ssh_base(port: int = SSH_PORT) -> list[str]:
    return ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=yes",
            "-o", f"UserKnownHostsFile={T2 / 'known_hosts'}", "-o", f"HostKeyAlias={HOST_KEY_ALIAS}",
            "-o", "LogLevel=ERROR", "-i", str(T2 / "id_ed25519"), "-p", str(port), "root@127.0.0.1"]


def scp(sources: list[Path], target: str) -> None:
    argv = ["scp", "-q"] + [a for a in ssh_base()[1:-3] if a != "-p"] + ["-P", str(SSH_PORT)]
    subprocess.run(argv + [str(s) for s in sources] + [f"root@127.0.0.1:{target}"], check=True)


def guest(command: str, timeout: float = 1800, check: bool = False, input: str | None = None
          ) -> subprocess.CompletedProcess:
    done = subprocess.run(ssh_base() + [command], input=input, stdin=None if input is not None else subprocess.DEVNULL,
                          capture_output=True, text=True, timeout=timeout)
    if check and done.returncode != 0:
        raise RuntimeError(f"guest {command!r} exited {done.returncode}: {done.stderr[-2000:]}")
    return done


def pid() -> int | None:
    try:
        value = int((DEV / "qemu.pid").read_text())
    except (FileNotFoundError, ValueError):
        return None
    try:
        os.kill(value, 0)
    except ProcessLookupError:
        return None
    return value


def _port_free(port: int) -> bool:
    with socket.socket() as s:
        # As qemu binds its forwards: a TIME_WAIT left by the last session
        # does not hold the port.
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            s.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


def boot(disk: Path) -> None:
    if pid():
        raise SystemExit("refused: the dev VM is already running")
    for port in (SSH_PORT, MCP_PORT):
        if not _port_free(port):
            raise SystemExit(f"refused: 127.0.0.1:{port} is in use")
    argv = ["qemu-system-x86_64", "-enable-kvm", "-cpu", "host", "-smp", str(VCPUS), "-m", str(MEMORY_MB),
            "-name", "opute-e2e-devvm",
            "-drive", f"file={disk},if=virtio,format=qcow2",
            "-drive", f"file={DEV / 'seed.img'},if=virtio,format=raw,readonly=on",
            "-netdev", f"user,id=net0,hostfwd=tcp:127.0.0.1:{SSH_PORT}-:22,hostfwd=tcp:127.0.0.1:{MCP_PORT}-:3014",
            "-device", "virtio-net-pci,netdev=net0", "-display", "none",
            "-serial", f"file:{DEV / 'serial.log'}", "-monitor", "none",
            "-daemonize", "-pidfile", str(DEV / "qemu.pid")]
    subprocess.run(argv, check=True, stdout=(DEV / "qemu.log").open("a"), stderr=subprocess.STDOUT)
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        if not pid():
            raise RuntimeError("qemu exited during boot; see qemu.log and serial.log")
        if subprocess.run(ssh_base() + ["true"], capture_output=True).returncode == 0:
            break
        time.sleep(2)
    else:
        raise RuntimeError("dev VM ssh did not come up")
    # As the T2 twins do: let boot finish (and fsckd leave) before anything is observed.
    guest("systemctl is-system-running --wait >/dev/null 2>&1; "
          "timeout 180 sh -c 'while systemctl is-active --quiet systemd-fsckd.service; do sleep 1; done'; "
          "incus admin waitready --timeout=180", timeout=400)


def shutdown() -> None:
    running = pid()
    if not running:
        return
    guest("poweroff", timeout=30)
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and pid():
        time.sleep(1)
    if pid():
        os.kill(running, 15)
        time.sleep(5)
    (DEV / "qemu.pid").unlink(missing_ok=True)


def state() -> dict:
    guest("install -d /opt/opute-e2e/guest", check=True)
    scp([SNAPSHOT_PY, GUEST / "state.py"], "/opt/opute-e2e/guest/")
    done = guest("python3 /opt/opute-e2e/guest/state.py", timeout=600, check=True)
    return json.loads(done.stdout)


def token() -> str:
    return (DEV / "token").read_text().strip()


def _verify_release(directory: Path, names: list[str]) -> dict:
    sums = dict(reversed(line.split()) for line in (directory / "SHA256SUMS").read_text().splitlines())
    out = {}
    for name in names:
        actual = sha256_file(directory / name)
        if sums.get(name) != actual:
            raise SystemExit(f"refused: {directory / name} does not match its SHA256SUMS")
        out[name] = actual
    return {"dir": str(directory), "SHA256SUMS": sha256_file(directory / "SHA256SUMS"), "files": out}


def build(rust_release: Path) -> dict:
    lease = e2elib.require_lease()
    if (DEV / "base.qcow2").exists():
        raise SystemExit(f"refused: {DEV / 'base.qcow2'} exists; the dev VM is built once (restore resets it)")
    golden = json.loads(GOLDEN_RECORD.read_text())
    if sha256_file(GOLDEN) != golden["goldenSha256"]:
        raise SystemExit("refused: golden-incus.qcow2 does not match its record")
    deploy_pins = json.loads(DISTRIBUTION.read_text())["deployFiles"]
    deploy = {}
    for name in ("opute-bootstrap-mcp.service", "opute-host-agent-sudoers"):
        actual = sha256_file(GO_DEPLOY / name)
        if deploy_pins[f"deploy/{name}"] != actual:
            raise SystemExit(f"refused: deploy/{name} does not match the recorded distribution contract")
        deploy[name] = actual
    releases = {
        "rust": _verify_release(rust_release, ["host-agent-linux-x64", "host-agent-linux-x64.gz"]),
        "go": _verify_release(GO_RELEASE, ["host-agent-linux-x64", "host-agent-linux-x64.gz"]),
    }
    pinned = json.loads(SOURCE_LOCK.read_text())["sourceReleaseSha256Sums"]
    if f"sha256:{releases['go']['SHA256SUMS']}" != pinned:
        raise SystemExit(f"refused: {GO_RELEASE}/SHA256SUMS is not the pinned Go release ({pinned})")
    DEV.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(T2 / "seed.img", DEV / "seed.img")
    (DEV / "token").touch(mode=0o600)
    (DEV / "token").write_text(f"e2e-test-token-{secrets.token_hex(24)}\n")
    env_file = DEV / "host-agent.env"
    env_file.touch(mode=0o600)
    env_file.write_text(f"OPUTE_REMOTE_AGENT_ID={AGENT_ID}\nMCP_AUTH_TOKEN={token()}\n")
    base = DEV / "base.qcow2"
    started = time.time()
    subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-F", "qcow2", "-b", str(GOLDEN), str(base), DISK],
                   check=True)
    log = []
    try:
        boot(base)
        guest("install -d /opt/opute-e2e/releases/rust/v0.2.4 /opt/opute-e2e/releases/go/v0.2.4 "
              "/opt/opute-e2e/deploy", check=True)
        for side, files in (("rust", releases["rust"]), ("go", releases["go"])):
            directory = Path(files["dir"])
            scp(sorted(p for p in directory.iterdir() if p.is_file()), f"/opt/opute-e2e/releases/{side}/v0.2.4/")
        scp([GO_DEPLOY / n for n in deploy], "/opt/opute-e2e/deploy/")
        scp([env_file], "/opt/opute-e2e/host-agent.env")
        for command in PROVISION:
            t0 = time.monotonic()
            done = guest(command)
            log.append({"command": command, "exit": done.returncode, "stderr": done.stderr[-2000:],
                        "seconds": round(time.monotonic() - t0, 1)})
            if done.returncode != 0:
                raise RuntimeError(f"provision {command!r}: {done.stderr[-2000:]}")
        # The baseline is taken after one more boot, so that a restored VM
        # booting from the snapshot is compared like with like.
        shutdown()
        boot(base)
        baseline = baseline_document()
        shutdown()
    except BaseException:
        shutdown()
        (DEV / "build-failed.json").write_text(json.dumps({"log": log, "at": e2elib.now()}, indent=1) + "\n")
        base.unlink(missing_ok=True)
        raise
    finally:
        env_file.unlink(missing_ok=True)
    base.chmod(0o444)
    (DEV / "baseline.json").write_text(json.dumps(baseline, indent=1, sort_keys=True) + "\n")
    record = {
        "scope": "E2E dev VM: an overlay of the pinned golden-incus, provisioned once and then read-only; "
                 "runs as the user under the dev-vm lease; no host change beyond .parity",
        "lease": {k: lease[k] for k in ("holder", "acquired", "expires")},
        "golden": {"name": "golden-incus", "sha256": golden["goldenSha256"]},
        "vm": {"vcpus": VCPUS, "memoryMb": MEMORY_MB, "disk": DISK, "sshPort": SSH_PORT, "mcpPort": MCP_PORT},
        "incus": {"pool": POOL, "bridge": BRIDGE, "project": PROJECT, "guestPrefix": "e2e-"},
        "agent": {"id": AGENT_ID, "unit": UNIT, "envFile": "/etc/opute/host-agent.env",
                  "binary": "/home/opute/.local/share/opute/opute-host-agent", "version": VERSION},
        "releases": releases, "deployFiles": deploy, "provision": PROVISION, "provisionLog": log,
        "harness": {"devvm.py": sha256_file(Path(__file__)), "state.py": sha256_file(GUEST / "state.py"),
                    "snapshot.py": sha256_file(SNAPSHOT_PY)},
        "baselineSha256": sha256_file(DEV / "baseline.json"),
        "baseSha256": sha256_file(base), "built": e2elib.now(), "seconds": round(time.time() - started),
    }
    (DEV / "build.json").write_text(json.dumps(record, indent=1) + "\n")
    new_run_overlay()
    return record


def baseline_document() -> dict:
    """The guest state plus what the serving agent says about itself."""
    listed = e2elib.mcp(MCP_PORT, token(), "tools/list")
    tools = ((listed.get("body") or {}).get("result") or {}).get("tools") or []
    unit = guest(f"systemctl show -p ActiveState,SubState,UnitFileState,User,FragmentPath,DropInPaths {UNIT}",
                 check=True).stdout
    return {"state": state(), "health": e2elib.health(MCP_PORT),
            "unit": dict(line.split("=", 1) for line in unit.splitlines() if "=" in line),
            "toolsList": {"status": listed.get("status"), "count": len(tools),
                          "sha256": e2elib.sha256_json(tools)}}


def new_run_overlay() -> None:
    run = DEV / "run.qcow2"
    run.unlink(missing_ok=True)
    subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-F", "qcow2", "-b", str(DEV / "base.qcow2"),
                    str(run)], check=True)


def check_base() -> None:
    record = json.loads((DEV / "build.json").read_text())
    if sha256_file(DEV / "baseline.json") != record["baselineSha256"]:
        raise SystemExit("refused: baseline.json does not match build.json")
    if os.access(DEV / "base.qcow2", os.W_OK):
        raise SystemExit("refused: base.qcow2 is writable; the snapshot must stay read-only")


def restore(check: bool) -> dict:
    """Discard the run overlay and boot a fresh one from the snapshot."""
    e2elib.require_lease()
    check_base()
    shutdown()
    t0 = time.monotonic()
    new_run_overlay()
    boot(DEV / "run.qcow2")
    out = {"restored": e2elib.now(), "seconds": round(time.monotonic() - t0, 1)}
    if check:
        baseline = json.loads((DEV / "baseline.json").read_text())
        current = baseline_document()
        diff = state_diff(baseline["state"], current["state"])
        out.update(equal=not diff and current["toolsList"] == baseline["toolsList"]
                   and current["health"] == baseline["health"], diff=diff,
                   toolsList=current["toolsList"] == baseline["toolsList"],
                   health=current["health"] == baseline["health"])
    return out


def state_diff(before: dict, after: dict, prefix: str = "") -> list[dict]:
    """Every leaf that differs between two state documents, as {path, before, after}."""
    out = []
    if isinstance(before, dict) and isinstance(after, dict):
        for key in sorted(set(before) | set(after)):
            path = f"{prefix}/{key}" if prefix else str(key)
            if key not in before:
                out.append({"path": path, "before": None, "after": after[key], "change": "added"})
            elif key not in after:
                out.append({"path": path, "before": before[key], "after": None, "change": "removed"})
            else:
                out += state_diff(before[key], after[key], path)
    elif before != after:
        out.append({"path": prefix, "before": before, "after": after, "change": "changed"})
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    lease = sub.add_parser("lease")
    lease.add_argument("action", choices=["acquire", "release", "show"])
    lease.add_argument("--holder", default="")
    lease.add_argument("--hours", type=float, default=8)
    b = sub.add_parser("build")
    b.add_argument("--rust-release", type=Path, required=True)
    for name in ("start", "stop", "status", "state"):
        sub.add_parser(name)
    r = sub.add_parser("restore")
    r.add_argument("--check", action="store_true")
    s = sub.add_parser("ssh")
    s.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    if args.command == "lease":
        if args.action == "acquire":
            print(json.dumps(e2elib.acquire_lease(args.holder, args.hours), indent=1))
        elif args.action == "release":
            e2elib.release_lease(args.holder)
        else:
            print(json.dumps(e2elib.lease_status(), indent=1))
        return 0
    e2elib.require_lease()
    if args.command == "build":
        record = build(args.rust_release.resolve())
        print(json.dumps({k: record[k] for k in ("built", "seconds", "baseSha256", "baselineSha256")}, indent=1))
    elif args.command == "start":
        check_base()
        if not (DEV / "run.qcow2").exists():
            new_run_overlay()
        boot(DEV / "run.qcow2")
    elif args.command == "stop":
        shutdown()
    elif args.command == "status":
        print(json.dumps({"running": bool(pid()), "built": (DEV / "build.json").exists(),
                          "health": e2elib.health(MCP_PORT) if pid() else None}, indent=1))
    elif args.command == "restore":
        print(json.dumps(restore(args.check), indent=1, default=str)[:20000])
    elif args.command == "state":
        json.dump(state(), sys.stdout, sort_keys=True)
    elif args.command == "ssh":
        argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
        return subprocess.run(ssh_base() + [shlex.join(argv)]).returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
