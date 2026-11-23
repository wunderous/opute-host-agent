"""m25's e2e-0 probes: one planted, undeclared change per kind the observer
did not see before m25. Each must fail with `undeclared-effect` alone.

    python3 make_m25_probes.py   # writes selfcheck.unseen-*.json next to this file
"""
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from make_e2e4a import OPUTE_UID, USER_SESSION  # noqa: E402

AGENT = {"agent": "the dev VM's production-style system unit (opute-bootstrap-mcp.service), test identity "
                  "e2e-devvm-agent, test bearer token"}
SESSION = "processes//user.slice/user-0.slice/session.scope"
WAIT_BUS = f"for i in $(seq 50); do [ -S /run/user/{OPUTE_UID}/bus ] && break; sleep 0.2; done"
# the manager stops asynchronously once linger is off
STOP_MANAGER = (f"loginctl disable-linger opute; systemctl stop user@{OPUTE_UID}.service; "
                f"for i in $(seq 50); do systemctl -q is-active user-{OPUTE_UID}.slice || break; sleep 0.2; done; true")
PROBES = [
    ("root", "a file under /root", "touch /root/opute-e2e-probe", "rm -f /root/opute-e2e-probe", []),
    ("home", "a home directory of a user the observer does not know", "install -d /home/opute-e2e-probe",
     "rmdir /home/opute-e2e-probe", []),
    ("tmp", "a file under /tmp", "touch /tmp/opute-e2e-probe", "rm -f /tmp/opute-e2e-probe", []),
    ("srv", "a directory under /srv", "install -d /srv/opute-e2e-probe", "rmdir /srv/opute-e2e-probe", []),
    ("cron", "a crontab", "echo '# m25 probe' | crontab -u opute -", "crontab -u opute -r", []),
    ("var-lib", "a directory under /var/lib", "install -d /var/lib/opute-e2e-probe",
     "rmdir /var/lib/opute-e2e-probe", []),
    ("user-unit", "a unit left running in the opute user's manager (the manager itself is declared)",
     f"loginctl enable-linger opute && {WAIT_BUS} && systemd-run --user -M opute@ --unit=e2e-probe sleep 3602",
     f"systemctl --user -M opute@ stop e2e-probe; {STOP_MANAGER}",
     USER_SESSION + [f"processes//user.slice/user-{OPUTE_UID}.slice/user@{OPUTE_UID}.service/"
                     "app.slice/e2e-probe.service sleep"]),
    ("process", "a process left in the step's ssh session", "setsid -f sleep 3601 </dev/null >/dev/null 2>&1",
     "pkill -x -f 'sleep 3601'; true", []),
    ("socket", "a listening socket (its process is declared)",
     "setsid -f python3 -m http.server --bind 127.0.0.1 3999 </dev/null >/dev/null 2>&1; "
     "for i in $(seq 50); do ss -ltnH 'sport = :3999' | grep -q . && break; sleep 0.2; done",
     "pkill -f 'http.server --bind 127.0.0.1 3999'; true", [f"{SESSION} python3"]),
    ("sysctl", "a runtime sysctl", "sysctl -qw vm.swappiness=37", "sysctl -qw vm.swappiness=60", []),
    ("module", "a loaded kernel module", "modprobe tcp_bbr", "modprobe -r tcp_bbr; true",
     ["sysctl/net.ipv4.tcp_allowed_congestion_control", "sysctl/net.ipv4.tcp_available_congestion_control"]),
    ("route", "a route", "ip route add blackhole 198.51.100.0/24", "ip route del blackhole 198.51.100.0/24; true", []),
    ("mount", "a mounted filesystem", "mount -t tmpfs -o size=1m m25-probe /mnt", "umount /mnt",
     ["system/units/mnt.mount"]),
    ("link", "a network link", "ip link add m25probe type dummy", "ip link del m25probe; modprobe -r dummy",
     ["modules/dummy", "system/units/sys-devices-virtual-net-m25probe.device",
      "system/units/sys-subsystem-net-devices-m25probe.device"]),
]


def probe(name: str, what: str, step: str, cleanup: str, allow: list) -> dict:
    effects = {"allow": list(allow), "persist": []}
    if name in ("mount", "link"):
        effects["optional"] = {pattern: "systemd may still be loading the unit or the module may already be present"
                               for pattern in allow}
    return {"id": f"selfcheck.unseen-{name}", "gate": "e2e-0",
            "description": f"Negative control for m25: {what}, undeclared. The observer before m25 did not see it.",
            "inputs": AGENT,
            "requirements": [{"spec": "evidence/e2e/README.md", "requirement": f"Harness: {what} is an effect",
                              "pass": [{"step": "plant", "path": "exit", "equals": 0}]}],
            "effects": effects,
            "steps": [{"id": "plant", "guest": step}],
            "cleanup": [{"id": "remove", "guest": cleanup}],
            "maxMinutes": 10}


def render() -> dict[str, str]:
    return {f"selfcheck.unseen-{p[0]}.json": json.dumps(probe(*p), indent=1) + "\n" for p in PROBES}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
