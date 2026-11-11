#!/usr/bin/env python3
"""Write the e2e-1 journeys (and the e2e-0 step-kinds self-check) as JSON.

  python3 evidence/e2e/journeys/make_e2e1.py

The committed JSON files are the journeys; this script is how they were
authored. Hash pins are computed from the inputs as they are now, so a
changed input changes its journey (and the offline test that compares
the committed files with this script's output fails until it is rerun).
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent
PARITY = Path(subprocess.run(["git", "-C", str(REPO), "rev-parse", "--path-format=absolute", "--git-common-dir"],
                             capture_output=True, text=True, check=True).stdout.strip()).parent / ".parity"

SPEC = "openspec/specs/host-agent-contract/spec.md"
GATE_SPEC = "openspec/changes/standalone-read-only-gate/specs/standalone-read-only-gate/spec.md"
OAUTH_SPEC = "openspec/changes/secure-oauth-issuance/specs/oauth-issuance/spec.md"
REJECT_SPEC = "openspec/changes/reject-without-audit-writes/specs/reject-without-audit-writes/spec.md"
LOCK_SPEC = "openspec/changes/state-directory-writer-lock/specs/state-directory-writer-lock/spec.md"
Q14_SPEC = "openspec/changes/ipv4-wildcard-bind/specs/mcp-listener/spec.md"
Q5_SPEC = "openspec/changes/truthful-interrupted-plan/specs/host-agent-contract/spec.md"
Q15_SPEC = "openspec/changes/reclaim-orphaned-plan-run-reservations/specs/plan-reservation-inheritance/spec.md"
TASKS_META = {"io.modelcontextprotocol/clientCapabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}}}}
DOCS = "published docs, host-agents c9f9e775 (v0.2.4) site/public/docs/"
DIST = "baseline/inventory/distribution.json"
PROPOSED = "evidence/cutover/proposed-0.4.0/README.md"

# The Rust release under test (m15): one candidate for every gate, and a newer local build of the same
# sources as the in-place upgrade target. Go stays the published v0.2.4. E2E_CANDIDATE points draft
# journeys (never committed) at a draft release set.
RUST_VERSION = "0.4.0"
CANDIDATE = os.environ.get("E2E_CANDIDATE", "${PARITY}/e2e/release/rust-0.4.0-rc2")
CANDIDATE_NEXT = CANDIDATE + "-next"
NEXT_VERSION = "0.4.1-e2e"
# The previous Rust release (0.3.x), for the platform-neutral upgrade and rollback.
PREVIOUS = "${PARITY}/e2e/release/rust-0.3.0-rc4"
# What the installed candidate prints after its hash line.
RUST_MATCH = "\n" + RUST_VERSION.replace(".", "\\.") + "\n$"
LAUNCHER = f"opute-host-agent-{RUST_VERSION}.tgz"

BIN = "/home/opute/.local/share/opute/opute-host-agent"
UNIT = "opute-bootstrap-mcp.service"
REL = f"/opt/opute-e2e/releases/rust/v{RUST_VERSION}"
GO_REL = "/opt/opute-e2e/releases/go/v0.2.4"
SDB = "/home/opute/.config/opute/instances/standalone/state/state.db"
ADB = "/home/opute/.config/opute/instances/standalone/state/authz.sqlite"
CRED = "/home/opute/.config/opute/instances/standalone/state/credentials/host-agent-provider.json"
T = "python3 /opt/opute-e2e/guest/e2etool.py"
GATE_TEXT = "Error: mutations are disabled; set OPUTE_HOST_ALLOW_MUTATIONS=true"  # platform-neutral-host-agent
WAIT_HEALTH = "for i in $(seq 120); do curl -fsS -o /dev/null http://127.0.0.1:{port}/health && break; sleep 0.5; done"
AGENT = (f"the dev VM's production-style system unit opute-bootstrap-mcp.service (Rust {RUST_VERSION} release candidate "
         "binary, "
         "test identity e2e-devvm-agent, test token)")


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inp(path: str) -> dict:
    real = Path(path.replace("${PARITY}", str(PARITY)).replace("${REPO}", str(REPO)))
    return {"path": path, "sha256": sha(real)}


TOOL_FILES = {"e2etool": inp("${REPO}/evidence/e2e/guest-tools/e2etool.py"),
              "mcpwire": inp("${REPO}/evidence/e2e/mcpwire.py")}
TOOL_STEPS = [{"id": "tool", "copy": "e2etool", "to": "/opt/opute-e2e/guest/e2etool.py"},
              {"id": "wire", "copy": "mcpwire", "to": "/opt/opute-e2e/guest/mcpwire.py"}]


def req(spec: str, requirement: str, *assertions: dict, scenario: str | None = None, question: str | None = None):
    out = {"spec": spec, "requirement": requirement, "pass": list(assertions)}
    if scenario:
        out["scenario"] = scenario
    if question:
        out["openQuestion"] = question
    return out


def a(step: str, path: str, **op) -> dict:
    return {"step": step, "path": path, **op}


def ok(step: str) -> dict:
    return a(step, "exit", equals=0)


def plan(plan_id: str, key: str, path: str, content: str = "e2e\n", slow: bool = False) -> dict:
    nodes = [{"id": "n1", "action": {"tool": "ensure_host_file",
                                     "args": {"path": path, "content": content, "scope": "user"}},
              "validate": {"tool": "inspect_host_file", "args": {"path": path, "scope": "user"},
                           "assert": [{"path": "/exists", "op": "eq", "value": True}]}}]
    if slow:
        nodes.append({"id": "n2", "dependsOn": ["n1"],
                      "action": {"tool": "probe_http_endpoint", "args": {"endpoint": "http://127.0.0.1:8799/"}}})
    return {"contractVersion": "host-plan.v1", "planId": plan_id, "generation": 1, "idempotencyKey": key,
            "nodes": nodes}


def mcp(id_: str, method: str, **kw) -> dict:
    return {"id": id_, "mcp": method, **kw}


def call(id_: str, tool: str, arguments: dict | None = None, **kw) -> dict:
    return {"id": id_, "mcp": "tools/call", "tool": tool, "arguments": arguments or {}, **kw}


def poll_run(id_: str, var: str, done=("completed", "failed", "cancelled"), timeout: int = 90, **kw) -> dict:
    return call(id_, "get_host_plan_run", {"runId": "${" + var + "}"},
                until={"path": "response.body.result.structuredContent.status", "in": list(done)},
                timeout=timeout, **kw)


def journey(id_: str, description: str, inputs: dict, requirements: list, steps: list, cleanup: list,
            allow=(), persist=(), minutes: int = 20, gate: str = "e2e-1") -> dict:
    return {"id": id_, "gate": gate, "description": description, "inputs": inputs, "requirements": requirements,
            "effects": {"allow": list(allow), "persist": list(persist)}, "steps": steps, "cleanup": cleanup,
            "maxMinutes": minutes}


def install_binary(gz: str) -> str:
    return (f"gzip -dc {gz} > /tmp/oha && install -o opute -g opute -m 0755 /tmp/oha {BIN} && rm /tmp/oha && "
            f"sha256sum {BIN} | cut -c1-64")


def swap(id_: str, gz: str) -> dict:
    return {"id": id_, "guest": f"systemctl stop {UNIT} && {install_binary(gz)} && systemctl start {UNIT} && "
                                 f"{WAIT_HEALTH.format(port=3014)} && {BIN} --version"}


# --- 1. system unit ---------------------------------------------------------------------

def j1() -> dict:
    steps = TOOL_STEPS + [
        {"id": "uninstall", "guest": f"systemctl disable --now {UNIT} && rm -f {BIN} /etc/systemd/system/{UNIT} "
                                     f"/etc/sudoers.d/opute-host-agent /etc/opute/host-agent.env && rm -rf "
                                     f"/etc/systemd/system/{UNIT}.d && systemctl daemon-reload"},
        {"id": "gone", "guest": "curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3014/health; true"},
        {"id": "verify", "guest": f"cd {REL} && sha256sum -c SHA256SUMS"},
        {"id": "pinned", "guest": f"awk '$2==\"host-agent-linux-x64\"{{print $1}}' {REL}/SHA256SUMS"},
        {"id": "binary", "guest": install_binary(f"{REL}/host-agent-linux-x64.gz")},
        {"id": "deploy", "guest": f"install -m 0644 /opt/opute-e2e/deploy/opute-bootstrap-mcp.service "
                                  f"/etc/systemd/system/{UNIT} && install -m 0440 "
                                  "/opt/opute-e2e/deploy/opute-host-agent-sudoers /etc/sudoers.d/opute-host-agent "
                                  "&& visudo -cf /etc/sudoers.d/opute-host-agent"},
        {"id": "deployHashes", "guest": f"sha256sum /etc/systemd/system/{UNIT} /etc/sudoers.d/opute-host-agent"},
        {"id": "envfile", "guest": "install -d -m 0750 -g opute /etc/opute && (umask 027 && printf "
                                   "'OPUTE_REMOTE_AGENT_ID=e2e-devvm-agent\\nMCP_AUTH_TOKEN=%s\\n' '${TOKEN}' > "
                                   "/etc/opute/host-agent.env) && chgrp opute /etc/opute/host-agent.env && chmod 0640 "
                                   f"/etc/opute/host-agent.env && install -d /etc/systemd/system/{UNIT}.d && printf "
                                   "'[Service]\\nEnvironmentFile=/etc/opute/host-agent.env\\n' > "
                                   f"/etc/systemd/system/{UNIT}.d/e2e.conf"},
        {"id": "check", "guest": f"runuser -u opute -- {BIN} --check --env-file /etc/opute/host-agent.env"},
        {"id": "enable", "guest": f"systemctl daemon-reload && systemctl enable --now {UNIT} && "
                                  f"{WAIT_HEALTH.format(port=3014)} && {T} unit {UNIT}"},
        {"id": "health", "health": True},
        {"id": "listen", "guest": f"{T} listen 3014"},
        {"id": "ipv6Refused", "guest": "curl -s -o /dev/null -w '%{http_code}' --max-time 3 http://[::1]:3014/health; true"},
        {"id": "ipv4", "guest": "curl -fsS http://$(hostname -I | awk '{print $1}'):3014/health"},
        mcp("list", "tools/list"),
        call("info", "get_host_info"),
    ]
    reqs = [
        req(SPEC, "Compatible modes, configuration, and distribution",
            a("uninstall", "exit", equals=0), a("gone", "stdout", equals="000"),
            ok("verify"), a("verify", "stdout", contains="host-agent-linux-x64.gz: OK"),
            a("binary", "stdout", equalsStep={"step": "pinned", "path": "stdout"}),
            ok("deploy"), ok("envfile"),
            ok("check"), a("check", "stdout", equals="configuration ok\n"),
            ok("enable"), a("enable", "json.ActiveState", equals="active"),
            a("enable", "json.SubState", equals="running"), a("enable", "json.User", equals="opute"),
            a("enable", "json.UnitFileState", equals="enabled"),
            a("enable", "json.EnvironmentFiles", equals="/etc/opute/host-agent.env"),
            ok("ipv4"), a("ipv4", "json.agentId", equals="e2e-devvm-agent"),
            scenario="install from the release artifact; the configured port applies and the configured bind "
                     "(HOST_MCP_BIND_HOST=0.0.0.0 in the recorded unit) serves the host's own IPv4 address"),
        req(Q14_SPEC, "An IPv4 wildcard bind listens on IPv4 only",
            a("listen", "json.addresses", equals=["0.0.0.0:3014"]),
            a("ipv6Refused", "stdout", equals="000"),
            scenario="HOST_MCP_BIND_HOST=0.0.0.0 opens one IPv4 socket; an IPv6 loopback connection is refused "
                     "(D28)"),
        req(DIST, "deployFiles: the installed sudoers is the recorded deploy file",
            a("deployHashes", "stdout",
              contains="38eca4cda3325c61ca1fcc7fe5220eabfb3be2628061b5faffe0c0f4734013ad  /etc/sudoers.d/"
                       "opute-host-agent")),
        req(PROPOSED, "The proposed 0.4.0 unit",
            a("deployHashes", "stdout",
              contains=f"{sha(REPO / 'evidence/cutover/proposed-0.4.0/opute-bootstrap-mcp.service')}  "
                       "/etc/systemd/system/opute-bootstrap-mcp.service"),
            scenario="The recorded unit (deployFiles 02246cf6...) passes --mode, which 0.4.0 rejects; the VM "
                     "installs the proposed unit, without --mode and with both mutation settings (owner "
                     "decision 2026-11-11)"),
        req(SPEC, "One exact Host Agent identity",
            a("health", "response.status", equals=200),
            a("health", "response.body.agentId", equals="e2e-devvm-agent"),
            a("info", "response.body.result.structuredContent.uri", matches=":e2e-devvm-agent$")),
        req(SPEC, "Compatible authenticated MCP surface",
            a("list", "response.status", equals=200), a("info", "response.status", equals=200),
            a("info", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            scenario="Read-only authenticated first call"),
    ]
    return journey(
        "e2e1.system-unit",
        "Flow 2, system unit: uninstall the agent, verify the release artifact against SHA256SUMS, reinstall the "
        "binary, Go's recorded deploy/opute-bootstrap-mcp.service and sudoers, the env file and drop-in, pass "
        "--check, enable, and serve. The published docs route a production host's install through Platform's "
        "Connect Remote Host flow (docs/install), so the recorded deploy files and distribution contract are the "
        "reference; reinstalling must reproduce the snapshot exactly (no declared effect).",
        {"agent": AGENT, "files": TOOL_FILES, "release": f"{REL} (local Rust {RUST_VERSION} release candidate set)"},
        reqs, steps, [])


# --- 2. user unit and npm launcher ---------------------------------------------------------

def j2() -> dict:
    user = "ubuntu"
    home = "/home/ubuntu"
    run = f"R=/run/user/$(id -u {user}); runuser -u {user} -- env XDG_RUNTIME_DIR=$R systemctl --user"
    npm = (f"cd {home} && runuser -u {user} -- env HOME={home} PATH=/opt/opute-e2e/node/bin:/usr/bin:/bin "
           "NPM_CONFIG_OFFLINE=true NPM_CONFIG_UPDATE_NOTIFIER=false "
           "OPUTE_HOST_AGENT_RELEASE_BASE_URL=http://127.0.0.1:8765 OPUTE_REMOTE_AGENT_ID=e2e-npm-agent "
           "MCP_AUTH_TOKEN=${SECRET:npm} HOST_MCP_PORT=3016 npm exec --yes --offline "
           f"--package=/opt/opute-e2e/{LAUNCHER} -- opute-host-agent")
    files = {**TOOL_FILES,
             "userUnit": inp("${PARITY}/e2e/go-src-v0.2.4/deploy/opute-host-agent.service"),
             "node": inp("${PARITY}/setup/node-v22.14.0-linux-x64.tar.xz"),
             "launcher": inp("${PARITY}/e2e/inputs/" + LAUNCHER)}
    steps = TOOL_STEPS + [
        {"id": "unitFile", "copy": "userUnit", "to": "/opt/opute-e2e/deploy-user/opute-host-agent.service"},
        {"id": "linger", "guest": f"loginctl enable-linger {user} && for i in $(seq 60); do test -S "
                                  f"/run/user/$(id -u {user})/systemd/private && break; sleep 0.5; done"},
        {"id": "install", "guest": f"U={user}; H={home}; install -d -o $U -g $U $H/.local $H/.local/share "
                                   "$H/.local/share/opute $H/.config $H/.config/opute $H/.config/systemd "
                                   f"$H/.config/systemd/user && {install_binary(f'{REL}/host-agent-linux-x64.gz')}"
                                   .replace(BIN, f"{home}/.local/share/opute/opute-host-agent")
                                   .replace("-o opute -g opute", f"-o {user} -g {user}") +
                                   f" && install -o $U -g $U -m 0644 /opt/opute-e2e/deploy-user/"
                                   "opute-host-agent.service $H/.config/systemd/user/opute-host-agent.service && "
                                   "(umask 077 && printf 'OPUTE_REMOTE_AGENT_ID=e2e-user-agent\\nMCP_AUTH_TOKEN=%s\\n"
                                   "HOST_MCP_PORT=3015\\n' '${SECRET:user}' > $H/.config/opute/host-agent.env) && "
                                   "chown $U:$U $H/.config/opute/host-agent.env"},
        {"id": "pinned", "guest": f"awk '$2==\"host-agent-linux-x64\"{{print $1}}' {REL}/SHA256SUMS"},
        {"id": "check", "guest": f"runuser -u {user} -- {home}/.local/share/opute/opute-host-agent --check "
                                 f"--env-file {home}/.config/opute/host-agent.env"},
        {"id": "enable", "guest": f"{run} daemon-reload && {run} enable --now opute-host-agent.service && "
                                  f"{WAIT_HEALTH.format(port=3015)} && {T} unit opute-host-agent.service --user {user}"},
        {"id": "userHealth", "health": True, "port": 3015},
        {"id": "userListen", "guest": f"{T} listen 3015"},
        mcp("userList", "tools/list", port=3015, auth="secret:user"),
        call("userInfo", "get_host_info", port=3015, auth="secret:user"),
        {"id": "userStop", "guest": f"{run} disable --now opute-host-agent.service"},
        {"id": "nodeCopy", "copy": "node", "to": "/opt/opute-e2e/node.tar.xz"},
        {"id": "launcherCopy", "copy": "launcher", "to": f"/opt/opute-e2e/{LAUNCHER}",
         "mode": "0644"},
        {"id": "node", "guest": "mkdir -p /opt/opute-e2e/node && tar -xJf /opt/opute-e2e/node.tar.xz -C "
                                "/opt/opute-e2e/node --strip-components=1 && /opt/opute-e2e/node/bin/node --version"},
        {"id": "releaseBase", "guest": "systemd-run --unit=e2e-release --collect python3 -m http.server 8765 "
                                       "--bind 127.0.0.1 --directory /opt/opute-e2e/releases/rust && "
                                       "for i in $(seq 40); do curl -fsS -o /dev/null "
                                       f"http://127.0.0.1:8765/v{RUST_VERSION}/SHA256SUMS && break; sleep 0.25; done"},
        {"id": "start", "guest": f"{npm} start --background", "timeout": 300},
        {"id": "url", "guest": f"{npm} url"},
        {"id": "status", "guest": f"{npm} status"},
        {"id": "npmHealth", "health": True, "port": 3016},
        {"id": "npmListen", "guest": f"{T} listen 3016"},
        {"id": "npmBinary", "guest": "pid=$(ss -ltnpH 'sport = :3016' | grep -o 'pid=[0-9]*' | head -1 | cut -d= "
                                     "-f2); sha256sum /proc/$pid/exe | cut -c1-64"},
        mcp("npmList", "tools/list", port=3016, auth="secret:npm"),
        call("npmInfo", "get_host_info", port=3016, auth="secret:npm"),
        {"id": "stop", "guest": f"{npm} stop"},
        {"id": "npmGone", "guest": "curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3016/health; true"},
        {"id": "releaseStop", "guest": "systemctl stop e2e-release.service"},
        {"id": "unlinger", "guest": f"loginctl disable-linger {user} && systemctl stop user@$(id -u {user}).service"},
    ]
    # None of .local, .config, .cache or .npm exists in ubuntu's home at the snapshot; the agent's own
    # incus client writes .config/incus (e2e-1-r1, retired).
    cleanup = [{"id": "remove", "guest": f"rm -rf {home}/.local {home}/.config {home}/.cache {home}/.npm "
                                         "/opt/opute-e2e/node /opt/opute-e2e/node.tar.xz "
                                         f"/opt/opute-e2e/{LAUNCHER} /opt/opute-e2e/deploy-user"}]
    reqs = [
        req(SPEC, "Compatible modes, configuration, and distribution",
            ok("install"), a("install", "stdout", equalsStep={"step": "pinned", "path": "stdout"}),
            ok("check"), a("check", "stdout", equals="configuration ok\n"),
            ok("enable"), a("enable", "json.ActiveState", equals="active"),
            a("enable", "json.FragmentPath", equals=f"{home}/.config/systemd/user/opute-host-agent.service"),
            a("enable", "json.EnvironmentFiles", equals=f"{home}/.config/opute/host-agent.env"),
            a("userListen", "json.addresses", equals=["127.0.0.1:3015"]),
            scenario="user unit: Go's recorded deploy/opute-host-agent.service; standalone loopback bind, "
                     "configured port"),
        req(DOCS + "install/index.html", "Published npm launcher: start --background, url, status, stop",
            ok("node"), ok("releaseBase"), ok("start"), a("start", "stdout", contains="http://127.0.0.1:3016/mcp"),
            ok("url"), a("url", "stdout", equals="http://127.0.0.1:3016/mcp\n"),
            ok("status"), a("status", "json.running", equals=True), a("status", "json.healthy", equals=True),
            a("npmListen", "json.addresses", equals=["127.0.0.1:3016"]),
            a("npmBinary", "stdout", equalsStep={"step": "pinned", "path": "stdout"}),
            ok("stop"), a("npmGone", "stdout", equals="000"),
            scenario=f"@opute/host-agent@{RUST_VERSION} packed locally from the pinned launcher tree with only "
                     "its version changed (never published), with a local release base; the launcher runs the "
                     "pinned release binary"),
        req(SPEC, "One exact Host Agent identity",
            a("userHealth", "response.body.agentId", equals="e2e-user-agent"),
            a("npmHealth", "response.body.agentId", equals="e2e-npm-agent"),
            a("userInfo", "response.body.result.structuredContent.uri", matches=":e2e-user-agent$"),
            a("npmInfo", "response.body.result.structuredContent.uri", matches=":e2e-npm-agent$")),
        req(SPEC, "Compatible authenticated MCP surface",
            a("userList", "response.status", equals=200), a("npmList", "response.status", equals=200),
            a("userInfo", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            a("npmInfo", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            scenario="Read-only authenticated first call"),
    ]
    allow = ["files//home/ubuntu/.local*", "files//home/ubuntu/.config*", "files//home/ubuntu/.cache*",
             "files//home/ubuntu/.npm*", "files//opt/opute-e2e/node*", f"files//opt/opute-e2e/{LAUNCHER}",
             "files//opt/opute-e2e/deploy-user*", "linger*", "user/*", "system/units/user*",
             "system/units/run-user-1000.mount*", "system/units/e2e-release*", "system/unitFiles/e2e-release*"]
    return journey(
        "e2e1.user-unit-npm",
        "Flow 2, user install: Go's recorded user unit (deploy/opute-host-agent.service) for the existing user "
        "ubuntu with its env file, --check, enable and serve on a configured port; then the npm launcher packed "
        "locally from the pinned tree (never published), started as the docs show against a local release base, "
        "with url, status and stop. The user unit is stopped first: both are the user's default instance, and "
        "under D25 they would refuse each other.",
        {"agent": f"Rust {RUST_VERSION} release candidate binary as ubuntu (user unit) and as the launcher's "
                  "cached binary",
         "files": files, "release": f"{REL} served at http://127.0.0.1:8765/v{RUST_VERSION}/"},
        reqs, steps, cleanup, allow=allow, minutes=20)


# --- 3. tutorial first success with scripted clients -----------------------------------------

def j3() -> dict:
    files = {**TOOL_FILES,
             "hostagentclient": inp("${PARITY}/e2e/inputs/hostagentclient"),
             "getStarted": inp("${PARITY}/e2e/go-src-v0.2.4/site/public/docs/get-started/index.html")}
    modern_plan = plan("e2e1-j3-modern", "e2e1-j3-modern-1", ".config/opute-e2e/j3-modern.txt")
    legacy_plan = plan("e2e1-j3-legacy", "e2e1-j3-legacy-1", ".config/opute-e2e/j3-legacy.txt")
    client_plan = plan("e2e1-j3-client", "e2e1-j3-client-1", ".config/opute-e2e/j3-client.txt")
    legacy = {"port": 3017, "auth": "secret:legacy", "modern": False}
    steps = TOOL_STEPS + [
        # (a) a 2026-07-28 MCP client against the system agent
        {"id": "health", "health": True},
        mcp("discover", "server/discover"),
        mcp("list", "tools/list"),
        call("catalog", "get_capability_catalog"),
        call("info", "get_host_info"),
        call("capacity", "get_host_capacity"),
        call("plan", "run_host_plan", {"plan": modern_plan},
             capture={"RUN": "response.body.result.structuredContent.runId"}),
        poll_run("planDone", "RUN"),
        # (b) a legacy-handshake client against an agent with the opt-in exception
        {"id": "legacyAgent", "guest": f"{T} agent start legacy --port 3017 --id e2e-legacy-agent "
                                       "--env MCP_AUTH_TOKEN=${SECRET:legacy} --env OPUTE_MCP_ALLOW_LEGACY_HANDSHAKE=true "
                                       "--env OPUTE_STANDALONE_ALLOW_MUTATIONS=true"},
        mcp("legacyInit", "initialize", params={"protocolVersion": "2025-06-18", "capabilities": {},
                                                "clientInfo": {"name": "e2e-legacy", "version": "1"}}, **legacy),
        mcp("legacyList", "tools/list", **legacy),
        call("legacyCatalog", "get_capability_catalog", **legacy),
        call("legacyInfo", "get_host_info", **legacy),
        call("legacyCapacity", "get_host_capacity", **legacy),
        call("legacyPlan", "run_host_plan", {"plan": legacy_plan},
             capture={"LRUN": "response.body.result.structuredContent.runId"}, **legacy),
        poll_run("legacyPlanDone", "LRUN", **legacy),
        mcp("legacyDiscover", "server/discover", **legacy),
        {"id": "legacyStop", "guest": f"{T} agent remove legacy"},
        # (c) the published Go client library, pkg/hostagentclient
        {"id": "clientCopy", "copy": "hostagentclient", "to": "/opt/opute-e2e/clients/hostagentclient", "mode": "0755"},
        {"id": "client", "guest": "printf '%s' '" + json.dumps(client_plan) + "' > /tmp/e2e-j3-plan.json && "
                                  "HOST_AGENT_URL=http://127.0.0.1:3014/mcp HOST_AGENT_TOKEN='${TOKEN}' "
                                  "/opt/opute-e2e/clients/hostagentclient /tmp/e2e-j3-plan.json; rc=$?; "
                                  "rm -f /tmp/e2e-j3-plan.json; exit $rc",
         "schemaResults": "json.results", "schemaTarget": "agent", "timeout": 300},
        # (d) the VS Code configuration the tutorial documents
        {"id": "docsCopy", "copy": "getStarted", "to": "/opt/opute-e2e/docs/get-started.html", "mode": "0644"},
        {"id": "ide", "guest": f"E2E_TOKEN='${{TOKEN}}' {T} ideconfig /opt/opute-e2e/docs/get-started.html",
         "schemaResults": "json.results", "schemaTarget": "agent"},
    ]
    cleanup = [{"id": "remove", "guest": "rm -rf /home/opute/.config/opute-e2e /opt/opute-e2e/clients "
                                         "/opt/opute-e2e/docs"}]
    completed = lambda step: a(step, "response.body.result.structuredContent.status", equals="completed")  # noqa: E731
    reqs = [
        req(DOCS + "get-started/index.html", "First success: health, connect, refresh tools, get_host_info {}",
            a("health", "response.body.agentId", equals="e2e-devvm-agent"),
            a("list", "response.status", equals=200),
            a("list", "response.body.result.tools[0].name", exists=True),
            a("info", "response.body.result.structuredContent.uri", exists=True),
            a("info", "response.body.result.structuredContent.hostName", exists=True),
            a("info", "response.body.result.structuredContent.providerId", exists=True),
            a("info", "response.body.result.structuredContent.lxcBinaryPath", exists=True),
            a("info", "response.body.result.structuredContent.systemctlPath", exists=True),
            a("info", "response.body.result.structuredContent.supportedTools", exists=True),
            scenario="the six required host facts the tutorial names"),
        req(DOCS + "get-started/index.html", "Connect VS Code: the documented .vscode/mcp.json",
            ok("ide"), a("ide", "json.config.type", equals="http"),
            a("ide", "json.config.url", equals="http://127.0.0.1:3014/mcp"),
            a("ide", "json.config.authorization", equals="Bearer ${input:opute-host-token}"),
            a("ide", "json.config.inputs[0].password", equals=True),
            a("ide", "json.toolsListStatus", equals=200), a("ide", "json.callStatus", equals=200),
            a("ide", "json.results[0].isError", equals=False)),
        req(SPEC, "Compatible authenticated MCP surface",
            a("discover", "response.status", equals=200), a("discover", "response.body.result", exists=True),
            a("catalog", "response.body.result.structuredContent.catalogRevision", matches="^sha256:"),
            a("capacity", "response.body.result.structuredContent.enforcement", exists=True),
            scenario="Read-only authenticated first call"),
        req(SPEC, "Compatible authenticated MCP surface",
            ok("legacyAgent"), a("legacyInit", "response.status", equals=200),
            a("legacyInit", "response.body.result.protocolVersion", equals="2025-06-18"),
            a("legacyList", "response.status", equals=200),
            a("legacyInfo", "response.body.result.structuredContent.uri", matches=":e2e-legacy-agent$"),
            a("legacyCatalog", "response.body.result.structuredContent.catalogRevision", matches="^sha256:"),
            completed("legacyPlanDone"),
            a("legacyDiscover", "response.status", equals=400),
            a("legacyDiscover", "response.body.error.code", exists=True),
            scenario="Legacy exception remains bounded: the opt-in legacy client completes the tutorial, a "
                     "modern-only method without modern metadata is refused"),
        req(SPEC, "One typed plan and recipe execution path",
            a("plan", "response.body.result.structuredContent.runId", exists=True),
            completed("planDone"),
            ok("client"), a("client", "json.plan.status", equals="completed"),
            a("client", "json.catalog.revision", matches="^sha256:"), a("client", "json.toolsList", gte=1),
            scenario="an approved mutation (mutations enabled on the agent) runs as a host plan; its run is "
                     "polled to completion by each client"),
    ]
    allow = ["files//home/opute/.config/opute-e2e*", "files//opt/opute-e2e/clients*", "files//opt/opute-e2e/docs*"]
    return journey(
        "e2e1.tutorial-clients",
        "Flow 1 and 3, tutorial first success with scripted clients, each from a clean connection: a 2026-07-28 "
        "MCP client; a legacy-handshake client (an agent started with OPUTE_MCP_ALLOW_LEGACY_HANDSHAKE=true, ADR "
        "0011); the published Go client library pkg/hostagentclient (v0.2.4, built offline); and the VS Code "
        "configuration the tutorial documents, used exactly as written. Each follows the published steps "
        "(tools/list, catalog, get_host_info, a read-only call), then, with mutations enabled, one approved "
        "mutation as a host plan polled to completion. The published tutorial is read-only and documents no "
        "mutation step, so the mutation is judged against the spec only.",
        {"agent": AGENT, "files": files}, reqs, steps, cleanup, allow=allow, minutes=20)


# --- 4. auth ---------------------------------------------------------------------------------

def j4() -> dict:
    foreign_cred = "/tmp/e2e-agents/foreign/state/credentials/host-agent-provider.json"
    read_cred = "python3 -c \"import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))\" "
    g = {"port": 3014}
    steps = TOOL_STEPS + [
        mcp("bootstrap", "tools/list", **g),
        {"id": "cred", "guest": read_cred + CRED,
         "capture": {"PROVIDER_ID": "json.client_id", "PROVIDER_SECRET": {"path": "json.client_secret", "secret": True}}},
        {"id": "issue", "http": "POST", "port": 3014, "path": "/oauth/token",
         "form": {"grant_type": "client_credentials", "resource": "http://127.0.0.1:3014/mcp"},
         "basic": ["${PROVIDER_ID}", "${PROVIDER_SECRET}"],
         "capture": {"ACCESS": {"path": "response.body.access_token", "secret": True}}},
        mcp("oauthList", "tools/list", auth="var:ACCESS", **g),
        call("oauthInfo", "get_host_info", auth="var:ACCESS", **g),
        {"id": "foreign", "guest": f"{T} agent start foreign --port 3018 --id e2e-foreign-agent "
                                   "--env MCP_AUTH_TOKEN=${SECRET:foreign}"},
        {"id": "foreignCred", "guest": read_cred + foreign_cred,
         "capture": {"FOREIGN_SECRET": {"path": "json.client_secret", "secret": True}}},
        {"id": "foreignIssue", "http": "POST", "port": 3018, "path": "/oauth/token",
         "form": {"grant_type": "client_credentials", "resource": "http://127.0.0.1:3018/mcp"},
         "basic": ["host-agent-provider", "${FOREIGN_SECRET}"],
         "capture": {"FOREIGN_ACCESS": {"path": "response.body.access_token", "secret": True}}},
        mcp("foreignOwn", "tools/list", port=3018, auth="var:FOREIGN_ACCESS"),
        {"id": "dbBefore", "guest": f"{T} db {SDB}"},
        {"id": "authzBefore", "guest": f"{T} db {ADB}"},
        mcp("wrong", "tools/list", auth="literal:intentionally-wrong", **g),
        mcp("missing", "tools/list", auth="none", **g),
        call("wrongCall", "get_host_info", auth="literal:intentionally-wrong", **g),
        mcp("foreignBootstrap", "tools/list", auth="secret:foreign", **g),
        mcp("foreignOauth", "tools/list", auth="var:FOREIGN_ACCESS", **g),
        call("foreignOauthCall", "get_host_info", auth="var:FOREIGN_ACCESS", **g),
        {"id": "dbAfter", "guest": f"{T} db {SDB}"},
        {"id": "authzAfter", "guest": f"{T} db {ADB}"},
        mcp("stillServes", "tools/list", **g),
        call("stillInfo", "get_host_info", **g),
        {"id": "foreignStop", "guest": f"{T} agent remove foreign"},
    ]
    refused = [a(s, "response.status", equals=401)
               for s in ("wrong", "missing", "wrongCall", "foreignBootstrap", "foreignOauth", "foreignOauthCall")]
    reqs = [
        req(SPEC, "Compatible authenticated MCP surface",
            a("bootstrap", "response.status", equals=200), a("oauthList", "response.status", equals=200),
            a("oauthInfo", "response.body.result.structuredContent.uri", matches=":e2e-devvm-agent$"),
            *refused,
            scenario="Invalid bearer token: the bootstrap token and an agent-issued access token are accepted; a "
                     "wrong, a missing, another instance's bootstrap token and another instance's access token "
                     "are refused with HTTP 401, the status the tutorial and troubleshooting docs state"),
        req(OAUTH_SPEC, "Provisioned credentials for built-in machine clients",
            ok("cred"), a("issue", "response.status", equals=200),
            a("issue", "response.body.token_type", equals="Bearer"),
            a("issue", "response.body.resource", equals="http://127.0.0.1:3014/mcp"),
            a("issue", "response.headers.cache-control", equals="no-store"),
            a("foreignIssue", "response.status", equals=200), a("foreignOwn", "response.status", equals=200)),
        req(REJECT_SPEC, "Rejected MCP calls leave no audit writes or execution effects",
            ok("dbBefore"), a("dbAfter", "json.counts", equalsStep={"step": "dbBefore", "path": "json.counts"}),
            a("dbAfter", "json.operationIds", equalsStep={"step": "dbBefore", "path": "json.operationIds"}),
            a("authzAfter", "json.counts", equalsStep={"step": "authzBefore", "path": "json.counts"}),
            scenario="Authentication rejected: no operation record or other durable row is written"),
        req(REJECT_SPEC, "Rejections preserve subsequent read availability",
            a("stillServes", "response.status", equals=200),
            a("stillInfo", "response.body.result.structuredContent.uri", matches=":e2e-devvm-agent$")),
        req(SPEC, "Compatible authenticated MCP surface",
            a("wrong", "response.status", equals=401), a("missing", "response.status", equals=401),
            a("wrong", "response.headers.www-authenticate", matches="^Bearer resource_metadata="),
            scenario="the refusal's status and the MCP authorization challenge are contract; its body text is "
                     "not (D28, Q1)"),
    ]
    return journey(
        "e2e1.auth",
        "Flow 1, auth: the bootstrap token and an access token issued by the agent (client_credentials with its "
        "provisioned credential) are accepted; a wrong token, a missing token, and another instance's bootstrap "
        "and access tokens (a second agent started for the journey) are refused with no durable row written, and "
        "the agent keeps serving.",
        {"agent": AGENT, "files": TOOL_FILES, "foreign": "a second Rust agent, /tmp/e2e-agents/foreign, port 3018"},
        reqs, steps, [], minutes=15)


# --- 5. admission ------------------------------------------------------------------------------

def j5() -> dict:
    gated = {"port": 3019, "auth": "secret:gated"}
    limited = {"port": 3020, "auth": "secret:limited"}
    gdb = "/tmp/e2e-agents/gated/state/state.db"
    ldb = "/tmp/e2e-agents/limited/state/state.db"
    target = {"path": ".config/opute-e2e/j5.txt", "content": "e2e\n", "scope": "user"}
    steps = TOOL_STEPS + [
        {"id": "gatedAgent", "guest": f"{T} agent start gated --port 3019 --id e2e-gated-agent "
                                      "--env MCP_AUTH_TOKEN=${SECRET:gated}"},
        mcp("gatedList", "tools/list", **gated),
        {"id": "gatedBefore", "guest": f"{T} db {gdb}"},
        call("mutation", "ensure_host_file", target, **gated),
        call("destructive", "remove_host_file", {"path": target["path"], "confirm": True}, **gated),
        call("planGate", "run_host_plan", {"plan": plan("e2e1-j5", "e2e1-j5-1", target["path"])}, **gated),
        {"id": "gatedAfter", "guest": f"{T} db {gdb}"},
        # An accepted read is audited (D11 keeps accepted calls' audit), so it runs after the window
        # (e2e-1-r1, retired).
        call("gatedRead", "get_host_info", **gated),
        {"id": "gatedNoFile", "guest": "test ! -e /tmp/e2e-agents/gated/home/.config/opute-e2e/j5.txt"},
        {"id": "limitedAgent", "guest": f"{T} agent start limited --port 3020 --id e2e-limited-agent "
                                        "--env MCP_AUTH_TOKEN=${SECRET:limited} --env OPUTE_STANDALONE_ALLOW_MUTATIONS=true "
                                        "--env OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES=1048576"},
        mcp("limitedList", "tools/list", **limited),
        call("limitedCapacity", "get_host_capacity", **limited),
        {"id": "limitedBefore", "guest": f"{T} db {ldb}"},
        call("saturated", "ensure_host_file", target, **limited),
        {"id": "limitedAfter", "guest": f"{T} db {ldb}"},
        {"id": "limitedNoFile", "guest": "test ! -e /tmp/e2e-agents/limited/home/.config/opute-e2e/j5.txt"},
        {"id": "stop", "guest": f"{T} agent remove gated && {T} agent remove limited"},
    ]
    gate = lambda s: [a(s, "response.body.result.isError", equals=True),  # noqa: E731
                      a(s, "response.body.result.content[0].text", equals=GATE_TEXT)]
    reqs = [
        req(GATE_SPEC, "Standalone without mutations runs read tools only",
            ok("gatedAgent"), *gate("mutation"), *gate("destructive"), *gate("planGate"),
            a("gatedRead", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            a("gatedRead", "response.body.result.structuredContent.uri", matches=":e2e-gated-agent$")),
        req(GATE_SPEC, "The gate refuses before any effect",
            ok("gatedNoFile"),
            a("gatedAfter", "json.counts", equalsStep={"step": "gatedBefore", "path": "json.counts"})),
        req(SPEC, "Typed target and effect admission",
            ok("limitedAgent"), a("saturated", "response.body.result.isError", equals=True),
            a("saturated", "response.body.result.structuredContent.code", equals="host_capacity_saturated"),
            a("saturated", "response.body.result.structuredContent.owner", equals="admission"),
            ok("limitedNoFile"),
            a("limitedAfter", "json.counts", equalsStep={"step": "limitedBefore", "path": "json.counts"}),
            scenario="a resource-limit refusal is typed and comes before any side effect or durable row"),
        req(SPEC, "Typed target and effect admission",
            a("saturated", "response.body.result.content[0].text", matches="^Error: host_capacity_saturated: "),
            scenario="the refusal names its error code; its message text is not contract (D28, Q3)"),
    ]
    return journey(
        "e2e1.admission",
        "Flow 3, admission: an agent with the documented defaults (mutations not enabled) refuses a mutation, a "
        "destructive call and a host plan with the D10 gate error and runs a read tool; an agent with mutations "
        "enabled and a 1 MiB memory capacity refuses a mutation with a typed resource-limit error. Neither writes "
        "a durable row or a file, and the VM diff is empty.",
        {"agent": "two Rust agents started for the journey under /tmp/e2e-agents (ports 3019, 3020)",
         "files": TOOL_FILES}, reqs, steps, [], minutes=15)


# --- 6. two agents in one workspace -----------------------------------------------------------------

def j6() -> dict:
    a_ = {"port": 3021, "auth": "secret:wsa"}
    b_ = {"port": 3022, "auth": "secret:wsb"}
    lock = "/tmp/e2e-agents/ws-a/instance/host-agent.lock"
    third = ("runuser -u opute -- env OPUTE_REMOTE_AGENT_ID=e2e-ws-a MCP_AUTH_TOKEN=${SECRET:wsa} HOST_MCP_PORT=3023 "
             "OPUTE_MCP_PREFIX_TOOL_NAMES=true OPUTE_HOST_AGENT_INSTANCE_ROOT=/tmp/e2e-agents/ws-a/instance "
             "OPUTE_STANDALONE_STATE_DIR=/tmp/e2e-agents/ws-a/state HOME=/tmp/e2e-agents/ws-a/home timeout 30 "
             f"{BIN} serve --transport http")
    steps = TOOL_STEPS + [
        {"id": "agentA", "guest": f"{T} agent start ws-a --port 3021 --id e2e-ws-a --env MCP_AUTH_TOKEN=${{SECRET:wsa}} "
                                  "--env OPUTE_MCP_PREFIX_TOOL_NAMES=true"},
        {"id": "agentB", "guest": f"{T} agent start ws-b --port 3022 --id e2e-ws-b --env MCP_AUTH_TOKEN=${{SECRET:wsb}} "
                                  "--env OPUTE_MCP_PREFIX_TOOL_NAMES=true"},
        {"id": "healthA", "health": True, "port": 3021, "capture": {"PA": "response.body.mcpToolNamePrefix"}},
        {"id": "healthB", "health": True, "port": 3022, "capture": {"PB": "response.body.mcpToolNamePrefix"}},
        mcp("listA", "tools/list", **a_),
        mcp("listB", "tools/list", **b_),
        {"id": "prefixed", "guest": "python3 /opt/opute-e2e/guest/mcpwire.py <<'EOF' > /tmp/e2e-j6-a.json\n"
                                    '{"kind": "mcp", "port": 3021, "token": "${SECRET:wsa}", "method": "tools/list"}\n'
                                    "EOF\npython3 /opt/opute-e2e/guest/mcpwire.py <<'EOF' > /tmp/e2e-j6-b.json\n"
                                    '{"kind": "mcp", "port": 3022, "token": "${SECRET:wsb}", "method": "tools/list"}\n'
                                    "EOF\npython3 - <<'EOF'\nimport json\n"
                                    "na = [t['name'] for t in json.load(open('/tmp/e2e-j6-a.json'))['body']['result']['tools']]\n"
                                    "nb = [t['name'] for t in json.load(open('/tmp/e2e-j6-b.json'))['body']['result']['tools']]\n"
                                    "pa, pb = '${PA}', '${PB}'\n"
                                    "print(json.dumps({'countA': len(na), 'countB': len(nb), "
                                    "'allPrefixedA': all(n.startswith(pa + '_') for n in na), "
                                    "'allPrefixedB': all(n.startswith(pb + '_') for n in nb), "
                                    "'overlap': len(set(na) & set(nb)), 'prefixesDiffer': pa != pb, "
                                    "'unprefixedA': sorted({n[len(pa) + 1:] for n in na}) == sorted({n[len(pb) + 1:] for n in nb})}))\n"
                                    "EOF\nrm -f /tmp/e2e-j6-a.json /tmp/e2e-j6-b.json"},
        call("infoA", "${PA}_get_host_info", **a_),
        call("infoB", "${PB}_get_host_info", **b_),
        {"id": "third", "guest": third + "; echo exit=$?", "timeout": 60},
        {"id": "aStillServes", "health": True, "port": 3021},
        call("aStillCalls", "${PA}_get_host_info", **a_),
        {"id": "stop", "guest": f"{T} agent remove ws-a && {T} agent remove ws-b"},
    ]
    reqs = [
        # /health reports the prefix; the wire name is <prefix>_<name> (e2e-1-r1, retired).
        req(DOCS + "configuration/index.html", "OPUTE_MCP_PREFIX_TOOL_NAMES: prefix wire tool names for "
                                               "multi-agent IDE workspaces",
            ok("agentA"), ok("agentB"),
            a("healthA", "response.body.mcpToolNamePrefix", exists=True),
            a("healthB", "response.body.mcpToolNamePrefix", exists=True),
            ok("prefixed"), a("prefixed", "json.prefixesDiffer", equals=True),
            a("prefixed", "json.allPrefixedA", equals=True), a("prefixed", "json.allPrefixedB", equals=True),
            a("prefixed", "json.overlap", equals=0), a("prefixed", "json.unprefixedA", equals=True),
            a("prefixed", "json.countA", gte=1),
            a("infoA", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            a("infoB", "response.body.result.isError", **{"in": [False, "<absent>"]})),
        req(SPEC, "One exact Host Agent identity",
            a("healthA", "response.body.agentId", equals="e2e-ws-a"),
            a("healthB", "response.body.agentId", equals="e2e-ws-b"),
            a("infoA", "response.body.result.structuredContent.uri", matches=":e2e-ws-a$"),
            a("infoB", "response.body.result.structuredContent.uri", matches=":e2e-ws-b$"),
            scenario="Two agents share a physical machine"),
        req(LOCK_SPEC, "One running agent per instance state",
            a("third", "stdout", equals="exit=1\n"),
            a("third", "stderr", equals=f"host agent instance is already in use: {lock} is locked by another process\n"),
            a("aStillServes", "response.body.agentId", equals="e2e-ws-a"),
            a("aStillCalls", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            scenario="A second agent on the same instance root"),
    ]
    return journey(
        "e2e1.two-agents",
        "Flow 1, two agents in one workspace: two Rust agents as one user with distinct IDs, ports, tokens and "
        "instance roots and OPUTE_MCP_PREFIX_TOOL_NAMES=true; each publishes only its own prefixed names and the "
        "/health prefix; a third process on the first agent's instance root is refused as D25 specifies while the "
        "first keeps serving.",
        {"agent": "two Rust agents started for the journey under /tmp/e2e-agents (ports 3021, 3022)",
         "files": TOOL_FILES}, reqs, steps, [], minutes=15)


# --- 7. restart and recovery -------------------------------------------------------------------------

def j7() -> dict:
    files = TOOL_FILES
    term_plan = plan("e2e1-j7-term", "e2e1-j7-term-1", ".config/opute-e2e/j7-term.txt", "${SECRET:j7content}", True)
    kill_plan = plan("e2e1-j7-kill", "e2e1-j7-kill-1", ".config/opute-e2e/j7-kill.txt", "${SECRET:j7content}", True)
    in_flight = {"path": "response.body.result.structuredContent.nodes.n2.attempts", "in": [1]}
    steps = TOOL_STEPS + [
        {"id": "slow", "guest": f"{T} slow start --port 8799 --delay 30"},
        call("termRun", "run_host_plan", {"plan": term_plan}, capture={"TERM_RUN": "response.body.result.structuredContent.runId"}),
        call("termInFlight", "get_host_plan_run", {"runId": "${TERM_RUN}"}, until=in_flight, timeout=30),
        {"id": "term", "guest": f"systemctl kill -s TERM {UNIT} && for i in $(seq 60); do systemctl is-active -q "
                                f"{UNIT} || break; sleep 0.5; done; {T} unit {UNIT}"},
        {"id": "termStart", "guest": f"systemctl start {UNIT} && {WAIT_HEALTH.format(port=3014)} && {T} unit {UNIT}"},
        mcp("termList", "tools/list"),
        call("termAfter", "get_host_plan_run", {"runId": "${TERM_RUN}"}),
        {"id": "termDb", "guest": f"{T} db {SDB} --plan-id e2e1-j7-term"},
        call("killRun", "run_host_plan", {"plan": kill_plan}, capture={"KILL_RUN": "response.body.result.structuredContent.runId"}),
        call("killInFlight", "get_host_plan_run", {"runId": "${KILL_RUN}"}, until=in_flight, timeout=30),
        {"id": "kill", "guest": f"systemctl kill -s KILL {UNIT} && sleep 1 && {WAIT_HEALTH.format(port=3014)} && "
                                f"{T} unit {UNIT}"},
        mcp("killList", "tools/list"),
        call("killAfter", "get_host_plan_run", {"runId": "${KILL_RUN}"}),
        {"id": "killDb", "guest": f"{T} db {SDB} --plan-id e2e1-j7-kill"},
        # The run's in-memory task (id = run id) is persisted only on failure or resume, so after the
        # SIGKILL there is no snapshot for restart reclaim to act on (Q15).
        mcp("killTask", "tasks/get", params={"taskId": "${KILL_RUN}"}, meta=TASKS_META),
        call("resubmit", "run_host_plan", {"plan": kill_plan}),
        {"id": "integrity", "guest": f"{T} db {SDB}"},
        {"id": "secretInDb", "guest": f"{T} contains {SDB} '${{SECRET:j7content}}'"},
        {"id": "secretInJournal", "guest": f"journalctl -u {UNIT} --no-pager -o cat | grep -c -F "
                                           "'${SECRET:j7content}'; true"},
        {"id": "slowStop", "guest": f"{T} slow stop"},
    ]
    cleanup = [{"id": "remove", "guest": "rm -rf /home/opute/.config/opute-e2e"}]
    reqs = [
        req(SPEC, "Durable and redacted observations",
            ok("slow"), a("termInFlight", "attempts", gte=1),
            a("termInFlight", "response.body.result.structuredContent.nodes.n2.attempts", equals=1),
            ok("term"), a("termStart", "json.ActiveState", equals="active"),
            a("termList", "response.status", equals=200),
            a("termAfter", "response.body.result.structuredContent.runId",
              equalsStep={"step": "termRun", "path": "response.body.result.structuredContent.runId"}),
            a("termDb", "json.integrity", equals="ok"),
            a("killInFlight", "response.body.result.structuredContent.nodes.n2.attempts", equals=1),
            a("kill", "json.ActiveState", equals="active"), a("kill", "json.NRestarts", matches="^[1-9][0-9]*$"),
            a("killList", "response.status", equals=200),
            a("killAfter", "response.body.result.structuredContent.runId",
              equalsStep={"step": "killRun", "path": "response.body.result.structuredContent.runId"}),
            a("killDb", "json.integrity", equals="ok"), a("integrity", "json.integrity", equals="ok"),
            a("secretInDb", "json.rows", equals=0), a("secretInJournal", "stdout", equals="0\n"),
            scenario="a run in flight survives SIGTERM and SIGKILL restarts as one durable record; the store "
                     "stays consistent and the write-only file content reaches no durable row or log"),
        req(SPEC, "Durable and redacted observations",
            a("termDb", "json.planRunIds[0]", equalsStep={"step": "termRun", "path": "response.body.result.structuredContent.runId"}),
            a("termDb", "json.planRunIds[1]", absent=True),
            a("killDb", "json.planRunIds[0]", equalsStep={"step": "killRun", "path": "response.body.result.structuredContent.runId"}),
            a("killDb", "json.planRunIds[1]", absent=True),
            scenario="exactly one durable run per interrupted plan: nothing lost or duplicated"),
        req(Q5_SPEC, "An interrupted plan or task reads back truthfully",
            a("termAfter", "response.body.result.structuredContent.status", equals="cancelled"),
            a("killAfter", "response.body.result.structuredContent.status", equals="unknown"),
            scenario="A plan in flight reads back cancelled after SIGTERM and unknown after SIGKILL, and neither "
                     "runs again (D28)"),
        req(Q15_SPEC, "A crashed plan run's orphaned reservation is reclaimed without a task",
            a("resubmit", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            a("resubmit", "response.body.result.structuredContent.code", absent=True),
            scenario="After a SIGKILL during a plain run_host_plan, whose task was never persisted, the "
                     "resubmission is admitted (D28)"),
    ]
    return journey(
        "e2e1.restart",
        "Flow 14, restart and recovery: the system agent is stopped with SIGTERM, then killed with SIGKILL (the "
        "unit's Restart=on-failure restarts it) each time with a host plan in flight (its first node wrote a file "
        "with write-only content, its second waits on a slow local endpoint). After each restart the run is "
        "served as one durable record, the store passes its integrity check, and the write-only content is in no "
        "durable row and no journal line. The resubmission after the SIGKILL is admitted (D28, Q15).",
        {"agent": AGENT, "files": files, "slow": "a local endpoint answering after 30 s (e2etool slow)"},
        reqs, steps, cleanup, allow=["files//home/opute/.config/opute-e2e*", "system/units/e2e-slow*",
                                     "system/unitFiles/e2e-slow*"], minutes=15)


# --- 8. migrate a Go-installed host, then upgrade -------------------------------------------------------

def j8() -> dict:
    files = {**TOOL_FILES, "rustNext": inp(CANDIDATE_NEXT + "/host-agent-linux-x64.gz")}
    done_plan = plan("e2e1-j8-done", "e2e1-j8-done-1", ".config/opute-e2e/j8-done.txt")
    flight_plan = plan("e2e1-j8-flight", "e2e1-j8-flight-1", ".config/opute-e2e/j8-flight.txt", slow=True)
    run_id = lambda step: {"step": step, "path": "response.body.result.structuredContent.runId"}  # noqa: E731
    steps = TOOL_STEPS + [
        {"id": "nextCopy", "copy": "rustNext", "to": "/opt/opute-e2e/releases/rust-next/host-agent-linux-x64.gz"},
        {"id": "aside", "guest": f"systemctl stop {UNIT} && mv /home/opute/.config/opute/instances "
                                 "/tmp/e2e-j8-rust-instances"},
        swap("goInstall", f"{GO_REL}/host-agent-linux-x64.gz"),
        {"id": "goHealth", "health": True},
        mcp("goList", "tools/list"),
        {"id": "slow", "guest": f"{T} slow start --port 8799 --delay 60"},
        call("goDone", "run_host_plan", {"plan": done_plan}, capture={"DONE": "response.body.result.structuredContent.runId"}),
        poll_run("goDoneWait", "DONE"),
        call("goFlight", "run_host_plan", {"plan": flight_plan}, capture={"FLIGHT": "response.body.result.structuredContent.runId"}),
        call("goInFlight", "get_host_plan_run", {"runId": "${FLIGHT}"},
             until={"path": "response.body.result.structuredContent.nodes.n2.attempts", "in": [1]}, timeout=30),
        {"id": "goStop", "guest": f"systemctl stop {UNIT} && {T} db {SDB}"},
        swap("rustInstall", f"{REL}/host-agent-linux-x64.gz"),
        {"id": "rustHealth", "health": True},
        mcp("rustList", "tools/list"),
        {"id": "rustDb", "guest": f"{T} db {SDB}"},
        call("rustDone", "get_host_plan_run", {"runId": "${DONE}"}),
        call("rustFlight", "get_host_plan_run", {"runId": "${FLIGHT}"}),
        swap("upgrade", "/opt/opute-e2e/releases/rust-next/host-agent-linux-x64.gz"),
        {"id": "nextHealth", "health": True},
        mcp("nextList", "tools/list"),
        {"id": "nextDb", "guest": f"{T} db {SDB}"},
        call("nextDone", "get_host_plan_run", {"runId": "${DONE}"}),
        call("nextFlight", "get_host_plan_run", {"runId": "${FLIGHT}"}),
        {"id": "slowStop", "guest": f"{T} slow stop"},
    ]
    cleanup = [{"id": "restoreRust", "guest": f"systemctl stop {UNIT} && {install_binary(f'{REL}/host-agent-linux-x64.gz')} "
                                              "&& rm -rf /home/opute/.config/opute/instances && mv /tmp/e2e-j8-rust-instances "
                                              f"/home/opute/.config/opute/instances && systemctl start {UNIT} && "
                                              f"{WAIT_HEALTH.format(port=3014)} && rm -rf /home/opute/.config/opute-e2e "
                                              "/opt/opute-e2e/releases/rust-next"}]
    reqs = [
        req(SPEC, "Durable and redacted observations",
            ok("aside"), ok("goInstall"), a("goInstall", "stdout", matches="\n0\\.2\\.4\n$"),
            a("goDoneWait", "response.body.result.structuredContent.status", equals="completed"),
            a("goInFlight", "response.body.result.structuredContent.nodes.n2.attempts", equals=1),
            ok("goStop"), a("goStop", "json.integrity", equals="ok"),
            ok("rustInstall"), a("rustInstall", "stdout", matches=RUST_MATCH),
            a("rustList", "response.status", equals=200),
            a("rustDb", "json.integrity", equals="ok"),
            a("rustDb", "json.planRunIds", equalsStep={"step": "goStop", "path": "json.planRunIds"}),
            a("rustDb", "json.operationIds", equalsStep={"step": "goStop", "path": "json.operationIds"}),
            a("rustDb", "json.counts", equalsStep={"step": "goStop", "path": "json.counts"}),
            a("rustDone", "response.body.result.structuredContent.runId", equalsStep=run_id("goDone")),
            a("rustDone", "response.body.result.structuredContent.status", equals="completed"),
            a("rustFlight", "response.body.result.structuredContent.runId", equalsStep=run_id("goFlight")),
            a("rustFlight", "response.body.result.structuredContent.status", exists=True),
            scenario="a Go v0.2.4 install's state, with a run stopped in flight, is served by Rust on the same "
                     "instance root: every operation and run is there once, unchanged (D1: the schema is "
                     "bit-compatible, so conversion is the identity)"),
        req(SPEC, "Compatible modes, configuration, and distribution",
            ok("upgrade"), a("upgrade", "stdout", matches="\n" + re.escape(NEXT_VERSION) + "\n$"),
            a("nextList", "response.status", equals=200), a("nextDb", "json.integrity", equals="ok"),
            a("nextDb", "json.planRunIds", equalsStep={"step": "goStop", "path": "json.planRunIds"}),
            a("nextDb", "json.operationIds", equalsStep={"step": "goStop", "path": "json.operationIds"}),
            a("nextDb", "json.counts", equalsStep={"step": "goStop", "path": "json.counts"}),
            a("nextDone", "response.body.result.structuredContent.status", equals="completed"),
            a("nextFlight", "response.body.result.structuredContent.runId", equalsStep=run_id("goFlight")),
            scenario="an in-place upgrade to a newer local Rust build keeps the state, unchanged"),
        req(SPEC, "One exact Host Agent identity",
            a("goHealth", "response.body.agentId", equals="e2e-devvm-agent"),
            a("rustHealth", "response.body.agentId", equals="e2e-devvm-agent"),
            a("nextHealth", "response.body.agentId", equals="e2e-devvm-agent")),
        req(Q5_SPEC, "An interrupted plan or task reads back truthfully",
            a("rustFlight", "response.body.result.structuredContent.status", equals="cancelled"),
            a("nextFlight", "response.body.result.structuredContent.status", equals="cancelled"),
            scenario="A run Go stopped gracefully in flight reads back cancelled after the migration and the "
                     "upgrade, and is not run again (D28)"),
    ]
    return journey(
        "e2e1.migrate-upgrade",
        "Flow 14, migration and upgrade: the host is reinstalled as a Go v0.2.4 install (the published release, "
        "the same unit, path, instance root and agent ID) and does work, one run completed and one stopped in "
        f"flight; the binary is then replaced by the Rust {RUST_VERSION} release candidate, and then upgraded in place to "
        f"a newer local Rust build ({NEXT_VERSION}, built locally; no tag, no publish). Go is the source install, "
        "not an oracle.",
        {"agent": AGENT, "files": files, "go": f"{GO_REL} (published Go v0.2.4 release, SHA256SUMS pinned)",
         "next": f"Rust release set built locally with VERSION={NEXT_VERSION}"},
        reqs, steps, cleanup,
        allow=["files//home/opute/.local/share/opute/opute-host-agent/sha256", "files//home/opute/.config/opute-e2e*",
               "files//opt/opute-e2e/releases/rust-next*", "system/units/e2e-slow*", "system/unitFiles/e2e-slow*"],
        minutes=20)


# --- e2e-0: the step kinds e2e-1 added ---------------------------------------------------------------

def step_kinds() -> dict:
    files = {**TOOL_FILES, "probe": inp("${REPO}/evidence/e2e/journeys/selfcheck.noop.json")}
    steps = TOOL_STEPS + [
        {"id": "copy", "copy": "probe", "to": "/tmp/e2e-copy-probe.json"},
        {"id": "copied", "guest": "sha256sum /tmp/e2e-copy-probe.json | cut -c1-64 | tr -d '\\n' | "
                                  "python3 -c \"import json,sys; print(json.dumps({'sha256': sys.stdin.read()}))\""},
        {"id": "secret", "guest": "printf '{\"s\": \"%s\"}' '${SECRET:probe}'", "capture": {"S": "json.s"}},
        {"id": "http", "http": "GET", "path": "/health"},
        {"id": "guestHealth", "health": True, "port": 3014, "capture": {"AGENT": "response.body.agentId"}},
        mcp("guestList", "tools/list", port=3014),
        call("guestInfo", "get_host_info", port=3014, until={"path": "response.status", "in": [200]}, timeout=10),
        {"id": "expanded", "guest": "printf '{\"agent\": \"%s\"}' '${AGENT}'"},
    ]
    cleanup = [{"id": "rm", "guest": "rm -f /tmp/e2e-copy-probe.json"}]
    reqs = [req("evidence/e2e/README.md", "Harness: the e2e-1 step kinds work",
                a("copy", "sha256", equals=files["probe"]["sha256"]),
                a("copied", "json.sha256", equalsStep={"step": "copy", "path": "sha256"}),
                a("secret", "json.s", equals="<secret:probe>"),
                a("http", "response.status", equals=200),
                a("http", "response.body.agentId", equalsStep={"step": "guestHealth", "path": "response.body.agentId"}),
                a("guestList", "response.status", equals=200), a("guestList", "target", equals="guest:3014"),
                a("guestInfo", "attempts", equals=1),
                a("guestInfo", "response.body.result.structuredContent.uri", matches=":e2e-devvm-agent$"),
                a("expanded", "json.agent", equals="e2e-devvm-agent"))]
    return journey("selfcheck.step-kinds",
                   "Positive control for the step kinds e2e-1 added: a hash-pinned copy, a per-run secret that is "
                   "redacted from the record, a plain HTTP request, health and MCP requests made inside the guest "
                   "(schema-validated against the guest port's own tools/list), polling, and captured values.",
                   {"agent": AGENT, "files": files}, reqs, steps, cleanup, gate="e2e-0", minutes=10)


JOURNEYS = [j1, j2, j3, j4, j5, j6, j7, j8, step_kinds]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
