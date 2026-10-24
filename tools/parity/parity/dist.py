"""M10: distribution parity (packaging, installer, npm launcher, site).

Go's release set (`make artifacts` in the pinned tree) and the Rust release
set (tools/release/build-artifacts.sh) are compared at four places:

* artifacts: the release names, gzip, SHA256SUMS, ELF linkage and glibc
  floor, and the `--version`/`-version` output, built with the same VERSION;
* machines: disposable clean twins (Ubuntu 24.04 and Debian 12) where one
  release set is installed, started under systemd, called read-only,
  stopped and uninstalled exactly as an operator does with the published
  files, and a Go -> Rust -> Go upgrade and downgrade on one identity;
* npm: the pinned launcher, packed locally, downloads, verifies, caches and
  launches each release set from a local release base;
* site: the pinned site generator, run on each binary's captured catalog.

Each place yields an observation per side; Go/Go runs prove the harness is
deterministic and Go/Rust runs prove parity, with every normalization
declared next to the observation it applies to.
"""
from __future__ import annotations

import concurrent.futures
import copy
import hashlib
import json
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from . import t2

TOOLS_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = TOOLS_DIR.parent.parent
GUEST_JOURNEY = TOOLS_DIR / "t2guest" / "distjourney.py"
WORK = t2.WORK
VERSION = "0.2.3"

# --- clean-machine goldens ----------------------------------------------------------
#
# Ubuntu: the T2 base golden (the signed noble cloud image with background
# timers disabled; nothing of Opute on it). Debian: the bookworm generic
# cloud image, built the same way. Debian publishes no signature for its
# cloud image checksums, so the image is pinned here by its SHA512 as
# published in that build's SHA512SUMS.
DEBIAN_IMAGE = "debian-12-genericcloud-amd64-20261018-2635.qcow2"
DEBIAN_SOURCE = f"https://cloud.debian.org/images/cloud/bookworm/20261018-2635/{DEBIAN_IMAGE}"
DEBIAN_SHA512 = ("dc0d7b526a3a9a9265142fe8e903984c03f8d605ac4ba6f14d15d27988a69581"
                 "fb05fc06cd2a6d79f6eaf68ba9725f2eb38b5ddd64ad166faadad279af776c9e")
GOLDENS = {
    "ubuntu": {"image": "golden.qcow2", "record": "golden.json", "seed": "seed.img",
               "knownHosts": "known_hosts", "alias": t2.HOST_KEY_ALIAS},
    "debian": {"image": "golden-debian.qcow2", "record": "golden-debian.json", "seed": "seed-debian.img",
               "knownHosts": "known_hosts-debian", "alias": "opute-parity-t2-debian"},
}


def _sha512(path: Path) -> str:
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha512").hexdigest()


def ssh_base(distro: str, port: int) -> list[str]:
    g = GOLDENS[distro]
    return ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "StrictHostKeyChecking=yes",
            "-o", f"UserKnownHostsFile={WORK / g['knownHosts']}", "-o", f"HostKeyAlias={g['alias']}",
            "-o", "LogLevel=ERROR", "-i", str(WORK / "id_ed25519"), "-p", str(port), "root@127.0.0.1"]


def _wait_ssh(distro: str, port: int, process: subprocess.Popen, timeout: float = 300) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"qemu exited with {process.returncode}")
        if subprocess.run(ssh_base(distro, port) + ["true"], capture_output=True).returncode == 0:
            return
        time.sleep(2)
    raise RuntimeError("guest ssh did not come up")


def build_debian_golden() -> dict:
    """The Debian 12 clean golden: cloud-init, the twin key, timers off. Built once."""
    g = GOLDENS["debian"]
    golden, record_path = WORK / g["image"], WORK / g["record"]
    if golden.exists():
        raise RuntimeError(f"{golden} exists; a golden image is never rebuilt in place")
    image = t2.BASE_DIR / DEBIAN_IMAGE
    actual = _sha512(image)
    if actual != DEBIAN_SHA512:
        raise RuntimeError(f"debian image sha512 {actual} != pinned {DEBIAN_SHA512}")
    with (WORK / "golden-debian.lease").open("a+") as lease:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        public = (WORK / "id_ed25519.pub").read_text().strip()
        user_data = ("#cloud-config\nhostname: opute-parity-t2\nssh_pwauth: false\ndisable_root: false\n"
                     f"ssh_authorized_keys:\n  - {public}\n")
        (WORK / "user-data-debian").write_text(user_data)
        (WORK / "meta-data-debian").write_text("instance-id: opute-parity-t2-debian\nlocal-hostname: opute-parity-t2\n")
        seed = WORK / g["seed"]
        subprocess.run(["cloud-localds", str(seed), str(WORK / "user-data-debian"), str(WORK / "meta-data-debian")],
                       check=True)
        subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-F", "qcow2", "-b", str(image), str(golden),
                        "32G"], check=True)
        port = t2._free_port()
        argv = t2._qemu(golden, seed, port, WORK / "golden-debian-serial.log")
        process = subprocess.Popen(argv, stdout=(WORK / "golden-debian-qemu.log").open("w"),
                                   stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 300
            scanned = ""
            while time.monotonic() < deadline and not scanned:
                time.sleep(3)
                scan = subprocess.run(["ssh-keyscan", "-t", "ed25519", "-p", str(port), "127.0.0.1"],
                                      capture_output=True, text=True)
                scanned = next((line for line in scan.stdout.splitlines()
                                if not line.startswith("#") and " ssh-ed25519 " in line), "")
            if not scanned:
                raise RuntimeError("debian golden never offered a host key")
            (WORK / g["knownHosts"]).write_text(f"{g['alias']} {scanned.split(None, 1)[1]}\n")
            _wait_ssh("debian", port, process)
            ssh = ssh_base("debian", port)
            subprocess.run(ssh + ["cloud-init", "status", "--wait"], capture_output=True)
            for command in t2.STABILIZE:
                done = subprocess.run(ssh + [command], capture_output=True, text=True)
                if done.returncode != 0:
                    raise RuntimeError(f"stabilize {command!r}: {done.stderr}")
            subprocess.run(ssh + ["poweroff"], capture_output=True)
            process.wait(timeout=180)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        record = {
            "scope": "M10 clean-machine golden: dedicated, disposable; nothing of Opute installed",
            "baseImage": str(image), "baseImageSha512": actual, "baseImageSource": DEBIAN_SOURCE,
            "seedSha256": t2.sha256_file(seed), "userData": user_data, "stabilize": t2.STABILIZE,
            "goldenSha256": t2.sha256_file(golden), "created": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "qemuArgv": argv,
        }
        golden.chmod(0o444)
        record_path.write_text(json.dumps(record, indent=1) + "\n")
        return record


def golden_record(distro: str) -> dict:
    g = GOLDENS[distro]
    record = json.loads((WORK / g["record"]).read_text())
    actual = t2.sha256_file(WORK / g["image"])
    if actual != record["goldenSha256"]:
        raise RuntimeError(f"{distro} golden changed: {actual} != {record['goldenSha256']}")
    return record


class CleanTwin:
    """A disposable overlay of a clean golden holding only the release sets,
    Go's deploy files and the journey program."""

    def __init__(self, label: str, distro: str, run_dir: Path, releases: dict[str, Path], deploy: Path):
        self.label, self.distro, self.run_dir = label, distro, run_dir
        self.releases, self.deploy = releases, deploy
        self.disk = run_dir / f"{label}.qcow2"
        self.port = t2._free_port()
        self.process: subprocess.Popen | None = None

    def __enter__(self) -> "CleanTwin":
        g = GOLDENS[self.distro]
        self.run_dir.mkdir(parents=True, exist_ok=True)
        subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", "-F", "qcow2", "-b", str(WORK / g["image"]),
                        str(self.disk)], check=True)
        argv = t2._qemu(self.disk, WORK / g["seed"], self.port, self.run_dir / f"{self.label}-serial.log")
        self.process = subprocess.Popen(argv, stdout=(self.run_dir / f"{self.label}-qemu.log").open("w"),
                                        stderr=subprocess.STDOUT)
        _wait_ssh(self.distro, self.port, self.process)
        ssh = ssh_base(self.distro, self.port)
        subprocess.run(ssh + ["systemctl is-system-running --wait >/dev/null 2>&1; "
                              "timeout 180 sh -c 'while systemctl is-active --quiet systemd-fsckd.service; "
                              "do sleep 1; done'; mkdir -p /opt/releases /opt/deploy"],
                       check=True, capture_output=True)
        scp = ["scp", "-q", "-r"] + [a for a in ssh[1:-3] if a != "-p"] + ["-P", str(self.port)]
        for name, directory in self.releases.items():
            subprocess.run(ssh + [f"mkdir -p /opt/releases/{name}/v{VERSION}"], check=True, capture_output=True)
            subprocess.run(scp + [str(p) for p in sorted(directory.iterdir())]
                           + [f"root@127.0.0.1:/opt/releases/{name}/v{VERSION}/"], check=True)
        subprocess.run(scp + [str(p) for p in sorted(self.deploy.iterdir())] + ["root@127.0.0.1:/opt/deploy/"],
                       check=True)
        subprocess.run(scp + [str(GUEST_JOURNEY), "root@127.0.0.1:/opt/distjourney.py"], check=True)
        return self

    def run(self, program: dict, timeout: float = 1800) -> dict:
        done = subprocess.run(ssh_base(self.distro, self.port) + ["python3", "/opt/distjourney.py"],
                              input=json.dumps(program), capture_output=True, text=True, timeout=timeout)
        if done.returncode != 0:
            raise RuntimeError(f"journey failed ({done.returncode}): {done.stderr[-2000:]}")
        return json.loads(done.stdout)

    def __exit__(self, *exc: Any) -> None:
        if self.process and self.process.poll() is None:
            subprocess.run(ssh_base(self.distro, self.port) + ["poweroff"], capture_output=True, timeout=30)
            try:
                self.process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.disk.unlink(missing_ok=True)


# --- release sets -------------------------------------------------------------------

# Go's checksum list (Makefile `checksums`), without the Windows archive: the
# Windows artifact is an owned M10 gap on both sides (Go's own Windows build
# also needs modernc.org/libc's Windows files, absent from the pinned
# offline module cache).
RELEASE_FILES = ["host-agent-linux-x64", "host-agent-linux-x64.gz", "host-agent-linux-arm64.gz",
                 "opute-provider-k3s-linux-x64", "opute-provider-cloudflare-linux-x64",
                 "opute-provider-tailscale-linux-x64"]
SETUP = REPO_ROOT / ".parity" / "setup"
GO_ENV = {"GOFLAGS": "-mod=readonly", "GOPROXY": "off", "GOTOOLCHAIN": "local"}


def _export(go_src: Path, into: Path) -> None:
    into.mkdir(parents=True)
    archive = subprocess.run(["git", "-C", str(go_src), "archive", "HEAD"], check=True, capture_output=True).stdout
    subprocess.run(["tar", "-x", "-C", str(into)], input=archive, check=True)


def build_go_release(go_src: Path, out: Path, version: str = VERSION) -> dict:
    """Go's release set, built by Go's own Makefile targets in a clean export.
    Go records the build directory in its binaries, so the export is always
    at the same path, and the same tree and toolchain give the same bytes."""
    import os
    env = {**os.environ, **GO_ENV, "PATH": f"{SETUP / 'go' / 'bin'}:{os.environ['PATH']}"}
    tree = REPO_ROOT / "target" / "dist" / "go-release-export"
    if tree.exists():
        shutil.rmtree(tree)
    _export(go_src, tree)
    subprocess.run(["make", "-s", "-C", str(tree), f"VERSION={version}", "build-linux-x64", "build-linux-arm64",
                    "build-provider-linux-x64", "build-provider-cloudflare-linux-x64",
                    "build-provider-tailscale-linux-x64"], check=True, env=env, capture_output=True)
    out.mkdir(parents=True, exist_ok=True)
    for name in RELEASE_FILES + ["host-agent-linux-arm64"]:
        shutil.copy2(tree / "dist" / name, out / name)
    shutil.rmtree(tree)
    _write_sums(out)
    return {"dir": str(out), "files": {n: t2.sha256_file(out / n) for n in sorted(p.name for p in out.iterdir())}}


def build_rust_release(out: Path, version: str = VERSION) -> dict:
    import os
    env = {**os.environ, "VERSION": version, "DIST": str(out)}
    subprocess.run([str(REPO_ROOT / "tools/release/build-artifacts.sh")], check=True, env=env, capture_output=True)
    return {"dir": str(out), "files": {n: t2.sha256_file(out / n) for n in sorted(p.name for p in out.iterdir())}}


def _write_sums(directory: Path) -> None:
    lines = [f"{t2.sha256_file(directory / name)}  {name}\n" for name in RELEASE_FILES]
    (directory / "SHA256SUMS").write_text("".join(lines))


def _verify_sums(directory: Path) -> dict:
    listed, ok = [], True
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split(None, 1)
        name = name.lstrip("*")
        listed.append(name)
        ok = ok and (directory / name).is_file() and t2.sha256_file(directory / name) == digest
    return {"listed": listed, "verified": ok}


def _gzip_header(path: Path) -> dict:
    head = path.read_bytes()[:64]
    flags = head[3]
    name = None
    if flags & 0x08:
        name = head[10:head.index(b"\0", 10)].decode()
    return {"magic": head[:2].hex(), "method": head[2], "flags": flags, "xfl": head[8], "os": head[9],
            "name": name, "mtimeSet": int.from_bytes(head[4:8], "little") != 0}


def _elf(path: Path) -> dict:
    data = path.read_bytes()
    machine = {0x3E: "x86-64", 0xB7: "aarch64"}.get(int.from_bytes(data[18:20], "little"), "other")
    info = {"class": data[4], "machine": machine}
    readelf = subprocess.run(["readelf", "-d", "-V", "-W", str(path)], capture_output=True, text=True).stdout
    info["linkage"] = "dynamic" if "(NEEDED)" in readelf else "static"
    versions = [tuple(int(x) for x in v.split(".")) for v in
                __import__("re").findall(r"Name: GLIBC_([0-9.]+)\s+Flags: none", readelf)]
    info["glibcFloor"] = ".".join(map(str, max(versions))) if versions else None
    return info


def observe_artifacts(go: Path, rust: Path) -> dict:
    """Release names, checksums, gzip headers, ELF linkage and the version
    output of each release set."""
    import gzip as gz
    out: dict = {}
    for side, directory in (("go", go), ("rust", rust)):
        sums = _verify_sums(directory)
        side_out: dict = {"sums": sums, "gzip": {}, "elf": {}, "version": {}}
        for name in ("host-agent-linux-x64.gz", "host-agent-linux-arm64.gz"):
            side_out["gzip"][name] = _gzip_header(directory / name)
            plain = directory / name[:-3]
            side_out["gzip"][name]["matchesPlain"] = gz.decompress((directory / name).read_bytes()) == plain.read_bytes()
            side_out["elf"][name[:-3]] = _elf(plain)
        binary = directory / "host-agent-linux-x64"
        for flag in ("--version", "-version"):
            done = subprocess.run([str(binary), flag], capture_output=True, text=True)
            side_out["version"][flag] = {"exit": done.returncode, "stdout": done.stdout, "stderr": done.stderr}
        out[side] = side_out
    return out


def evaluate_artifacts(out: dict) -> dict:
    go_floor = out["go"]["elf"]["host-agent-linux-x64"]["glibcFloor"]
    rust_floor = out["rust"]["elf"]["host-agent-linux-x64"]["glibcFloor"]
    as_tuple = lambda v: tuple(int(x) for x in v.split("."))
    checks = {
        "names": out["go"]["sums"]["listed"] == out["rust"]["sums"]["listed"] == RELEASE_FILES,
        "checksumsVerify": out["go"]["sums"]["verified"] is True and out["rust"]["sums"]["verified"] is True,
        "gzip": all(out["go"]["gzip"][n] == out["rust"]["gzip"][n] and out["go"]["gzip"][n]["matchesPlain"]
                    for n in out["go"]["gzip"]),
        "elf": all(out["go"]["elf"][n]["machine"] == out["rust"]["elf"][n]["machine"]
                   and out["go"]["elf"][n]["linkage"] == out["rust"]["elf"][n]["linkage"] for n in out["go"]["elf"]),
        # Rust may need an older glibc than Go, never a newer one.
        "glibcFloor": rust_floor is not None and go_floor is not None and as_tuple(rust_floor) <= as_tuple(go_floor),
        "version": out["go"]["version"] == out["rust"]["version"]
        and out["go"]["version"]["--version"]["stdout"].strip() == VERSION,
    }
    return {"checks": checks, "pass": all(checks.values())}


# --- the agent's log lines per request class ------------------------------------------
#
# The journal journeys see the lines a first call produces; this matrix
# drives every class of request the HTTP handler distinguishes (answered by
# the agent, refused before the SDK's session, refused by the transport,
# refused by the session, removed in the new protocol, a notification, the
# per-request log level) and compares each side's stderr line for line.
LOG_TOKEN = "parity-m10-logs-token-0123456789abcdef"


def _log_meta(level: Any = None, version: str = "2026-07-28", caps: bool = True) -> dict:
    meta: dict = {"io.modelcontextprotocol/protocolVersion": version,
                  "io.modelcontextprotocol/clientInfo": {"name": "opute-parity-m10", "version": "1"}}
    if caps:
        meta["io.modelcontextprotocol/clientCapabilities"] = {}
    if level is not None:
        meta["io.modelcontextprotocol/logLevel"] = level
    return meta


LOG_CASES = [
    ("discover", {"method": "server/discover", "params": {"_meta": _log_meta()}}),
    ("toolsList", {"method": "tools/list", "params": {"_meta": _log_meta()}}),
    ("toolsListDebug", {"method": "tools/list", "params": {"_meta": _log_meta("debug")}}),
    ("toolsListNumericLevel", {"method": "tools/list", "params": {"_meta": _log_meta(3)}}),
    ("toolsCall", {"method": "tools/call", "name": "get_host_info",
                   "params": {"name": "get_host_info", "arguments": {}, "_meta": _log_meta()}}),
    ("pingRemoved", {"method": "ping", "params": {"_meta": _log_meta()}}),
    ("setLevelRemoved", {"method": "logging/setLevel", "params": {"level": "debug", "_meta": _log_meta()}}),
    ("notification", {"method": "notifications/cancelled", "id": None,
                      "params": {"requestId": 1, "_meta": _log_meta()}}),
    ("notificationRemoved", {"method": "notifications/initialized", "id": None, "params": {"_meta": _log_meta()}}),
    ("unknownMethod", {"method": "foo/bar", "params": {"_meta": _log_meta()}}),
    ("missingCapabilities", {"method": "tools/list", "params": {"_meta": _log_meta(caps=False)}}),
    ("unsupportedVersion", {"method": "tools/list", "proto": "2027-01-01",
                            "params": {"_meta": _log_meta(version="2027-01-01")}}),
    ("contentType", {"method": "tools/list", "contentType": "text/plain", "params": {"_meta": _log_meta()}}),
    ("invalidJson", {"method": "tools/list", "body": "{nope"}),
    ("headerMismatch", {"method": "tools/list", "headers": {"Mcp-Method": "tools/call"},
                        "params": {"_meta": _log_meta()}}),
    ("wrongToken", {"method": "tools/list", "headers": {"Authorization": "Bearer nope"},
                    "params": {"_meta": _log_meta()}}),
    ("oldProtocol", {"method": "tools/list", "proto": "2025-11-25", "params": {}}),
    ("oldSetLevel", {"method": "logging/setLevel", "proto": "2025-11-25", "params": {"level": "warning"}}),
]


def _log_request(port: int, case: dict) -> int:
    import urllib.error
    import urllib.request
    headers = {"Accept": "application/json, text/event-stream", "Content-Type": case.get("contentType", "application/json"),
               "Authorization": f"Bearer {LOG_TOKEN}", "Origin": f"http://127.0.0.1:{port}",
               "MCP-Protocol-Version": case.get("proto", "2026-07-28"), "Mcp-Method": case["method"]}
    if case.get("name"):
        headers["Mcp-Name"] = case["name"]
    headers.update(case.get("headers", {}))
    if "body" in case:
        body = case["body"].encode()
    else:
        message: dict = {"jsonrpc": "2.0", "method": case["method"], "params": case["params"]}
        if case.get("id", 1) is not None:
            message["id"] = case.get("id", 1)
        body = json.dumps(message).encode()
    request = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code


def _log_lines(binary: Path) -> dict:
    import os
    import re
    import tempfile
    import urllib.request
    with tempfile.TemporaryDirectory() as tmp:
        port = t2._free_port()
        stderr_path = Path(tmp) / "stderr"
        env = {"PATH": os.environ["PATH"], "HOME": tmp, "OPUTE_REMOTE_AGENT_ID": "parity-m10-logs",
               "OPUTE_AGENT_MODE": "standalone", "MCP_AUTH_TOKEN": LOG_TOKEN, "HOST_MCP_BIND_HOST": "127.0.0.1",
               "HOST_MCP_PORT": str(port), "OPUTE_STANDALONE_STATE_DIR": f"{tmp}/state"}
        with stderr_path.open("w") as log:
            process = subprocess.Popen([str(binary)], env=env, stdout=subprocess.DEVNULL, stderr=log)
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                try:
                    urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1)
                    break
                except OSError:
                    time.sleep(0.1)
            startup = stderr_path.read_text().splitlines()
            # The listening port is this run's own; the time is the clock's.
            clean = lambda line: re.sub(r"^time=\S+ ", "", line).replace(f":{port}", ":<port>")
            out = {"_startup": [clean(line) for line in startup]}
            for name, case in LOG_CASES:
                before = stderr_path.stat().st_size
                status = _log_request(port, case)
                time.sleep(0.2)
                with stderr_path.open() as f:
                    f.seek(before)
                    lines = f.read().splitlines()
                out[name] = {"status": status, "lines": [clean(line) for line in lines]}
        finally:
            process.terminate()
            process.wait()
    return out


def _drop_oauth(lines: list[str]) -> list[str]:
    # D8 (secure-oauth-issuance): Rust writes one audit line per OAuth
    # provisioning, migration and issuance event; Go has no such issuer.
    # Same lines as the D8.audit-server-log divergence (contains "msg=oauth ").
    return [line for line in lines if "level=INFO msg=oauth event=" not in line]


def observe_logs(go_binary: Path, rust_binary: Path) -> dict:
    """Each request class's log lines from Go twice (the control) and Rust."""
    return {"go": _log_lines(go_binary), "go2": _log_lines(go_binary), "rust": _log_lines(rust_binary)}


def evaluate_logs(obs: dict) -> dict:
    cases = {}
    for name in ["_startup"] + [n for n, _ in LOG_CASES]:
        g, g2, r = obs["go"][name], obs["go2"][name], obs["rust"][name]
        if name == "_startup":
            g, g2, r = _drop_oauth(g), _drop_oauth(g2), _drop_oauth(r)
        cases[name] = {"goGo": g == g2, "goRust": g == r}
    return {"cases": cases, "pass": all(c["goGo"] and c["goRust"] for c in cases.values())}


# --- the npm launcher ---------------------------------------------------------------
#
# The pinned launcher (go-src/npm/local-host-agent) is packed locally with
# npm and run with `npm exec --package=<tarball>`, as the readonly canary
# runs it, against a release base served on 127.0.0.1 from one side's release
# set. The launcher is Go's own and unchanged; what differs between the sides
# is only the release set it downloads, verifies, caches and starts.
NODE_BIN = SETUP / "node-v22.14.0-linux-x64" / "bin"
NPM_AGENT_ID = "parity-m10-npm-agent"
NPM_TOKEN = "parity-m10-npm-token-0123456789abcdef"


class ReleaseServer:
    """A local release base: /v<version>/<file> from one release set, with a
    request log, an optional delay on the archive and an optional manifest
    override (to serve a SHA256SUMS that does not match the archive)."""

    def __init__(self, directory: Path, delay: float = 0.0, sums: str | None = None):
        import http.server
        import threading
        self.directory, self.delay, self.sums = directory, delay, sums
        self.requests: list[str] = []
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                owner.requests.append(self.path)
                prefix = f"/v{VERSION}/"
                name = self.path[len(prefix):] if self.path.startswith(prefix) else ""
                if name == "SHA256SUMS" and owner.sums is not None:
                    body = owner.sums.encode()
                elif name and "/" not in name and (owner.directory / name).is_file():
                    body = (owner.directory / name).read_bytes()
                else:
                    self.send_response(404)
                    self.end_headers()
                    return
                if name.endswith(".gz") and owner.delay:
                    time.sleep(owner.delay)
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args: Any) -> None:
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self) -> "ReleaseServer":
        self.thread.start()
        return self

    def __exit__(self, *exc: Any) -> None:
        self.server.shutdown()
        self.server.server_close()


def _node_env(extra: dict) -> dict:
    import os
    keep = {k: v for k, v in os.environ.items()
            if not k.startswith("OPUTE_") and k not in ("MCP_AUTH_TOKEN", "NPM_TOKEN", "NODE_AUTH_TOKEN")}
    return {**keep, "PATH": f"{NODE_BIN}:{keep['PATH']}", "NPM_CONFIG_UPDATE_NOTIFIER": "false", **extra}


def pack_launcher(go_src: Path, out: Path) -> dict:
    """`npm pack` of the pinned launcher, from a clean export of the tree."""
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        tree = Path(tmp) / "src"
        _export(go_src, tree)
        out.mkdir(parents=True, exist_ok=True)
        done = subprocess.run(["npm", "pack", "--json", "--pack-destination", str(out)],
                              cwd=tree / "npm" / "local-host-agent", capture_output=True, text=True, check=True,
                              env=_node_env({"NPM_CONFIG_CACHE": f"{tmp}/npm-cache", "NPM_CONFIG_OFFLINE": "true"}))
    packed = json.loads(done.stdout)[0]
    tarball = out / packed["filename"]
    return {"tarball": str(tarball), "sha256": t2.sha256_file(tarball),
            "files": sorted(f["path"] for f in packed["files"]), "version": packed["version"]}


def _launcher(tarball: Path, root: Path, env: dict, *args: str, timeout: float = 120) -> dict:
    cache = root / "npm-cache"
    done = subprocess.run(["npm", "exec", "--yes", "--offline", "--cache", str(cache), f"--package={tarball}", "--",
                           "opute-host-agent", *args], cwd=root, env=env, capture_output=True, text=True,
                          timeout=timeout)
    return {"exit": done.returncode, "stdout": done.stdout.strip(), "stderr": done.stderr.strip()}


def _mcp(port: int, method: str, params: dict | None = None, token: str = NPM_TOKEN) -> tuple[int, Any]:
    import urllib.error
    import urllib.request
    message = {"jsonrpc": "2.0", "id": 1, "method": method,
               "params": {**(params or {}), "_meta": {
                   "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                   "io.modelcontextprotocol/clientCapabilities": {},
                   "io.modelcontextprotocol/clientInfo": {"name": "opute-parity-m10", "version": "1"}}}}
    headers = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json",
               "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": method, "Origin": f"http://127.0.0.1:{port}",
               "Authorization": f"Bearer {token}"}
    if method == "tools/call":
        headers["Mcp-Name"] = params["name"]
    request = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=json.dumps(message).encode(),
                                     headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as error:
        return error.code, None


def _health(port: int) -> dict:
    import urllib.request
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=5) as response:
        return json.loads(response.read())


def _safe_health(port: int) -> dict:
    try:
        return _health(port)
    except OSError as error:
        return {"error": str(error)}


def _cache_tree(cache: Path) -> dict:
    tree = {}
    for path in sorted(cache.rglob("*")):
        tree[str(path.relative_to(cache))] = {"type": "dir" if path.is_dir() else "file",
                                              "mode": oct(path.stat().st_mode & 0o777)}
    return tree


def _launcher_env(root: Path, base: str, port: int, name: str = "a", cache: str = "agent-cache") -> dict:
    return _node_env({"NPM_CONFIG_CACHE": str(root / "npm-cache"), "NPM_CONFIG_OFFLINE": "true",
                      "OPUTE_HOST_AGENT_RELEASE_BASE_URL": base, "OPUTE_HOST_AGENT_CACHE_DIR": str(root / cache),
                      "OPUTE_LOCAL_HOST_AGENT_RUNTIME_DIR": str(root / f"runtime-{name}"),
                      "OPUTE_STANDALONE_STATE_DIR": str(root / f"state-{name}"),
                      "OPUTE_REMOTE_AGENT_ID": NPM_AGENT_ID, "MCP_AUTH_TOKEN": NPM_TOKEN,
                      "HOST_MCP_BIND_HOST": "127.0.0.1", "HOST_MCP_PORT": str(port), "HOME": str(root / "home")})


def _started(root: Path, tarball: Path, env: dict, port: int) -> dict:
    """One background start, its health and status, a read-only first call,
    and the stop."""
    out: dict = {"start": _launcher(tarball, root, env, "start", "--background")}
    out["start"]["stdout"] = out["start"]["stdout"].replace(f":{port}/", ":<port>/")
    if out["start"]["exit"] != 0:
        return out
    state = json.loads((Path(env["OPUTE_LOCAL_HOST_AGENT_RUNTIME_DIR"]) / "daemon.json").read_text()) \
        if (Path(env["OPUTE_LOCAL_HOST_AGENT_RUNTIME_DIR"]) / "daemon.json").is_file() else {}
    health = _health(port)
    out["health"] = {"keys": sorted(health), "agentId": health.get("agentId"),
                     "ownsInstance": bool(state) and health.get("localInstanceId") == state.get("instanceId")}
    status = _launcher(tarball, root, env, "status")
    payload = json.loads(status["stdout"]) if status["exit"] == 0 else {}
    out["status"] = {"exit": status["exit"], "running": payload.get("running"), "healthy": payload.get("healthy")}
    code, body = _mcp(port, "tools/list")
    out["toolsList"] = {"status": code, "count": len(((body or {}).get("result") or {}).get("tools", []))}
    code, body = _mcp(port, "tools/call", {"name": "get_host_info", "arguments": {}})
    result = (body or {}).get("result") or {}
    out["getHostInfo"] = {"status": code, "isError": result.get("isError", False),
                          "keys": sorted((result.get("structuredContent") or {}).keys())}
    out["wrongToken"] = _mcp(port, "tools/list", token="wrong")[0]
    stop = _launcher(tarball, root, env, "stop")
    out["stop"] = {"exit": stop["exit"]}
    return out


UNSUPPORTED = {"darwin-arm64": ("darwin", "arm64"), "linux-ia32": ("linux", "ia32"), "win32-x64": ("win32", "x64")}


def npm_journey(tarball: Path, release: Path, work: Path) -> dict:
    """Download, cache hit, checksum mismatch, concurrent download lock and
    unsupported platforms, for one release set."""
    import tarfile
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    plain = {n: t2.sha256_file(release / n) for n in ("host-agent-linux-x64",)}
    sums = dict(reversed(line.split(None, 1)) for line in (release / "SHA256SUMS").read_text().splitlines())
    out: dict = {}

    # 1. A first start downloads, verifies and caches; a second start reuses
    # the verified cache and fetches only the manifest.
    root = work / "download"
    root.mkdir()
    with ReleaseServer(release) as server:
        port = t2._free_port()
        env = _launcher_env(root, server.base, port)
        first = _started(root, tarball, env, port)
        first_requests = list(server.requests)
        second = _started(root, tarball, env, port)
        second_requests = server.requests[len(first_requests):]
    cache = root / "agent-cache"
    marker = json.loads((cache / VERSION / "host-agent-linux-x64.verified.json").read_text())
    out["download"] = {"first": first, "firstRequests": first_requests, "second": second,
                       "secondRequests": second_requests, "cache": _cache_tree(cache),
                       "marker": {"keys": sorted(marker),
                                  "archiveMatchesSums": marker.get("archiveSha256") == sums["host-agent-linux-x64.gz"],
                                  "binaryMatchesRelease": marker.get("binarySha256") == plain["host-agent-linux-x64"],
                                  "cachedMatchesRelease": t2.sha256_file(cache / VERSION / "host-agent-linux-x64")
                                  == plain["host-agent-linux-x64"]}}

    # 2. A manifest that does not match the archive is refused, and nothing
    # is cached.
    root = work / "mismatch"
    root.mkdir()
    wrong = "".join(f"{'0' * 64 if name == 'host-agent-linux-x64.gz' else digest}  {name}\n"
                    for name, digest in sums.items())
    with ReleaseServer(release, sums=wrong) as server:
        port = t2._free_port()
        env = _launcher_env(root, server.base, port)
        attempt = _launcher(tarball, root, env, "start", "--background")
        out["mismatch"] = {"start": attempt, "requests": list(server.requests),
                           "cache": _cache_tree(root / "agent-cache") if (root / "agent-cache").exists() else {},
                           "listening": _listening(port)}

    # 3. Two launchers sharing one cache while the archive download is slow:
    # one holds the lock and downloads, the other waits and reuses it.
    root = work / "concurrent"
    root.mkdir()
    with ReleaseServer(release, delay=3.0) as server:
        ports = [t2._free_port(), t2._free_port()]
        envs = [_launcher_env(root, server.base, ports[i], name=str(i)) for i in range(2)]
        lock = root / "agent-cache" / f"{VERSION}.lock"
        # npm exec installs the tarball into its own cache first; two cold
        # npm execs racing on that install is npm's concurrency, not the
        # launcher's, so the install happens once before the race (help
        # downloads nothing).
        out["concurrentPrewarm"] = _launcher(tarball, root, envs[0], "help")["exit"]
        with concurrent.futures.ThreadPoolExecutor(2) as pool:
            futures = [pool.submit(_launcher, tarball, root, envs[i], "start", "--background") for i in range(2)]
            lock_seen, deadline = False, time.monotonic() + 20
            while time.monotonic() < deadline and not all(f.done() for f in futures):
                lock_seen = lock_seen or lock.is_dir()
                time.sleep(0.05)
            starts = [f.result() for f in futures]
        healthy = [_safe_health(p).get("agentId") == NPM_AGENT_ID for p in ports]
        stops = [_launcher(tarball, root, envs[i], "stop")["exit"] for i in range(2)]
        out["concurrent"] = {"starts": [{"exit": s["exit"], "stderr": s["stderr"]} for s in starts], "healthy": healthy, "stops": stops,
                             "lockSeen": lock_seen, "lockLeft": lock.exists(),
                             "archiveDownloads": sum(r.endswith(".gz") for r in server.requests),
                             "cache": _cache_tree(root / "agent-cache")}

    # 4. Unsupported platforms: the packed launcher itself, run by node with
    # process.platform/arch reported as another target.
    root = work / "unsupported"
    (root / "pkg").mkdir(parents=True)
    with tarfile.open(tarball) as archive:
        archive.extractall(root / "pkg", filter="data")
    out["unsupported"] = {}
    for label, (platform, arch) in UNSUPPORTED.items():
        preload = root / f"{label}.js"
        preload.write_text(f"Object.defineProperty(process, 'platform', {{ value: {json.dumps(platform)} }})\n"
                           f"Object.defineProperty(process, 'arch', {{ value: {json.dumps(arch)} }})\n")
        env = _launcher_env(root, "http://127.0.0.1:9", t2._free_port(), name=label)
        done = subprocess.run(["node", "--require", str(preload), str(root / "pkg" / "package" / "index.js"),
                               "start"], env=env, capture_output=True, text=True, timeout=60)
        out["unsupported"][label] = {"exit": done.returncode, "stderr": done.stderr.strip()}
    return out


def _listening(port: int) -> bool:
    import socket
    with socket.socket() as sock:
        sock.settimeout(0.5)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def npm_unit_tests(go_src: Path) -> dict:
    """The launcher's own unit suite (index.test.js), from a clean export."""
    import re
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        tree = Path(tmp) / "src"
        _export(go_src, tree)
        done = subprocess.run(["node", "--test", "index.test.js"], cwd=tree / "npm" / "local-host-agent",
                              env=_node_env({"HOME": tmp}), capture_output=True, text=True, timeout=600)
    counts = {k: int(v) for k, v in re.findall(r"^# (tests|pass|fail|skipped|cancelled) (\d+)$", done.stdout, re.M)}
    return {"exit": done.returncode, "counts": counts, "tail": done.stdout.splitlines()[-12:]}


def npm_canary(go_src: Path, tarball: Path, binary: Path) -> dict:
    """The pinned readonly canary against the packed tarball, starting the
    binary the launcher downloaded and verified from the local release base
    (the canary passes only OPUTE_HOST_AGENT_BINARY through to the launcher)."""
    import re
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        tree = Path(tmp) / "src"
        _export(go_src, tree)
        done = subprocess.run(["node", "--test", "published-readonly-canary.test.js"],
                              cwd=tree / "npm" / "local-host-agent", capture_output=True, text=True, timeout=600,
                              env=_node_env({"HOME": tmp, "NPM_CONFIG_OFFLINE": "true",
                                             "RUN_PUBLISHED_READONLY_NPM_CANARY": "true",
                                             "PUBLISHED_NPM_PACKAGE": str(tarball),
                                             "OPUTE_HOST_AGENT_BINARY": str(binary)}))
    counts = {k: int(v) for k, v in re.findall(r"^# (tests|pass|fail|skipped|cancelled) (\d+)$", done.stdout, re.M)}
    return {"exit": done.returncode, "counts": counts, "tail": done.stdout.splitlines()[-15:]}


# --- the site -----------------------------------------------------------------------
#
# Go's `make check-site`, step for step, in a clone of the pinned tree: the
# capture test, the catalog capture from a binary, the docs generator and
# the eight site checks. Go runs the generator with bun; bun is not part of
# the pinned toolchain here, so it runs on the pinned node with
# --experimental-strip-types, and the committed site reproducing byte for
# byte from Go's binary is the proof that the substitution is faithful.
SITE_CHECKS = ["check_site_release_boundary", "check_site_release_parity", "test_promote_site_release_catalog",
               "test_promote_archived_site_release_catalog", "test_validate_generated_site",
               "test_check_generated_site_clean", "validate-generated-site", "check_generated_site_clean"]
SITE_CATALOG = Path("site/context/release-catalog.json")


def _site_clone(go_src: Path, into: Path) -> None:
    if into.exists():
        shutil.rmtree(into)
    head = subprocess.run(["git", "-C", str(go_src), "rev-parse", "HEAD"], capture_output=True, text=True,
                          check=True).stdout.strip()
    subprocess.run(["git", "clone", "-q", "--shared", str(go_src), str(into)], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(into), "-c", "advice.detachedHead=false", "checkout", "-q", head], check=True)


def _site_run(tree: Path, argv: list[str], env: dict | None = None) -> dict:
    done = subprocess.run(argv, cwd=tree, capture_output=True, text=True, timeout=900,
                          env=_node_env({"HOME": str(tree.parent), **(env or {})}))
    # The binary's own log lines (relayed by the capture script) and node's
    # warnings are not the step's result.
    lines = [line for line in (done.stdout + done.stderr).strip().splitlines()
             if not ("time=" in line and " level=" in line) and not line.startswith(("(node:", "(Use `node"))]
    return {"exit": done.returncode, "last": (lines[-1:] or [""])[0][:400]}


def _site_capture(tree: Path, binary: Path) -> dict:
    return _site_run(tree, ["node", "site/scripts/capture-catalog.mjs"], {"OPUTE_HOST_AGENT_BINARY": str(binary)})


def _site_generate(tree: Path) -> dict:
    return _site_run(tree, ["node", "--experimental-strip-types", "site/scripts/generate-docs.ts"])


def _site_status(tree: Path) -> list[str]:
    return subprocess.run(["git", "-C", str(tree), "status", "--porcelain", "--untracked-files=all"],
                          capture_output=True, text=True, check=True).stdout.splitlines()


def _tree_files(root: Path) -> dict[str, str]:
    return {str(p.relative_to(root)): t2.sha256_file(p) for p in sorted(root.rglob("*")) if p.is_file()}


def _rename_rule() -> dict:
    rules = json.loads((TOOLS_DIR / "divergences.json").read_text())
    return next(r for r in rules if r["id"] == "D19.neutral-host-tool-names")


def _properties_maps(value: Any) -> list[dict]:
    found = []
    if isinstance(value, dict):
        if isinstance(value.get("properties"), dict):
            found.append(value["properties"])
        for item in value.values():
            found += _properties_maps(item)
    elif isinstance(value, list):
        for item in value:
            found += _properties_maps(item)
    return found


def _properties_sorted(catalog: Any) -> bool:
    return all(list(p) == sorted(p) for p in _properties_maps(catalog))


def _sorted_properties(value: Any) -> Any:
    """Each schema's properties in key order, as both binaries write them
    (asserted per capture as propertiesSorted): a field the rename puts back
    under its Go name goes back to its Go position."""
    if isinstance(value, dict):
        return {k: (dict(sorted((pk, _sorted_properties(pv)) for pk, pv in v.items()))
                    if k == "properties" and isinstance(v, dict) else _sorted_properties(v))
                for k, v in value.items()}
    if isinstance(value, list):
        return [_sorted_properties(item) for item in value]
    return value


def observe_site(go_src: Path, go_binary: Path, rust_binary: Path, work: Path) -> dict:
    """The committed site from each binary, a preview site from each binary,
    and the preview from Rust's catalog spelled the Go way."""
    from . import canon
    work.mkdir(parents=True, exist_ok=True)
    out: dict = {}

    # 1. Go's own check-site, committed site, from each binary.
    for side, binary in (("go", go_binary), ("rust", rust_binary)):
        tree = work / f"committed-{side}"
        _site_clone(go_src, tree)
        steps = {"captureTest": _site_run(tree, ["node", "--test", "site/scripts/capture-catalog.test.mjs"]),
                 "capture": _site_capture(tree, binary)}
        if steps["capture"]["exit"] == 0:
            steps["generate"] = _site_generate(tree)
            for check in SITE_CHECKS:
                steps[check] = _site_run(tree, ["python3", f"scripts/{check}.py"])
        out[f"committed-{side}"] = {"steps": steps, "status": _site_status(tree)}

    # 2. A preview site (no stable release catalog) from each binary.
    for side, binary in (("go", go_binary), ("rust", rust_binary)):
        tree = work / f"preview-{side}"
        _site_clone(go_src, tree)
        (tree / SITE_CATALOG).unlink()
        out[f"preview-{side}"] = {"capture": _site_capture(tree, binary), "generate": _site_generate(tree)}

    # 3. Rust's preview catalog with the declared D19-D24 renames reversed,
    # generated again: it must reproduce Go's preview byte for byte.
    tree = work / "preview-rust-as-go"
    _site_clone(go_src, tree)
    rust_catalog = json.loads((work / "preview-rust" / SITE_CATALOG).read_text())
    rule = _rename_rule()
    as_go = _sorted_properties(canon._rename_to_go(rust_catalog, canon._rename_tables(rule)))
    (tree / SITE_CATALOG).write_text(json.dumps(as_go, indent=2, ensure_ascii=False) + "\n")
    out["preview-rust-as-go"] = {"generate": _site_generate(tree)}

    go_files = _tree_files(work / "preview-go" / "site")
    differ = lambda other: sorted(n for n in set(go_files) | set(other) if go_files.get(n) != other.get(n))
    out["previewDiff"] = differ(_tree_files(work / "preview-rust" / "site"))
    out["previewAsGoDiff"] = differ(_tree_files(tree / "site"))
    go_catalog = json.loads((work / "preview-go" / SITE_CATALOG).read_text())
    out["catalogAsGoEqual"] = as_go == go_catalog
    out["propertiesSorted"] = {"go": _properties_sorted(go_catalog), "rust": _properties_sorted(rust_catalog)}
    out["renameRule"] = rule["id"]
    return out


# --- clean machines -----------------------------------------------------------------
#
# Each journey runs on its own disposable twin of a clean golden. The install
# journey installs one release set the way the published files are used (the
# user unit with its env file; the bootstrap system unit as published, which
# has no identity and must refuse to start; the bootstrap unit with an
# identity drop-in) and removes it again. The upgrade journey keeps one
# identity, env file and state across Go -> <middle> -> Go.
MACHINE_ENV = {"OPUTE_REMOTE_AGENT_ID": "parity-m10-agent", "MCP_AUTH_TOKEN": "parity-m10-token-not-a-secret-0123456789",
               "OPUTE_AGENT_MODE": "standalone", "HOST_MCP_BIND_HOST": "127.0.0.1", "HOST_MCP_PORT": "3014"}
BOOTSTRAP_ID = "parity-m10-bootstrap"
HOME_ROOTS = ["/home/opute"]
# Binaries are compared through their release receipts, and SQLite files
# through their tables and row counts (the state step), not their bytes.
NO_HASH = ["opute-host-agent$", r"\.sqlite(-wal|-shm)?$", r"\.db(-wal|-shm)?$"]


def install_program(release: str) -> dict:
    token = MACHINE_ENV["MCP_AUTH_TOKEN"]
    tree = {"roots": HOME_ROOTS, "noHashPatterns": NO_HASH}
    return {"steps": [
        {"as": "serve", "serve": {}},
        {"as": "install", "userInstall": {"release": release, "version": VERSION, "env": MACHINE_ENV}},
        {"as": "health", "health": {}},
        {"as": "firstCall", "firstCall": {"token": token}},
        {"as": "unitUp", "unit": {}},
        {"as": "treeUp", "tree": tree},
        {"as": "stop", "stop": {}},
        {"as": "unitDown", "unit": {}},
        {"as": "journal", "journal": {}},
        {"as": "state", "state": {"root": "/home/opute"}},
        {"as": "uninstall", "uninstall": {}},
        {"as": "treeAfter", "tree": tree},
        {"as": "bootInstall", "bootstrapInstall": {"release": release, "version": VERSION}},
        {"as": "bootWait", "sleep": {"seconds": 12}},
        {"as": "bootUnit", "unit": {"scope": "system"}},
        {"as": "bootStop", "stop": {"scope": "system"}},
        {"as": "bootJournal", "journal": {"scope": "system"}},
        {"as": "bootUninstall", "uninstall": {"scope": "system"}},
        {"as": "idInstall", "bootstrapInstall": {"release": release, "version": VERSION, "dropIn": {
            "OPUTE_REMOTE_AGENT_ID": BOOTSTRAP_ID, "MCP_AUTH_TOKEN": token}}},
        {"as": "idHealth", "health": {}},
        {"as": "idFirstCall", "firstCall": {"token": token}},
        {"as": "idUnit", "unit": {"scope": "system"}},
        {"as": "idTree", "tree": {"roots": ["/etc/sudoers.d/opute-host-agent",
                                            "/etc/systemd/system/opute-bootstrap-mcp.service",
                                            "/etc/systemd/system/opute-bootstrap-mcp.service.d"],
                                  "noHashPatterns": NO_HASH}},
        {"as": "idStop", "stop": {"scope": "system"}},
        {"as": "idUnitDown", "unit": {"scope": "system"}},
        {"as": "idJournal", "journal": {"scope": "system"}},
        {"as": "idUninstall", "uninstall": {"scope": "system"}},
        {"as": "treeEnd", "tree": {"roots": HOME_ROOTS + ["/etc/sudoers.d", "/etc/systemd/system"],
                                   "noHashPatterns": NO_HASH}},
    ]}


def upgrade_program(middle: str) -> dict:
    token = MACHINE_ENV["MCP_AUTH_TOKEN"]
    steps: list[dict] = [
        {"as": "serve", "serve": {}},
        {"as": "install", "userInstall": {"release": "go", "version": VERSION, "env": MACHINE_ENV}},
    ]
    for phase, release in (("first", None), ("middle", middle), ("last", "go")):
        if release:
            steps += [{"as": f"{phase}Stop", "stop": {}},
                      {"as": f"{phase}Swap", "swap": {"release": release, "version": VERSION}},
                      {"as": f"{phase}Start", "start": {}}]
        steps += [{"as": f"{phase}Health", "health": {}},
                  {"as": f"{phase}FirstCall", "firstCall": {"token": token}},
                  {"as": f"{phase}Unit", "unit": {}},
                  {"as": f"{phase}State", "state": {"root": "/home/opute"}},
                  {"as": f"{phase}Tree", "tree": {"roots": ["/home/opute/.config/opute"],
                                                  "noHashPatterns": NO_HASH}},
                  {"as": f"{phase}Journal", "journal": {}}]
    steps += [{"as": "stop", "stop": {}}, {"as": "uninstall", "uninstall": {}}]
    return {"steps": steps}


def _go_revision(value: Any) -> Any:
    pairs = json.loads((REPO_ROOT / "parity-manifest.json").read_text())["catalogRevisions"]
    to_go = {p["rust"]: p["go"] for p in pairs.values() if isinstance(p, dict) and p.get("rust")}
    return to_go.get(value, value)


_D8_TABLES = {"pending_authorizations", "consents", "schema_migrations"}


def canonical_machine(obs: dict, sums: dict[str, dict]) -> tuple[dict, list[str]]:
    """One journey's observation with every declared mask applied; the list
    names each mask that changed something (a mask that never applies is
    stale and is reported as such by the caller)."""
    applied: set[str] = set()
    doc = copy.deepcopy(obs)

    def walk(value: Any, key: str = "") -> Any:
        if isinstance(value, dict):
            # Release receipts: the shas name the release set; what is
            # compared is that the downloaded archive and binary are the
            # ones that release set lists.
            if "archiveSha256" in value and "binarySha256" in value:
                release = sums.get(value.get("_release", ""), {})
                out = {k: walk(v, k) for k, v in value.items()
                       if k not in ("archiveSha256", "binarySha256", "_release")}
                out["matchesRelease"] = release.get(value.get("artifact")) == value["archiveSha256"] and \
                    release.get(value["artifact"][:-3]) == value["binarySha256"]
                applied.add("release-receipt")
                return out
            out = {}
            for k, v in value.items():
                # D8: Rust's provisioned credentials and OAuth tables.
                if "/state/credentials" in k:
                    applied.add("D8.credential-files")
                    continue
                if k in _D8_TABLES:
                    applied.add("D8.authz-tables")
                    continue
                if k == "clients" and key.endswith("authz.sqlite"):
                    applied.add("D8.authz-row-counts")
                    continue
                out[k] = walk(v, k)
            if key == "catalog" and isinstance(out.get("revision"), str):
                revision = _go_revision(out["revision"])
                if revision != out["revision"]:
                    applied.add("catalog-revision-pair")
                out["revision"] = revision
            return out
        if isinstance(value, list):
            kept = [walk(v, key) for v in value]
            if key.lower().endswith("journal"):
                dropped = _drop_oauth(kept)
                if len(dropped) != len(kept):
                    applied.add("D8.audit-server-log")
                # systemd logs a unit's "Consumed ... CPU time, ... memory
                # peak" line only when its usage crosses systemd's reporting
                # thresholds: a measure of how much the process used, not
                # of what it did.
                accounted = [line for line in dropped if ".service: Consumed " not in line]
                if len(accounted) != len(dropped):
                    applied.add("systemd-accounting")
                kept = accounted
            return kept
        return value

    return walk(doc), sorted(applied)


def _diff(a: Any, b: Any, path: str = "") -> list[str]:
    if isinstance(a, dict) and isinstance(b, dict):
        out = []
        for k in sorted(set(a) | set(b)):
            if k not in a or k not in b:
                out.append(f"{path}/{k}: only {'rust' if k in b else 'go'}")
            else:
                out += _diff(a[k], b[k], f"{path}/{k}")
        return out
    if a != b:
        return [f"{path}: {json.dumps(a)[:200]} != {json.dumps(b)[:200]}"]
    return []


def release_sums(directory: Path) -> dict:
    """A release set's SHA256SUMS as a map (it lists the plain x64 binary
    too, as Go's does)."""
    return {name.lstrip("*"): digest for digest, name in
            (line.split(None, 1) for line in (directory / "SHA256SUMS").read_text().splitlines())}


def _tag_receipts(obs: dict, program: dict) -> dict:
    """Name the release set each receipt came from (the step's own spec)."""
    for step in program["steps"]:
        (kind, spec), = [(k, v) for k, v in step.items() if k != "as"]
        result = obs["steps"].get(step["as"])
        if isinstance(spec, dict) and "release" in spec and isinstance(result, dict) and "archiveSha256" in result:
            result["_release"] = spec["release"]
    return obs


def identity_checks(obs: dict) -> dict:
    """The upgrade journey's own claims: one explicit identity and token
    across every phase, the env file untouched, and no new identity."""
    steps = obs["steps"]
    phases = ("first", "middle", "last")
    env_path = "/home/opute/.config/opute/host-agent.env"
    env_shas = [steps.get(f"{p}Tree", {}).get(env_path, {}).get("sha256") for p in phases]
    return {
        "agentIdKept": all(steps.get(f"{p}Health", {}).get("body", {}).get("agentId")
                           == MACHINE_ENV["OPUTE_REMOTE_AGENT_ID"] for p in phases),
        "instanceIdKept": len({steps.get(f"{p}Health", {}).get("body", {}).get("instanceId") for p in phases}) == 1,
        "fingerprintKept": len({steps.get(f"{p}Health", {}).get("body", {}).get("fingerprint") for p in phases}) == 1,
        "tokenAccepted": all(steps.get(f"{p}FirstCall", {}).get("toolsList", {}).get("status") == 200
                             for p in phases),
        "wrongTokenRefused": all(steps.get(f"{p}FirstCall", {}).get("wrongToken") == 401 for p in phases),
        "envFileUnchanged": len(set(env_shas)) == 1 and env_shas[0] is not None,
        "noError": "_error" not in steps,
    }


MACHINE_JOURNEYS = ["install-go-1", "install-go-2", "install-rust", "upgrade-go-go-go", "upgrade-go-rust-go"]


def observe_machines(distro: str, releases: dict[str, Path], deploy: Path, run_dir: Path) -> dict:
    """Install journeys for go (twice, the control) and rust, and upgrade
    journeys Go->Go->Go and Go->Rust->Go, each on a fresh twin of one
    distro's golden."""
    golden_record(distro)
    run_dir.mkdir(parents=True, exist_ok=True)
    jobs = {"install-go-1": ({"go": releases["go"]}, install_program("go")),
            "install-go-2": ({"go": releases["go"]}, install_program("go")),
            "install-rust": ({"rust": releases["rust"]}, install_program("rust")),
            "upgrade-go-go-go": ({"go": releases["go"]}, upgrade_program("go")),
            "upgrade-go-rust-go": (releases, upgrade_program("rust"))}

    def one(label: str) -> dict:
        sets, program = jobs[label]
        with CleanTwin(f"{distro}-{label}", distro, run_dir, sets, deploy) as twin:
            return _tag_receipts(twin.run(program), program)

    with concurrent.futures.ThreadPoolExecutor(3) as pool:
        journeys = dict(zip(jobs, pool.map(one, jobs)))
    return {"distro": distro, "golden": golden_record(distro)["goldenSha256"], "journeys": journeys}


def evaluate_machines(obs: dict, sums: dict[str, dict]) -> dict:
    raw = obs["journeys"]
    if sorted(raw) != sorted(MACHINE_JOURNEYS):
        return {"pass": False, "errors": {"_journeys": sorted(raw)}}
    canon: dict[str, Any] = {}
    masks: dict[str, list[str]] = {}
    for label, journey in raw.items():
        canon[label], masks[label] = canonical_machine(journey, sums)
    errors = {label: j["steps"]["_error"] for label, j in raw.items() if "_error" in j["steps"]}
    comparisons = {
        "install go/go": _diff(canon["install-go-1"], canon["install-go-2"]),
        "install go/rust": _diff(canon["install-go-1"], canon["install-rust"]),
        "upgrade go-go-go/go-rust-go": _diff(canon["upgrade-go-go-go"], canon["upgrade-go-rust-go"]),
    }
    identity = {label: identity_checks(raw[label]) for label in ("upgrade-go-go-go", "upgrade-go-rust-go")}
    try:
        expectations = machine_expectations(canon["install-go-1"])
    except (KeyError, TypeError) as error:
        expectations = {"complete": False, "error": str(error)}
    return {"errors": errors, "comparisons": comparisons, "masksApplied": masks, "identity": identity,
            "expectations": expectations,
            "pass": not errors and not any(comparisons.values())
            and all(all(v.values()) for v in identity.values()) and all(v is True for v in expectations.values())}


def machine_expectations(obs: dict) -> dict:
    """What the Go journey itself must show for the comparison to mean
    anything: the user unit healthy and answering, then stopped; the
    published bootstrap unit refusing to start without an identity; the
    bootstrap unit with an identity healthy; nothing left after uninstall."""
    s = obs["steps"]
    return {
        "userUnitActive": s["unitUp"].get("ActiveState") == "active",
        "userUnitStopped": s["unitDown"].get("ActiveState") == "inactive",
        "firstCall": s["firstCall"]["toolsList"]["status"] == 200 and s["firstCall"]["toolsList"]["count"] > 0
        and s["firstCall"]["wrongToken"] == 401 and s["firstCall"]["getHostInfo"]["isError"] is False,
        "receiptVerified": s["install"].get("verified") is True and s["install"].get("matchesRelease") is True,
        "bootstrapRefusesWithoutIdentity": s["bootUnit"].get("ActiveState") != "active"
        and any("OPUTE_REMOTE_AGENT_ID is required" in line for line in s["bootJournal"]),
        "bootstrapWithIdentityHealthy": s["idHealth"].get("body", {}).get("agentId") == BOOTSTRAP_ID,
        "sudoersValid": s["idInstall"].get("visudo") == 0,
        "uninstalled": not any(p.endswith(("opute-host-agent", "host-agent.env", "opute-host-agent.service"))
                               for p in s["treeAfter"]),
    }


def observe_npm(go_src: Path, releases: dict[str, Path], work: Path) -> dict:
    """The packed launcher's unit suite, its journeys against each release
    set (Go twice, the control) and the readonly canary per side."""
    pack = pack_launcher(go_src, work / "pack")
    tarball = Path(pack["tarball"])
    journeys = {label: npm_journey(tarball, releases[side], work / label)
                for label, side in (("go", "go"), ("go2", "go"), ("rust", "rust"))}
    canary = {side: npm_canary(go_src, tarball, work / side / "download" / "agent-cache" / VERSION
                               / "host-agent-linux-x64") for side in ("go", "rust")}
    return {"pack": {k: v for k, v in pack.items() if k != "tarball"}, "unit": npm_unit_tests(go_src),
            "journeys": journeys, "canary": canary}


UNSUPPORTED_MESSAGES = {
    "darwin-arm64": "unsupported local host-agent target: darwin/arm64; supported targets are Linux x64 and arm64 "
                    "(Windows via WSL)",
    "linux-ia32": "unsupported local host-agent target: linux/ia32; supported targets are Linux x64 and arm64 "
                  "(Windows via WSL)",
    "win32-x64": "native Windows is not supported by the Incus host agent; run the Linux binary inside WSL and "
                 "point your MCP client at http://127.0.0.1:3014/mcp",
}


def _npm_started_ok(run: dict) -> bool:
    return (run["start"]["exit"] == 0 and run["health"]["agentId"] == NPM_AGENT_ID and run["health"]["ownsInstance"]
            and run["status"] == {"exit": 0, "running": True, "healthy": True}
            and run["toolsList"]["status"] == 200 and run["toolsList"]["count"] > 0
            and run["getHostInfo"]["status"] == 200 and run["getHostInfo"]["isError"] is False
            and run["wrongToken"] == 401 and run["stop"]["exit"] == 0)


def npm_journey_checks(j: dict) -> dict:
    sums, gz = f"/v{VERSION}/SHA256SUMS", f"/v{VERSION}/host-agent-linux-x64.gz"
    d, m, c = j["download"], j["mismatch"], j["concurrent"]
    return {
        "download": _npm_started_ok(d["first"]) and d["firstRequests"] == [sums, gz]
        and all(d["marker"][k] is True for k in ("archiveMatchesSums", "binaryMatchesRelease", "cachedMatchesRelease")),
        "cacheReused": _npm_started_ok(d["second"]) and d["secondRequests"] == [sums],
        "cacheModes": d["cache"] == {VERSION: {"type": "dir", "mode": "0o700"},
                                     f"{VERSION}/host-agent-linux-x64": {"type": "file", "mode": "0o700"},
                                     f"{VERSION}/host-agent-linux-x64.verified.json": {"type": "file", "mode": "0o600"}},
        "checksumMismatchRejected": m["start"]["exit"] == 1
        and m["start"]["stderr"].splitlines()[:1] == ["host-agent artifact checksum mismatch"]
        and m["requests"] == [sums, gz] and not any(k.startswith(f"{VERSION}/") for k in m["cache"])
        and m["listening"] is False,
        "concurrentLock": [s["exit"] for s in c["starts"]] == [0, 0] and c["healthy"] == [True, True]
        and c["stops"] == [0, 0] and j["concurrentPrewarm"] == 0 and c["lockSeen"] is True and c["lockLeft"] is False and c["archiveDownloads"] == 1,
        "unsupportedPlatform": {k: v for k, v in j["unsupported"].items()} ==
        {k: {"exit": 1, "stderr": msg} for k, msg in UNSUPPORTED_MESSAGES.items()},
    }


def evaluate_npm(obs: dict) -> dict:
    unit = obs["unit"]
    checks = {
        "unitSuite": unit["exit"] == 0 and unit["counts"].get("fail") == 0 and unit["counts"].get("pass", 0) > 0,
        **{f"canary-{side}": obs["canary"][side]["exit"] == 0 and obs["canary"][side]["counts"].get("pass") == 1
           and obs["canary"][side]["counts"].get("fail") == 0 for side in ("go", "rust")},
    }
    for label, journey in obs["journeys"].items():
        for name, ok in npm_journey_checks(journey).items():
            checks[f"{label}-{name}"] = ok
    diffs = {"go/go": _diff(obs["journeys"]["go"], obs["journeys"]["go2"]),
             "go/rust": _diff(obs["journeys"]["go"], obs["journeys"]["rust"])}
    return {"checks": checks, "diffs": diffs, "pass": all(checks.values()) and not any(diffs.values())}


STABLE_REFUSAL = "a stable package version cannot publish a different catalog revision"


def evaluate_site(obs: dict, reviewed: list[str]) -> dict:
    """Go's own check-site passes on the committed site from Go's binary
    (the control); Go's release policy refuses the Rust catalog for the
    stable 0.2.3 package (a finding, recorded as expected); the preview
    sites differ in exactly the reviewed files, and not at all once Rust's
    catalog is spelled the Go way by the declared D19-D24 rule."""
    go = obs["committed-go"]
    rust = obs["committed-rust"]["steps"]
    checks = {
        "goCheckSite": all(step["exit"] == 0 for step in go["steps"].values())
        and set(go["steps"]) == {"captureTest", "capture", "generate", *SITE_CHECKS} and go["status"] == [],
        "rustStableRefused": rust["captureTest"]["exit"] == 0 and rust["capture"]["exit"] == 1
        and rust["capture"]["last"] == STABLE_REFUSAL and obs["committed-rust"]["status"] == [],
        "previewsGenerate": all(obs[k][step]["exit"] == 0 for k in ("preview-go", "preview-rust")
                                for step in ("capture", "generate"))
        and obs["preview-rust-as-go"]["generate"]["exit"] == 0,
        "previewDiffReviewed": obs["previewDiff"] == sorted(reviewed),
        "previewAsGoByteForByte": obs["previewAsGoDiff"] == [] and obs["catalogAsGoEqual"] is True
        and obs["propertiesSorted"] == {"go": True, "rust": True},
    }
    return {"checks": checks, "pass": all(checks.values())}


# --- one round ----------------------------------------------------------------------
#
# A round writes one receipt per place and a summary pinning what produced
# them: the pinned Go tree, both release sets (by their SHA256SUMS), the
# goldens, and the harness files whose content decides the observations.
# tools/parity/parity/dist_verify.py re-derives every result from the
# receipts; editing a harness file after the capture invalidates it.
HARNESS_FILES = ["tools/parity/parity/dist.py", "tools/parity/t2guest/distjourney.py",
                 "tools/release/build-artifacts.sh", "tools/parity/divergences.json",
                 "crates/host-agent/catalog/neutral-names.json", "crates/host-agent/catalog/write-only.json"]
DISTROS = ["ubuntu", "debian"]


def harness_hashes() -> dict:
    hashes = {path: t2.sha256_file(REPO_ROOT / path) for path in HARNESS_FILES}
    revisions = json.loads((REPO_ROOT / "parity-manifest.json").read_text())["catalogRevisions"]
    hashes["parity-manifest.json#catalogRevisions"] = hashlib.sha256(
        json.dumps(revisions, sort_keys=True).encode()).hexdigest()
    return hashes


def release_record(directory: Path) -> dict:
    text = (directory / "SHA256SUMS").read_text()
    return {"sha256sums": hashlib.sha256(text.encode()).hexdigest(), "text": text,
            "verified": _verify_sums(directory)["verified"]}


def run_round(out: Path, go_src: Path, releases: dict[str, Path], deploy: Path, work: Path,
              reviewed_site: list[str], distros: list[str] = DISTROS) -> dict:
    out.mkdir(parents=True, exist_ok=False)
    work.mkdir(parents=True, exist_ok=True)
    head = subprocess.run(["git", "-C", str(go_src), "rev-parse", "HEAD"], capture_output=True, text=True,
                          check=True).stdout.strip()
    before = {"harness": harness_hashes(), "releases": {k: release_record(v) for k, v in releases.items()}}
    started = time.time()
    receipts: dict[str, Any] = {
        "releases": before["releases"],
        "artifacts": observe_artifacts(releases["go"], releases["rust"]),
        "logs": observe_logs(releases["go"] / "host-agent-linux-x64", releases["rust"] / "host-agent-linux-x64"),
        "npm": observe_npm(go_src, releases, work / "npm"),
        "site": observe_site(go_src, releases["go"] / "host-agent-linux-x64",
                             releases["rust"] / "host-agent-linux-x64", work / "site"),
    }
    for distro in distros:
        receipts[f"machines-{distro}"] = observe_machines(distro, releases, deploy, work / f"machines-{distro}")
    after = {"harness": harness_hashes(), "releases": {k: release_record(v) for k, v in releases.items()}}
    hashes = {}
    for name, receipt in receipts.items():
        path = out / f"{name}.json"
        path.write_text(json.dumps(receipt, indent=1, sort_keys=True) + "\n")
        hashes[name] = t2.sha256_file(path)
    summary = {"tier": "dist", "version": VERSION, "goSourceCommit": head, "harness": before["harness"],
               "releases": {k: v["sha256sums"] for k, v in before["releases"].items()},
               "goldens": {d: golden_record(d)["goldenSha256"] for d in distros},
               "launcherTarballSha256": receipts["npm"]["pack"]["sha256"],
               "changedDuringRun": before != after, "seconds": round(time.time() - started),
               "receipts": hashes}
    (out / "summary.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
    results = evaluate_round(receipts, reviewed_site, distros)
    (out / "results.json").write_text(json.dumps(results, indent=1, sort_keys=True) + "\n")
    return {"summary": summary, "pass": {name: r["pass"] for name, r in results.items()}}


def evaluate_round(receipts: dict[str, Any], reviewed_site: list[str], distros: list[str]) -> dict:
    releases = receipts["releases"]
    sums = {side: {name.lstrip("*"): digest for digest, name in
                   (line.split(None, 1) for line in record["text"].splitlines())}
            for side, record in releases.items()}
    results = {
        "releases": {"checks": {side: record["verified"] is True for side, record in releases.items()},
                     "pass": all(record["verified"] is True for record in releases.values())},
        "artifacts": evaluate_artifacts(receipts["artifacts"]),
        "logs": evaluate_logs(receipts["logs"]),
        "npm": evaluate_npm(receipts["npm"]),
        "site": evaluate_site(receipts["site"], reviewed_site),
    }
    for distro in distros:
        results[f"machines-{distro}"] = evaluate_machines(receipts[f"machines-{distro}"], sums)
    return results
