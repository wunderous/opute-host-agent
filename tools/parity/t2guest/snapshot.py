"""T2 effect snapshot, taken as root inside a disposable twin VM.

One snapshot records what a host-domain operation can change on a Linux
host: the watched file trees (type, mode, owner, sha256, symlink target),
the system and user service managers (unit files, loaded units, and the
stable `systemctl show` properties of every unit that is not in the
golden image), user lingering, and the firewall rule sets. Timestamps,
PIDs, sizes of logs and other run-local values are never recorded, so two
twins that did the same thing produce the same snapshot.

This file runs on the guest's own python3 (Ubuntu 24.04) with no imports
beyond the standard library.
"""
from __future__ import annotations

import hashlib
import os
import pwd
import grp
import stat
import subprocess

# Trees an operation under test may change. The agent's own binary, state
# and logs live under AGENT_ROOT, which is excluded: they are the subject's
# process, not its external effect.
ROOTS = ["/etc", "/home/ubuntu", "/opt", "/usr/local", "/usr/bin", "/usr/sbin", "/usr/lib/systemd",
         "/var/lib/systemd/linger", "/var/lib/incus", "/var/lib/dpkg/info"]
AGENT_ROOT = "/opt/parity-t2"
# Paths that change on every boot or by the guest's own housekeeping,
# identically meaningless in both twins.
EXCLUDE_PREFIXES = [
    AGENT_ROOT,
    "/etc/ld.so.cache",
    "/home/ubuntu/.cache",
    "/home/ubuntu/.local/state/opute-host-agent",
    "/etc/.resolv.conf.systemd-resolved.bak",
    "/var/lib/incus/database",
    "/var/lib/incus/logs",
    # Incus's own runtime and storage trees: what an operation does to them is
    # recorded through the incus() projection, not as file churn.
    "/var/lib/incus/storage-pools",
    "/var/lib/incus/disks",
    "/var/lib/incus/devices",
    "/var/lib/incus/shmounts",
    "/var/lib/incus/guestapi",
    "/var/lib/incus/security",
    "/var/lib/incus/networks",
    "/var/lib/incus/images",
    "/var/lib/incus/backups",
    "/var/lib/incus/unix.socket",
    "/var/lib/incus/containers",
    "/var/lib/incus/virtual-machines",
]
EXCLUDE_NAMES = {"__pycache__"}
SHOW_PROPERTIES = ["LoadState", "ActiveState", "SubState", "UnitFileState", "FragmentPath", "Slice",
                   "Description", "Type", "Restart", "KillMode", "EnvironmentFiles", "WantedBy",
                   "CPUQuotaPerSecUSec", "CPUWeight", "MemoryMax", "MemoryHigh", "TasksMax", "IOWeight"]
# Dependency lists systemd prints in its own internal (unordered) order.
UNORDERED_PROPERTIES = {"WantedBy"}
USER_UID = 1000


def _excluded(path: str) -> bool:
    return any(path == p or path.startswith(p + "/") for p in EXCLUDE_PREFIXES) or \
        os.path.basename(path) in EXCLUDE_NAMES


def _owner(uid: int, gid: int) -> str:
    try:
        user = pwd.getpwuid(uid).pw_name
    except KeyError:
        user = str(uid)
    try:
        group = grp.getgrgid(gid).gr_name
    except KeyError:
        group = str(gid)
    return f"{user}:{group}"


def _sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def files() -> dict:
    out = {}
    for root in ROOTS:
        if not os.path.lexists(root):
            continue
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            dirnames[:] = sorted(d for d in dirnames if not _excluded(os.path.join(dirpath, d)))
            for name in [""] + sorted(filenames) + [d for d in dirnames if os.path.islink(os.path.join(dirpath, d))]:
                path = dirpath if name == "" else os.path.join(dirpath, name)
                if _excluded(path) or path in out:
                    continue
                try:
                    st = os.lstat(path)
                except FileNotFoundError:
                    continue
                entry = {"mode": oct(stat.S_IMODE(st.st_mode)), "owner": _owner(st.st_uid, st.st_gid)}
                if stat.S_ISLNK(st.st_mode):
                    entry.update(type="symlink", target=os.readlink(path))
                elif stat.S_ISDIR(st.st_mode):
                    entry["type"] = "dir"
                elif stat.S_ISREG(st.st_mode):
                    entry["type"] = "file"
                    try:
                        entry["sha256"] = _sha256(path)
                    except OSError as e:
                        entry["sha256"] = f"unreadable: {e.strerror}"
                else:
                    entry["type"] = "other"
                out[path] = entry
    return out


def _run(argv: list[str], user: bool = False) -> tuple[int, str]:
    env = dict(os.environ)
    if user:
        runtime = f"/run/user/{USER_UID}"
        env.update(XDG_RUNTIME_DIR=runtime, DBUS_SESSION_BUS_ADDRESS=f"unix:path={runtime}/bus")
        argv = ["setpriv", f"--reuid={USER_UID}", f"--regid={USER_UID}", "--init-groups"] + argv
    p = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=60)
    return p.returncode, p.stdout


def _manager(user: bool) -> dict:
    if user and not os.path.exists(f"/run/user/{USER_UID}/bus"):
        return {"running": False}
    base = ["systemctl"] + (["--user"] if user else [])
    _, unit_files = _run(base + ["list-unit-files", "--no-legend", "--plain", "--no-pager"], user)
    _, units = _run(base + ["list-units", "--all", "--no-legend", "--plain", "--no-pager"], user)
    files_ = {}
    for line in unit_files.splitlines():
        parts = line.split()
        if len(parts) >= 2:
            files_[parts[0]] = parts[1]
    loaded = {}
    for line in units.splitlines():
        parts = line.split(None, 4)
        if len(parts) >= 4:
            loaded[parts[0]] = {"load": parts[1], "active": parts[2], "sub": parts[3]}
    return {"running": True, "unitFiles": files_, "units": loaded}


def show(units: list[str], user: bool) -> dict:
    out = {}
    base = ["systemctl"] + (["--user"] if user else [])
    for unit in units:
        _, text = _run(base + ["show", unit, "--no-pager", "-p", ",".join(SHOW_PROPERTIES)], user)
        props = dict(line.split("=", 1) for line in text.splitlines() if "=" in line)
        for key in UNORDERED_PROPERTIES & props.keys():
            props[key] = " ".join(sorted(props[key].split()))
        out[unit] = props
    return out


def firewall() -> dict:
    out = {}
    for name, argv in (("nft", ["nft", "list", "ruleset"]), ("iptables", ["iptables-save"]),
                       ("ip6tables", ["ip6tables-save"])):
        try:
            code, text = _run(argv)
        except FileNotFoundError:
            out[name] = None
            continue
        # iptables-save prints generation comments with a timestamp.
        out[name] = [line for line in text.splitlines() if not line.startswith("#")] if code == 0 else None
    return out


# Fields of each Incus object that are stable across twins; timestamps,
# volatile.* keys (MACs, UUIDs, last state), runtime state and the image
# server's update times are left out.
INCUS_FIELDS = {
    "instances": ("name", "type", "status", "architecture", "profiles", "config", "devices", "ephemeral",
                  "stateful", "description"),
    "profiles": ("name", "config", "devices", "used_by", "description"),
    "networks": ("name", "type", "managed", "config", "used_by", "status", "description"),
    "storage": ("name", "driver", "config", "used_by", "status", "description"),
    "images": ("fingerprint", "aliases", "properties", "type", "public", "cached", "auto_update", "architecture"),
}


def _stable(obj: dict, fields: tuple) -> dict:
    out = {k: obj.get(k) for k in fields if k in obj}
    if isinstance(out.get("config"), dict):
        out["config"] = {k: v for k, v in sorted(out["config"].items()) if not k.startswith("volatile.")}
    if isinstance(out.get("used_by"), list):
        out["used_by"] = sorted(out["used_by"])
    if isinstance(out.get("aliases"), list):
        out["aliases"] = sorted(a.get("name", "") for a in out["aliases"])
    return out


def _incus_json(argv: list[str]):
    import json
    code, text = _run(["incus"] + argv + ["--format", "json"])
    return json.loads(text) if code == 0 and text.strip() else None


def incus() -> dict | None:
    """Incus objects by kind and name, or None when Incus is not installed."""
    if not os.path.exists("/var/lib/incus/unix.socket"):
        return None
    out = {}
    for kind, argv in (("instances", ["list"]), ("profiles", ["profile", "list"]),
                       ("networks", ["network", "list"]), ("storage", ["storage", "list"]),
                       ("images", ["image", "list"])):
        items = _incus_json(argv) or []
        key = "fingerprint" if kind == "images" else "name"
        out[kind] = {item[key]: _stable(item, INCUS_FIELDS[kind]) for item in items}
    volumes = {}
    for pool in out["storage"]:
        for vol in _incus_json(["storage", "volume", "list", pool]) or []:
            name = f"{pool}/{vol.get('type')}/{vol.get('name')}"
            volumes[name] = _stable(vol, ("name", "type", "content_type", "config", "used_by"))
    out["volumes"] = volumes
    return out


def snapshot() -> dict:
    return {
        "files": files(),
        "system": _manager(False),
        "user": _manager(True),
        "linger": sorted(os.listdir("/var/lib/systemd/linger")) if os.path.isdir("/var/lib/systemd/linger") else [],
        "firewall": firewall(),
        "incus": incus(),
    }
