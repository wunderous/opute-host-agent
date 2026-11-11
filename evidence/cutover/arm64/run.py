#!/usr/bin/env python3
"""One arm64 clean-machine install of the Rust release, under emulation.

  python3 evidence/cutover/arm64/run.py --release DIR

Boots a throwaway overlay of the cached Debian 12 genericcloud arm64 image
under qemu-system-aarch64 (TCG, -cpu cortex-a72, no KVM; -cpu max
# never passed the firmware in 20 min), with a fresh cloud-init seed and
ssh key. In the guest, as the image's unprivileged `debian` user, it
verifies host-agent-linux-arm64.gz against the release's SHA256SUMS,
installs it, starts `serve --transport http` with a test
token, answers /health, initialize-free tools/list and get_host_info over
MCP, stops it with SIGTERM, starts and stops it again, and uninstalls it.
Nothing outside .parity/ on the host is touched; the VM is discarded.

Writes evidence/cutover/arm64/receipt.json: each step's exit and output
(test token removed), the release and image hashes, and the verdict. The
token never reaches the receipt. Exit 0 only when every step passed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
SSH_PORT = 42422
LIMIT_S = 30 * 60
IMAGE_NAME = "debian-12-genericcloud-arm64-20261018-2635.qcow2"


def parity() -> Path:
    common = subprocess.run(["git", "-C", str(REPO), "rev-parse", "--path-format=absolute", "--git-common-dir"],
                            capture_output=True, text=True, check=True).stdout.strip()
    return Path(common).parent / ".parity"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--release", type=Path, required=True)
    args = parser.parse_args()
    started = time.monotonic()
    image = parity() / "local-native/t2" / IMAGE_NAME
    work = parity() / "cutover-arm64"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    key = work / "id_ed25519"
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)], check=True)
    user_data = work / "user-data"
    user_data.write_text("#cloud-config\nssh_pwauth: false\nusers:\n  - name: debian\n    shell: /bin/bash\n"
                         "    sudo: ALL=(ALL) NOPASSWD:ALL\n    ssh_authorized_keys:\n      - "
                         + (work / "id_ed25519.pub").read_text().strip() + "\n")
    subprocess.run(["cloud-localds", str(work / "seed.img"), str(user_data)], check=True)
    disk = work / "run.qcow2"
    subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-F", "qcow2", "-b", str(image), str(disk), "8G"],
                   check=True)
    shutil.copy("/usr/share/AAVMF/AAVMF_VARS.fd", work / "vars.fd")
    qemu = ["qemu-system-aarch64", "-M", "virt", "-cpu", "cortex-a72", "-accel", "tcg,thread=multi", "-smp", "4",
            "-m", "4096", "-display", "none", "-monitor", "none",
            "-drive", "if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.no-secboot.fd",
            "-drive", f"if=pflash,format=raw,file={work / 'vars.fd'}",
            "-drive", f"file={disk},if=virtio,format=qcow2",
            "-drive", f"file={work / 'seed.img'},if=virtio,format=raw,readonly=on",
            "-netdev", f"user,id=n0,hostfwd=tcp:127.0.0.1:{SSH_PORT}-:22", "-device", "virtio-net-pci,netdev=n0",
            "-serial", f"file:{work / 'serial.log'}", "-daemonize", "-pidfile", str(work / "qemu.pid")]
    subprocess.run(qemu, check=True)
    ssh = ["ssh", "-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=no",
           "-o", f"UserKnownHostsFile={work / 'known_hosts'}", "-i", str(key), "-p", str(SSH_PORT), "debian@127.0.0.1"]
    token = secrets.token_hex(24)
    steps: list[dict] = []

    def step(name: str, command: str, expect=None, timeout: float = 600) -> dict:
        t0 = time.monotonic()
        try:
            done = subprocess.run(ssh + [command], capture_output=True, text=True, timeout=timeout)
            code, out, err = done.returncode, done.stdout, done.stderr
        except subprocess.TimeoutExpired:
            code, out, err = None, "", "timeout"
        out, err = out.replace(token, "[test-token]")[-3000:], err.replace(token, "[test-token]")[-2000:]
        ok = code == 0 and (expect is None or expect(out))
        record = {"step": name, "exit": code, "stdout": out, "stderr": err, "ok": ok,
                  "seconds": round(time.monotonic() - t0, 1)}
        steps.append(record)
        return record

    booted = False
    try:
        deadline = time.monotonic() + 20 * 60
        while time.monotonic() < deadline:
            if subprocess.run(ssh + ["true"], capture_output=True).returncode == 0:
                booted = True
                break
            time.sleep(5)
        steps.append({"step": "boot", "ok": booted, "seconds": round(time.monotonic() - started, 1)})
        if booted:
            subprocess.run(["scp", "-q", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no",
                            "-o", f"UserKnownHostsFile={work / 'known_hosts'}", "-i", str(key), "-P", str(SSH_PORT),
                            str(args.release / "host-agent-linux-arm64.gz"), str(args.release / "SHA256SUMS"),
                            str(REPO / "evidence/e2e/mcpwire.py"), "debian@127.0.0.1:/tmp/"], check=True)
            bin_ = "$HOME/.local/share/opute/opute-host-agent"
            env = (f"OPUTE_REMOTE_AGENT_ID=arm64-clean MCP_AUTH_TOKEN={token} HOST_MCP_PORT=3014")
            # `cd ... ;` not `cd ... &&`: an `&&` list sent to the background is a subshell, so $! would
            # name the subshell rather than the agent. The launch must leave its own pid alive.
            serve = (f"cd $HOME; {env} nohup {bin_} serve --transport http "
                     "</dev/null >/tmp/agent.log 2>&1 & echo $! > /tmp/agent.pid; for i in $(seq 120); do "
                     "curl -fsS --max-time 5 http://127.0.0.1:3014/health >/dev/null 2>&1 && break; sleep 1; done; "
                     "curl -fsS --max-time 5 http://127.0.0.1:3014/health && sleep 2 && kill -0 $(cat /tmp/agent.pid) && "
                     "ps -o args= -p $(cat /tmp/agent.pid) | grep -q 'opute-host-agent serve' && echo alive")
            # The agent must be running when stopped: a failed kill fails the step.
            stop = ("kill -TERM $(cat /tmp/agent.pid) || exit 1; for i in $(seq 60); do kill -0 $(cat /tmp/agent.pid) "
                    "2>/dev/null || break; sleep 1; done; ! kill -0 $(cat /tmp/agent.pid) 2>/dev/null && echo stopped")
            # Diagnostics only (always ok): the pid file, the agent processes and the log tail.
            observe = ("echo pid=$(cat /tmp/agent.pid); pgrep -a opute-host || true; "
                       "tail -n 15 /tmp/agent.log 2>/dev/null; true")
            mcp = ("cd /tmp && python3 -c 'import json, sys, mcpwire; t = sys.argv[1]; "
                   "a = mcpwire.mcp(3014, t, \"tools/list\"); "
                   "b = mcpwire.mcp(3014, t, \"tools/call\", {\"name\": \"get_host_info\", \"arguments\": {}}); "
                   "tools = (a[\"body\"] or {}).get(\"result\", {}).get(\"tools\", []); "
                   "r = (b[\"body\"] or {}).get(\"result\", {}); "
                   "print(json.dumps({\"listStatus\": a[\"status\"], \"tools\": len(tools), "
                   "\"revision\": (a[\"body\"] or {}).get(\"result\", {}).get(\"_meta\", {}), "
                   "\"callStatus\": b[\"status\"], \"isError\": r.get(\"isError\", False), "
                   "\"agentId\": r.get(\"structuredContent\", {}).get(\"agent\", {}).get(\"agentId\")}))' " + token)
            step("machine", "uname -m && . /etc/os-release && echo $PRETTY_NAME",
                 lambda o: o.startswith("aarch64\n"))
            step("clean", "test ! -e $HOME/.local/share/opute && test ! -e $HOME/.config/opute && echo clean",
                 lambda o: o == "clean\n")
            step("verify", "cd /tmp && grep ' host-agent-linux-arm64.gz$' SHA256SUMS | sha256sum -c -",
                 lambda o: o.strip().endswith(": OK"))
            step("install", f"install -d $HOME/.local/share/opute && gzip -dc /tmp/host-agent-linux-arm64.gz > {bin_} "
                            f"&& chmod 0755 {bin_} && file -b {bin_} | cut -d, -f2 && {bin_} --version",
                 lambda o: "ARM aarch64" in o)
            step("help", f"cd $HOME && {bin_} help", lambda o: o.startswith("Usage: opute-host-agent"))
            step("launch", serve, lambda o: '"agentId":"arm64-clean"' in o and o.endswith("alive\n"), timeout=300)
            step("afterLaunch", observe)
            step("mcp", mcp, lambda o: (lambda d: d["listStatus"] == 200 and d["tools"] > 0 and d["callStatus"] == 200
                                        and not d["isError"] and d["agentId"] == "arm64-clean")(json.loads(o)),
                 timeout=300)
            step("stop", stop, lambda o: o.endswith("stopped\n"))
            step("afterStop", observe)
            step("relaunch", serve, lambda o: '"agentId":"arm64-clean"' in o and o.endswith("alive\n"), timeout=300)
            step("afterRelaunch", observe)
            step("stopAgain", stop, lambda o: o.endswith("stopped\n"))
            step("afterStopAgain", observe)
            step("uninstall", "rm -rf $HOME/.local/share/opute $HOME/.config/opute && "
                              "test ! -e $HOME/.local/share/opute && echo removed", lambda o: o == "removed\n")
    finally:
        pid = (work / "qemu.pid").read_text().strip() if (work / "qemu.pid").exists() else ""
        if pid:
            try:
                os.kill(int(pid), 15)
            except ProcessLookupError:
                pass
    seconds = round(time.monotonic() - started, 1)
    passed = booted and all(s["ok"] for s in steps) and seconds <= LIMIT_S
    receipt = {
        "schemaVersion": "opute-arm64-clean-install.v1",
        "emulation": "qemu-system-aarch64 TCG (-cpu cortex-a72), 4 vCPU, 4 GiB; no KVM",
        "image": {"name": IMAGE_NAME, "sha256": sha256(image)},
        "release": {"artifact": "host-agent-linux-arm64.gz",
                    "sha256": sha256(args.release / "host-agent-linux-arm64.gz"),
                    "sha256sums": sha256(args.release / "SHA256SUMS")},
        "steps": steps, "seconds": seconds, "limitSeconds": LIMIT_S, "pass": passed,
    }
    (HERE / "receipt.json").write_text(json.dumps(receipt, indent=1) + "\n")
    print(json.dumps({"pass": passed, "seconds": seconds,
                      "failed": [s["step"] for s in steps if not s["ok"]]}))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
