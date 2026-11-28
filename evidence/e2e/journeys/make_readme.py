#!/usr/bin/env python3
"""Write the README journey (m33) as JSON.

  python3 evidence/e2e/journeys/make_readme.py

A newcomer follows README.md as written, against the published @opute/host-agent (npm `latest`) and the
published GitHub release, with no candidate binary anywhere. The newcomer is a fresh sudo user who logs in over
SSH, as the README's slice note assumes: one held login session keeps the user manager up, as a user's own
terminal would, and each README step runs in a login of its own. The scripted MCP calls are the ones an assistant
makes for the README's two questions. The VM's own agent is stopped for the run, because the README's port is
3014 and that agent listens there; cleanup starts it again.

Declared deviation: `MCP_AUTH_TOKEN` is the harness's per-run random secret instead of `$(openssl rand -hex 32)`,
so the harness can call the agent; `openssl rand -hex 32` is run on its own to show it yields 64 hex digits.
"""
from __future__ import annotations

import json
import shlex

import make_e2e1 as base
import make_e2e4a as e4
from make_e2e1 import HERE, T, TASKS_META, a, inp, ok, req
from make_e2e3a import call, direct, res, task

GOAL = ".agents/goals/m33-readme-as-written.md (work 2)"
README = "README.md at the commit under test (m33)"
VERSION = "0.5.4"
# the published v0.5.4 x64 binary (rust-0.5.4-rc3's file, journey e2e8.local-llm's binary)
BINARY_SHA = "3b74ecc8ef848ee1c03d23e972537735164ff4a446fbf251f0e024c87e7126d6"
RELEASE = f"https://github.com/wunderous/opute-host-agent/releases/download/v{VERSION}"
TOOLS = 138

U, H = "newcomer", "/home/newcomer"
QS = "/opt/opute-e2e/guest/qs"
NODE = "/opt/opute-e2e/node"
VM_AGENT = "opute-bootstrap-mcp.service"
ON = {"port": 3014, "auth": "secret:qs"}
SSH = (f"ssh -i {QS}/id -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile={QS}/known_hosts "
       f"-o LogLevel=ERROR {U}@127.0.0.1")


def login(script: str, shell: bool = True) -> str:
    """One SSH login as the newcomer running `script`; Node is on PATH as an installed Node would be, and
    `shell` re-reads the two exports step 1 made, as the README's "same shell" keeps them."""
    pre = f"export PATH={NODE}/bin:$PATH; " + (". ~/.qs-shell; " if shell else "")
    return f"{SSH} {shlex.quote(pre + script)}"


START = ('export OPUTE_REMOTE_AGENT_ID="local-$(openssl rand -hex 8)"   # this agent\'s identity\n'
         'export MCP_AUTH_TOKEN="${SECRET:qs}"                           # the bearer token (harness secret)\n'
         "npx -y @opute/host-agent start --background\n"
         "(umask 077; export -p | grep -E ' (OPUTE_REMOTE_AGENT_ID|MCP_AUTH_TOKEN)=' > ~/.qs-shell)")
RESTART = "npx @opute/host-agent stop\nnpx -y @opute/host-agent start --background --allow-mutations"
PID = "ss -ltnpH 'sport = :3014' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2"
# every tool in the live list declares readOnlyHint (and a non-read tool destructiveHint), as "Safe by default" says
HINTS = ("python3 -c 'import sys, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
         "tools = mcpwire.mcp(3014, sys.argv[1], \"tools/list\")[\"body\"][\"result\"][\"tools\"]\n"
         "ann = [t.get(\"annotations\") or {} for t in tools]\n"
         "print(json.dumps({\"tools\": len(tools), \"withoutReadOnlyHint\": sum(\"readOnlyHint\" not in x for x in ann), "
         "\"writeWithoutDestructiveHint\": sum(x.get(\"readOnlyHint\") is False and \"destructiveHint\" not in x "
         "for x in ann)}))' '${SECRET:qs}'")
NO_NPM = (f"mkdir -p ~/rel && cd ~/rel && curl -fsSLO {RELEASE}/host-agent-linux-x64 && "
          f"curl -fsSLO {RELEASE}/SHA256SUMS && sha256sum -c --ignore-missing SHA256SUMS && "
          "chmod +x host-agent-linux-x64 && ./host-agent-linux-x64 --version && "
          "(nohup ./host-agent-linux-x64 > agent.log 2>&1 < /dev/null &) && "
          "for i in $(seq 60); do curl -fsS -o /dev/null http://127.0.0.1:3014/health && break; sleep 0.5; done; "
          "curl -s -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:3014/health")
UNIT_DIR = f"{H}/.config/systemd/user"
SLICE = f"{UNIT_DIR}/opute-workload.slice"
SANDBOX = "vm:local:sandbox"


def readme() -> dict:
    steps = base.TOOL_STEPS + [
        {"id": "nodeCopy", "copy": "node", "to": "/opt/opute-e2e/node.tar.xz"},
        {"id": "node", "guest": f"mkdir -p {NODE} && tar -xJf /opt/opute-e2e/node.tar.xz -C {NODE} "
                                f"--strip-components=1 && rm /opt/opute-e2e/node.tar.xz && {NODE}/bin/node --version"},
        # the newcomer: a fresh sudo user who logs in over SSH, not in incus-admin
        {"id": "user", "guest": (
            f"useradd -m -s /bin/bash {U} && printf '{U} ALL=(ALL) NOPASSWD:ALL\\n' > /etc/sudoers.d/90-{U} && "
            f"chmod 0440 /etc/sudoers.d/90-{U} && visudo -cf /etc/sudoers.d/90-{U} >/dev/null && "
            f"install -d -m 0700 {QS} && ssh-keygen -q -t ed25519 -N '' -f {QS}/id && "
            f"install -d -o {U} -g {U} -m 0700 {H}/.ssh && install -o {U} -g {U} -m 0600 {QS}/id.pub "
            f"{H}/.ssh/authorized_keys && id -nG {U}")},
        {"id": "session", "guest": (
            f"systemd-run --unit=e2e-qs-session --collect {SSH} 'exec sleep 1700' && "
            f"for i in $(seq 120); do [ -S /run/user/$(id -u {U})/bus ] && break; sleep 0.25; done; "
            f"loginctl show-user {U} -p State --value; test -S /run/user/$(id -u {U})/bus && echo bus")},
        {"id": "sudo", "guest": login("sudo -n true && echo sudo; node --version | cut -d. -f1", shell=False)},
        {"id": "token", "guest": login("openssl rand -hex 32 | tr -d '\\n' | wc -c", shell=False)},
        {"id": "vmAgentStop", "guest": f"systemctl stop {VM_AGENT} && systemctl is-active {VM_AGENT}; true"},
        # Quickstart 1: start the agent
        {"id": "start", "guest": login(START, shell=False), "timeout": 300},
        {"id": "status", "guest": login("npx @opute/host-agent status")},
        {"id": "health", "health": True, "port": 3014},
        {"id": "listen", "guest": f"{T} listen 3014"},
        {"id": "binary", "guest": f"pid=$({PID}); sha256sum /proc/$pid/exe | cut -c1-64; "
                                  f"stat -c %U /proc/$pid; /proc/$pid/exe --version"},
        # Quickstart 2 and 3: a client connects and asks what the host is
        base.mcp("list", "tools/list", **ON),
        call("info", "get_host_info", {}, ON),
        {"id": "hints", "guest": HINTS},
        # Safe by default: the bearer cannot change the host yet, and the refusal names the grant
        call("refused", "create_vm", {"name": "sandbox"}, ON),
        # Quickstart 4: allow changes
        {"id": "restart", "guest": login(RESTART), "timeout": 300},
        {"id": "slice", "guest": f"test -f {SLICE} && echo slice; " + login(
            "systemctl --user show -p LoadState --value opute-workload.slice", shell=False),
         "confirms": ["restart"]},
        {"id": "status2", "guest": login("npx @opute/host-agent status")},
        # "Install Incus and create a small Ubuntu VM called sandbox", as an assistant's calls
        *task("virt", "install_virtualization_stack", {}, ON, timeout=900),
        {"id": "groups", "guest": f"id -nG {U}; grep ^Groups /proc/$({PID})/status; getent group incus-admin | "
                                  "cut -d: -f3", "confirms": ["virt"]},
        *task("sandbox", "create_vm", {"name": "sandbox"}, ON, timeout=900),
        call("vms", "list_vms", {}, ON),
        {"id": "sandboxState", "guest": "incus list sandbox -c ns -f csv; incus list --all-projects -c ns -f csv "
                                        "| grep -c sandbox; true", "confirms": ["sandbox"]},
        *task("deleteSandbox", "delete_vm", {"uri": SANDBOX}, ON, timeout=600),
        {"id": "sandboxGone", "guest": "incus list --all-projects -c n -f csv | grep -c '^sandbox$'; true",
         "confirms": ["deleteSandbox"]},
        # Install without npm: the release binary, checked against SHA256SUMS, serves the same port
        {"id": "stop", "guest": login("npx @opute/host-agent stop")},
        {"id": "stopped", "guest": "curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3014/health; true",
         "confirms": ["stop"]},
        {"id": "noNpm", "guest": login(NO_NPM), "timeout": 300},
        call("noNpmInfo", "get_host_info", {}, ON),
        {"id": "noNpmStop", "guest": f"pkill -u {U} -f host-agent-linux-x64; for i in $(seq 40); do "
                                     "curl -fsS -o /dev/null http://127.0.0.1:3014/health || break; sleep 0.25; "
                                     "done; curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3014/health; true"},
    ]
    hex16 = "^local-[0-9a-f]{16}$"
    reqs = [
        req(README, "Quickstart 1: start the agent with the published launcher",
            a("user", "stdout", equals=f"{U}\n"), a("session", "stdout", equals="active\nbus\n"),
            a("sudo", "stdout", equals="sudo\nv22\n"), a("token", "stdout", equals="64\n"),
            ok("start"), a("start", "stdout", contains="http://127.0.0.1:3014/mcp"),
            ok("status"), a("status", "json.running", equals=True), a("status", "json.healthy", equals=True),
            a("health", "response.body.agentId", matches=hex16),
            a("listen", "json.addresses", equals=["127.0.0.1:3014"]),
            a("binary", "stdout", equals=f"{BINARY_SHA}\n{U}\n{VERSION}\n"),
            scenario="A fresh sudo user with Node 22, logged in over SSH, runs the quickstart's three lines: npm "
                     f"`latest` starts the published {VERSION} binary in the background on 127.0.0.1:3014 only, "
                     "with the identity the README's line generates; status reports it running and healthy"),
        req(README, "Quickstart 3 and Safe by default: a read-only first answer",
            a("list", "response.status", equals=200),
            direct("info", "isError", **{"in": [False, "<absent>"]}),
            direct("info", "structuredContent.uri", matches=":local-[0-9a-f]{16}$"),
            a("hints", "json.tools", equals=TOOLS), a("hints", "json.withoutReadOnlyHint", equals=0),
            a("hints", "json.writeWithoutDestructiveHint", equals=0),
            direct("refused", "isError", equals=True),
            direct("refused", "content[0].text", matches="--allow-mutations"),
            scenario="The client lists the tools and calls get_host_info; every tool declares its effect hints; "
                     "a change is refused to the bearer and the refusal names --allow-mutations"),
        req(README, "Quickstart 4: allow changes, then the sandbox VM",
            ok("restart"), a("slice", "stdout", equals="slice\nloaded\n"),
            a("status2", "json.running", equals=True), a("status2", "json.healthy", equals=True),
            res("virt", "isError", equals=False), a("groups", "stdout", matches=r"^(\S+ )*incus-admin( |\n)"),
            res("sandbox", "isError", equals=False),
            direct("vms", "isError", **{"in": [False, "<absent>"]}),
            a("sandboxState", "stdout", matches="^sandbox,RUNNING\n"),
            res("deleteSandbox", "isError", equals=False), a("sandboxGone", "stdout", equals="0\n"),
            scenario="Restarted with --allow-mutations, the launcher creates opute-workload.slice and the agent "
                     "serves; install_virtualization_stack succeeds, then create_vm makes a running Ubuntu VM "
                     "called sandbox, which delete_vm removes"),
        req(README, "Install without npm: the GitHub release binary",
            ok("stop"), a("stopped", "stdout", equals="000"),
            ok("noNpm"), a("noNpm", "stdout", equals=f"host-agent-linux-x64: OK\n{VERSION}\n200\n"),
            direct("noNpmInfo", "isError", **{"in": [False, "<absent>"]}),
            a("noNpmStop", "stdout", equals="000"),
            scenario=f"The v{VERSION} x64 binary downloaded from the GitHub release matches SHA256SUMS, reports "
                     "its version and serves MCP on 127.0.0.1:3014 with the same identity and token"),
    ]
    cleanup = [{"id": "cleanReadme", "guest": (
        "incus delete -f sandbox 2>/dev/null; "
        f"pkill -u {U} -f host-agent 2>/dev/null; systemctl stop e2e-qs-session.service 2>/dev/null; "
        f"loginctl terminate-user {U} 2>/dev/null; for i in $(seq 40); do pgrep -u {U} >/dev/null || break; "
        f"sleep 0.25; done; userdel -r {U} 2>/dev/null; rm -f /etc/sudoers.d/90-{U}; rm -rf {QS} {NODE} "
        "/tmp/node-compile-cache; "
        f"systemctl start {VM_AGENT} && {base.WAIT_HEALTH.format(port=3014)}; true")}]
    allow = [f"files/{H}*", f"files//etc/sudoers.d/90-{U}", f"files/{QS}*", f"files/{NODE}*",
             "files//tmp/node-compile-cache*",
             *(f"files//etc/{f}" for f in ("passwd", "passwd-", "group", "group-", "shadow", "shadow-", "gshadow",
                                            "gshadow-", "subuid", "subuid-", "subgid", "subgid-")),
             f"system/units/{VM_AGENT}", f"processes//system.slice/{VM_AGENT} opute-host-agen",
             "system/units/e2e-qs-session.service", "processes//system.slice/e2e-qs-session.service ssh",
             *e4.STACK_REINSTALL, *e4.APT_STATE, *e4.VM_KERNEL]
    persist = [*e4.STACK_REINSTALL, *e4.APT_STATE, *e4.VM_KERNEL]
    optional = {**e4.APT_OPTIONAL}
    files = {**base.TOOL_FILES, "node": inp("${PARITY}/setup/node-v22.14.0-linux-x64.tar.xz")}
    return e4.journey(
        "e2e9.readme", "e2e-readme",
        "The README as written (m33): a fresh sudo user logged in over SSH follows the quickstart with the "
        f"published @opute/host-agent ({VERSION}), allows changes, has the sandbox VM created and removed through "
        "the tools an assistant calls, and runs the GitHub release binary checked against SHA256SUMS. Pass: every "
        "step the README promises holds on that path.",
        reqs, steps, cleanup, allow=allow, persist=persist, minutes=30, optional=optional,
        inputs={"agent": f"the published @opute/host-agent (npm latest, {VERSION}) and the GitHub release "
                         f"v{VERSION}, both downloaded during the run; the VM's own agent is stopped",
                "files": files})


JOURNEYS = [readme]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
