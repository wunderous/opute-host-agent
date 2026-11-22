#!/usr/bin/env python3
"""Dev VM state, taken as root inside the E2E dev VM.

Reuses the T2 effect snapshot (snapshot.py, copied next to this file) with
the dev VM's roots: the production-style agent lives under /home/opute and
/etc/opute, and E2E guests live in the Incus project opute-e2e. The agent's
own state directory and caches are excluded: they are the subject's process,
judged through its MCP results, not an external effect. Prints one JSON
document.
"""
import json
import os
import pwd
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import snapshot  # noqa: E402

PROJECT = "opute-e2e"
# m25: every home, root's, the temporary and service trees, cron and the
# rest of /var/lib, not only the trees an operation was expected to touch.
snapshot.ROOTS = [r for r in snapshot.ROOTS if r != "/home/ubuntu" and not r.startswith("/var/lib/")] + [
    "/home", "/root", "/srv", "/tmp", "/var/tmp", "/var/spool", "/var/lib"]
snapshot.EXCLUDE_PREFIXES = snapshot.EXCLUDE_PREFIXES + [
    "/opt/opute-e2e/guest",
    "/home/opute/.local/state",
    "/home/opute/.cache",
    # The default instance root (state database, instance lock) and the
    # host resource coordinator's reservations (e2e-0-r1, retired).
    "/home/opute/.config/opute/instances",
    "/home/opute/.config/opute/host-resource-coordinator",
    # m25, measured across two restores: rewritten on every boot or by the
    # guest's own timers, the same in every run and attributable to none.
    "/var/lib/cloud",
    "/var/lib/systemd/random-seed",
    "/var/lib/systemd/timers",
    "/var/lib/landscape",
    "/var/lib/logrotate",
    "/var/lib/apt/periodic",
    "/var/lib/update-notifier",
    "/var/lib/ubuntu-advantage",
    # FUSE views of /proc (lxcfs), not files.
    "/var/lib/incus-lxcfs",
    "/var/lib/lxcfs",
]
# A service with PrivateTmp gets a fresh, randomly named directory in each
# boot; its unit is recorded, so its directory is not.
PRIVATE_TMP = re.compile(r"^/(var/)?tmp/systemd-private-[0-9a-f]{32}-")
_excluded = snapshot._excluded
snapshot._excluded = lambda path: _excluded(path) or bool(PRIVATE_TMP.match(path))
# Counters, random values, per-boot ids, memory autotuned at boot, and the
# TCP ULPs, which list the loaded modules recorded under `modules`; interfaces are recorded under
# `links`, so their per-interface subtrees are not repeated here.
VOLATILE_SYSCTL = re.compile(
    r"^(fs[.](dentry-state|file-nr|inode-nr|inode-state|aio-nr|quota[.].*)|kernel[.](ns_last_pid|pty[.]nr|random[.].*)"
    r"|net[.]ipv4[.](tcp_fastopen_key|tcp_mem|udp_mem|tcp_available_ulp)|net[.]netfilter[.]nf_conntrack_count"
    r"|net[.]ipv[46][.](conf|neigh)[.](?!all[.]|default[.]).*)$")


OPUTE = "opute"


def keyed(items: list[str]) -> dict[str, int]:
    """A list as `{item: count}`, so that the effects diff names each item."""
    out = {}
    for item in items:
        out[item] = out.get(item, 0) + 1
    return out


def processes() -> list[str]:
    """Every userspace process as `<cgroup> <comm>`, sorted, with a session
    scope's number dropped and the observer's own ssh session left out. A
    container's own processes are its state, recorded under `incus`: each
    running container is one `/lxc.payload.<name> guest`."""
    own, pid = set(), os.getpid()
    while pid > 1:
        own.add(pid)
        try:
            with open(f"/proc/{pid}/stat") as f:
                pid = int(f.read().rsplit(")", 1)[1].split()[1])
        except OSError:
            break
    out, guests = [], set()
    for entry in os.listdir("/proc"):
        if not entry.isdigit() or int(entry) in own:
            continue
        try:
            with open(f"/proc/{entry}/cmdline", "rb") as f:
                if not f.read(1):
                    continue  # a kernel thread
            with open(f"/proc/{entry}/comm") as f:
                comm = f.read().strip()
            if comm == "(udev-worker)":
                continue  # udev forks one per device event and reaps it seconds later
            with open(f"/proc/{entry}/cgroup") as f:
                cgroup = f.read().strip().split("::", 1)[-1]
        except OSError:
            continue
        guest = re.match(r"/lxc[.]payload[.][^/]+", cgroup)
        if guest:
            guests.add(f"{guest.group(0)} guest")
            continue
        out.append(f"{re.sub(r'session-[0-9]+[.]scope', 'session.scope', cgroup)} {comm}")
    return sorted(out + list(guests))


def inspect(argv: list[str]) -> list[str]:
    code, text = snapshot._run(argv)
    if code != 0:
        raise RuntimeError(f"observer command {argv!r} exited {code}")
    return text.splitlines()


def sockets() -> list[str]:
    """Listening TCP and UDP sockets as `<proto> <address:port> <process>`.
    UDP sockets in the ephemeral range are left out because `ss` also lists
    the dynamically allocated sockets opened by DNS and NTP clients."""
    with open("/proc/sys/net/ipv4/ip_local_port_range") as f:
        low, high = (int(x) for x in f.read().split())
    out = []
    for line in inspect(["ss", "-Hlntup"]):
        cols = line.split()
        if cols[0] == "udp" and low <= int(cols[4].rsplit(":", 1)[1]) <= high:
            continue
        name = re.search(r'"([^"]+)"', line)
        out.append(f"{cols[0]} {cols[4]} {name.group(1) if name else '-'}")
    return sorted(out)


def sysctls() -> dict:
    out = {}
    for dirpath, _, filenames in os.walk("/proc/sys"):
        for name in filenames:
            path = os.path.join(dirpath, name)
            try:
                with open(path) as f:
                    key = path[len("/proc/sys/"):].replace("/", ".")
                    if not VOLATILE_SYSCTL.match(key):
                        out[key] = f.read().strip()
            except OSError:
                continue
    return out


def modules() -> list[str]:
    with open("/proc/modules") as f:
        return sorted(line.split()[0] for line in f)


def mounts() -> list[str]:
    with open("/proc/self/mountinfo") as f:
        return sorted({f"{line.split()[4]} {line.split(' - ')[1].split()[0]}" for line in f})


def routes() -> list[str]:
    # `expires` counts down and `linkdown` follows a bridge's carrier (links)
    return sorted(re.sub(r" expires [0-9]+sec| linkdown", "", line).strip()
                  for line in inspect(["ip", "-o", "route", "show", "table", "all"]))


def links() -> list[str]:
    return sorted(line.split(":")[1].strip().split("@")[0]
                  for line in inspect(["ip", "-o", "link", "show"]))


MANAGER_KINDS = ("service", "socket", "timer", "path", "scope")


def opute_manager() -> dict:
    """The opute user's own manager (the observer's `user` is ubuntu's): its
    active services, sockets, timers, paths and scopes. Its devices, mounts
    and targets mirror the system's, and its unit files are files under the
    observed roots."""
    try:
        uid = pwd.getpwnam(OPUTE).pw_uid
    except KeyError:
        return None
    ubuntu, snapshot.USER_UID = snapshot.USER_UID, uid
    try:
        manager = snapshot._manager(True)
    finally:
        snapshot.USER_UID = ubuntu
    units = {name: unit["sub"] for name, unit in manager.get("units", {}).items()
             if name.rsplit(".", 1)[-1] in MANAGER_KINDS and unit["active"] != "inactive"}
    return {"running": manager["running"], "units": units}


def project() -> dict:
    """The E2E project's own objects, keyed as snapshot.incus() keys them."""
    out = {}
    for kind, argv in (("instances", ["list"]), ("profiles", ["profile", "list"])):
        items = snapshot._incus_json(argv + ["--project", PROJECT]) or []
        out[kind] = {item["name"]: snapshot._stable(item, snapshot.INCUS_FIELDS[kind]) for item in items}
    _, text = snapshot._run(["incus", "project", "show", PROJECT])
    out["project"] = text
    return out


def nft_blocks(lines: list[str] | None) -> list[str] | None:
    """The nft ruleset with each table's chains and sets in name order.

    Incus creates one chain set per managed network as it brings networks
    up, in no fixed order across boots (e2e-0-r2, retired); the rules in
    each chain are kept exactly as listed.
    """
    if lines is None:
        return None
    out, blocks, current, in_table = [], [], None, False
    for line in lines:
        if line.startswith("table "):
            in_table = True
            out.append(line)
        elif in_table and line == "}":
            out += [row for block in sorted(blocks) for row in block]
            out.append(line)
            blocks, in_table = [], False
        elif in_table and line.startswith("\t") and not line.startswith("\t\t") and line.endswith("{"):
            current = [line]
        elif in_table and current is not None:
            current.append(line)
            if line == "\t}":
                blocks.append(current)
                current = None
        elif not (in_table and line == ""):
            out.append(line)
    return out


if __name__ == "__main__":
    state = snapshot.snapshot()
    state["firewall"]["nft"] = nft_blocks(state["firewall"].get("nft"))
    state["e2eProject"] = project() if state.get("incus") is not None else None
    state.update({kind: keyed(items()) for kind, items in (
        ("processes", processes), ("sockets", sockets), ("modules", modules), ("mounts", mounts),
        ("routes", routes), ("links", links))})
    state.update(sysctl=sysctls(), opute=opute_manager())
    json.dump(state, sys.stdout, sort_keys=True)
