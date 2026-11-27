#!/usr/bin/env python3
"""Write the provider teardown journey (m30, reclaim-provider-host-service) as JSON.

  python3 evidence/e2e/journeys/make_teardown.py

No other journey tears a provider down: they run the k3s provider as a transient
root unit and leave it to cleanup. Here the provider runs the way an installed
provider does, as a user unit of the agent's user, written and started through
typed calls. It is installed and activated, then torn down with its serviceName.
The journey checks that the teardown completes, the unit file is gone, the unit
is inactive, its port is closed and its tools leave tools/list.
"""
from __future__ import annotations

import json

import make_e2e1 as base
import make_e2e3a as ha
import make_e2e4a as e4
from make_e2e1 import HERE, a, ok, req
from make_e2e3a import call, direct, res, task
from make_e2e4a import OPUTE_UID, USER_SESSION, USER_SYSTEMCTL, linger, svc

SPEC = "openspec/changes/reclaim-provider-host-service/specs/provider-teardown/spec.md"
UNIT = "opute-provider-k3s.service"
UNIT_PATH = f"~/.config/systemd/user/{UNIT}"
UNIT_FILE = f"{e4.HOME}/.config/systemd/user/{UNIT}"
UNIT_TEXT = ("[Unit]\nDescription=Opute k3s provider (e2e)\n\n[Service]\nEnvironment=OPUTE_PROVIDER_PORT=4320\n"
             f"ExecStart={ha.PROVIDER}\n\n[Install]\nWantedBy=default.target\n")
PROVIDER = "com.opute.k3s"
PORT_OPEN = ("for i in $(seq 60); do ss -ltnH 'sport = :4320' | grep -q . && break; sleep 0.5; done; "
             "ss -ltnH 'sport = :4320' | wc -l")
FILES = {**base.TOOL_FILES, "providerK3s": ha.FILES["providerK3s"], "activate": ha.FILES["activate"]}


def teardown() -> dict:
    steps = base.TOOL_STEPS + [
        {"id": "providerCopy", "copy": "providerK3s", "to": ha.PROVIDER, "mode": "0755"},
        {"id": "recipeCopy", "copy": "activate", "to": ha.RECIPE, "mode": "0644"},
        {"id": "recipeOwner", "guest": "chown -R opute /home/opute/.config/opute-e2e && echo owned"},
        {"id": "catalog", "mcp": "tools/list"},
        # the provider as a user unit of the agent's user, through typed calls
        call("supervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        linger(),
        call("unit", "ensure_host_file", {"path": UNIT_PATH, "content": UNIT_TEXT}),
        svc("enable", "enable", UNIT),
        svc("start", "start", UNIT),
        {"id": "unitEnabled", "guest": f"{USER_SYSTEMCTL} is-enabled {UNIT}; test -f {UNIT_FILE} && echo file",
         "confirms": ["unit", "enable"]},
        {"id": "listening", "guest": PORT_OPEN, "confirms": ["start"]},
        call("installA", "opute.provider.install",
             {"descriptor": ha.descriptor(4320), "activate": True, "recipeSource": ha.RECIPE}),
        {"id": "providerToolsA", "guest": ha.wait_tool(3014, "${TOKEN}"), "confirms": ["installA"]},
        {"id": "catalogAfterA", "mcp": "tools/list"},
        call("statusBefore", "opute.provider.status", {"provider": PROVIDER}),
        # the teardown under test
        *task("teardown", "opute.provider.teardown",
              {"provider": PROVIDER, "confirm": True, "inputs": {"serviceName": UNIT}}),
        {"id": "unitGone", "guest": f"test ! -e {UNIT_FILE} && echo gone", "confirms": ["teardown"]},
        {"id": "unitState", "guest": f"{USER_SYSTEMCTL} show -p ActiveState -p UnitFileState {UNIT} | sort",
         "confirms": ["teardown"]},
        {"id": "portClosed", "guest": "ss -ltnH 'sport = :4320' | wc -l", "confirms": ["teardown"]},
        {"id": "catalogAfter", "mcp": "tools/list"},
        {"id": "toolsAfter", "guest": (
            "python3 -c 'import sys, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
            "r = mcpwire.mcp(3014, sys.argv[1], \"tools/list\", request_id=1)\n"
            "names = [t[\"name\"] for t in r[\"body\"][\"result\"][\"tools\"]]\n"
            f"print(json.dumps({{\"provision\": \"{ha.K}provision\" in names}}))' '${{TOKEN}}'"),
         "confirms": ["teardown"]},
        call("statusAfter", "opute.provider.status", {"provider": PROVIDER}),
    ]
    reqs = [
        req(SPEC, "Provider teardown reclaims the provider's host service",
            direct("unit", "structuredContent.changed", equals=True),
            direct("enable", "structuredContent.status", equals="applied"),
            direct("start", "structuredContent.status", equals="applied"),
            a("unitEnabled", "stdout", equals="enabled\nfile\n"), a("listening", "stdout", equals="1\n"),
            direct("installA", "isError", **{"in": [False, "<absent>"]}),
            a("providerToolsA", "json.listed", equals=True),
            direct("statusBefore", "isError", **{"in": [False, "<absent>"]}),
            a("teardown", "response.body.result.status", equals="completed"),
            res("teardown", "isError", **{"in": [False, "<absent>"]}),
            a("unitGone", "stdout", equals="gone\n"),
            a("unitState", "stdout", equals="ActiveState=inactive\nUnitFileState=\n"),
            a("portClosed", "stdout", equals="0\n"),
            a("toolsAfter", "json.provision", equals=False),
            direct("statusAfter", "isError", equals=True),
            direct("statusAfter", "content[0].text", matches="no active generation|not installed|not active"),
            scenario="A provider running as a user unit is torn down"),
        req(e4.CATALOG, "ensure_host_service_supervisor settles the user manager",
            direct("supervisor", "isError", **{"in": [False, "<absent>"]}),
            direct("supervisorAgain", "structuredContent.status", equals="ready"),
            a("linger", "stdout", equals="yes\nbus\n"), ok("recipeOwner"), a("userBus", "stdout", equals="up\n"),
            scenario="The user manager the provider's unit runs under"),
    ]
    cleanup = [{"id": "cleanTeardown", "guest": (
        f"uid=$(id -u opute); {USER_SYSTEMCTL} disable --now {UNIT} 2>/dev/null; rm -f {UNIT_FILE}; "
        f"{USER_SYSTEMCTL} daemon-reload; loginctl disable-linger opute; "
        "rmdir /home/opute/.config/systemd/user /home/opute/.config/systemd 2>/dev/null; "
        "rm -rf /home/opute/.config/opute-e2e /opt/opute-e2e/providers; true")}]
    proc = f"processes//user.slice/user-{OPUTE_UID}.slice/user@{OPUTE_UID}.service/app.slice/{UNIT} opute-provider-"
    allow = (["files//home/opute/.config/systemd", "files//home/opute/.config/systemd/user",
              f"files/{UNIT_FILE}", "files//home/opute/.config/opute-e2e",
              "files//home/opute/.config/opute-e2e/k3s-activate.yaml", "files//opt/opute-e2e/providers",
              "files//opt/opute-e2e/providers/opute-provider-k3s", f"opute/units/{UNIT}", proc,
              "sockets/tcp 127.0.0.1:4320 opute-provider-"] + USER_SESSION)
    return e4.journey(
        "e2e2.provider-teardown", "e2e-teardown",
        "Provider teardown (m30): the k3s provider runs as a user unit of the agent's user, written, enabled "
        "and started through typed calls; it is installed and activated, then torn down with its serviceName. "
        "Pass: the teardown completes, the unit file is gone, the unit is inactive, its port is closed and the "
        "provider's tools leave tools/list.",
        reqs, steps, cleanup, allow=allow, minutes=10,
        inputs={"agent": base.AGENT + ", with the Rust k3s provider 1.0.2 from the release as user unit "
                                      f"{UNIT} (port 4320)", "files": FILES})


JOURNEYS = [teardown]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
