#!/usr/bin/env python3
"""Write the e2e-4a journeys (the remaining Use cases: e2e-2, e2e-3, e2e-4) as JSON.

  python3 evidence/e2e/journeys/make_e2e4a.py

The committed JSON files are the journeys; this script is how they were
authored. Every journey starts from the snapshot and drives the dev VM's
system agent (opute-bootstrap-mcp.service, user opute) through typed calls,
with tools resolved from the agent's own tools/list (D22 neutral names; a
hidden alias only where a recipe or page names it). Guest steps set up
inputs, observe and clean up.

Gaps are recorded where a journey hits them (evidence/gaps/e2e-4a.json), never
by sweeping the catalog.
"""
from __future__ import annotations

import json

import make_e2e1 as base
import make_e2e3a as ha
from make_e2e1 import HERE, SPEC, T, TASKS_META, a, inp, ok, req
from make_e2e3a import call, direct, res, task

QUESTIONS = "openspec/changes/archive/2026-11-07-rust-spec-authority/questions.md"
CH = "openspec/changes/"
Q20_SPEC = CH + "settle-user-service-supervisor/specs/host-service-supervisor/spec.md"
Q16_SPEC = CH + "incus-project-selection/specs/incus-project-selection/spec.md"
Q21_SPEC = CH + "declare-read-inputs/specs/capability-input-schemas/spec.md"
Q22_SPEC = CH + "nullable-output-schemas/specs/capability-output-schemas/spec.md"
Q23_SPEC = CH + "privileged-host-service-restart/specs/host-service-restart/spec.md"
Q24_SPEC = CH + "report-stopped-clusters/specs/cluster-discovery/spec.md"
Q25_SPEC = CH + "truthful-operation-status/specs/operation-records/spec.md"
Q26_SPEC = CH + "container-runtime-stdout/specs/container-storage/spec.md"
Q27_SPEC = CH + "truthful-serving-launch/specs/serving-assignment/spec.md"
Q28_SPEC = CH + "bind-standalone-service-domain/specs/service-exposure/spec.md"
Q5_SPEC = CH + "truthful-interrupted-plan/specs/host-agent-contract/spec.md"
SEAM_SPEC = ha.SEAM_SPEC
CATALOG = "the agent's published tools/list (inputSchema, outputSchema, description) for the tool named"
E2E2_GOAL = ".agents/goals/e2e-2-host-workloads.md (flows 4, 5, 7, 8)"
HOME = "/home/opute"
SVC = "host-service:local:system/dbus.service"  # the binding set_host_service_state is admitted against
UNIT_TEXT = ("[Unit]\nDescription=e2e web\n\n[Service]\nExecStart=/usr/bin/python3 -m http.server 8781 "
             "--bind 127.0.0.1 --directory /tmp\n\n[Install]\nWantedBy=default.target\n")
SUMS_URI = "https://github.com/k3s-io/k3s/releases/download/v1.31.8%2Bk3s1/sha256sum-amd64.txt"
SUMS_SHA = "726d24fbdca8e47c01733ad73d1791efcc1ac3e96bb3daf40b17696c28176689"
INPUTS = {"agent": base.AGENT, "files": dict(base.TOOL_FILES)}


def journey(id_, gate, description, requirements, steps, cleanup, allow=(), persist=(), minutes=20, inputs=None):
    allow = list(allow) + [p for p in persist if p not in allow]  # what may remain must also be allowed
    for step in list(steps) + list(cleanup):  # a refusal for capacity is waited out, as a client must
        if step.get("mcp") == "tools/call":
            step.setdefault("retryOn", ["host_capacity_saturated"])
    return base.journey(id_, description, inputs or INPUTS, requirements, steps, cleanup, allow=allow,
                        persist=list(persist), minutes=minutes, gate=gate)


def svc(id_: str, state: str, name: str = "e2e-web.service", scope: str = "user") -> dict:
    return call(id_, "set_host_service_state", {"uri": SVC, "serviceName": name, "state": state, "scope": scope})


# --- e2e-2 / 1. host operations -------------------------------------------------------------------------

def host_ops() -> dict:
    steps = base.TOOL_STEPS + [
        {"id": "catalog", "mcp": "tools/list"},
        # services: a user unit through its whole lifecycle
        call("supervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        call("unit", "ensure_host_file", {"path": "~/.config/systemd/user/e2e-web.service", "content": UNIT_TEXT}),
        svc("enable", "enable"), svc("start", "start"),
        call("inspect", "inspect_host_service", {"uri": "host-service:local:user/e2e-web.service",
                                                 "listenPort": 8781}),
        call("services", "list_host_services", {}),
        *task("curl", "run_host_command", {"command": "curl -s -o /dev/null -w %{http_code} http://127.0.0.1:8781/"}),
        svc("restart", "restart"),
        {"id": "running", "guest": f"{T} unit e2e-web.service --user opute"},
        svc("stop", "stop"), svc("disable", "disable"),
        *task("removeUnconfirmed", "remove_host_file", {"path": "~/.config/systemd/user/e2e-web.service"}),
        *task("removeUnit", "remove_host_file", {"path": "~/.config/systemd/user/e2e-web.service", "confirm": True}),
        call("inspectGone", "inspect_host_file", {"path": "~/.config/systemd/user/e2e-web.service"}),
        # system scope: the agent's user cannot write system unit files
        call("systemUnitDenied", "ensure_host_file",
             {"path": "/etc/systemd/system/e2e-sys.service", "content": UNIT_TEXT, "scope": "system"}),
        call("restartIncus", "restart_host_service", {"uri": "host-service:local:system/incus.service"}),
        call("restartIncusNamed", "restart_host_service",
             {"uri": "host-service:local:system/incus.service", "serviceName": "incus.service"}),
        # D28 (Q23): a system service is restarted through set_host_service_state, scope system
        call("restartIncusSystem", "set_host_service_state",
             {"uri": "host-service:local:system/incus.service", "serviceName": "incus.service", "state": "restart",
              "scope": "system"}),
        {"id": "incusActive", "guest": "for i in $(seq 60); do systemctl is-active --quiet incus && break; sleep 1; "
                                       "done; systemctl is-active incus"},
        # exec of workloads
        *task("execFails", "run_host_command", {"command": "echo err >&2; exit 3"}),
        *task("execTimeout", "run_host_command", {"command": "sleep 30", "timeoutMs": 1500}),
        # packages
        *task("ninjaTool", "ensure_host_tool", {"tool": "ninja"}),
        *task("toolAgain", "ensure_host_tool", {"tool": "ninja"}),
        {"id": "ninja", "guest": "dpkg -s ninja-build | sed -n 's/^Status: //p'"},
        # artifact download with a checksum
        *task("artifact", "ensure_host_artifact", {"uri": SUMS_URI, "destination": "e2e/art/sums.txt",
                                                   "sha256": SUMS_SHA}),
        *task("artifactAgain", "ensure_host_artifact", {"uri": SUMS_URI, "destination": "e2e/art/sums.txt",
                                                        "sha256": SUMS_SHA}),
        *task("artifactBad", "ensure_host_artifact", {"uri": SUMS_URI, "destination": "e2e/art/bad.txt",
                                                      "sha256": "0" * 64}),
        call("artifactFile", "inspect_host_file", {"path": "e2e/art/sums.txt"}),
        call("artifactBadFile", "inspect_host_file", {"path": "e2e/art/bad.txt"}),
        # files and archives: the archive is made by a typed host command, then extracted
        *task("archive", "run_host_command",
              {"command": "mkdir -p $HOME/e2e/src/d && printf hello > $HOME/e2e/src/d/a.txt && "
                          "tar --zstd -C $HOME/e2e/src -cf $HOME/e2e/in.tar.zst d && echo made"}),
        call("extract", "extract_host_archive", {"archivePath": "e2e/in.tar.zst", "destination": "e2e/out"}),
        call("extracted", "inspect_host_file", {"path": "e2e/out/d/a.txt"}),
        *task("traversal", "run_host_command",
              {"command": "cd $HOME/e2e && printf x > evil && tar --zstd -cf bad.tar.zst --transform 's,^,../,' "
                          "evil && echo made"}),
        call("extractTraversal", "extract_host_archive", {"archivePath": "e2e/bad.tar.zst",
                                                          "destination": "e2e/out2"}),
        {"id": "noEscape", "guest": f"test -e {HOME}/evil && echo escaped || echo contained"},
        # the virtualization stack (already installed on the snapshot: an idempotent reconcile)
        *task("virt", "install_virtualization_stack", {}, timeout=900),
        # the bridge control plane: none is installed on a standalone host
        call("bridge", "diagnose_bridge", {}),
        call("recoverBridge", "recover_bridge", {}),
        {"id": "after", "guest": f"ls -la {HOME}/e2e {HOME}/e2e/out/d 2>&1 | wc -l"},
    ]
    reqs = [
        req(CATALOG, "ensure_host_service_supervisor, ensure_host_file, set_host_service_state, "
                      "inspect_host_service, list_host_services, remove_host_file, inspect_host_file",
            direct("supervisorAgain", "structuredContent.status", equals="ready"),
            direct("supervisorAgain", "structuredContent.linger", equals=True),
            direct("unit", "structuredContent.changed", equals=True),
            direct("unit", "structuredContent.path", equals=f"{HOME}/.config/systemd/user/e2e-web.service"),
            direct("enable", "structuredContent.status", equals="applied"),
            direct("start", "structuredContent.status", equals="applied"),
            direct("inspect", "structuredContent.active", equals=True),
            direct("inspect", "structuredContent.enabled", equals=True),
            direct("inspect", "structuredContent.listenerOwned", equals=True),
            direct("services", "structuredContent.services", exists=True),
            res("curl", "structuredContent.stdout", equals="200"),
            direct("restart", "structuredContent.status", **{"in": ["applied", "scheduled"]}),
            a("running", "json.ActiveState", equals="active"),
            direct("stop", "structuredContent.status", equals="applied"),
            direct("disable", "structuredContent.status", equals="applied"),
            res("removeUnconfirmed", "isError", equals=True),
            res("removeUnconfirmed", "structuredContent.code", equals="invalid_arguments"),
            res("removeUnit", "isError", equals=False),
            direct("inspectGone", "structuredContent.exists", equals=False),
            direct("systemUnitDenied", "isError", equals=True),
            scenario="A user service: unit file written, enabled, started, owning its port and serving HTTP, "
                     "restarted, stopped, disabled and its file removed only with confirmation; a system unit "
                     "file is refused to the agent's unprivileged user"),
        req(CATALOG, "run_host_command",
            res("execFails", "isError", equals=True), res("execFails", "structuredContent.exitCode", equals=3),
            res("execTimeout", "isError", equals=True),
            res("execTimeout", "content[0].text", matches="Finished with result: timeout"),
            scenario="Bounded host commands: a failing command reports its exit status, a command past its "
                     "timeout is stopped and reported as failed"),
        req(CATALOG, "ensure_host_tool",
            res("ninjaTool", "isError", equals=False), res("ninjaTool", "structuredContent.available", equals=True),
            res("ninjaTool", "structuredContent.path", equals="/usr/bin/ninja"),
            res("toolAgain", "structuredContent.alreadyAvailable", equals=True),
            a("ninja", "stdout", equals="install ok installed\n"),
            scenario="An allowlisted package installed through apt once, then reported already available"),
        req(CATALOG, "ensure_host_artifact",
            res("artifact", "structuredContent.changed", equals=True),
            res("artifact", "structuredContent.sha256", equals="sha256:" + SUMS_SHA),
            res("artifactAgain", "structuredContent.changed", equals=False),
            res("artifactBad", "isError", equals=True),
            res("artifactBad", "content[0].text", matches="sha256 mismatch"),
            direct("artifactFile", "structuredContent.sha256", equals="sha256:" + SUMS_SHA),
            direct("artifactBadFile", "structuredContent.exists", equals=False),
            scenario="An HTTPS artifact downloaded and verified against its SHA-256; a wrong checksum installs "
                     "nothing"),
        req(CATALOG, "extract_host_archive",
            direct("extract", "structuredContent.changed", equals=True),
            direct("extract", "structuredContent.entries", equals=2),
            direct("extracted", "structuredContent.exists", equals=True),
            direct("extractTraversal", "isError", equals=True),
            a("noEscape", "stdout", equals="contained\n"),
            scenario="A tar.zst archive extracted beneath the home directory; a traversal entry is rejected and "
                     "nothing escapes"),
        req(CATALOG, "install_virtualization_stack",
            res("virt", "isError", equals=False),
            res("virt", "structuredContent.incus", matches="Server version"),
            scenario="The virtualization stack reconciled on a host that has it: Incus reported, nothing broken"),
        req(Q21_SPEC, "A tool declares every input it reads",
            direct("restartIncus", "isError", equals=True),
            direct("restartIncus", "content[0].text", matches='missing required property \\"serviceName\\"'),
            scenario="restart_host_service publishes serviceName as required; a call with uri only is refused "
                     "by the published schema before anything runs (D28)"),
        req(Q23_SPEC, "restart_host_service restarts Opute user services only",
            direct("restartIncusNamed", "isError", equals=True),
            direct("restartIncusNamed", "content[0].text", matches="set_host_service_state"),
            direct("restartIncusSystem", "isError", **{"in": [False, "<absent>"]}),
            direct("restartIncusSystem", "structuredContent.status", equals="scheduled"),
            a("incusActive", "stdout", equals="active\n"),
            scenario="Restarting Incus with restart_host_service is refused before acting and names "
                     "set_host_service_state; Incus is restarted through it with scope system (D28)"),
        req(Q20_SPEC, "The first ensure call settles the user service manager",
            direct("supervisor", "structuredContent.status", equals="ready"),
            scenario="The first ensure call on a host whose user manager is not running yet reports ready once "
                     "the user bus is up (D28)"),
        req(Q22_SPEC, "A result the published output schema allows is not refused",
            direct("bridge", "isError", **{"in": [False, "<absent>"]}),
            direct("bridge", "structuredContent.bridgeStatus", equals="offline"),
            direct("bridge", "structuredContent.lastHeartbeat.at", equals=None),
            direct("recoverBridge", "isError", equals=True),
            scenario="Bridge recovery on a standalone host with no bridge: diagnose reports it offline with a "
                     "null heartbeat (D28: nullable honoured), recover fails closed"),
    ]
    cleanup = [{"id": "cleanHost", "guest": (
        "uid=$(id -u opute); runuser -u opute -- env XDG_RUNTIME_DIR=/run/user/$uid systemctl --user disable "
        "--now e2e-web.service 2>/dev/null; rm -f /home/opute/.config/systemd/user/e2e-web.service; "
        "runuser -u opute -- env XDG_RUNTIME_DIR=/run/user/$uid systemctl --user daemon-reload; "
        f"rm -rf {HOME}/e2e {HOME}/evil; DEBIAN_FRONTEND=noninteractive apt-get purge -y -q ninja-build "
        ">/dev/null 2>&1; apt-get autoremove -y -q >/dev/null 2>&1; loginctl disable-linger opute; "
        "rmdir /home/opute/.config/systemd/user /home/opute/.config/systemd 2>/dev/null; true")}]
    allow = ["files//home/opute/e2e*", "files//home/opute/evil", "files//home/opute/.config/systemd*",
             "packages/ninja-build*", "files//usr/bin/ninja*", "files//var/lib/dpkg*", "files//var/lib/apt*",
             "files//var/cache*", "files//var/log*", "files//usr/share/doc/ninja-build*",
             "files//usr/share/man*", "files//usr/share/bash-completion*", "files//usr/share/zsh*",
             "files//usr/share/vim*", "files//usr/share/emacs*",
             "system/units/opute-host-workload-*", "user/units/e2e-web*", "user/unitFiles/e2e-web*",
             "linger/*", "users/opute/linger*", "system/units/user@*", "system/units/user-runtime-dir@*",
             "system/units/user-*.slice", "incus/*", "files//var/lib/systemd/linger*", "linger",
             "system/units/run-user-*", "system/units/packagekit.service",
             # install_virtualization_stack installs *or upgrades* Incus from its package source: a declared,
             # lasting effect that cleanup cannot undo
             "files//opt/incus*", "files//usr/lib/systemd/system/incus*", "system/units/linstor-satellite.service"]
    persist = ["files//opt/incus*", "files//usr/lib/systemd/system/incus*", "system/units/linstor-satellite.service",
               "system/units/packagekit.service", "files//var/lib/dpkg/info/incus*"]
    return journey(
        "e2e2.host-ops", "e2e-2",
        "Host operations on the dev VM itself through typed calls: a user service's lifecycle (file, enable, "
        "start, listener, HTTP, restart, stop, disable, confirmed removal), a refused system unit, bounded host "
        "commands, an apt tool, a checksum-verified HTTPS artifact, a tar.zst archive and a traversal archive, "
        "the virtualization stack and the bridge control plane. Pass: each effect is observed on the VM and "
        "cleanup returns the VM diff to the baseline.",
        reqs, steps, cleanup, allow=allow, persist=persist, minutes=20)


# --- e2e-2 / 2. Incus containers and VMs ----------------------------------------------------------------

B_PORT = 3032
B = {"port": B_PORT, "auth": "secret:tokenB"}
B_ID = "e2e-incus-b"
STATIC_IP = "10.0.100.50"
C1 = "container:local:e2e-c1"
V1 = "vm:local:e2e-v1"
FOREIGN = "container:local:e2e-foreign"


def incus() -> dict:
    steps = base.TOOL_STEPS + [
        # host B: a second agent on the VM with Incus ownership enforced; it owns what it creates
        {"id": "agentB", "guest": (
            f"{T} agent start b --port {B_PORT} --id {B_ID} --env OPUTE_STANDALONE_ALLOW_MUTATIONS=true "
            f"--env OPUTE_HOST_AGENT_INSTANCE={B_ID} --env OPUTE_INCUS_OWNERSHIP_MODE=enforce "
            f"--env OPUTE_SHARED_HOST_OWNER_INSTANCE={B_ID} --env MCP_AUTH_TOKEN='${{SECRET:tokenB}}' >/dev/null "
            "&& echo started")},
        {"id": "catalogB", "mcp": "tools/list", **B},
        {"id": "foreign", "guest": "incus launch parity-noble e2e-foreign -q && incus list e2e-foreign -c s -f csv"},
        # Q16: agent B's own Incus client (user opute) has another current project
        {"id": "otherProject", "guest": "cp -p /home/opute/.config/incus/config.yml /root/e2e-incus-config.yml && "
                                        "incus project create e2e-other -q && runuser -u opute -- incus project "
                                        "switch e2e-other && runuser -u opute -- incus project list -f csv | "
                                        "grep -c '(current)' && runuser -u opute -- incus project list -f csv | "
                                        "grep '(current)' | cut -d, -f1"},
        call("listBefore", "list_vms", {}, B),
        *task("container", "provision_container",
              {"containerName": "e2e-c1", "image": "parity-noble", "disk": "4GiB", "cpus": 1, "memory": "1GiB",
               "ipv4Address": STATIC_IP}, B, timeout=600),
        *task("vm", "provision_vm", {"name": "e2e-v1", "instanceType": "virtual-machine", "image": "parity-noble-vm",
                                     "cpus": 1, "memory": "1GiB", "disk": "10GiB"}, B, timeout=900),
        call("infoC", "get_vm_info", {"uri": C1}, B),
        {"id": "inDefault", "guest": "incus list e2e-c1 --project default -c n -f csv; "
                                     "incus list --project e2e-other -c n -f csv | wc -l"},
        {"id": "vmAddress", "guest": "for i in $(seq 150); do incus list e2e-v1 -c 4 -f csv | grep -q '10\\.0\\.100\\.' "
                                     "&& break; sleep 2; done; incus list e2e-v1 -c 4 -f csv | grep -c '10\\.0\\.100\\.'"},
        call("infoV", "get_vm_info", {"uri": V1}, B,
             until={"path": "response.body.result.structuredContent.agentReady", "in": [True]}, timeout=300,
             interval=5, capture={"V1_IP": "response.body.result.structuredContent.ipv4[0]"}),
        call("listBoth", "list_vms", {}, B),
        call("serveC", "run_instance_command",
              {"uri": C1, "command": "sh", "args": ["-c", "nohup python3 -m http.server 8080 --directory /etc "
                                                         ">/dev/null 2>&1 & sleep 1; echo serving"]}, B),
        {"id": "reachC", "guest": f"ping -c1 -W5 {STATIC_IP} >/dev/null && echo ping-ok; curl -s -o /dev/null -w "
                                  f"'%{{http_code}}' --max-time 10 http://{STATIC_IP}:8080/hostname"},
        call("runV", "run_instance_command", {"uri": V1, "command": "sh", "args": ["-c", "nproc; uname -s"]}, B),
        {"id": "reachV", "guest": "ping -c1 -W5 ${V1_IP} >/dev/null && echo ping-ok; "
                                  "timeout 5 bash -c 'echo > /dev/tcp/${V1_IP}/22' 2>/dev/null && echo ssh-port"},
        # lifecycle
        *task("stopC", "stop_vm", {"uri": C1}, B), *task("startC", "start_vm", {"uri": C1}, B),
        *task("restartC", "restart_vm", {"uri": C1}, B),
        call("infoAfter", "get_vm_info", {"uri": C1}, B,
             until={"path": "response.body.result.structuredContent.ipv4[0]", "in": [STATIC_IP]}, timeout=120,
             interval=3),
        *task("stopV", "stop_vm", {"uri": V1}, B),
        call("resizeV", "update_vm_resources", {"uri": V1, "cpus": 2, "memory": "1536MiB"}, B),
        *task("startV", "start_vm", {"uri": V1}, B, timeout=300),
        {"id": "limitsV", "guest": "incus config get e2e-v1 limits.cpu; incus config get e2e-v1 limits.memory"},
        # root disk and storage quota
        call("grow", "update_vm_resources", {"uri": C1, "disk": "5GiB"}, B),
        {"id": "rootSize", "guest": "incus config device get e2e-c1 root size"},
        call("overQuota", "run_instance_command",
              {"uri": C1, "command": "sh", "args": ["-c", "head -c 6G /dev/zero > /root/fill; r=$?; rm -f /root/fill; "
                                                         "exit $r"], "timeoutMs": 240000}, B),
        # ownership: B refuses the instance it does not own, and leaves it untouched
        *task("stopForeign", "stop_vm", {"uri": FOREIGN}, B),
        *task("deleteForeign", "delete_vm", {"uri": FOREIGN}, B),
        *task("resetForeign", "reset_virtualization_stack",
              {"confirm": True, "reinstall": True, "instanceNames": ["e2e-foreign"],
               "disposableHostFingerprint": "e2e-devvm", "expectedHostFingerprint": "e2e-devvm",
               "disposableHostAuthorization": "dispose:e2e-devvm"}, B),
        {"id": "foreignAfter", "guest": "incus list e2e-foreign -c s -f csv"},
        # reset of an owned container: deleted, the stack reinstalled and verified
        *task("reset", "reset_virtualization_stack",
              {"confirm": True, "reinstall": True, "instanceNames": ["e2e-c1"],
               "disposableHostFingerprint": "e2e-devvm", "expectedHostFingerprint": "e2e-devvm",
               "disposableHostAuthorization": "dispose:e2e-devvm"}, B, timeout=900),
        {"id": "resetGone", "guest": "incus list e2e-c1 -c n -f csv | wc -l"},
        *task("deleteV", "delete_vm", {"uri": V1}, B),
        *task("deleteAgain", "delete_vm", {"uri": V1}, B),
        call("listAfter", "list_vms", {}, B),
        {"id": "left", "guest": "incus list -c n -f csv"},
        {"id": "projectBack", "guest": "runuser -u opute -- incus project switch default && "
                                       "incus project delete e2e-other >/dev/null && cp -p /root/e2e-incus-config.yml "
                                       "/home/opute/.config/incus/config.yml && rm /root/e2e-incus-config.yml && echo restored"},
    ]
    reqs = [
        req(CATALOG, "provision_container, provision_vm, get_vm_info, list_vms, run_instance_command",
            direct("listBefore", "structuredContent.vms", **{"in": [[], "<absent>"]}),
            res("container", "isError", equals=False), res("container", "structuredContent.uri", equals=C1),
            res("vm", "isError", equals=False), res("vm", "structuredContent.uri", equals=V1),
            direct("infoC", "structuredContent.status", equals="running"),
            a("vmAddress", "stdout", equals="1\n"),
            direct("infoV", "structuredContent.status", equals="running"),
            direct("infoV", "structuredContent.agentReady", equals=True),
            direct("listBoth", "structuredContent.vms[0].uri", equals=C1),
            direct("listBoth", "structuredContent.vms[1].uri", equals=V1),
            direct("serveC", "structuredContent.exitCode", equals=0),
            a("reachC", "stdout", equals="ping-ok\n200"),
            direct("runV", "structuredContent.stdout", equals="1\nLinux\n"),
            a("reachV", "stdout", matches="^ping-ok\n"),
            scenario="A container and a virtual machine provisioned in the default project; both boot and answer "
                     "at their addresses (ping and a TCP service)"),
        req(Q16_SPEC, "One Incus project per agent",
            a("otherProject", "stdout", equals="1\ne2e-other (current)\n"),
            res("container", "isError", equals=False),
            direct("infoC", "structuredContent.status", equals="running"),
            a("inDefault", "stdout", equals="e2e-c1\n0\n"),
            a("projectBack", "stdout", equals="restored\n"),
            scenario="With agent B's Incus client switched to project e2e-other, the container it provisions is "
                     "created and observed in default, and e2e-other stays empty (D28)"),
        req(Q21_SPEC, "A tool's published input schema declares every input it reads",
            direct("infoC", "structuredContent.ipv4[0]", equals=STATIC_IP),
            direct("infoAfter", "structuredContent.ipv4[0]", equals=STATIC_IP),
            scenario="provision_container's declared ipv4Address pins the container's bridge address across "
                     "stop, start and restart (D28)"),
        req(CATALOG, "stop_vm, start_vm, restart_vm, update_vm_resources",
            res("stopC", "structuredContent.status", equals="stopped"),
            res("startC", "structuredContent.status", equals="running"),
            res("restartC", "isError", equals=False),
            res("stopV", "structuredContent.status", equals="stopped"),
            direct("resizeV", "isError", **{"in": [False, "<absent>"]}),
            res("startV", "structuredContent.status", equals="running"),
            a("limitsV", "stdout", equals="2\n1536MiB\n"),
            scenario="Lifecycle of both kinds; a stopped VM resized and started with its new limits"),
        req(CATALOG, "update_vm_resources disk is admitted only where the pool enforces it",
            direct("grow", "isError", **{"in": [False, "<absent>"]}), a("rootSize", "stdout", equals="5GiB\n"),
            direct("overQuota", "structuredContent.exitCode", gte=1),
            direct("overQuota", "structuredContent.stderr", matches="(Disk quota exceeded|No space left on device)"),
            scenario="The container's root disk grows to 5GiB on the btrfs pool, and a 6GiB write inside it fails "
                     "at the quota"),
        req(CATALOG, "Incus ownership (OPUTE_INCUS_OWNERSHIP_MODE=enforce)",
            direct("listBefore", "structuredContent.vms", **{"in": [[], "<absent>"]}),
            res("stopForeign", "isError", equals=True),
            res("stopForeign", "structuredContent.code", equals="resource_binding"),
            res("deleteForeign", "isError", equals=True),
            res("deleteForeign", "structuredContent.code", equals="resource_binding"),
            res("resetForeign", "isError", equals=True),
            res("resetForeign", "content[0].text", matches="refuses unowned instance"),
            a("foreignAfter", "stdout", equals="RUNNING\n"),
            scenario="An instance created outside the agent is not listed by the enforcing agent, so stop and "
                     "delete are refused at admission (unknown resource), reset refuses it as unowned, and it "
                     "keeps running"),
        req(CATALOG, "reset_virtualization_stack resets explicitly selected owned instances",
            res("reset", "isError", equals=False),
            a("resetGone", "stdout", equals="0\n"),
            scenario="The owned container is deleted and the stack reinstalled and verified, behind matching "
                     "disposable-host fingerprints and authorization"),
        req(CATALOG, "delete_vm",
            res("deleteV", "structuredContent.deleted", equals=True),
            res("deleteAgain", "isError", **{"in": [False, True]}),
            direct("listAfter", "structuredContent.vms", **{"in": [[], "<absent>"]}),
            a("left", "stdout", equals="e2e-foreign\n"),
            scenario="Every owned guest is gone after delete; only the foreign instance remains"),
    ]
    cleanup = [{"id": "cleanIncus", "guest": (
        f"{T} agent remove b >/dev/null; runuser -u opute -- incus project switch default 2>/dev/null; "
        "incus project delete e2e-other -q 2>/dev/null; test -f /root/e2e-incus-config.yml && "
        "cp -p /root/e2e-incus-config.yml /home/opute/.config/incus/config.yml && rm -f /root/e2e-incus-config.yml; for n in e2e-c1 e2e-v1 e2e-foreign; do incus delete -f $n 2>/dev/null; "
        "done; incus storage volume list default -f csv | awk -F, '$1==\"image\"{print $2}' | "
        "xargs -r -I{} incus storage volume delete default image/{}; true")}]
    allow = ["incus/*", "files//home/opute/.config/incus*", "system/units/e2e-*", "system/unitFiles/e2e-*", "files//opt/incus*",
             "files//usr/lib/systemd/system/incus*", "system/units/linstor-satellite.service",
             "system/units/packagekit.service", "files//var/lib/dpkg*", "files//var/lib/apt*", "files//var/cache*",
             "files//var/log*", "system/units/sys-devices-virtual-net-veth*",
             "system/units/sys-subsystem-net-devices-veth*", "system/units/sys-devices-virtual-net-tap*",
             "system/units/sys-subsystem-net-devices-tap*"]
    persist = ["files//opt/incus*", "files//usr/lib/systemd/system/incus*", "system/units/linstor-satellite.service",
               "system/units/packagekit.service", "files//var/lib/dpkg*", "files//var/lib/apt*", "files//var/cache*",
               "files//var/log*"]
    inputs = {"agent": base.AGENT, "agentB": f"a transient Rust agent {B_ID} on port {B_PORT} (e2etool agent start) "
                                             "with Incus ownership enforced and itself as the shared host owner",
              "images": "the snapshot's local Incus images parity-noble (container) and parity-noble-vm",
              "files": dict(base.TOOL_FILES)}
    return journey(
        "e2e2.incus", "e2e-2",
        "Incus containers and VMs through typed calls on a second agent with ownership enforced: a container with "
        "a pinned address and a VM, both reachable; lifecycle; a VM resize; root disk growth and its quota; "
        "refusals on an instance the agent does not own; reset of an owned container; delete. Guests live in the "
        "default project (Q16).",
        reqs, steps, cleanup, allow=allow, persist=persist, minutes=28, inputs=inputs)


# --- e2e-2 / 3. plans across domains ---------------------------------------------------------------------

def cross_plan(plan_id: str, key: str, name: str, slow: bool = False) -> dict:
    """A host file, an Incus container and a SQLite database in one host-plan.v1 run."""
    nodes = [
        {"id": "file", "action": {"tool": "ensure_host_file",
                                  "args": {"path": f"~/e2e/{name}.txt", "content": f"{name}\n", "scope": "user"}},
         "validate": {"tool": "inspect_host_file", "args": {"path": f"~/e2e/{name}.txt", "scope": "user"},
                      "assert": [{"path": "/exists", "op": "eq", "value": True}]}},
    ]
    if slow:
        nodes.append({"id": "wait", "dependsOn": ["file"],
                      "action": {"tool": "probe_http_endpoint", "args": {"endpoint": "http://127.0.0.1:8799/"}}})
    nodes += [
        {"id": "guest", "dependsOn": [nodes[-1]["id"]], "timeoutMs": 600000,
         "action": {"tool": "provision_vm", "args": {"name": f"e2e-{name}", "image": "parity-noble",
                                                     "instanceType": "container", "cpus": 1, "memory": "512MiB"}},
         "validate": {"tool": "get_vm_info", "args": {"uri": f"container:local:e2e-{name}"},
                      "assert": [{"path": "/status", "op": "eq", "value": "running"}]}},
        {"id": "data", "dependsOn": ["guest"],
         "action": {"tool": "ensure_sqlite_database", "args": {"consumerId": f"e2e-{name}", "databaseName": "main"}},
         "validate": {"tool": "get_sqlite_database_status",
                      "args": {"consumerId": f"e2e-{name}", "databaseName": "main"},
                      "assert": [{"path": "/exists", "op": "eq", "value": True}]}},
    ]
    return {"contractVersion": "host-plan.v1", "planId": plan_id, "generation": 1, "idempotencyKey": key,
            "nodes": nodes}


RECIPE = """contractVersion: host-recipe.v1
recipeId: com.opute.e2e.cross-domain
recipeVersion: 1.0.0
execution:
  coordinator: host-agent
  mode: local
compatibility:
  requiredTools: [ensure_host_file, inspect_host_file, provision_vm, get_vm_info, ensure_sqlite_database,
    get_sqlite_database_status]
inputs:
  name:
    required: true
    schema: {type: string, pattern: '^[a-z][a-z0-9-]{0,20}$'}
plan:
  contractVersion: host-plan.v1
  planId: com.opute.e2e.cross-domain
  generation: 1
  idempotencyKey: e2e-cross-domain-${vars.inputs.name}
  nodes:
    - id: file
      action:
        tool: ensure_host_file
        args: {path: '~/e2e/${vars.inputs.name}.txt', content: "recipe\\n", scope: user}
      validate:
        tool: inspect_host_file
        args: {path: '~/e2e/${vars.inputs.name}.txt', scope: user}
        assert:
          - {path: /exists, op: eq, value: true}
    - id: guest
      dependsOn: [file]
      timeoutMs: 600000
      action:
        tool: provision_vm
        args: {name: 'e2e-${vars.inputs.name}', image: parity-noble, instanceType: container, cpus: 1,
          memory: 512MiB}
      validate:
        tool: get_vm_info
        args: {uri: 'container:local:e2e-${vars.inputs.name}'}
        assert:
          - {path: /status, op: eq, value: running}
    - id: data
      dependsOn: [guest]
      action:
        tool: ensure_sqlite_database
        args: {consumerId: 'e2e-${vars.inputs.name}', databaseName: main}
      validate:
        tool: get_sqlite_database_status
        args: {consumerId: 'e2e-${vars.inputs.name}', databaseName: main}
        assert:
          - {path: /exists, op: eq, value: true}
"""


def plans() -> dict:
    files = {**base.TOOL_FILES, "recipe": inp("${REPO}/evidence/e2e/journeys/inputs/e2e-cross-domain-recipe.yaml")}
    recipe_path = f"{HOME}/e2e/recipe.yaml"
    steps = base.TOOL_STEPS + [
        {"id": "recipeCopy", "copy": "recipe", "to": recipe_path, "mode": "0644"},
        {"id": "own", "guest": f"chown -R opute:opute {HOME}/e2e && echo ok"},
        {"id": "slow", "guest": f"{T} slow start --port 8799 --delay 25"},
        # one plan across the three domains, to its terminal state
        call("validate", "validate_host_plan", {"plan": cross_plan("e2e.cross", "e2e-cross-1", "p1")}),
        call("run", "run_host_plan", {"plan": cross_plan("e2e.cross", "e2e-cross-1", "p1")},
             capture={"RUN": "response.body.result.structuredContent.runId"}),
        base.poll_run("done", "RUN", timeout=600),
        # a repeat re-runs the same run: every node revalidates and re-applies only what drifted (Go's design)
        call("again", "run_host_plan", {"plan": cross_plan("e2e.cross", "e2e-cross-1", "p1")}),
        base.poll_run("againDone", "RUN", timeout=600),
        {"id": "released", "guest": "sleep 5"},
        call("operation", "get_operation", {"operationId": "${RUN}"}),
        call("operations", "list_operations", {"limit": 50}),
        # the recipe: the same three domains through run_host_local_recipe
        call("recipeValidate", "validate_host_local_recipe", {"source": recipe_path, "inputs": {"name": "r1"}}),
        call("recipeRun", "run_host_local_recipe", {"source": recipe_path, "inputs": {"name": "r1"}},
             capture={"RRUN": "response.body.result.structuredContent.runId"}),
        base.poll_run("recipeDone", "RRUN", timeout=600),
        {"id": "released2", "guest": "sleep 5"},
        # cancel mid-plan: the file node completes, the run waits on the slow endpoint, then is cancelled
        call("slowRun", "run_host_plan", {"plan": cross_plan("e2e.cancel", "e2e-cancel-1", "c1", slow=True)},
             capture={"CRUN": "response.body.result.structuredContent.runId"}),
        call("waiting", "get_host_plan_run", {"runId": "${CRUN}"},
             until={"path": "response.body.result.structuredContent.nodes.file.status", "in": ["applied"]},
             timeout=60),
        {"id": "settle", "guest": "sleep 3"},
        call("cancel", "cancel_operation", {"operationId": "${CRUN}"}),
        base.poll_run("cancelled", "CRUN", timeout=120),
        {"id": "afterCancel", "guest": "sleep 30; incus list e2e-c1 -c n -f csv | wc -l; "
                                       f"test -e {HOME}/e2e/c1.txt && echo file-kept"},
        call("cancelAgain", "cancel_operation", {"operationId": "${CRUN}"}),
        call("unknown", "get_operation", {"operationId": "no-such-operation"}),
    ]
    reqs = [
        req(SPEC, "One typed plan and recipe execution path",
            direct("validate", "structuredContent.valid", equals=True),
            direct("done", "structuredContent.status", equals="completed"),
            direct("done", "structuredContent.nodes.file.status", equals="applied"),
            direct("done", "structuredContent.nodes.guest.status", equals="applied"),
            direct("done", "structuredContent.nodes.data.status", equals="applied"),
            {"step": "again", "path": "response.body.result.structuredContent.runId",
             "equalsStep": {"step": "run", "path": "response.body.result.structuredContent.runId"}},
            direct("againDone", "structuredContent.status", equals="completed"),
            direct("recipeValidate", "structuredContent.plan.valid", equals=True),
            direct("recipeDone", "structuredContent.status", equals="completed"),
            direct("recipeDone", "structuredContent.nodes.data.status", equals="applied"),
            scenario="One plan and one host-local recipe combining a host file, an Incus container and a SQLite "
                     "database reach completed; their evidence records each node; a repeat re-runs the same run "
                     "to completed"),
        req(SPEC, "Durable and redacted observations",
            direct("operation", "isError", **{"in": [False, "<absent>"]}),
            direct("operation", "structuredContent.taskId", equalsStep={"step": "run",
                   "path": "response.body.result.structuredContent.runId"}),
            direct("operations", "isError", **{"in": [False, "<absent>"]}),
            direct("unknown", "isError", equals=True),
            direct("unknown", "content[0].text", equals="Error: operation not found: no-such-operation"),
            scenario="The plan run reads back by id through get_operation; the operation list answers; an "
                     "unknown id is an error"),
        req(Q25_SPEC, "An operation record reports its task's terminal status",
            direct("operation", "structuredContent.status", equals="completed"),
            scenario="The completed plan run's operation row reports completed (D28)"),
        req(SPEC, "One typed plan and recipe execution path (Interrupted plan)",
            ok("slow"),
            direct("waiting", "structuredContent.nodes.file.status", equals="applied"),
            direct("cancel", "isError", **{"in": [False, "<absent>"]}),
            direct("cancel", "structuredContent.status", equals="cancelled"),
            direct("cancelled", "structuredContent.status", equals="cancelled"),
            direct("cancelled", "structuredContent.nodes.file.status", equals="applied"),
            direct("cancelled", "structuredContent.nodes.guest.status", **{"in": ["pending", "<absent>"]}),
            a("afterCancel", "stdout", equals="0\nfile-kept\n"),
            direct("cancelAgain", "isError", equals=True),
            direct("cancelAgain", "content[0].text", matches="cannot be cancelled in status"),
            scenario="A running plan cancelled through cancel_operation: the completed node's effect stays, the "
                     "nodes after it never run, and the run's durable state is cancelled; a second cancel is "
                     "refused"),
    ]
    cleanup = [{"id": "cleanPlans", "guest": (
        f"{T} slow stop >/dev/null; for n in e2e-p1 e2e-r1 e2e-c1; do incus delete -f $n 2>/dev/null; done; "
        f"rm -rf {HOME}/e2e {HOME}/.opute/databases/e2e-*; "
        "incus storage volume list default -f csv | awk -F, '$1==\"image\"{print $2}' | "
        "xargs -r -I{} incus storage volume delete default image/{}; true")}]
    allow = ["incus/*", "files//home/opute/e2e*", "files//home/opute/.opute*", "system/units/e2e-slow*",
             "system/unitFiles/e2e-slow*", "system/units/sys-devices-virtual-net-veth*",
             "system/units/sys-subsystem-net-devices-veth*"]
    persist = ["files//home/opute/.opute*"]
    inputs = {"agent": base.AGENT, "slow": "a local endpoint answering after 25 s (e2etool slow)", "files": files}
    return journey(
        "e2e2.plans", "e2e-2",
        "Plans across domains: one host-plan.v1 run and one host-local recipe, each combining a host file, an "
        "Incus container and a SQLite database, using published primitives only, to completion; the run read "
        "back as a durable operation; then a running plan cancelled mid-way through cancel_operation.",
        reqs, steps, cleanup, allow=allow, persist=persist, minutes=25, inputs=inputs)


# --- a single-node K3s cluster through typed calls (the e2e-3a build's first server) ------------------------

K = ha.K
CLUSTER = ha.CLUSTER
CL = ha.CL
K1 = "container:local:e2e-k1"
RELEASE = "${PARITY}/e2e/release/rust-e2e13a"  # the release the dev VM was rebuilt on (this branch)
CLUSTER_FILES = {**base.TOOL_FILES, "providerK3s": inp(f"{RELEASE}/opute-provider-k3s-linux-x64"),
                 "activate": ha.FILES["activate"]}


def cluster_steps(vm: dict | None = None) -> list[dict]:
    """Host A's k3s provider, one Incus server and K3s with clusterInit: e2e3a.build's first half."""
    return base.TOOL_STEPS + [
        {"id": "providerCopy", "copy": "providerK3s", "to": ha.PROVIDER, "mode": "0755"},
        {"id": "recipeCopy", "copy": "activate", "to": ha.RECIPE, "mode": "0644"},
        {"id": "setup", "guest": (
            f"chown -R opute /home/opute/.config/opute-e2e && systemd-run --collect --unit=e2e-provider-k3s "
            f"--uid=opute --gid=opute --setenv=HOME=/home/opute --setenv=OPUTE_PROVIDER_PORT=4320 "
            f"--property=Restart=no {ha.PROVIDER} >/dev/null && for i in $(seq 60); do ss -ltnH 'sport = :4320' | "
            "grep -q . && break; sleep 0.5; done; ss -ltnH 'sport = :4320' | wc -l")},
        call("installA", "opute.provider.install",
             {"descriptor": ha.descriptor(4320), "activate": True, "recipeSource": ha.RECIPE}),
        {"id": "providerToolsA", "guest": ha.wait_tool(3014, "${TOKEN}")},
        {"id": "catalogAfterA", "mcp": "tools/list"},
        *task("vm1", "provision_vm", {"name": "e2e-k1", **(vm or ha.VM)}),
        call("info1", "get_vm_info", {"uri": K1}, capture={"K1_IP": "response.body.result.structuredContent.ipv4[0]"}),
        *task("provision", K + "provision", {**ha.target("e2e-k1"), "version": ha.VERSION, "clusterInit": True}),
        call("ready", K + "status", CL,
             until={"path": "response.body.result.structuredContent.readyNodeCount", "in": [1]}, timeout=300,
             interval=5),
    ]


def cluster_requirements() -> list[dict]:
    return [req(ha.CATALOG, "A K3s server provisioned through typed Host Agent operations",
                ok("setup"), a("setup", "stdout", equals="1\n"),
                direct("installA", "isError", **{"in": [False, "<absent>"]}),
                a("providerToolsA", "json.listed", equals=True),
                res("vm1", "isError", equals=False), res("provision", "structuredContent.provisioned", equals=True),
                direct("ready", "structuredContent.readyNodeCount", equals=1),
                scenario="The single-node cluster the journey runs on")]


def cluster_cleanup(extra: str = "") -> list[dict]:
    return [*task("cleanRemove", K + "remove", CL), *task("cleanDelete1", "delete_vm", {"uri": K1}),
            {"id": "cleanSetup", "guest": (
                f"{extra}systemctl stop e2e-provider-k3s 2>/dev/null; incus delete -f e2e-k1 2>/dev/null; "
                "incus storage volume list default -f csv | awk -F, '$1==\"image\"{print $2}' | "
                "xargs -r -I{} incus storage volume delete default image/{}; "
                "rm -rf /home/opute/.config/opute-e2e /opt/opute-e2e/providers; true")}]


CLUSTER_EFFECTS = ["incus/*", "files//home/opute/.config/opute-e2e*", "files//home/opute/.config/incus*",
                   "files//opt/opute-e2e/providers*", "system/units/e2e-*", "system/unitFiles/e2e-*",
                   "system/units/sys-devices-virtual-net-veth*", "system/units/sys-subsystem-net-devices-veth*"]
CLUSTER_INPUTS = {"agent": base.AGENT + ", with the Rust k3s provider 1.0.2 from the release (e2e-provider-k3s, "
                                        "port 4320)",
                  "k3s": ha.INPUTS["k3s"]}


# --- e2e-2 / 4. OCI build, push and pull ------------------------------------------------------------------

APP = {"Dockerfile": "FROM docker.io/library/busybox:1.36\nCOPY src/ /app/\nCMD [\"sh\", \"-c\", \"cat /app/main.txt; "
                     "sleep 3600\"]\n",
       "src/main.txt": "hello from the e2e registry\n"}
CTX = f"{HOME}/.opute/build-contexts/e2e-app"
def registry_digest(repository: str, tag: str) -> str:
    """The manifest digest the e2e registry serves for repository:tag."""
    return ("curl -fsSI -H 'Accept: application/vnd.oci.image.manifest.v1+json' -H 'Accept: application/vnd.docker."
            f"distribution.manifest.v2+json' http://${{K1_IP}}:30500/v2/{repository}/manifests/{tag} | tr -d '\\r' | "
            "awk 'tolower($1)==\"docker-content-digest:\"{printf \"%s\", $2}'")


POD = ("apiVersion: v1\nkind: Pod\nmetadata:\n  name: e2e-pull\n  namespace: default\nspec:\n  containers:\n"
       "  - name: app\n    image: ${K1_IP}:30500/e2e/app@${DIGEST}\n")


# what installing and purging Podman leaves: its packages' config, and the agent user's container state
PKGS = "dpkg -l | awk '/^ii/{print $2}' | sort"
PKGS_BEFORE = {"id": "pkgsBefore", "guest": f"{PKGS} > /tmp/e2e-pkgs-before && echo recorded"}
PODMAN_CLEANUP = (
    "rm -rf /home/opute/.local/share/containers /home/opute/.config/containers /home/opute/.config/cni "
    "/home/opute/.opute/build-contexts; rmdir /home/opute/.opute 2>/dev/null; "
    f"if [ -f /tmp/e2e-pkgs-before ]; then {PKGS} > /tmp/e2e-pkgs-after; comm -13 /tmp/e2e-pkgs-before "
    "/tmp/e2e-pkgs-after | DEBIAN_FRONTEND=noninteractive xargs -r apt-get purge -y -q >/dev/null 2>&1; fi; "
    "rm -f /tmp/e2e-pkgs-before /tmp/e2e-pkgs-after; "
    # what the purge and apt leave behind: Podman's runtime network config, the empty apparmor disable directory
    # passt's purge removes, and the units apt started
    "rm -rf /etc/containers; mkdir -p -m 0755 /etc/apparmor.d/disable; systemctl daemon-reload; "
    "systemctl stop packagekit.service boot-complete.target 2>/dev/null; "
    "systemctl reset-failed podman-restart.service 2>/dev/null; ")
PODMAN_EFFECTS = ["files//home/opute/.local/share/containers*", "files//home/opute/.config/containers*",
                  "files//home/opute/.config/cni*", "files//usr*", "files//etc*", "files//var*", "files//opt/cni*",
                  "system/*", "packages/*", "users/*", "groups/*"]
SUPERVISOR_EFFECTS = ["user/*", "linger*", "files//var/lib/systemd/linger*", "files//home/opute/.config/systemd*"]


def oci() -> dict:
    vm = {**ha.VM, "cpus": 3, "memory": "4GiB", "disk": "20GiB"}
    steps = cluster_steps(vm) + [
        PKGS_BEFORE,
        *task("builder", "ensure_oci_builder", {}, timeout=900),
        *task("stage", "stage_build_context", {"destDir": CTX, "files": APP}),
        *task("registry", "install_oci_registry", {"uri": CLUSTER, "image": "registry:2.8.3", "storageSize": "1Gi"},
              timeout=600),
        call("registryStatus", "get_oci_registry_status", {"uri": CLUSTER},
             until={"path": "response.body.result.structuredContent.deployment.status.readyReplicas", "in": [1]},
             timeout=300, interval=5),
        *task("push", "build_and_push_oci_image",
              {"contextDir": CTX, "image": "${K1_IP}:30500/e2e/app:1", "insecureRegistry": True}, timeout=900),
        # the push result names no digest; the registry's manifest answer does
        {"id": "digest", "guest": registry_digest("e2e/app", "1"), "capture": {"DIGEST": "stdout"}},
        *task("trust", K + "configure-registry",
              {**CL, "endpoint": "http://${K1_IP}:30500", "registry": "${K1_IP}:30500", "insecure": True},
              timeout=600),
        call("readyAgain", K + "status", CL,
             until={"path": "response.body.result.structuredContent.readyNodeCount", "in": [1]}, timeout=300,
             interval=5),
        *task("pull", "apply_manifest", {"uri": CLUSTER, "manifest": POD}),
        call("pulled", "get_cluster_resource_status",
             {"uri": CLUSTER, "resourceKind": "Pod", "namespace": "default", "resourceName": "e2e-pull"},
             until={"path": "response.body.result.structuredContent.status", "in": ["ready"]}, timeout=300,
             interval=5),
        call("logs", "exec_cluster_command", {"uri": CLUSTER, "command": "logs", "args": ["e2e-pull", "-n", "default"]}),
        # storage policy and cleanup. Without a user session Podman warns on stderr; the report is parsed from
        # stdout alone (D28, Q26). The user service supervisor then provides the session
        call("inspectNoSession", "inspect_container_storage", {}),
        call("supervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        call("inspect", "inspect_container_storage", {}),
        *task("policy", "configure_oci_storage", {"uri": CLUSTER, "minAgeSeconds": 3600}),
        *task("dryRun", "cleanup_container_storage", {"dryRun": True}),
        *task("cleanup", "cleanup_container_storage", {"minAgeSeconds": 3600}),
        call("inspectAfter", "inspect_container_storage", {}),
        *task("deleteRegistry", "delete_oci_registry", {"uri": CLUSTER}),
    ]
    reqs = cluster_requirements() + [
        req(CATALOG, "ensure_oci_builder, stage_build_context, install_oci_registry, build_and_push_oci_image",
            res("builder", "isError", equals=False),
            res("stage", "isError", equals=False),
            res("registry", "isError", equals=False),
            direct("registryStatus", "structuredContent.status", equals="installed"),
            direct("registryStatus", "structuredContent.deployment.status.readyReplicas", equals=1),
            res("push", "isError", equals=False),
            res("push", "structuredContent.pushed", equals=True),
            a("digest", "stdout", matches="^sha256:[0-9a-f]{64}$"),
            scenario="An image built on the host from a staged context and pushed to a registry running in an "
                     "e2e- guest's cluster"),
        req(CATALOG, "The pushed image is pulled by digest and runs",
            res("trust", "isError", equals=False),
            res("pull", "isError", equals=False),
            direct("pulled", "structuredContent.status", equals="ready"),
            direct("logs", "structuredContent.stdout", matches="hello from the e2e registry"),
            scenario="The cluster guest pulls the image by its digest from the registry and runs it"),
        req(Q26_SPEC, "A runtime warning does not fail a storage report",
            direct("inspectNoSession", "isError", **{"in": [False, "<absent>"]}),
            direct("inspectNoSession", "structuredContent.runtime", equals="container-engine"),
            scenario="Right after the builder is installed, with no user session, the storage report is read "
                     "while Podman warns on stderr (D28)"),
        req(CATALOG, "configure_oci_storage, inspect_container_storage, cleanup_container_storage",
            direct("supervisorAgain", "structuredContent.status", equals="ready"),
            direct("inspect", "isError", **{"in": [False, "<absent>"]}),
            res("policy", "isError", equals=False),
            res("dryRun", "isError", equals=False),
            res("cleanup", "isError", equals=False),
            direct("inspectAfter", "isError", **{"in": [False, "<absent>"]}),
            res("deleteRegistry", "isError", equals=False),
            scenario="With the user service supervisor ready: the storage policy applied, a dry run, a cleanup "
                     "with the shortest allowed age, and the registry deleted"),
    ]
    cleanup = cluster_cleanup(f"rm -rf {CTX}; loginctl disable-linger opute; " + PODMAN_CLEANUP)
    allow = CLUSTER_EFFECTS + ["files//home/opute/.opute*"] + PODMAN_EFFECTS + SUPERVISOR_EFFECTS
    return journey(
        "e2e2.oci", "e2e-2",
        "OCI build, push and pull on a single-node K3s cluster built through typed calls: ensure a builder, stage "
        "a context, install a registry in the cluster, build and push an image, trust the registry in K3s and "
        "run the image by digest in a pod; then apply the storage policy, inspect and clean up.",
        reqs, steps, cleanup, allow=allow, persist=["files//var/lib/dpkg*", "files//var/log*", "files//var/cache*"],
        minutes=28, inputs={**CLUSTER_INPUTS, "files": CLUSTER_FILES})


# --- e2e-2 / 5. data services -----------------------------------------------------------------------------

PG = {"uri": CLUSTER, "clusterName": "pg", "namespace": "e2e-data", "databases": ["app"],
      "consumerSecretName": "pg-consumer", "consumerSecretLabel": "opute.io/pg-consumer", "serviceOwner": "e2e",
      "servicePartOf": "e2e", "consumerDatabaseKeys": {"app": "DATABASE_URL"}}
RELAY = {"sessionId": "e2e-session", "relayToken": "${SECRET:relayToken}", "listenPort": 15433, "persistent": True}
SDB = base.SDB


def data() -> dict:
    vm = {**ha.VM, "cpus": 3, "memory": "5GiB", "disk": "20GiB"}
    psql = ("incus exec e2e-k1 -- k3s kubectl -n e2e-data exec pg-1 -c postgres -- psql -d app -Atqc")
    steps = cluster_steps(vm) + [
        *task("reconcile", "reconcile_postgresql_service", PG, timeout=1200),
        call("relay", "get_postgresql_service_status", {**PG, "relayDeviceName": "e2e-pg-relay", "localRelay": RELAY}),
        {"id": "relayClient", "guest": (
            "python3 - <<'PY'\nimport socket\ns = socket.create_connection(('127.0.0.1', 15433), timeout=20)\n"
            "s.sendall(b'${SECRET:relayToken}\\n')\ns.sendall(bytes([0, 0, 0, 8, 4, 210, 22, 47]))\n"
            "print((s.recv(1) or b'closed').decode())\nPY")},
        {"id": "relayWrongToken", "guest": (
            "python3 - <<'PY'\nimport socket\ns = socket.create_connection(('127.0.0.1', 15433), timeout=20)\n"
            "try:\n    s.sendall(b'not-the-token\\n'); s.sendall(bytes([0, 0, 0, 8, 4, 210, 22, 47]))\n"
            "    print((s.recv(1) or b'closed').decode())\nexcept OSError:\n    print('closed')\nPY")},
        {"id": "write", "guest": f"{psql} \"CREATE TABLE e2e(note text); INSERT INTO e2e VALUES ('written')\""},
        {"id": "read", "guest": f"{psql} 'SELECT note FROM e2e'"},
        {"id": "tokenRows", "guest": f"{T} contains {SDB} '${{SECRET:relayToken}}'"},
        call("releaseWrong", "release_postgresql_service_relay", {"sessionId": "e2e-session",
                                                                  "relayToken": "not-the-relay-token-0123456789abcdef"}),
        call("release", "release_postgresql_service_relay", {"sessionId": "e2e-session",
                                                             "relayToken": "${SECRET:relayToken}"}),
        *task("remove", "remove_postgresql_service", {**PG, "confirm": True, "retentionPolicy": "delete"}, timeout=600),
        # the service and its data go; the namespace is the caller's and stays
        {"id": "gone", "guest": (
            "for i in $(seq 60); do n=$(incus exec e2e-k1 -- k3s kubectl -n e2e-data get "
            "clusters.postgresql.cnpg.io,pvc -o name | wc -l); [ \"$n\" = 0 ] && break; sleep 5; done; echo $n")},
        # SQLite
        call("sqlite", "ensure_sqlite_database", {"consumerId": "e2e-a", "databaseName": "main"}),
        call("sqliteAgain", "ensure_sqlite_database", {"consumerId": "e2e-a", "databaseName": "main"}),
        call("sqliteStatus", "get_sqlite_database_status", {"consumerId": "e2e-a", "databaseName": "main"}),
        call("sqliteUnconfirmed", "remove_sqlite_database", {"consumerId": "e2e-a", "databaseName": "main",
                                                             "confirm": False}),
        call("sqliteRemove", "remove_sqlite_database", {"consumerId": "e2e-a", "databaseName": "main",
                                                        "confirm": True}),
        {"id": "sqliteGone", "guest": f"ls {HOME}/.opute/databases/e2e-a 2>/dev/null | wc -l"},
    ]
    reqs = cluster_requirements() + [
        req(CATALOG, "reconcile_postgresql_service, get_postgresql_service_status (localRelay)",
            res("reconcile", "isError", equals=False),
            direct("relay", "isError", **{"in": [False, "<absent>"]}),
            a("relayClient", "stdout", **{"in": ["S\n", "N\n"]}),
            a("relayWrongToken", "stdout", equals="closed\n"),
            ok("write"), a("read", "stdout", equals="written\n"),
            scenario="A client reaches PostgreSQL through the relay with the issued token (the server answers "
                     "the SSL probe); a wrong token is dropped; a row is written and read back"),
        req(SPEC, "Durable and redacted observations",
            a("tokenRows", "json.rows", equals=0),
            scenario="Secret-bearing input (D24): the relay token is in no durable row"),
        req(CATALOG, "release_postgresql_service_relay, remove_postgresql_service",
            direct("releaseWrong", "isError", equals=True),
            direct("release", "isError", **{"in": [False, "<absent>"]}),
            res("remove", "isError", equals=False),
            a("gone", "stdout", equals="0\n"),
            scenario="The relay released only with its token; removal leaves no service or data behind"),
        req(CATALOG, "ensure_sqlite_database, get_sqlite_database_status, remove_sqlite_database",
            direct("sqlite", "isError", **{"in": [False, "<absent>"]}),
            direct("sqliteAgain", "isError", **{"in": [False, "<absent>"]}),
            direct("sqliteStatus", "structuredContent.exists", equals=True),
            direct("sqliteUnconfirmed", "isError", equals=True),
            direct("sqliteRemove", "isError", **{"in": [False, "<absent>"]}),
            a("sqliteGone", "stdout", equals="0\n"),
            scenario="A caller-scoped SQLite database provisioned, reported, refused removal without "
                     "confirmation, and removed"),
    ]
    cleanup = cluster_cleanup(f"rm -rf {HOME}/.opute/databases; ")
    allow = CLUSTER_EFFECTS + ["files//home/opute/.opute*", "system/units/opute-*", "user/units/*",
                               "user/unitFiles/*", "files//home/opute/.config/systemd*", "linger*",
                               "files//var/lib/systemd/linger*", "system/units/user*", "system/units/run-user-*"]
    return journey(
        "e2e2.data", "e2e-2",
        "Data services on a single-node K3s cluster built through typed calls: a CloudNativePG service reconciled "
        "with a local relay, a client through the relay with the issued token writing and reading a row, the "
        "token in no durable row, relay release and service removal; then a SQLite database's lifecycle.",
        reqs, steps, cleanup, allow=allow, persist=["files//home/opute/.opute*"], minutes=28,
        inputs={**CLUSTER_INPUTS, "files": CLUSTER_FILES})


# --- e2e-3 / 1. cluster discovery -------------------------------------------------------------------------

USE_CASES = ("published docs, host-agents c9f9e775 (v0.2.4) site/public/use-cases/ (\"Discover Kubernetes "
             "clusters when configured\")")


def discovery() -> dict:
    steps = base.TOOL_STEPS + [
        {"id": "catalog", "mcp": "tools/list"},
        # no provider: discovery is not configured
        call("noProvider", "list_compute_clusters", {}),
        *cluster_steps()[2:5],  # provider copy, recipe copy, setup (TOOL_STEPS already ran)
        call("installA", "opute.provider.install",
             {"descriptor": ha.descriptor(4320), "activate": True, "recipeSource": ha.RECIPE}),
        {"id": "providerToolsA", "guest": ha.wait_tool(3014, "${TOKEN}")},
        {"id": "catalogAfterA", "mcp": "tools/list"},
        # an available provider and no cluster: an empty inventory
        call("empty", "list_compute_clusters", {}),
        {"id": "names", "guest": (
            "python3 -c 'import sys, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
            "r = mcpwire.mcp(3014, sys.argv[1], \"tools/list\", request_id=1)\n"
            "names = {t[\"name\"] for t in r[\"body\"][\"result\"][\"tools\"]}\n"
            "print(json.dumps({n: n in names for n in (\"list_compute_clusters\", \"list_kubernetes_clusters\")}))' "
            "'${TOKEN}'")},
        # one healthy cluster
        *task("vm1", "provision_vm", {"name": "e2e-k1", **ha.VM}),
        call("info1", "get_vm_info", {"uri": K1}, capture={"K1_IP": "response.body.result.structuredContent.ipv4[0]"}),
        *task("provision", K + "provision", {**ha.target("e2e-k1"), "version": ha.VERSION, "clusterInit": True}),
        call("ready", K + "status", CL,
             until={"path": "response.body.result.structuredContent.readyNodeCount", "in": [1]}, timeout=300,
             interval=5),
        call("healthy", "list_compute_clusters", {}),
        call("namespaces", "list_namespaces", {"uri": CLUSTER}),
        call("pods", "list_pods", {"uri": CLUSTER, "namespace": "kube-system"}),
        call("services", "list_services", {"uri": CLUSTER, "namespace": "kube-system"}),
        call("events", "list_cluster_events", {"uri": CLUSTER, "namespace": "kube-system"}),
        call("configmapKeys", "list_configmap_keys", {"uri": CLUSTER, "namespace": "kube-system",
                                                      "configMapName": "coredns"}),
        call("secretKeys", "list_secret_keys", {"uri": CLUSTER, "namespace": "kube-system",
                                                "secretName": "k3s-serving"}),
        # a failed probe: the cluster's server is stopped
        *task("stop", "stop_vm", {"uri": K1}),
        call("stoppedList", "list_compute_clusters", {}),
        call("stoppedEvents", "list_cluster_events", {"uri": CLUSTER, "namespace": "kube-system"}),
        # a failed probe: the provider behind discovery is unreachable
        {"id": "providerDown", "guest": "systemctl stop e2e-provider-k3s && echo stopped"},
        call("unreachable", "list_compute_clusters", {}),
        {"id": "providerUp", "guest": (
            f"systemd-run --collect --unit=e2e-provider-k3s --uid=opute --gid=opute --setenv=HOME=/home/opute "
            f"--setenv=OPUTE_PROVIDER_PORT=4320 --property=Restart=no {ha.PROVIDER} >/dev/null && for i in "
            "$(seq 60); do ss -ltnH 'sport = :4320' | grep -q . && break; sleep 0.5; done; echo started")},
        *task("start", "start_vm", {"uri": K1}),
        call("recovered", "list_compute_clusters", {},
             until={"path": "response.body.result.structuredContent.total", "in": [1]}, timeout=180, interval=5),
        {"id": "joinToken", "guest": "incus exec e2e-k1 -- cat /var/lib/rancher/k3s/server/token | tr -d '\\n'",
         "capture": {"K3S_TOKEN": {"path": "stdout", "secret": True}}},
        {"id": "tokenLeaks", "guest": (
            f"{{ grep -a -r -o -F '${{K3S_TOKEN}}' {ha.ADB_A} 2>/dev/null; for u in opute-bootstrap-mcp "
            "e2e-provider-k3s; do journalctl -u $u --no-pager -o cat; done | grep -o -F '${K3S_TOKEN}'; } | wc -l")},
    ]
    reqs = [
        req(USE_CASES, "An empty inventory and a failed inventory probe are different outcomes",
            direct("noProvider", "isError", equals=True),
            direct("noProvider", "content[0].text", equals="Error: no active Kubernetes provider is installed"),
            direct("empty", "isError", **{"in": [False, "<absent>"]}),
            direct("empty", "structuredContent.total", equals=0),
            direct("healthy", "structuredContent.total", equals=1),
            direct("healthy", "structuredContent.clusters[0].uri", equals=CLUSTER),
            direct("unreachable", "isError", equals=True),
            direct("recovered", "structuredContent.total", equals=1),
            scenario="No provider, an available provider with no cluster, one healthy cluster, and an unreachable "
                     "provider: the empty inventory is a successful result with no clusters, the failed probe an "
                     "error result"),
        req(USE_CASES, "Discovery under the name tools/list resolves",
            a("names", "json.list_compute_clusters", equals=True),
            a("names", "json.list_kubernetes_clusters", equals=False),
            scenario="The page names list_kubernetes_clusters, Go v0.2.4's published name and the deployed "
                     "product's. Rust's tools/list resolves discovery to the neutral list_compute_clusters (D22); "
                     "the Go name is a hidden alias until cutover, so the page must change with the cutover"),
        req(CATALOG, "list_namespaces, list_pods, list_services, list_cluster_events on a discovered cluster",
            direct("namespaces", "isError", **{"in": [False, "<absent>"]}),
            direct("pods", "isError", **{"in": [False, "<absent>"]}),
            direct("services", "isError", **{"in": [False, "<absent>"]}),
            direct("events", "isError", **{"in": [False, "<absent>"]}),
            scenario="Resources and events of the discovered cluster are read through typed calls"),
        req(CATALOG, "A probe of a stopped cluster fails closed",
            res("stop", "structuredContent.status", equals="stopped"),
            direct("stoppedEvents", "isError", equals=True),
            direct("stoppedEvents", "content[0].text", matches="Instance is not running"),
            scenario="With its server stopped, reading the cluster's events fails closed with an error result"),
        req(Q24_SPEC, "A stopped cluster never reads as an empty inventory",
            direct("stoppedList", "isError", **{"in": [False, "<absent>"]}),
            direct("stoppedList", "structuredContent.total", equals=1),
            direct("stoppedList", "structuredContent.clusters[0].uri", equals=CLUSTER),
            direct("stoppedList", "structuredContent.clusters[0].status", equals="Stopped"),
            scenario="With its only server stopped, the registered cluster is listed with its instance status "
                     "and no node data (D28)"),
        req(SEAM_SPEC, "Every capability has its three roles",
            direct("configmapKeys", "structuredContent.code", equals="not_implemented"),
            direct("secretKeys", "structuredContent.code", equals="not_implemented"),
            scenario="The e2e-3 goal lists config and secret keys: both tools are published but have no "
                     "provider in Rust and no handler in Go v0.2.4, so they fail closed (Q18)"),
        req(SPEC, "Durable and redacted observations",
            ok("tokenLeaks"), a("tokenLeaks", "stdout", equals="0\n"),
            scenario="No discovery result, state row or journal line carries the cluster's join token"),
    ]
    inputs = {**CLUSTER_INPUTS, "files": CLUSTER_FILES}
    return journey(
        "e2e3.discovery", "e2e-3",
        "Cluster discovery through the neutral name, as the Use cases page describes it: no provider; an "
        "available provider with an empty inventory; one healthy single-node K3s cluster with its resources, "
        "events and keys; the cluster's server stopped; the provider unreachable (a failed probe), then back.",
        reqs, steps, cluster_cleanup(), allow=CLUSTER_EFFECTS, minutes=25, inputs=inputs)


# --- e2e-3 / 2. site deploy through its recipe -------------------------------------------------------------

SITE_DEPLOY = ("wunderous/opute-site-deploy 9cdd932 site/recipes/www-opute-io.yaml (recipe 1.1.6) and "
               "scripts/deploy_site.py, as the e2e-3 goal adapts them: local image by digest, local test "
               "hostname, no tunnel")
SITE_SRC = "https://codeload.github.com/wunderous/host-agents/tar.gz/c9f9e775d8648a54ebaccc052c840cc1dcbdc42e"
SITE_SRC_SHA = "d995d8410af9219d38d8b5f3aa792669c27ca0f3916953e1a541f0fb4404b930"
SITE_CTX = f"{HOME}/.opute/build-contexts/e2e-site"
SITE_RECIPE = f"{HOME}/e2e/www.yaml"
SITE_TEARDOWN = f"{HOME}/e2e/www-teardown.yaml"
SITE_HOST = "www.e2e.test"


def site() -> dict:
    files = {**CLUSTER_FILES,
             "recipe": inp("${REPO}/evidence/e2e/journeys/inputs/e2e-site-www.yaml"),
             "teardown": inp("${REPO}/evidence/e2e/journeys/inputs/e2e-site-www-teardown.yaml"),
             "workload": inp("${REPO}/evidence/e2e/journeys/inputs/e2e-site-workload.yaml")}
    vm = {**ha.VM, "cpus": 3, "memory": "4GiB", "disk": "20GiB"}
    image = "${K1_IP}:30500/host-agent-www@${DIGEST}"
    inputs = {"host": "e2e-devvm-agent", "clusterUri": CLUSTER, "image": image, "workloadManifest": "${MANIFEST}"}
    steps = cluster_steps(vm) + [
        {"id": "recipeCopy2", "copy": "recipe", "to": SITE_RECIPE, "mode": "0644"},
        {"id": "teardownCopy", "copy": "teardown", "to": SITE_TEARDOWN, "mode": "0644"},
        {"id": "workloadCopy", "copy": "workload", "to": f"{HOME}/e2e/workload.yaml", "mode": "0644"},
        {"id": "own", "guest": f"chown -R opute:opute {HOME}/e2e && echo ok"},
        # the site image, built locally from the published source and pushed to an e2e registry
        *task("source", "ensure_host_artifact", {"uri": SITE_SRC, "destination": "e2e/site-src.tar.gz",
                                                 "sha256": SITE_SRC_SHA}),
        # host commands run under the user service manager
        call("supervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        *task("unpack", "run_host_command",
              {"command": f"mkdir -p {SITE_CTX} && tar -xzf $HOME/e2e/site-src.tar.gz -C {SITE_CTX} "
                          "--strip-components=2 host-agents-c9f9e775d8648a54ebaccc052c840cc1dcbdc42e/site && "
                          f"ls {SITE_CTX}"}),
        PKGS_BEFORE,
        *task("builder", "ensure_oci_builder", {}, timeout=900),
        *task("registry", "install_oci_registry", {"uri": CLUSTER, "image": "registry:2.8.3", "storageSize": "1Gi"},
              timeout=600),
        call("registryStatus", "get_oci_registry_status", {"uri": CLUSTER},
             until={"path": "response.body.result.structuredContent.deployment.status.readyReplicas", "in": [1]},
             timeout=300, interval=5),
        *task("push", "build_and_push_oci_image",
              {"contextDir": SITE_CTX, "image": "${K1_IP}:30500/host-agent-www:e2e", "insecureRegistry": True},
              timeout=900),
        {"id": "digest", "guest": registry_digest("host-agent-www", "e2e"), "capture": {"DIGEST": "stdout"}},
        *task("trust", K + "configure-registry",
              {**CL, "endpoint": "http://${K1_IP}:30500", "registry": "${K1_IP}:30500", "insecure": True},
              timeout=600),
        call("readyAgain", K + "status", CL,
             until={"path": "response.body.result.structuredContent.readyNodeCount", "in": [1]}, timeout=300,
             interval=5),
        # what deploy_site.py does before it calls the agent: fill in the image
        {"id": "render", "guest": (
            f"sed -i 's|__EXPECTED_IMAGE__|{image}|' {SITE_RECIPE} && sed 's|__IMAGE__|{image}|' "
            f"{HOME}/e2e/workload.yaml > {HOME}/e2e/workload.rendered.yaml && chown opute:opute {HOME}/e2e/* && "
            f"python3 -c 'import json,sys; print(json.dumps({{\"manifest\": open(sys.argv[1]).read()}}))' "
            f"{HOME}/e2e/workload.rendered.yaml"),
         "capture": {"MANIFEST": "json.manifest"}},
        call("validate", "validate_host_local_recipe", {"source": SITE_RECIPE, "inputs": inputs}),
        call("deploy", "run_host_local_recipe", {"source": SITE_RECIPE, "inputs": inputs},
             capture={"RUN": "response.body.result.structuredContent.runId"}),
        base.poll_run("deployed", "RUN", timeout=900),
        # D28 (Q28): the hostname is mapped through configure_service_domain, bound to the cluster by uri
        *task("domain", "configure_service_domain",
              {"uri": CLUSTER, "namespace": "host-agent-site", "ingressName": "e2e-www", "hostname": SITE_HOST,
               "serviceName": "host-agent-www", "servicePort": 80}),
        {"id": "serve", "guest": (
            f"for i in $(seq 60); do code=$(curl -s -o /tmp/e2e-site.html -w '%{{http_code}}' --max-time 5 -H "
            f"'Host: {SITE_HOST}' http://${{K1_IP}}/); [ \"$code\" = 200 ] && break; sleep 2; done; echo $code; "
            "grep -c 'Opute Host Agent' /tmp/e2e-site.html")},
        call("image", "get_cluster_resource_status",
             {"uri": CLUSTER, "resourceKind": "Deployment", "namespace": "host-agent-site",
              "resourceName": "host-agent-www"}),
        call("again", "run_host_local_recipe", {"source": SITE_RECIPE, "inputs": inputs}),
        base.poll_run("againDone", "RUN", timeout=600),
        call("imageAgain", "get_cluster_resource_status",
             {"uri": CLUSTER, "resourceKind": "Deployment", "namespace": "host-agent-site",
              "resourceName": "host-agent-www"}),
        {"id": "released", "guest": "sleep 5"},
        # removal: the domain mapping, then the teardown recipe's workload node
        *task("unmap", "remove_service_domain",
              {"uri": CLUSTER, "namespace": "host-agent-site", "ingressName": "e2e-www"}),
        call("teardown", "run_host_local_recipe", {"source": SITE_TEARDOWN,
                                                   "inputs": {"host": "e2e-devvm-agent", "clusterUri": CLUSTER}},
             capture={"TRUN": "response.body.result.structuredContent.runId"}),
        base.poll_run("tornDown", "TRUN", timeout=600),
        call("gone", "get_cluster_resource_status",
             {"uri": CLUSTER, "resourceKind": "Deployment", "namespace": "host-agent-site",
              "resourceName": "host-agent-www"},
             until={"path": "response.body.result.structuredContent.status", "in": ["missing"]}, timeout=300,
             interval=5),
        {"id": "unserved", "guest": f"curl -s -o /dev/null -w '%{{http_code}}' --max-time 5 -H 'Host: {SITE_HOST}' "
                                    "http://${K1_IP}/"},
    ]
    reqs = cluster_requirements() + [
        req(SITE_DEPLOY, "The site answers over HTTP at the test hostname with the pinned image",
            res("source", "isError", equals=False), res("unpack", "structuredContent.exitCode", equals=0),
            a("digest", "stdout", matches="^sha256:[0-9a-f]{64}$"),
            res("trust", "isError", equals=False),
            direct("validate", "structuredContent.valid", equals=True),
            direct("deployed", "structuredContent.status", equals="completed"),
            res("domain", "isError", equals=False),
            a("serve", "stdout", matches="^200\n[1-9]"),
            direct("image", "structuredContent.resource.spec.template.spec.containers[0].image",
                   matches=r"@sha256:[0-9a-f]{64}$"),
            scenario="The site image built from the published source at c9f9e775, pushed to the e2e registry and "
                     "deployed through the recipe's own nodes by digest; the page is served at www.e2e.test"),
        req(SITE_DEPLOY, "A rerun is a no-op",
            {"step": "again", "path": "response.body.result.structuredContent.runId",
             "equalsStep": {"step": "deploy", "path": "response.body.result.structuredContent.runId"}},
            direct("againDone", "structuredContent.status", equals="completed"),
            direct("imageAgain", "structuredContent.resource.metadata.generation",
                   equalsStep={"step": "image", "path": "response.body.result.structuredContent.resource.metadata."
                                                        "generation"}),
            scenario="The same recipe and inputs re-run the same run, which revalidates and completes without "
                     "changing the Deployment (its generation is unchanged)"),
        req(Q28_SPEC, "A published service domain call can succeed",
            res("domain", "isError", equals=False),
            res("unmap", "isError", equals=False),
            scenario="Standalone mode publishes the cluster-bound descriptors: the hostname is mapped and unmapped "
                     "through configure_service_domain and remove_service_domain with their listed inputs (D28)"),
        req(SITE_DEPLOY, "Removal leaves nothing behind",
            direct("tornDown", "structuredContent.status", equals="completed"),
            direct("gone", "structuredContent.status", equals="missing"),
            a("unserved", "stdout", equals="404"),
            scenario="The teardown recipe's workload node deletes the namespace, and with it the Ingress"),
    ]
    cleanup = cluster_cleanup(f"rm -rf {SITE_CTX} {HOME}/e2e /tmp/e2e-site.html; loginctl disable-linger opute; "
                              + PODMAN_CLEANUP)
    allow = (CLUSTER_EFFECTS + ["files//home/opute/.opute*", "files//home/opute/e2e*"] + PODMAN_EFFECTS
             + SUPERVISOR_EFFECTS)
    return journey(
        "e2e3.site", "e2e-3",
        "Site deploy through the opute-site-deploy recipe on a single-node K3s cluster: the site image built "
        "locally from the published source and pushed to an e2e registry, the recipe's own nodes run with the "
        "image pinned by digest, the page served at a local test hostname (no tunnel, no public host), a rerun, "
        "and removal through the teardown recipe.",
        reqs, steps, cleanup, allow=allow, persist=["files//var/lib/dpkg*", "files//var/log*", "files//var/cache*"],
        minutes=28, inputs={**CLUSTER_INPUTS, "source": f"{SITE_SRC} (sha256 {SITE_SRC_SHA})", "files": files})


# --- e2e-3 / 3. local model serving -----------------------------------------------------------------------

SERVE_GOAL = (".agents/goals/e2e-3-k3s-deploy-serving.md journey 4 (a serving assignment, then a request "
              "through the gateway)")
FIXTURE = f"{HOME}/e2e/model/serve.py"


def assignment(generation: int, key: str, state: str = "start") -> dict:
    return {"contractVersion": "serving-assignment.v1", "assignmentId": "e2e/model", "generation": generation,
            "idempotencyKey": key, "service": "model", "mode": "dev-process", "runtime": "process",
            "target": {"hostId": "e2e-devvm-agent", "resourceId": "model", "kind": "host", "instanceType": "host"},
            "artifact": {"kind": "source", "sourceDir": f"{HOME}/e2e/model",
                         "command": ["python3", FIXTURE, "18091"], "environment": {"PYTHONDONTWRITEBYTECODE": "1"}},
            "endpoints": [{"name": "http", "protocol": "http", "port": 18091}],
            "readiness": [{"endpointName": "http", "path": "/v1/models"}], "exposure": {"kind": "none"},
            "restartPolicy": "on-failure", "desiredState": state}


CHAT = ("python3 - <<'PY'\nimport json, urllib.request, urllib.error\n"
        "req = urllib.request.Request('http://127.0.0.1:18434/v1/chat/completions', method='POST', "
        "data=json.dumps({'model': 'e2e-fixture', 'messages': [{'role': 'user', 'content': 'hi'}]}).encode())\n"
        "req.add_header('Content-Type', 'application/json')\n{auth}"
        "try:\n    r = urllib.request.urlopen(req, timeout=10); body = json.loads(r.read())\n"
        "    print(json.dumps({'status': r.status, 'content': body['choices'][0]['message']['content']}))\n"
        "except urllib.error.HTTPError as e:\n    print(json.dumps({'status': e.code}))\nPY")


def serving() -> dict:
    files = {**base.TOOL_FILES, "fixture": inp("${REPO}/evidence/e2e/journeys/inputs/e2e-openai-fixture.py")}
    relay = {"sessionId": "e2e-gateway", "listenHost": "127.0.0.1", "listenPort": 18434, "targetHost": "127.0.0.1",
             "targetPort": 18091, "incomingToken": "${SECRET:gatewayToken}", "upstreamToken": "",
             "allowedSourceCIDRs": ["127.0.0.0/8"]}
    steps = base.TOOL_STEPS + [
        {"id": "fixtureCopy", "copy": "fixture", "to": FIXTURE, "mode": "0755"},
        {"id": "own", "guest": f"chown -R opute:opute {HOME}/e2e && echo ok"},
        call("supervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        call("assign", "reconcile_serving_assignment", assignment(1, "e2e-model-1")),
        call("assignAgain", "reconcile_serving_assignment", assignment(1, "e2e-model-1")),
        # the process's own record: its supervisor log and the user manager's journal for it
        {"id": "assignLog", "guest": ("sleep 3; tail -n 20 /tmp/serving-assignment-*-supervisor.log 2>/dev/null; "
                                      "journalctl _UID=$(id -u opute) --no-pager -n 40 -o cat; "
                                      "ss -ltnH 'sport = :18091' | wc -l")},
        # D28 (Q27): the assignment's own process is the runtime the gateway reaches
        {"id": "modelUp", "guest": "for i in $(seq 50); do ss -ltnH 'sport = :18091' | grep -q . && break; sleep 0.2; "
                                   "done; ss -ltnH 'sport = :18091' | wc -l"},
        call("gateway", "ensure_local_llm_relay", relay),
        {"id": "chat", "guest": CHAT.replace("{auth}", "req.add_header('Authorization', 'Bearer ${SECRET:gatewayToken}')\n")},
        {"id": "chatNoToken", "guest": CHAT.replace("{auth}", "")},
        call("probe", "probe_openai_compatible_server",
             {"endpoint": "http://127.0.0.1:18434", "bearerToken": "${SECRET:gatewayToken}", "includeChat": True}),
        {"id": "tokenRows", "guest": f"{T} contains {SDB} '${{SECRET:gatewayToken}}'"},
        call("removeGateway", "remove_local_llm_relay", {"sessionId": "e2e-gateway"}),
        {"id": "gatewayGone", "guest": "curl -s -o /dev/null -w '%{http_code}' --max-time 3 "
                                       "http://127.0.0.1:18434/v1/models || true"},
        call("units", "list_host_services", {}),
    ]
    reqs = [
        req(SERVE_GOAL, "A prompt returns a completion through the gateway",
            direct("supervisorAgain", "structuredContent.status", equals="ready"),
            a("modelUp", "stdout", equals="1\n"),
            direct("gateway", "isError", **{"in": [False, "<absent>"]}),
            a("chat", "json.status", equals=200),
            a("chat", "json.content", equals="e2e fixture completion"),
            a("chatNoToken", "json.status", equals=401),
            direct("probe", "structuredContent.ready", equals=True),
            scenario="The serving assignment's OpenAI-compatible runtime runs on the host, the gateway admits a "
                     "request with its token and refuses one without, and a chat completion comes back. The "
                     "runtime is a test fixture: a real model runtime and its weights need downloads beyond test "
                     "fixtures and are recorded blocked"),
        req(Q27_SPEC, "A serving launch is reported only when it started",
            direct("assign", "isError", **{"in": [False, "<absent>"]}),
            direct("assign", "structuredContent.started", equals=True),
            direct("assignAgain", "isError", **{"in": [False, "<absent>"]}),
            a("assignLog", "stdout", matches="\n1\n$"),
            scenario="On a system-unit agent, reconcile_serving_assignment launches the process under the user "
                     "service manager with the user bus, reports it started, and it listens (D28)"),
        req(SPEC, "Durable and redacted observations",
            a("tokenRows", "json.rows", equals=0),
            scenario="Secret-bearing input: the gateway's incoming token is in no durable row"),
        req(SERVE_GOAL, "Removing the gateway and runtime leaves nothing behind",
            direct("removeGateway", "isError", **{"in": [False, "<absent>"]}),
            a("gatewayGone", "stdout", equals="000"),
            scenario="The gateway is gone after removal; the runtime's transient unit is stopped in cleanup"),
    ]
    cleanup = [{"id": "cleanServing", "guest": (
        "uid=$(id -u opute); for u in $(runuser -u opute -- env XDG_RUNTIME_DIR=/run/user/$uid systemctl --user "
        "list-units --plain --no-legend 'opute-*' 2>/dev/null | awk '{print $1}'); do runuser -u opute -- env "
        "XDG_RUNTIME_DIR=/run/user/$uid systemctl --user stop $u; done; "
        f"rm -rf {HOME}/e2e /tmp/serving-assignment-* {HOME}/.config/systemd/user/e2e-model.service; "
        f"rmdir {HOME}/.config/systemd/user {HOME}/.config/systemd 2>/dev/null; loginctl disable-linger opute; true")}]
    allow = ["files//home/opute/e2e*", "files//home/opute/.opute*", "files//home/opute/.config/systemd*",
             "files//tmp/serving-assignment*", "user/*", "linger*", "files//var/lib/systemd/linger*",
             "system/units/user*", "system/units/run-user-*", "system/units/opute-*"]
    return journey(
        "e2e3.serving", "e2e-3",
        "Local model serving through the Host Agent's serving path: a serving assignment runs an OpenAI-compatible "
        "runtime (a test fixture), the gateway (ensure_local_llm_relay) admits a token-bearing request and "
        "returns a completion, the probe reports it ready, and removal leaves nothing behind. A real runtime and "
        "weights are blocked: they need downloads beyond test fixtures.",
        reqs, steps, cleanup, allow=allow, persist=["files//home/opute/.opute*"], minutes=15,
        inputs={"agent": base.AGENT, "runtime": "evidence/e2e/journeys/inputs/e2e-openai-fixture.py (a fixture, "
                                                 "not a model)", "files": files})


# --- e2e-4 / durability and cutover -----------------------------------------------------------------------

CUTOVER = ("openspec/changes/reimplement-host-agent-in-rust/milestones.md M12 (forward and rollback: Go rolled "
           "back onto Rust's state reports every task, Rust-era work included, with the status Rust last "
           "reported) and D1 (bit-compatible schema: conversion is the identity)")
UNIT = base.UNIT
SDB_ = base.SDB
SYNC_CALL = ("nohup python3 -c 'import sys, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
             "r = mcpwire.mcp(3014, sys.argv[1], \"tools/call\", {\"name\": \"probe_http_endpoint\", \"arguments\": "
             "{\"endpoint\": \"http://127.0.0.1:8799/\"}}, timeout=120)\n"
             "open(\"/tmp/e2e-sync.json\", \"w\").write(json.dumps({\"status\": r.get(\"status\"), \"result\": "
             "((r.get(\"body\") or {}).get(\"result\") or {}).get(\"structuredContent\")}))' '${TOKEN}' "
             ">/dev/null 2>&1 & sleep 2; echo open")


def work_plan(plan_id: str, key: str, name: str) -> dict:
    return cross_plan(plan_id, key, name)


def fail_plan() -> dict:
    return {"contractVersion": "host-plan.v1", "planId": "e2e.fail", "generation": 1, "idempotencyKey": "e2e-fail-1",
            "nodes": [{"id": "denied", "action": {"tool": "ensure_host_file",
                                                  "args": {"path": "~/e2e/fail.txt", "content": "x\n",
                                                           "scope": "user"}},
                       # a readiness check that holds neither before nor after the write: the run fails at
                       # validation
                       "validate": {"tool": "inspect_host_file", "args": {"path": "~/e2e/fail.txt", "scope": "user"},
                                    "assert": [{"op": "eq", "path": "/exists", "value": "never"}]}}]}



def second_writer(id_: str, gz: str) -> dict:
    """D25: another release started on the serving agent's instance root, as the unit runs it; it must
    refuse with the decided line and exit 1 while the first keeps serving."""
    return {"id": id_, "timeout": 60, "guest": (
        f"args=$(systemctl show -p ExecStart --value {UNIT} | sed -n 's/.*argv\\[\\]=\\([^;]*\\) ;.*/\\1/p' "
        f"| cut -d' ' -f2-) && gzip -dc {gz} > /tmp/e2e-second && chmod 0755 /tmp/e2e-second && "
        "systemd-run --quiet --wait --pipe --collect --uid=opute --gid=opute "
        "-p EnvironmentFile=/etc/opute/host-agent.env --setenv=HOME=/home/opute --setenv=HOST_MCP_PORT=3099 "
        "timeout 30 /tmp/e2e-second $args 2>&1; echo exit=$?; rm -f /tmp/e2e-second")}

def cutover() -> dict:
    files = {**base.TOOL_FILES}
    steps = base.TOOL_STEPS + [
        {"id": "aside", "guest": f"systemctl stop {UNIT} && mv /home/opute/.config/opute/instances "
                                 "/tmp/e2e-cutover-rust-instances && echo aside"},
        base.swap("goInstall", f"{base.GO_REL}/host-agent-linux-x64.gz"),
        {"id": "goHealth", "health": True},
        {"id": "goList", "mcp": "tools/list"},
        {"id": "slow", "guest": f"{T} slow start --port 8799 --delay 25"},
        call("goSupervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("goSupervisorAgain", "ensure_host_service_supervisor", {}),
        # Go's real history: a completed cross-domain plan, a failed plan, a completed task
        call("goPlan", "run_host_plan", {"plan": work_plan("e2e.go", "e2e-go-1", "g1")},
             capture={"GO_RUN": "response.body.result.structuredContent.runId"}),
        base.poll_run("goPlanDone", "GO_RUN", timeout=600),
        call("goFail", "run_host_plan", {"plan": fail_plan()},
             capture={"GO_FAIL": "response.body.result.structuredContent.runId"}),
        base.poll_run("goFailDone", "GO_FAIL", timeout=120),
        *task("goTask", "run_host_command", {"command": "echo go-era"}),
        # work in flight when Go stops: a working task and an open synchronous call. The coordinator never
        # admits normal work beside heavy work, and a running plan is heavy, so a plan cannot be in flight with
        # them; e2e1.migrate-upgrade stops Go with a plan in flight, and e2e4.restart stops Rust with one.
        {"id": "settle", "guest": "sleep 5"},
        call("goLong", "run_host_command", {"command": "sleep 120"}, meta=TASKS_META,
             capture={"LONG": "response.body.result.taskId"}),
        {"id": "goSync", "guest": SYNC_CALL},
        {"id": "goStop", "guest": f"systemctl stop {UNIT} && {T} db {SDB_} && sha256sum {SDB_} | cut -c1-64 > "
                                  "/tmp/e2e-go-state.sha"},
        {"id": "syncAnswer", "guest": "cat /tmp/e2e-sync.json"},
        # forward: the Rust release on the same instance root and identity
        base.swap("rustInstall", f"{base.REL}/host-agent-linux-x64.gz"),
        {"id": "rustHealth", "health": True},
        second_writer("goSecond", f"{base.GO_REL}/host-agent-linux-x64.gz"),
        {"id": "rustStillServes", "health": True},
        {"id": "rustList", "mcp": "tools/list"},
        {"id": "rustDb", "guest": f"{T} db {SDB_}"},
        call("rustOps", "list_operations", {"limit": 100}),
        call("rustLong", "get_operation", {"operationId": "${LONG}"}),
        {"id": "rustLongWire", "mcp": "tasks/get", "params": {"taskId": "${LONG}"}, "meta": TASKS_META},
        call("rustTask", "get_operation", {"operationId": "${TID_goTask}"}),
        {"id": "rustTaskWire", "mcp": "tasks/get", "params": {"taskId": "${TID_goTask}"}, "meta": TASKS_META},
        call("rustPlan", "get_host_plan_run", {"runId": "${GO_RUN}"}),
        call("rustFailed", "get_host_plan_run", {"runId": "${GO_FAIL}"}),
        call("rustGuest", "get_vm_info", {"uri": "container:local:e2e-g1"}),
        call("rustSqlite", "get_sqlite_database_status", {"consumerId": "e2e-g1", "databaseName": "main"}),
        # Rust-era work
        call("rustNew", "run_host_plan", {"plan": {"contractVersion": "host-plan.v1", "planId": "e2e.rust",
                                                   "generation": 1, "idempotencyKey": "e2e-rust-1",
                                                   "nodes": cross_plan("x", "x", "n1")["nodes"][:1]}},
             capture={"RUST_RUN": "response.body.result.structuredContent.runId"}),
        base.poll_run("rustNewDone", "RUST_RUN", timeout=120),
        *task("rustTaskNew", "run_host_command", {"command": "echo rust-era"}),
        {"id": "rustStop", "guest": f"systemctl stop {UNIT} && {T} db {SDB_}"},
        # rollback: Go on Rust's state
        base.swap("goBack", f"{base.GO_REL}/host-agent-linux-x64.gz"),
        {"id": "goBackHealth", "health": True},
        second_writer("rustSecond", f"{base.REL}/host-agent-linux-x64.gz"),
        {"id": "goStillServes", "health": True},
        call("goBackOps", "list_operations", {"limit": 100}),
        call("goBackRustTask", "get_operation", {"operationId": "${TID_rustTaskNew}"}),
        call("goBackLong", "get_operation", {"operationId": "${LONG}"}),
        {"id": "goBackRustTaskWire", "mcp": "tasks/get", "params": {"taskId": "${TID_rustTaskNew}"},
         "meta": TASKS_META},
        {"id": "goBackLongWire", "mcp": "tasks/get", "params": {"taskId": "${LONG}"}, "meta": TASKS_META},
        call("goBackRustPlan", "get_host_plan_run", {"runId": "${RUST_RUN}"}),
        call("goBackGoPlan", "get_host_plan_run", {"runId": "${GO_RUN}"}),
        {"id": "goBackDb", "guest": f"{T} db {SDB_}"},
    ]
    lock = "host agent instance is already in use: /home/opute/.config/opute/instances/standalone/host-agent.lock " \
           "is locked by another process\n"
    reqs = [
        req(base.LOCK_SPEC, "One running agent per instance state",
            a("goSecond", "stdout", matches="exit=1\n$"), a("goSecond", "stdout", contains=lock),
            a("rustStillServes", "response.body.agentId", equals="e2e-devvm-agent"),
            a("rustSecond", "stdout", matches="exit=1\n$"), a("rustSecond", "stdout", contains=lock),
            a("goStillServes", "response.body.agentId", equals="e2e-devvm-agent"),
            scenario="Go and Rust never write one instance root together: Go started beside the serving Rust "
                     "agent, and Rust beside the serving Go agent after rollback, each refuses with the decided "
                     "line and exit 1 while the first keeps serving"),
        req(CUTOVER, "Durable and redacted observations: Go's real state is served by Rust unchanged",
            ok("aside"), a("goInstall", "stdout", matches="\n0\\.2\\.4\n$"),
            direct("goPlanDone", "structuredContent.status", equals="completed"),
            direct("goFailDone", "structuredContent.status", equals="failed"),
            direct("goSupervisorAgain", "structuredContent.status", equals="ready"),
            res("goTask", "structuredContent.stdout", equals="go-era\n"),
            ok("goStop"), a("goStop", "json.integrity", equals="ok"),
            a("rustInstall", "stdout", matches="\n0\\.2\\.4\n$"),
            a("rustDb", "json.integrity", equals="ok"),
            a("rustDb", "json.counts", equalsStep={"step": "goStop", "path": "json.counts"}),
            a("rustDb", "json.operationIds", equalsStep={"step": "goStop", "path": "json.operationIds"}),
            a("rustDb", "json.planRunIds", equalsStep={"step": "goStop", "path": "json.planRunIds"}),
            direct("rustOps", "isError", **{"in": [False, "<absent>"]}),
            {"step": "rustTaskWire", "path": "response.body.result.status", "equals": "completed"},
            direct("rustPlan", "structuredContent.status", equals="completed"),
            direct("rustFailed", "structuredContent.status", equals="failed"),
            direct("rustGuest", "structuredContent.status", equals="running"),
            direct("rustSqlite", "structuredContent.exists", equals=True),
            scenario="A Go v0.2.4 install with real history (a completed cross-domain plan with its guest and "
                     "database, a failed plan, a completed task) is stopped and served by the Rust release on the "
                     "same instance root: counts and ids reconcile per table, every record reads back with its "
                     "status, and the guest and database keep working"),
        req(SPEC, "One exact Host Agent identity",
            a("goHealth", "response.body.agentId", equals="e2e-devvm-agent"),
            a("rustHealth", "response.body.agentId", equals="e2e-devvm-agent"),
            a("goBackHealth", "response.body.agentId", equals="e2e-devvm-agent")),
        req(CUTOVER, "Forward cutover with work in flight",
            a("syncAnswer", "json.status", equals=200),
            a("syncAnswer", "json.result.ready", exists=True),
            {"step": "goLong", "path": "response.body.result.status", "equals": "working"},
            scenario="Go stopped with a working task and an open synchronous call: the call is answered before "
                     "Go exits"),
        req(Q25_SPEC, "An operation record reports its task's terminal status",
            direct("rustLong", "structuredContent.status", equals="cancelled"),
            scenario="The row Go left working beside its cancelled snapshot reads back cancelled in Rust (D28)"),
        req(CUTOVER, "Forward cutover: the stopped task's terminal state",
            {"step": "rustLongWire", "path": "response.body.result.status", "equals": "cancelled"},
            {"step": "rustLongWire", "path": "response.body.result.statusMessage",
             "equals": "The Host Agent shut down before the task completed."},
            scenario="The task Go was running at shutdown is restored by Rust from Go's snapshot as cancelled, "
                     "with Go's shutdown message (M12 in-flight drill)"),
        req(CUTOVER, "Rollback: Go reports every task, Rust-era work included, with the status Rust last reported",
            direct("rustNewDone", "structuredContent.status", equals="completed"),
            res("rustTaskNew", "structuredContent.stdout", equals="rust-era\n"),
            a("rustStop", "json.integrity", equals="ok"),
            a("goBack", "stdout", matches="\n0\\.2\\.4\n$"),
            {"step": "goBackRustTaskWire", "path": "response.body.result.status", "equals": "completed"},
            {"step": "goBackLongWire", "path": "response.body.result.status", "equals": "cancelled"},
            direct("goBackRustTask", "structuredContent.operationId", exists=True),
            direct("goBackRustPlan", "structuredContent.status", equals="completed"),
            direct("goBackGoPlan", "structuredContent.status", equals="completed"),
            a("goBackDb", "json.operationIds", equalsStep={"step": "rustStop", "path": "json.operationIds"}),
            a("goBackDb", "json.planRunIds", equalsStep={"step": "rustStop", "path": "json.planRunIds"}),
            scenario="After Rust did new work, Go v0.2.4 started on Rust's state reads every operation and plan "
                     "run, Rust-era work included, with the status Rust last reported"),
    ]
    cleanup = [{"id": "restoreRust", "guest": (
        f"systemctl stop {UNIT}; {T} slow stop >/dev/null; {base.install_binary(f'{base.REL}/host-agent-linux-x64.gz')} "
        ">/dev/null && rm -rf /home/opute/.config/opute/instances && mv /tmp/e2e-cutover-rust-instances "
        f"/home/opute/.config/opute/instances && systemctl start {UNIT} && {base.WAIT_HEALTH.format(port=3014)}; "
        "loginctl disable-linger opute; for n in e2e-g1 e2e-f1 e2e-n1; do incus delete -f $n 2>/dev/null; done; "
        f"rm -rf {HOME}/e2e {HOME}/.opute/databases /tmp/e2e-sync.json /tmp/e2e-go-state.sha; "
        "incus storage volume list default -f csv | awk -F, '$1==\"image\"{print $2}' | "
        "xargs -r -I{} incus storage volume delete default image/{}; true")}]
    allow = ["files//home/opute/.local/share/opute/opute-host-agent/sha256", "incus/*", "files//home/opute/e2e*",
             "files//home/opute/.opute*", "system/units/e2e-slow*", "system/unitFiles/e2e-slow*",
             "system/units/opute-host-workload-*", "system/units/sys-devices-virtual-net-veth*",
             "system/units/sys-subsystem-net-devices-veth*"] + ["linger", "files//var/lib/systemd/linger*", "system/units/user*", "system/units/run-user-*", "user/*"]
    return journey(
        "e2e4.cutover", "e2e-4",
        "Forward cutover and rollback on a Go host's own accumulated state: the host is reinstalled as Go v0.2.4 "
        "(same unit, instance root and agent ID), does real mixed work and is stopped with an open synchronous "
        "call, a working task and a running plan in flight; the Rust release starts on that state, which must "
        "reconcile per table and read back; Rust does new work; then Go starts again on Rust's state and must "
        "report every record with the status Rust last reported.",
        reqs, steps, cleanup, allow=allow, persist=["files//home/opute/.opute*"], minutes=28,
        inputs={"agent": base.AGENT, "go": f"{base.GO_REL} (published Go v0.2.4 release, SHA256SUMS pinned)",
                "slow": "a local endpoint answering after 25 s (e2etool slow)", "files": files})


NEXT = "/opt/opute-e2e/releases/rust-next/host-agent-linux-x64.gz"


def restart() -> dict:
    files = {**base.TOOL_FILES, "rustNext": inp("${PARITY}/e2e/release/rust-next-e2e13a/host-agent-linux-x64.gz")}
    kill = f"systemctl kill -s KILL {UNIT} && sleep 1 && systemctl start {UNIT} && {base.WAIT_HEALTH.format(port=3014)}"
    steps = base.TOOL_STEPS + [
        {"id": "nextCopy", "copy": "rustNext", "to": NEXT},
        {"id": "slow", "guest": f"{T} slow start --port 8799 --delay 25"},
        # host commands run under the user service manager
        call("supervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        # SIGTERM with a plan in flight, then SIGTERM with a task in flight (the coordinator never admits normal
        # work beside a running plan, which is heavy)
        call("termPlan", "run_host_plan", {"plan": base.plan("e2e.term", "e2e-term-1", "e2e/term.txt", slow=True)},
             capture={"TERM_RUN": "response.body.result.structuredContent.runId"}),
        base.poll_run("termWaiting", "TERM_RUN", done=("running", "working"), timeout=60),
        {"id": "termP", "guest": f"systemctl restart {UNIT} && {base.WAIT_HEALTH.format(port=3014)} && echo up"},
        {"id": "settleP", "guest": "sleep 5"},
        call("termTask", "run_host_command", {"command": "sleep 120"}, meta=TASKS_META,
             capture={"TERM_TASK": "response.body.result.taskId"}),
        {"id": "term", "guest": f"systemctl restart {UNIT} && {base.WAIT_HEALTH.format(port=3014)} && echo up"},
        call("termTaskAfter", "get_operation", {"operationId": "${TERM_TASK}"}),
        {"id": "termTaskWire", "mcp": "tasks/get", "params": {"taskId": "${TERM_TASK}"}, "meta": TASKS_META},
        call("termPlanAfter", "get_host_plan_run", {"runId": "${TERM_RUN}"}),
        # SIGKILL with a task in flight
        # distinct commands: identical run_host_command tasks share one workload unit (Q29)
        call("killTask", "run_host_command", {"command": "sleep 121"}, meta=TASKS_META,
             capture={"KILL_TASK": "response.body.result.taskId"}),
        {"id": "kill", "guest": kill + " && echo up"},
        call("killTaskAfter", "get_operation", {"operationId": "${KILL_TASK}"}),
        {"id": "killTaskWire", "mcp": "tasks/get", "params": {"taskId": "${KILL_TASK}"}, "meta": TASKS_META},
        call("opsAfterKill", "list_operations", {"limit": 100}),
        call("cancelAfterKill", "cancel_operation", {"operationId": "${KILL_TASK}"}),
        # an in-place upgrade with a task in flight
        call("upTask", "run_host_command", {"command": "sleep 122"}, meta=TASKS_META,
             capture={"UP_TASK": "response.body.result.taskId"}),
        base.swap("upgrade", NEXT),
        call("upTaskAfter", "get_operation", {"operationId": "${UP_TASK}"}),
        {"id": "upTaskWire", "mcp": "tasks/get", "params": {"taskId": "${UP_TASK}"}, "meta": TASKS_META},
        *task("afterUpgrade", "run_host_command", {"command": "echo upgraded"}),
        {"id": "db", "guest": f"{T} db {SDB_}"},
    ]
    reqs = [
        req(SPEC, "Durable and redacted observations (restart)",
            direct("supervisorAgain", "structuredContent.status", equals="ready"),
            {"step": "termTask", "path": "response.body.result.status", "equals": "working"},
            a("termP", "stdout", equals="up\n"), a("term", "stdout", equals="up\n"),
            a("termTaskWire", "response.body.result.status", equals="cancelled"),
            a("termTaskWire", "response.body.result.statusMessage",
              equals="The Host Agent shut down before the task completed."),
            a("kill", "stdout", equals="up\n"),
            direct("opsAfterKill", "isError", **{"in": [False, "<absent>"]}),
            a("upgrade", "stdout", matches="\n0\\.2\\.5-e2e\n$"),
            res("afterUpgrade", "structuredContent.stdout", equals="upgraded\n"),
            a("db", "json.integrity", equals="ok"),
            scenario="A task in flight at SIGTERM is persisted cancelled and reads back so, through get_operation "
                     "and tasks/get; the agent restarts after SIGKILL and an in-place upgrade and keeps serving"),
        req(Q5_SPEC, "An interrupted plan or task reads back truthfully",
            direct("termPlanAfter", "structuredContent.status", equals="cancelled"),
            {"step": "killTaskWire", "path": "response.body.result.status", "equals": "failed"},
            {"step": "upTaskWire", "path": "response.body.result.status", "equals": "cancelled"},
            direct("cancelAfterKill", "isError", equals=True),
            scenario="A plan running at SIGTERM reads back cancelled; a task killed by SIGKILL reads back failed "
                     "and cannot be cancelled; a task in flight at the in-place upgrade's graceful stop reads back "
                     "cancelled (D28)"),
        req(Q25_SPEC, "An operation record reports its task's terminal status",
            direct("termTaskAfter", "structuredContent.status", equals="cancelled"),
            direct("killTaskAfter", "structuredContent.status", equals="failed"),
            direct("upTaskAfter", "structuredContent.status", equals="cancelled"),
            direct("opsAfterKill", "structuredContent.operations", exists=True),
            scenario="get_operation reads each task's terminal status, as tasks/get does (D28)"),
    ]
    cleanup = [{"id": "restoreRust", "guest": (
        f"systemctl stop {UNIT}; {T} slow stop >/dev/null; {base.install_binary(f'{base.REL}/host-agent-linux-x64.gz')} "
        f">/dev/null && systemctl start {UNIT} && {base.WAIT_HEALTH.format(port=3014)}; loginctl disable-linger opute; "
        "incus delete -f e2e-t1 "
        f"2>/dev/null; rm -rf {HOME}/e2e {HOME}/.opute/databases /opt/opute-e2e/releases/rust-next; "
        "incus storage volume list default -f csv | awk -F, '$1==\"image\"{print $2}' | "
        "xargs -r -I{} incus storage volume delete default image/{}; true")}]
    allow = ["files//home/opute/.local/share/opute/opute-host-agent/sha256", "incus/*", "files//home/opute/e2e*",
             "files//home/opute/.opute*", "files//opt/opute-e2e/releases/rust-next*", "system/units/e2e-slow*",
             "system/unitFiles/e2e-slow*", "system/units/opute-host-workload-*",
             "system/units/sys-devices-virtual-net-veth*", "system/units/sys-subsystem-net-devices-veth*"] + ["linger", "files//var/lib/systemd/linger*", "system/units/user*", "system/units/run-user-*", "user/*"]
    return journey(
        "e2e4.restart", "e2e-4",
        "Kill, restart and upgrade mid-work on the Rust system unit: SIGTERM with a task and a plan in flight, "
        "SIGKILL with a task in flight, then an in-place upgrade to a newer local Rust build with a task in "
        "flight; each item's durable state is read back through get_operation, tasks/get and get_host_plan_run.",
        reqs, steps, cleanup, allow=allow, persist=["files//home/opute/.opute*"], minutes=20,
        inputs={"agent": base.AGENT, "next": "Rust release set built locally from this branch with "
                                             "VERSION=0.2.5-e2e (no tag, no publish)",
                "slow": "a local endpoint answering after 25 s (e2etool slow)", "files": files})


JOURNEYS = [host_ops, incus, plans, oci, data, discovery, site, serving, cutover, restart]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
