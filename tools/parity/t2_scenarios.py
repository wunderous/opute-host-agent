#!/usr/bin/env python3
"""Generate scenarios-t2/host-domain.json: the M8a host-domain T2 matrix.

Run from tools/parity: python3 t2_scenarios.py scenarios-t2/host-domain.json

Each scenario runs in two disposable twin VMs booted from one golden image
(parity/t2.py). A "snapshot" step with a baseline is compared as the
delta against it: files, both service managers, lingering and firewall.
Every scenario ends with a post-teardown snapshot against the first
baseline, so residue is compared too; residue Go leaves behind is listed
in the scenario's "residue" note and must be reproduced, never cleaned
up by the harness.
"""
import hashlib
import json
import re
import sys

TS = "wall-clock timestamp of this twin's run"
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")]
SERVE = {"as": "serve", "serve": {}}
# get_host_info lists, and stale-revision errors name, the served catalog:
# neutral names and the Rust revision on the Rust side (owner decision D19).
D19 = "D19.neutral-host-tool-names"
STOP = {"as": "stop", "stop": {}}
DBUS = "host-service:local:system/dbus.service"


def snap(as_, baseline=None):
    return {"as": as_, "snapshot": {"baseline": baseline} if baseline else {}}


def call(as_, name, args):
    return {"as": as_, "mcp": {"method": "tools/call", "name": name, "params": {"name": name, "arguments": args}}}


def task_call(as_, name, args, timeout=600):
    """A task-aware tool answers with a task handle; poll tasks/get until terminal."""
    var = "T_" + re.sub(r"[^A-Z0-9]", "_", as_.upper())
    step = call(as_, name, args)
    step["mcp"]["capture"] = {var: ["body", "result", "taskId"]}
    done = {"as": as_ + "Done", "mcp": {
        "method": "tasks/get", "name": "${%s}" % var, "params": {"taskId": "${%s}" % var},
        "until": {"path": ["body", "result", "status"], "in": ["completed", "failed", "cancelled"]},
        "timeout": timeout}}
    return [step, done]


def shell(as_, argv, user="root"):
    return {"as": as_, "shell": {"argv": argv, "user": user}}


def flat(steps):
    out = []
    for step in steps:
        out.extend(step if isinstance(step, list) else [step])
    return out


# T2 calls use Go's tool names: Go knows no other, and Rust accepts them as
# the hidden D19 aliases (the aliases themselves are compared in T1).
# Real host commands each scenario executes in the sandbox (not shims): the
# inventory capture records them as the command's T2 effect tier. Matches
# the commands the same tools run in their T1 shim traces.
COMMANDS = {
    "services.user-lifecycle": ["loginctl", "systemctl"],
    "services.system-lifecycle": ["sudo", "systemctl"],
    "exec.workloads": ["loginctl", "systemd-run"],
    "policy.user-agent-unit": ["systemctl", "systemd-run"],
    "policy.system-agent-unit": ["systemctl"],
    "connection.restart-user-service": ["loginctl", "systemctl"],
    "public.tunnel-and-quick": ["loginctl", "systemctl"],
    "tools.apt-ninja": ["sudo", "apt-get"],
    "artifacts.download-and-agent-build": ["sudo", "apt-get", "bash", "systemd-run"],
    "bridge.recover": ["systemctl"],
    "virt.stack-install-uninstall": ["sudo", "apt-get", "apt-cache", "curl", "dpkg", "dpkg-query", "install",
                                     "usermod", "systemctl", "rm", "incus"],
}


def scenario(id_, steps, anchors, residue, masks=(), sets=(), parse_json=(), rejected_only=False,
             divergences=()):
    compare = {"masks": MASKS + list(masks)}
    if divergences:
        compare["divergences"] = list(divergences)
    if parse_json:
        compare["parseJson"] = list(parse_json)
    if sets:
        compare["sets"] = list(sets)
    out = {"id": "t2.host-domain." + id_, "tier": "T2", "owner": "host", "anchors": anchors,
           "commands": COMMANDS.get(id_, []), "residue": residue, "steps": flat(steps), "compare": compare}
    if rejected_only:
        out["rejectedOnly"] = True
    return out


def done(as_):
    """The terminal tasks/get result of a task_call step."""
    return ["steps", as_ + "Done", "body", "result", "result"]


# systemd-run --wait (the workload launcher) frames the workload's own
# stderr with a per-run unit name, sometimes an invocation id, and resource
# accounting. The mask pins everything that is the workload's outcome (its
# own stderr, the result and the exit status) and admits only the run-local
# values around it.
def workload_stderr(path, own, result, terminated, prefix="", trimmed=False):
    pattern = (re.escape(prefix) + r"Running as unit: opute-host-workload-[0-9a-f]{24}\.service(; invocation ID: [0-9a-f]{32})?\n"
               + re.escape(own) + r"Finished with result: " + re.escape(result) + r"\n"
               r"Main processes terminated with: " + re.escape(terminated) + r"\n"
               r"Service runtime: [0-9.]+m?s\nCPU time consumed: [0-9.]+m?s\n"
               r"Memory peak: [0-9.]+[BKMG]?\nMemory swap peak: [0-9.]+[BKMG]?" + ("" if trimmed else r"\n"))
    return {"path": path, "type": "string", "match": pattern,
            "reason": "systemd-run per-run unit name, invocation id and resource accounting"}


COORDINATOR = "/home/ubuntu/.config/opute/host-resource-coordinator"
RESERVATIONS = COORDINATOR + "/reservations.json"
RESERVATION_MASK = {"path": ["steps", "*", "files", "*", RESERVATIONS, "sha256"], "type": "hex", "optional": True,
                    "reason": "reservation ledger records per-run ids and times; its presence, mode and owner are compared"}


SERVICE = ["internal/domain/host/host_service.go", "internal/domain/host/file.go",
           "internal/domain/host/service_probe.go", "internal/tools/dispatch_unassigned.go"]
UNIT = ("[Unit]\nDescription=Opute parity T2 web\n\n[Service]\nExecStart=/bin/sleep infinity\n\n"
        "[Install]\nWantedBy=default.target\n")


def state(as_, service, st, scope):
    return call(as_, "set_host_service_state", {"uri": DBUS, "serviceName": service, "state": st, "scope": scope})


LINGER = shell("linger", ["loginctl", "enable-linger", "ubuntu"])
WAIT_USER = shell("userManager", ["sh", "-c", "for i in $(seq 1 100); do [ -S /run/user/1000/bus ] && exit 0; "
                                               "sleep 0.1; done; exit 1"])


def wait_user(as_="userManager"):
    """ensure_host_service_supervisor enables lingering; the user manager then starts asynchronously."""
    return dict(WAIT_USER, **{"as": as_})


USER_URI = "host-service:local:user/parity-web.service"

cases = [
    scenario("services.user-lifecycle", [
        snap("baseline"), SERVE,
        call("supervisor", "ensure_host_service_supervisor", {}),
        wait_user(),
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        call("supervisorInspect", "inspect_host_service_supervisor", {}),
        call("unit", "ensure_host_file", {"path": "~/.config/systemd/user/parity-web.service", "content": UNIT}),
        state("enable", "parity-web.service", "enable", "user"),
        state("start", "parity-web.service", "start", "user"),
        call("inspectNoUri", "inspect_host_service", {"serviceName": "parity-web.service", "scope": "user"}),
        call("inspect", "inspect_host_service", {"uri": USER_URI}),
        snap("running", "baseline"),
        state("restart", "parity-web.service", "restart", "user"),
        state("stop", "parity-web.service", "stop", "user"),
        state("disable", "parity-web.service", "disable", "user"),
        task_call("remove", "remove_host_file", {"path": "~/.config/systemd/user/parity-web.service", "confirm": True}),
        state("stopMissing", "parity-web.service", "stop", "user"),
        snap("removed", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], SERVICE, masks=[{
        "path": ["steps", "supervisor", "body", "result", "content", 0, "text"], "type": "string",
        "match": r"Error: user service supervisor bus is unavailable: Failed to connect to bus: "
                 r"(No such file or directory|Permission denied|Connection refused)",
        "reason": "Go probes the user bus right after enabling lingering, racing systemd's start of "
                  "user@1000; which connect error it sees is timing. The linger effect itself and the "
                  "settled second call are compared exactly"}],
        residue="Lingering for ubuntu stays enabled (Go has no inverse of ensure_host_service_supervisor), "
                "and the user unit directory it created stays."),
]

EXEC = ["internal/hostruntime/runtime.go:RunHostWorkloadContext", "internal/exec/process.go",
        "internal/tools/dispatch_unassigned.go"]
POLICY_REV = "opute-host-resource-policy.v2"  # resource.HostResourcePolicyRevision
POLICY = ["internal/domain/host/resource_policy.go", "internal/resource/service.go:Reconcile"]
CONNECT = ["internal/domain/host/generic_agent_connection.go", "internal/domain/host/host_service.go"]
FIREWALL = ["internal/domain/host/firewall.go"]
SYSUNIT = "/etc/systemd/system/parity-sys.service"

OK = ("success", "code=exited/status=0")
WORKLOADS = [("write", "") + OK, ("fails", "err\n", "exit-code", "code=exited/status=3"), ("quiet", "") + OK,
             ("timeout", "", "timeout", "code=killed/status=TERM"), ("env", "") + OK]

cases += [
    scenario("services.system-lifecycle", [
        snap("baseline"), SERVE,
        call("unitDenied", "ensure_host_file", {"path": SYSUNIT, "content": UNIT, "scope": "system"}),
        {"as": "unitByRoot", "write": {"path": SYSUNIT, "content": UNIT}},
        state("enable", "parity-sys.service", "enable", "system"),
        state("start", "parity-sys.service", "start", "system"),
        call("inspect", "inspect_host_service", {"uri": "host-service:local:system/parity-sys.service"}),
        call("supervisor", "ensure_host_service_supervisor", {"scope": "system"}),
        snap("running", "baseline"),
        call("restart", "restart_host_service", {"uri": DBUS, "serviceName": "parity-sys.service"}),
        state("restartState", "parity-sys.service", "restart", "system"),
        state("stop", "parity-sys.service", "stop", "system"),
        state("disable", "parity-sys.service", "disable", "system"),
        task_call("removeDenied", "remove_host_file", {"path": SYSUNIT, "scope": "system", "confirm": True}),
        shell("removeByRoot", ["rm", "-f", SYSUNIT]),
        state("reloadAfter", "parity-sys.service", "stop", "system"),
        snap("removed", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], SERVICE, masks=[{"path": ["steps", "unitDenied", "body", "result", "content", 0, "text"], "type": "string",
                        "reason": "Go names its temporary file with a random suffix",
                        "match": r"Error: create managed host file temporary: open /etc/systemd/system/"
                                 r"\.opute-host-file-[0-9]+: permission denied"}],
        residue="None from the agent: the unit file is written and removed by the harness as root, because "
                "the agent runs unprivileged and system-scope file writes are refused."),
    scenario("exec.workloads", [
        snap("baseline"), SERVE,
        LINGER, WAIT_USER,
        call("supervisor", "ensure_host_service_supervisor", {}),
        task_call("write", "run_host_command", {"command": "echo hello > \"$HOME/parity-out.txt\" && echo written"}),
        task_call("fails", "run_host_command", {"command": "echo err >&2; exit 3"}),
        task_call("quiet", "run_host_command", {"command": "true"}),
        task_call("timeout", "run_host_command", {"command": "sleep 30", "timeoutMs": 1500}),
        call("shell", "agent_shell", {"command": "id -un; pwd"}),
        task_call("env", "run_host_command", {"command": "systemctl --user show -p Slice --value \"$(cat /proc/self/cgroup "
                                                    "| sed -n 's#.*/\\([^/]*\\.service\\)$#\\1#p')\" 2>/dev/null; true"}),
        snap("after", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], EXEC, "The file the first command wrote, and lingering (enabled by the harness so the supervisor call is not a race).",
        masks=[workload_stderr(done(k) + ["structuredContent", "stderr"], own, r, t)
               for k, own, r, t in WORKLOADS]
        + [workload_stderr(done(k) + ["content", 0, "text"], own, r, t)
           for k, own, r, t in WORKLOADS if k in ("fails", "quiet", "timeout")]
        + [workload_stderr(["steps", "shell", "body", "result", "structuredContent", "stderr"], "", "success",
                           "code=exited/status=0")]
        + [RESERVATION_MASK]),
    scenario("policy.user-agent-unit", [
        snap("baseline"), LINGER, WAIT_USER,
        {"as": "serve", "serve": {"manager": "user", "unit": "opute-host-agent.service"}},
        call("reconcile", "reconcile_host_resource_policy",
             {"policyRevision": POLICY_REV, "uri": "host-service:local:user/opute-host-agent.service"}),
        call("again", "reconcile_host_resource_policy",
             {"policyRevision": POLICY_REV, "uri": "host-service:local:user/opute-host-agent.service"}),
        call("staleRevision", "reconcile_host_resource_policy",
             {"policyRevision": "opute-host-resource-policy.v1", "uri": "host-service:local:user/opute-host-agent.service"}),
        task_call("workload", "run_host_command", {"command": "cat /proc/self/cgroup"}),
        call("wrongUnit", "reconcile_host_resource_policy",
             {"policyRevision": POLICY_REV, "uri": "host-service:local:user/parity-web.service"}),
        snap("after", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], POLICY + EXEC, masks=[workload_stderr(done("workload") + ["structuredContent", "stderr"], "", *OK)],
        residue="The two managed slice units and the agent unit's persisted property drop-ins; lingering "
                      "(enabled by the harness before the agent starts)."),
    scenario("policy.system-agent-unit", [
        snap("baseline"),
        {"as": "serve", "serve": {"user": "root", "unit": "opute-host-agent.service"}},
        call("reconcile", "reconcile_host_resource_policy",
             {"policyRevision": POLICY_REV, "uri": "host-service:local:system/opute-host-agent.service"}),
        call("again", "reconcile_host_resource_policy",
             {"policyRevision": POLICY_REV, "uri": "host-service:local:system/opute-host-agent.service"}),
        snap("after", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], POLICY, "The two managed system slice units and the agent unit's persisted property drop-ins."),
    scenario("connection.restart-user-service", [
        snap("baseline"), SERVE,
        LINGER, WAIT_USER,
        call("supervisor", "ensure_host_service_supervisor", {}),
        call("unit", "ensure_host_file", {"path": "~/.config/systemd/user/parity-web.service", "content": UNIT}),
        state("start", "parity-web.service", "start", "user"),
        call("configure", "configure_agent_connection", {
            "envFile": "/home/ubuntu/.config/parity/agent.env", "environment": {"OPUTE_PARITY": "one"},
            "serviceName": "parity-web.service", "restart": True, "scope": "user"}),
        call("rewrite", "configure_agent_connection", {
            "envFile": "/home/ubuntu/.config/parity/agent.env", "environment": {"OPUTE_PARITY": "two"},
            "remove": ["MISSING"]}),
        call("badRestart", "configure_agent_connection", {
            "envFile": "/home/ubuntu/.config/parity/agent.env", "environment": {"OPUTE_PARITY": "three"},
            "serviceName": "parity-none.service", "restart": True, "scope": "user"}),
        snap("after", "baseline"),
        state("stop", "parity-web.service", "stop", "user"),
        task_call("remove", "remove_host_file", {"path": "~/.config/systemd/user/parity-web.service", "confirm": True}),
        STOP, snap("teardown", "baseline"),
    ], CONNECT + SERVICE, "The connection env file, lingering (enabled by the harness), and the user unit directory."),
]

# A stand-in connector: it prints a quick-tunnel origin, as cloudflared
# does, and then idles. Served from inside the twin over a throwaway CA;
# the quick-tunnel hostname resolves to the twin's own loopback, so no
# probe and no tunnel ever leaves the VM.
CONNECTOR = "#!/bin/sh\necho 'INF |  https://parity-t2.trycloudflare.com  |' >&2\nexec sleep infinity\n"
CONNECTOR_SHA = hashlib.sha256(CONNECTOR.encode()).hexdigest()
TLS = "/opt/parity-t2/tls"
STUB_PORT = 8443
ARTIFACT = f"https://127.0.0.1:{STUB_PORT}/cloudflared"
PUBLIC = ["internal/domain/host/public_mcp.go:EnsurePublicMcpTunnel",
          "internal/domain/host/public_mcp_quick.go:EnsurePublicMcpQuickTunnel",
          "internal/domain/host/public_mcp_quick.go:RemovePublicMcpQuickTunnel", "internal/mcpprobe/probe.go"]
TLS_SETUP = [
    shell("tlsInit", ["python3", "/opt/parity-t2/tlsstub.py", "init", TLS]),
    {"as": "connector", "write": {"path": TLS + "/artifact", "content": CONNECTOR}},
    shell("tlsServe", ["systemd-run", "--unit=parity-tls.service", "--collect", "python3", "/opt/parity-t2/tlsstub.py",
                       "serve", TLS, str(STUB_PORT)]),
    shell("tlsReady", ["sh", "-c", f"for i in $(seq 1 100); do python3 -c 'import socket; "
                                   f"socket.create_connection((\"127.0.0.1\", {STUB_PORT}), 1)' 2>/dev/null && exit 0; "
                                   "sleep 0.1; done; exit 1"]),
    shell("quickHost", ["sh", "-c", "echo '127.0.0.1 parity-t2.trycloudflare.com mcp.parity-t2.test' >> /etc/hosts"]),
]
TLS_SERVE = {"as": "serve", "serve": {"env": {"SSL_CERT_FILE": TLS + "/ca.pem"}}}
APT = ["internal/domain/host/host_tool.go", "internal/domain/host/incus_install.go",
       "internal/domain/host/incus_uninstall.go", "internal/hostruntime/elevation.go"]

cases += [
    scenario("public.tunnel-and-quick", TLS_SETUP + [
        snap("baseline"), TLS_SERVE,
        LINGER, WAIT_USER,
        task_call("tunnel", "ensure_public_mcp_tunnel", {
            "bindingId": "parity-public", "endpoint": "https://mcp.parity-t2.test/mcp",
            "localTarget": "http://127.0.0.1:9/mcp", "tunnelToken": "parity-tunnel-token-xxxxxxxxxxxxxxxxxxxxxxxx",
            "artifactUri": ARTIFACT, "artifactSha256": CONNECTOR_SHA}),
        task_call("quick", "ensure_public_mcp_quick_tunnel", {
            "bindingId": "parity-quick", "localTarget": "http://127.0.0.1:9/mcp",
            "artifactUri": ARTIFACT, "artifactSha256": CONNECTOR_SHA}, timeout=200),
        snap("installed", "baseline"),
        task_call("quickRemove", "remove_public_mcp_quick_tunnel", {"bindingId": "parity-quick", "confirm": True}),
        snap("removed", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], PUBLIC, "The named tunnel's artifact, token file and user unit (Go has no remove for a named tunnel), "
               "lingering, and whatever the quick removal leaves; the harness's TLS stub and hosts entry predate "
               "the baseline."),
    scenario("tools.apt-ninja", [
        snap("baseline"), SERVE,
        task_call("install", "ensure_host_tool", {"tool": "ninja"}, timeout=600),
        task_call("again", "ensure_host_tool", {"tool": "ninja"}),
        task_call("notOffered", "ensure_host_tool", {"tool": "jq"}),
        snap("after", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], APT, "The installed ninja-build package: ensure_host_tool has no inverse."),
    scenario("virt.stack-install-uninstall", [
        snap("baseline"), SERVE,
        task_call("refused", "uninstall_incus_stack", {}),
        task_call("install", "install_incus_stack", {}, timeout=1800),
        snap("installed", "baseline"),
        # install grants ubuntu incus-admin, which a running process does not
        # gain; an operator restarts the agent, and so does the harness.
        {"as": "stopForGroup", "stop": {}},
        {"as": "serveWithGroup", "serve": {}},
        task_call("installAgain", "install_incus_stack", {}, timeout=1800),
        snap("initialized", "baseline"),
        task_call("uninstall", "uninstall_incus_stack", {"confirm": True}, timeout=1800),
        snap("removed", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], APT, masks=[
        {"path": ["steps", "*", "files", "*", f"/var/lib/incus/server.{ext}", "sha256"], "type": "hex",
         "optional": True, "reason": "incusd generates its own server key pair on first start"}
        for ext in ("crt", "key")] + [
        {"path": ["steps", "*", "files", "*", "/var/lib/incus/networks/incusbr0/dnsmasq.pid", "sha256"],
         "type": "hex", "optional": True, "reason": "the bridge's dnsmasq writes its own process id"}] + [{
            "path": done("install") + ["content", 0, "text"], "type": "string",
            "reason": "the incus CLI's first-run hint names a randomly chosen example image",
            "match": r"Error: initialize Incus container runtime: inspect storage pools: If this is your first "
                     r"time running Incus on this machine, you should also run: incus admin init\n"
                     r"To start your first container, try: incus launch images:[a-z0-9./-]+\n"
                     r"Or for a virtual machine: incus launch images:[a-z0-9./-]+ --vm\n\n"
                     r"Error: You don't have the needed permissions to talk to the incus daemon "
                     r"\(socket path: /var/lib/incus/unix\.socket\)"}],
        residue="Inventoried from Go's own run: what uninstall leaves of the package set, the apt source and key, "
            "and group membership."),
]

def _t1_archives():
    """The T1 archive payloads (zstd tars: basic, GNU and PAX long names, legacy, sparse)."""
    with open("scenarios/host-domain.json") as f:
        t1 = {sc["id"]: sc for sc in json.load(f)}["host-domain.archives.valid"]
    out = []
    for step in t1["steps"]:
        if "write" in step and step["write"]["path"].endswith(".tar.zst"):
            out.append((step["write"]["path"].rsplit("/", 1)[-1][:-len(".tar.zst")], step["write"]["base64"]))
    return out


ARCHIVES = ["internal/domain/host/archive.go:ExtractHostArchive"]
ARTIFACTS = ["internal/domain/host/artifacts.go:EnsureHostArtifact",
             "internal/domain/host/artifacts_agent.go:PrepareHostAgentArtifacts"]
BRIDGE = ["internal/domain/host/host_service.go:RecoverBridge"]
BRIDGE_UNIT = ("[Unit]\nDescription=Opute parity T2 bridge stand-in\n\n[Service]\nExecStart=/bin/sleep infinity\n\n"
               "[Install]\nWantedBy=default.target\n")
USER_ENV = ["env", "XDG_RUNTIME_DIR=/run/user/1000", "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"]

archive_steps = []
for name, payload in _t1_archives():
    archive_steps.append({"as": "put-" + name, "write": {"path": f"/home/ubuntu/in/{name}.tar.zst", "base64": payload,
                                                         "owner": "ubuntu:ubuntu"}})
cases += [
    scenario("archives.extract", [
        {"as": "inDir", "shell": {"argv": ["install", "-d", "-o", "ubuntu", "-g", "ubuntu", "/home/ubuntu/in"]}},
        *archive_steps,
        snap("baseline"), SERVE,
        *[call(name, "extract_host_archive", {"archivePath": f"in/{name}.tar.zst", "destination": f"out/{name}"})
          for name, _ in _t1_archives()],
        call("overExisting", "extract_host_archive", {"archivePath": "in/basic.tar.zst", "destination": "out/basic"}),
        snap("after", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], ARCHIVES, "The extracted trees: extract_host_archive has no inverse."),
    scenario("artifacts.download-and-agent-build", TLS_SETUP + [
        {"as": "srcDir", "shell": {"argv": ["install", "-d", "-o", "ubuntu", "-g", "ubuntu", "/home/ubuntu/src"]}},
        snap("baseline"), TLS_SERVE,
        LINGER, WAIT_USER,
        task_call("create", "ensure_host_artifact", {"uri": ARTIFACT, "destination": "bin/a.bin",
                                                     "sha256": CONNECTOR_SHA, "executable": True}),
        task_call("reuse", "ensure_host_artifact", {"uri": ARTIFACT, "destination": "bin/a.bin",
                                                    "sha256": CONNECTOR_SHA, "executable": True}),
        task_call("plain", "ensure_host_artifact", {"uri": ARTIFACT, "destination": "lib/b.bin",
                                                    "sha256": "sha256:" + CONNECTOR_SHA.upper()}),
        task_call("wrongSha", "ensure_host_artifact", {"uri": ARTIFACT, "destination": "lib/c.bin", "sha256": "0" * 64}),
        task_call("prepare", "prepare_host_agent_artifacts", {"sourceDir": "/home/ubuntu/src",
                                                              "destDir": "/home/ubuntu/src/out", "archs": ["amd64"]},
                  timeout=1200),
        snap("after", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], ARTIFACTS, masks=[workload_stderr(
        done("prepare") + ["content", 0, "text"],
        "no required module provides package github.com/wunderous/host-agents/cmd/opute-host-agent: go.mod file "
        "not found in current directory or any parent directory; see 'go help modules'\n",
        "exit-code", "code=exited/status=1", prefix="Error: ", trimmed=True)],
        residue="The downloaded artifacts, and from prepare_host_agent_artifacts the Go toolchain package it "
                  "installs, the output directory and the Go build cache (the build itself fails: the source "
                  "directory holds no module, and nothing is fetched)."),
    scenario("bridge.recover", [
        # recover_bridge restarts opute-bridge in the agent's own (user) scope.
        LINGER, WAIT_USER,
        shell("bridgeDir", ["install", "-d", "-o", "ubuntu", "-g", "ubuntu", "/home/ubuntu/.config",
                            "/home/ubuntu/.config/systemd", "/home/ubuntu/.config/systemd/user"]),
        {"as": "bridgeUnit", "write": {"path": "/home/ubuntu/.config/systemd/user/opute-bridge.service",
                                       "content": BRIDGE_UNIT, "owner": "ubuntu:ubuntu"}},
        shell("bridgeStart", USER_ENV + ["sh", "-c", "systemctl --user daemon-reload && "
                                                    "systemctl --user start opute-bridge.service"], user="ubuntu"),
        snap("baseline"), SERVE,
        call("recover", "recover_bridge", {}),
        snap("after", "baseline"),
        STOP, snap("teardown", "baseline"),
    ], BRIDGE, "None: recover_bridge only restarts the harness's stand-in opute-bridge user unit."),
    scenario("linux.no-effect-tools", [
        SERVE, snap("baseline"),
        call("docker", "ensure_docker", {}),
        call("info", "get_host_info", {}),
        call("rule", "ensure_host_firewall_rule", {"uri": "host:local:parity-t2-agent", "port": 8443}),
        {"as": "after", "snapshot": {"baseline": "baseline", "expectEmpty": {"except": [COORDINATOR]}}},
    ], FIREWALL + ["internal/domain/host/host_service.go:EnsureDocker"],
        "None: on a Linux Incus host ensure_docker is unsupported and no firewall rule applies; the snapshot "
        "must be empty.",
        divergences=[D19],
        masks=[{"path": ["steps", "info", "body", "result"] + p + ["system"], "type": "object",
                "reason": "live host load and memory figures; their shape is compared in T1"}
               for p in (["structuredContent"], ["content", 0, "text", "$json"])],
        parse_json=[["steps", "info", "body", "result", "content", 0, "text"]],
        sets=[["steps", "info", "body", "result", "content", "*", "text", "$json", "supportedTools"],
              ["steps", "info", "body", "result", "structuredContent", "supportedTools"]]),
]

# M8 validation 4: every admission-rejected host mutation from M4, re-run
# with arguments that would change this host if admitted. Each phase's
# snapshot must be empty, not merely equal across twins (expectEmpty).
KEEP = "/home/ubuntu/parity-keep.txt"
REJECTED = [
    ("configure_agent_connection", {"envFile": "/home/ubuntu/.config/parity/rejected.env",
                                    "environment": {"OPUTE_PARITY": "x"}}),
    ("recover_bridge", {}),
    ("ensure_docker", {}),
    ("extract_host_archive", {"archivePath": "/home/ubuntu/none.tar.gz", "destination": "~/rejected"}),
    ("ensure_host_artifact", {"uri": ARTIFACT, "sha256": CONNECTOR_SHA, "path": "~/.local/bin/rejected"}),
    ("prepare_host_agent_artifacts", {}),
    ("run_host_command", {"command": "touch /home/ubuntu/rejected-run"}),
    ("ensure_host_file", {"path": "~/.config/systemd/user/rejected.service", "content": UNIT}),
    ("remove_host_file", {"path": KEEP, "confirm": True}),
    ("reconcile_host_resource_policy", {"policyRevision": POLICY_REV,
                                        "uri": "host-service:local:system/opute-host-agent.service"}),
    ("ensure_host_service_supervisor", {}),
    ("restart_host_service", {"uri": DBUS, "serviceName": "cron.service"}),
    ("set_host_service_state", {"uri": DBUS, "serviceName": "cron.service", "state": "stop", "scope": "system"}),
    ("ensure_host_tool", {"tool": "ninja"}),
    ("compact_wsl_disk", {"confirm": True}),
    ("shutdown_wsl", {"confirm": True}),
    ("terminate_wsl_distribution", {"distribution": "Ubuntu"}),
    ("run_instance_command", {"uri": "vm:local:web1", "command": "true"}),
    ("ensure_public_mcp_quick_tunnel", {"bindingId": "rejected", "localTarget": "http://127.0.0.1:9/mcp",
                                        "artifactUri": ARTIFACT, "artifactSha256": CONNECTOR_SHA}),
    ("ensure_public_mcp_tunnel", {"bindingId": "rejected", "endpoint": "https://mcp.parity-t2.test/mcp",
                                  "localTarget": "http://127.0.0.1:9/mcp",
                                  "tunnelToken": "parity-tunnel-token-xxxxxxxxxxxxxxxxxxxxxxxx",
                                  "artifactUri": ARTIFACT, "artifactSha256": CONNECTOR_SHA}),
    ("remove_public_mcp_quick_tunnel", {"bindingId": "parity-quick", "confirm": True}),
    ("install_incus_stack", {}),
    ("uninstall_incus_stack", {"confirm": True}),
]


def _m4_rejected():
    """Host tools M4 rejects, by matrix scenario, and the binding URIs it rejects them with."""
    with open("scenarios/admission-tasks.json") as f:
        m4 = {sc["id"]: sc for sc in json.load(f)}

    def calls(sid):
        return [st["mcp"] for st in m4[sid]["steps"] if "mcp" in st and st["mcp"].get("method") == "tools/call"]
    gated = {c["params"]["name"] for c in calls("admission.matrix.gate-closed")}
    binding = [(c["params"]["name"], c["params"]["arguments"]) for c in calls("admission.matrix.binding")]
    return gated, binding


M4_GATED, M4_BINDING = _m4_rejected()
assert {n for n, _ in REJECTED} <= M4_GATED, sorted({n for n, _ in REJECTED} - M4_GATED)
# Every M4-rejected host tool is re-run; a host tool M4 rejects that is not
# listed here fails generation instead of silently going untested.
HOST_TOOLS = {t["tool"] for t in json.load(open("../../baseline/inventory/host-domain.json"))["tools"]}
assert M4_GATED & HOST_TOOLS == {n for n, _ in REJECTED}, sorted((M4_GATED & HOST_TOOLS) ^ {n for n, _ in REJECTED})
ADMISSION = ["internal/admission/admission.go", "internal/tools/dispatch.go",
             "internal/runtime/standalone.go:MutationsAllowed", "internal/domain/host"]


def rejected(prefix, meta=None):
    out = []
    for name, args in REJECTED:
        step = call(prefix + "." + name, name, args)
        if meta:
            step["mcp"]["meta"] = meta
        out.append(step)
    return out


def wrong_binding():
    """M4's binding rejections, with the effectful arguments merged under the rejected URI."""
    defaults = dict(REJECTED)
    out = []
    for i, (name, args) in enumerate(b for b in M4_BINDING if b[0] in HOST_TOOLS):
        merged = {k: v for k, v in defaults[name].items() if k != "uri"}
        merged.update(args)
        out.append(call(f"wrongBinding.{name}.{i}", name, merged))
    return out


def empty_snap(as_):
    return {"as": as_, "snapshot": {"baseline": "baseline", "expectEmpty": True}}


cases += [
    scenario("admission.rejected-no-effect", [
        {"as": "keep", "write": {"path": KEEP, "content": "kept\n", "owner": "ubuntu:ubuntu"}},
        {"as": "serveClosed", "serve": {"unset": ["OPUTE_STANDALONE_ALLOW_MUTATIONS"]}},
        snap("baseline"),
        rejected("gateClosed"),
        empty_snap("afterGateClosed"),
        {"as": "stopClosed", "stop": {}},
        {"as": "serveOpen", "serve": {}},
        rejected("staleRevision", meta={"catalogRevision": "sha256:0000"}),
        wrong_binding(),
        empty_snap("afterOpen"),
        STOP, empty_snap("teardown"),
    ], ADMISSION, "None: every call is refused before it reaches the host. The baseline is taken after the agent "
                  "starts, so the agent's own startup state is not attributed to a refused call.",
        rejected_only=True, divergences=[D19]),
]

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "scenarios-t2/host-domain.json"
    with open(out, "w") as f:
        json.dump(cases, f, indent=1)
        f.write("\n")
    print(f"wrote {len(cases)} T2 scenarios to {out}")
