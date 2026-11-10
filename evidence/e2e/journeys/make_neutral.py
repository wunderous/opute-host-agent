#!/usr/bin/env python3
"""Write the platform-neutral (0.4.0) journeys as JSON.

  python3 evidence/e2e/journeys/make_neutral.py

The dev VM keeps its 0.3.x install. Each journey copies the 0.4.0 release
binary in and starts its own agents from it with `serve` and explicit
settings only (no `--mode`), under /tmp/e2e-agents. An agent that must
listen on the profile's default 127.0.0.1:3014, which the VM's own agent
holds, runs in a private network namespace and is reached with nsenter.
"""
from __future__ import annotations

import json

import make_e2e1 as base
from make_e2e1 import HERE, T, a, inp, ok, req
from make_e2e3a import res, task

GATE = "e2e-neutral"
NSPEC = "openspec/changes/platform-neutral-host-agent/specs/platform-neutral-host-agent/spec.md"
REL4 = "/opt/opute-e2e/releases/rust-0.4.0"
BIN4 = f"{REL4}/opute-host-agent"
P = "python3 /opt/opute-e2e/guest/neutralprobe.py"
GATE4 = "Error: mutations are disabled; set OPUTE_HOST_ALLOW_MUTATIONS=true"
ROOT = "/tmp/e2e-agents"

FILES = {**base.TOOL_FILES,
         "probe": inp("${REPO}/evidence/e2e/guest-tools/neutralprobe.py"),
         "rust04": inp("${PARITY}/e2e/release/rust-neutral/host-agent-linux-x64.gz")}
SETUP = base.TOOL_STEPS + [
    {"id": "probeCopy", "copy": "probe", "to": "/opt/opute-e2e/guest/neutralprobe.py"},
    {"id": "copy04", "copy": "rust04", "to": f"{REL4}/host-agent-linux-x64.gz"},
    {"id": "unpack04", "guest": f"gzip -dc {REL4}/host-agent-linux-x64.gz > {BIN4} && chmod 0755 {BIN4} && "
                                f"{BIN4} --version"},
]
EFFECTS = ["files//opt/opute-e2e/releases/rust-0.4.0*", "system/units/e2e-*", "system/unitFiles/e2e-*"]
AGENT04 = ("the 0.4.0 release binary built locally from this branch (VERSION=0.4.0-e2e, no tag, no publish), "
           "started by the journey with `serve` and explicit settings")


def pid(name: str) -> str:
    return f"$(systemctl show -p MainPID --value e2e-agent-{name})"


def ns(name: str) -> str:
    return f"nsenter -t {pid(name)} -n "


def start(name: str, env: dict, port: int = 3014, private: bool = False, binary: str = BIN4) -> str:
    """Start an agent as a transient unit with exactly `env` (plus HOME), and print its /health."""
    env = {"HOME": f"{ROOT}/{name}/home", **env}
    setenv = " ".join(f"--setenv='{k}={v}'" for k, v in env.items())
    reach = ns(name) if private else ""
    return (f"install -d -o opute -g opute {ROOT} {ROOT}/{name} {ROOT}/{name}/home && "
            f"systemd-run --quiet --collect --unit=e2e-agent-{name} --uid=opute --gid=opute "
            f"--property=Restart=no {'--property=PrivateNetwork=yes ' if private else ''}{setenv} {binary} serve && "
            f"for i in $(seq 120); do {reach}curl -fsS -o /dev/null http://127.0.0.1:{port}/health && break; "
            f"sleep 0.5; done; {reach}curl -fsS http://127.0.0.1:{port}/health")


def stop(name: str) -> str:
    return f"{T} agent stop {name}"


def wire(name: str, token: str, method: str, params: dict | None = None, port: int = 3014) -> str:
    """One MCP request to an agent in its own network namespace."""
    spec = {"kind": "mcp", "port": port, "token": token, "method": method, **({"params": params} if params else {})}
    return f"{ns(name)}python3 /opt/opute-e2e/guest/mcpwire.py <<'EOF'\n{json.dumps(spec)}\nEOF"


def wcall(name: str, token: str, tool: str, arguments: dict) -> str:
    return wire(name, token, "tools/call", {"name": tool, "arguments": arguments})


def summary(name: str, token: str, port: int = 3014, private: bool = False, binary: str = BIN4) -> str:
    return (f"{ns(name) if private else ''}{P} summary --port {port} --token '{token}' "
            f"--unit e2e-agent-{name} --binary {binary}")


def listen(name: str) -> str:
    return f"{ns(name)}ss -ltnH 'sport = :3014' | awk '{{print $4}}'"


def journey(id_, description, requirements, steps, cleanup, files=FILES, allow=(), minutes=15, **inputs):
    return base.journey(id_, description, {"agent": AGENT04, **inputs, "files": files}, requirements, steps,
                        cleanup, allow=EFFECTS + list(allow), minutes=minutes, gate=GATE)


def clean(*names: str, more: str = "") -> list[dict]:
    removes = "; ".join(f"{T} agent remove {n} >/dev/null" for n in names)
    return [{"id": "clean", "guest": f"{removes}; {more}rm -rf {REL4} /opt/opute-e2e/guest/neutralprobe.py; true"}]


# --- 1. the single profile, the old variables, ordinary clients -------------------------------------------

def profile() -> dict:
    target = {"path": ".config/opute-e2e/n1.txt", "content": "e2e\n", "scope": "user"}
    plain = {"OPUTE_REMOTE_AGENT_ID": "e2e-plain", "MCP_AUTH_TOKEN": "${SECRET:plain}"}
    state = lambda n: f"{ROOT}/{n}/home/.config/opute/instances"  # noqa: E731
    creds = lambda n: f"ls {state(n)}/standalone/state/credentials"  # noqa: E731
    exposed = {"port": 3004, "auth": "secret:exposed"}
    oauth = {"port": 3051, "auth": "secret:oauth"}
    oenv = {"OPUTE_REMOTE_AGENT_ID": "e2e-oauth", "MCP_AUTH_TOKEN": "${SECRET:oauth}", "HOST_MCP_PORT": "3051"}
    cred = f"{state('oauth')}/standalone/state/credentials/ci-runner.json"
    steps = SETUP + [
        # only an id and a token
        {"id": "plain", "guest": start("plain", plain, private=True)},
        {"id": "plainListen", "guest": listen("plain")},
        {"id": "plainSummary", "guest": summary("plain", "${SECRET:plain}", private=True)},
        {"id": "plainMutation", "guest": wcall("plain", "${SECRET:plain}", "ensure_host_file", target)},
        {"id": "plainRead", "guest": wcall("plain", "${SECRET:plain}", "get_host_info", {})},
        {"id": "plainNoFile", "guest": f"test ! -e {ROOT}/plain/home/.config/opute-e2e/n1.txt"},
        {"id": "plainState", "guest": f"ls {state('plain')}"},
        {"id": "plainCreds", "guest": creds("plain")},
        # the old mode variable, and nothing else
        {"id": "oldMode", "guest": start("oldmode", {**plain, "OPUTE_REMOTE_AGENT_ID": "e2e-oldmode",
                                                     "OPUTE_AGENT_MODE": "platform"}, private=True)},
        {"id": "oldModeListen", "guest": listen("oldmode")},
        {"id": "oldModeSummary", "guest": summary("oldmode", "${SECRET:plain}", private=True)},
        {"id": "oldModeMutation", "guest": wcall("oldmode", "${SECRET:plain}", "ensure_host_file", target)},
        {"id": "oldModeState", "guest": f"ls {state('oldmode')}"},
        # the old mutation variable
        {"id": "oldAllow", "guest": start("oldallow", {**plain, "OPUTE_REMOTE_AGENT_ID": "e2e-oldallow",
                                                       "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true"}, private=True)},
        {"id": "oldAllowMutation", "guest": wcall("oldallow", "${SECRET:plain}", "ensure_host_file", target)},
        {"id": "oldAllowSummary", "guest": summary("oldallow", "${SECRET:plain}", private=True)},
        # exposed on purpose
        {"id": "exposed", "guest": start("exposed", {
            "OPUTE_REMOTE_AGENT_ID": "e2e-exposed", "MCP_AUTH_TOKEN": "${SECRET:exposed}",
            "HOST_MCP_BIND_HOST": "0.0.0.0", "HOST_MCP_PORT": "3004", "OPUTE_HOST_ALLOW_MUTATIONS": "true"},
            port=3004)},
        {"id": "exposedListen", "guest": f"{T} listen 3004"},
        base.mcp("exposedList", "tools/list", **exposed),
        {"id": "exposedSummary", "guest": summary("exposed", "${SECRET:exposed}", port=3004)},
        base.call("exposedWrite", "ensure_host_file", target, **exposed),
        {"id": "exposedFile", "guest": f"cat {ROOT}/exposed/home/.config/opute-e2e/n1.txt"},
        # machine clients come from the operator
        {"id": "oauthAgent", "guest": start("oauth", {**oenv, "OPUTE_OAUTH_CLIENTS": "ci-runner"}, port=3051)},
        {"id": "oauthCreds", "guest": creds("oauth")},
        {"id": "oauthMint", "guest": f"{P} oauth --port 3051 --client ci-runner --cred {cred} "
                                     f"--save {ROOT}/oauth/token"},
        {"id": "oldClient", "guest": f"{P} oauth --port 3051 --client opute-mcp-host --secret not-a-secret"},
        base.mcp("oauthBearer", "tools/list", **oauth),
        {"id": "oauthStop", "guest": stop("oauth")},
        {"id": "oauthAgain", "guest": start("oauth", oenv, port=3051)},
        {"id": "oauthCredsAfter", "guest": creds("oauth")},
        {"id": "oauthReuse", "guest": f"{P} reuse --port 3051 --token-file {ROOT}/oauth/token"},
        base.mcp("oauthBearerAfter", "tools/list", **oauth),
    ]
    default_line = ["bind=127.0.0.1 port=3014 mutations=false"]
    refused = lambda s: [a(s, "json.body.result.isError", equals=True),  # noqa: E731
                         a(s, "json.body.result.content[0].text", equals=GATE4)]
    reqs = [
        req(NSPEC, "One serving profile",
            a("unpack04", "stdout", equals="0.4.0-e2e\n"),
            a("plain", "json.agentId", equals="e2e-plain"),
            a("plainListen", "stdout", equals="127.0.0.1:3014\n"),
            *refused("plainMutation"), ok("plainNoFile"),
            a("plainRead", "json.body.result.isError", **{"in": [False, "<absent>"]}),
            a("plainRead", "json.body.result.structuredContent.uri", matches=":e2e-plain$"),
            a("plainSummary", "json.hits", equals=[]),
            a("plainSummary", "json.tools", gte=100), a("plainSummary", "json.helpBytes", gte=100),
            scenario="Started with only an id and a token: it serves on 127.0.0.1:3014 with read tools only, and "
                     "its journal, tools/list and help name no Platform identifier"),
        req(NSPEC, "One serving profile",
            a("exposed", "json.agentId", equals="e2e-exposed"),
            a("exposedListen", "json.addresses", equals=["0.0.0.0:3004"]),
            a("exposedWrite", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            a("exposedFile", "stdout", equals="e2e\n"),
            a("exposedSummary", "json.catalogSha", equalsStep={"step": "plainSummary", "path": "json.catalogSha"}),
            scenario="A host exposed on purpose (bind 0.0.0.0, port 3004, mutations true) serves there with "
                     "mutations admitted and the same catalog"),
        req(NSPEC, "One serving profile",
            a("plainSummary", "json.profileLines", equals=default_line),
            a("exposedSummary", "json.profileLines", equals=["bind=0.0.0.0 port=3004 mutations=true"]),
            scenario="The effective bind, port and mutation policy are logged once"),
        req(NSPEC, "One serving profile", *refused("plainMutation"),
            scenario="Mutations not enabled: the refusal names OPUTE_HOST_ALLOW_MUTATIONS"),
        req(NSPEC, "No mode setting exists",
            a("oldMode", "json.agentId", equals="e2e-oldmode"),
            a("oldModeListen", "stdout", equals="127.0.0.1:3014\n"),
            a("oldModeSummary", "json.profileLines", equals=default_line),
            a("oldModeSummary", "json.catalogSha", equalsStep={"step": "plainSummary", "path": "json.catalogSha"}),
            a("oldModeSummary", "json.hits", equals=[]),
            *refused("oldModeMutation"),
            a("oldModeState", "stdout", equalsStep={"step": "plainState", "path": "stdout"}),
            a("plainState", "stdout", equals="standalone\n"),
            scenario="A host that still carries OPUTE_AGENT_MODE=platform and no other setting starts exactly as "
                     "one without it, and nothing it prints mentions the variable"),
        req(NSPEC, "No mode setting exists",
            *refused("oldAllowMutation"),
            a("oldAllowSummary", "json.profileLines", equals=default_line),
            a("oldAllowSummary", "json.hits", equals=[]),
            scenario="OPUTE_STANDALONE_ALLOW_MUTATIONS=true admits nothing"),
        req(NSPEC, "Every orchestrator is an ordinary client",
            a("plainCreds", "stdout", equals="host-agent-provider.json\n"),
            a("plainRead", "json.status", equals=200),
            scenario="No OAuth clients configured: no machine client is provisioned, and the bearer works"),
        req(NSPEC, "Every orchestrator is an ordinary client",
            a("oauthCreds", "stdout", equals="ci-runner.json\nhost-agent-provider.json\n"),
            a("oauthMint", "json.tokenStatus", equals=200), a("oauthMint", "json.listStatus", equals=200),
            a("oauthMint", "json.tools", equalsStep={"step": "plainSummary", "path": "json.tools"}),
            a("oldClient", "json.tokenStatus", equals=401), a("oldClient", "json.error", equals="invalid_client"),
            a("oauthBearer", "response.status", equals=200),
            a("oauthCredsAfter", "stdout", equals="host-agent-provider.json\n"),
            a("oauthReuse", "json.listStatus", equals=401),
            a("oauthBearerAfter", "response.status", equals=200),
            scenario="A client listed in OPUTE_OAUTH_CLIENTS gets a credential and a working token; the earlier "
                     "built-in client id is unknown; once unlisted, its credential and token are gone"),
    ]
    return journey(
        "neutral.profile",
        "The single serving profile of 0.4.0: an agent with only an id and a token; agents that still carry the old "
        "mode and mutation variables; a host exposed on purpose; and OAuth machine clients from operator "
        "configuration.",
        reqs, steps, clean("plain", "oldmode", "oldallow", "exposed", "oauth"))


# --- 2. reserved hostnames ------------------------------------------------------------------------------

PROVIDER = "/opt/opute-e2e/providers/opute-provider-cloudflare"
RECIPE = "/home/opute/.config/opute-e2e/cloudflare-activate-tunneling.yaml"
TUNNEL = "opute.capability.tunneling.ensure-host-tunnel"


def descriptor(port: int) -> dict:
    return {"schema": "opute-provider-plugin.v1", "pluginId": "com.opute.cloudflare", "version": "1.0.1",
            "capabilities": [{"id": f"opute.capability.{c}.v1", "version": 1}
                             for c in ("tunneling", "mesh-runtime", "mesh-membership", "public-ingress")],
            "server": {"transport": "streamable_http", "endpoint": f"http://127.0.0.1:{port}/mcp"}}


def wait_tool(port: int, token: str) -> str:
    return ("python3 -c 'import sys, time, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
            "for i in range(120):\n"
            f"    r = mcpwire.mcp({port}, sys.argv[1], \"tools/list\", request_id=i + 1)\n"
            "    names = [t[\"name\"] for t in ((r.get(\"body\") or {}).get(\"result\") or {}).get(\"tools\") or []]\n"
            f"    if \"{TUNNEL}\" in names: break\n"
            "    time.sleep(0.5)\n"
            f"print(json.dumps({{\"listed\": \"{TUNNEL}\" in names}}))' '{token}'")


def reserved() -> dict:
    files = {**FILES,
             "providerCloudflare": inp("${PARITY}/e2e/release/rust-neutral/opute-provider-cloudflare-linux-x64"),
             # The pinned Go tree's recipe, byte for byte: the provider's install manifest pins its sha256.
             "activate": inp("${REPO}/evidence/e2e/journeys/inputs/cloudflare-activate-tunneling.yaml")}
    r_ = {"port": 3041, "auth": "secret:r"}
    n_ = {"port": 3042, "auth": "secret:n"}

    def agent(name: str, port: int) -> str:
        return start(name, {"OPUTE_REMOTE_AGENT_ID": f"e2e-cf-{name}", "MCP_AUTH_TOKEN": "${SECRET:" + name + "}",
                            "HOST_MCP_PORT": str(port), "OPUTE_HOST_ALLOW_MUTATIONS": "true"}, port=port)

    def tunnel(hostname: str, port: int) -> dict:
        return {"bindingId": "e2e", "placement": "host", "localTarget": f"http://127.0.0.1:{port}/mcp",
                "hostname": hostname, "tunnelName": "e2e-tunnel"}

    steps = SETUP + [
        {"id": "providerCopy", "copy": "providerCloudflare", "to": PROVIDER, "mode": "0755"},
        {"id": "recipeCopy", "copy": "activate", "to": RECIPE, "mode": "0644"},
        {"id": "agentR", "guest": agent("r", 3041)},
        {"id": "agentN", "guest": agent("n", 3042)},
        base.mcp("listR", "tools/list", **r_),
        base.mcp("listN", "tools/list", **n_),
        {"id": "providers", "guest": (
            "chown -R opute /home/opute/.config/opute-e2e && "
            "systemd-run --quiet --collect --unit=e2e-provider-cf-r --uid=opute --gid=opute "
            "--setenv=HOME=/home/opute --setenv=OPUTE_PROVIDER_PORT=4330 "
            f"--setenv='OPUTE_RESERVED_HOSTNAMES=*.opute.io' --property=Restart=no {PROVIDER} && "
            "systemd-run --quiet --collect --unit=e2e-provider-cf-n --uid=opute --gid=opute "
            f"--setenv=HOME=/home/opute --setenv=OPUTE_PROVIDER_PORT=4331 --property=Restart=no {PROVIDER} && "
            "for i in $(seq 60); do ss -ltnH 'sport = :4330' | grep -q . && ss -ltnH 'sport = :4331' | grep -q . "
            "&& break; sleep 0.5; done; ss -ltnH '( sport = :4330 or sport = :4331 )' | wc -l")},
        base.call("installR", "opute.provider.install",
                  {"descriptor": descriptor(4330), "activate": True, "recipeSource": RECIPE}, **r_),
        base.call("installN", "opute.provider.install",
                  {"descriptor": descriptor(4331), "activate": True, "recipeSource": RECIPE}, **n_),
        {"id": "toolsR", "guest": wait_tool(3041, "${SECRET:r}")},
        {"id": "toolsN", "guest": wait_tool(3042, "${SECRET:n}")},
        base.mcp("catalogR", "tools/list", **r_),
        base.mcp("catalogN", "tools/list", **n_),
        *task("reservedName", TUNNEL, tunnel("mcp.opute.io", 3041), r_, timeout=120),
        *task("otherName", TUNNEL, tunnel("agent.example.test", 3041), r_, timeout=120),
        *task("nothingReserved", TUNNEL, tunnel("mcp.opute.io", 3042), n_, timeout=120),
    ]
    no_credentials = "CLOUDFLARE_ACCOUNT_ID"
    reqs = [
        req(NSPEC, "Reserved names are operator configuration",
            a("providers", "stdout", equals="2\n"),
            a("toolsR", "json.listed", equals=True),
            res("reservedName", "isError", equals=True),
            res("reservedName", "structuredContent.code", equals="reserved_hostname"),
            res("reservedName", "structuredContent.hostname", equals="mcp.opute.io"),
            res("reservedName", "structuredContent.pattern", equals="*.opute.io"),
            res("otherName", "isError", equals=True),
            res("otherName", "content[0].text", contains=no_credentials),
            scenario="A deployment protects its names: with OPUTE_RESERVED_HOSTNAMES=*.opute.io a tunnel for "
                     "mcp.opute.io is refused with reserved_hostname. The provider has no Cloudflare credentials, "
                     "so no API call is possible; another hostname gets past the guard to that credentials error"),
        req(NSPEC, "Reserved names are operator configuration",
            a("toolsN", "json.listed", equals=True),
            res("nothingReserved", "isError", equals=True),
            res("nothingReserved", "content[0].text", contains=no_credentials),
            res("nothingReserved", "structuredContent.code", absent=True),
            scenario="Nothing reserved: mcp.opute.io is accepted like any other hostname and reaches the same "
                     "credentials error"),
    ]
    more = ("systemctl stop e2e-provider-cf-r e2e-provider-cf-n 2>/dev/null; "
            "rm -rf /home/opute/.config/opute-e2e /opt/opute-e2e/providers; ")
    return journey(
        "neutral.reserved",
        "The Cloudflare provider's reserved-name guard as operator configuration: two 0.4.0 agents, each with "
        "its own Cloudflare provider and no Cloudflare credentials; one provider runs with "
        "OPUTE_RESERVED_HOSTNAMES=*.opute.io.",
        reqs, steps, clean("r", "n", more=more), files=files,
        allow=["files//home/opute/.config/opute-e2e*", "files//opt/opute-e2e/providers*"],
        provider="the Rust Cloudflare provider 1.0.1 from the same release set, without Cloudflare credentials")


# --- 3. upgrade and rollback with both settings sets ------------------------------------------------------

def rollback() -> dict:
    env = {"OPUTE_REMOTE_AGENT_ID": "e2e-roll", "MCP_AUTH_TOKEN": "${SECRET:roll}",
           # the 0.3.x settings of a Platform-managed host
           "OPUTE_AGENT_MODE": "platform",
           # the explicit settings its owners add before 0.4.0
           "HOST_MCP_BIND_HOST": "0.0.0.0", "HOST_MCP_PORT": "3004", "OPUTE_HOST_ALLOW_MUTATIONS": "true",
           "OPUTE_HOST_AGENT_INSTANCE": "platform", "OPUTE_OAUTH_CLIENTS": "opute-mcp-host"}
    host = {"port": 3004, "auth": "secret:roll"}
    state = f"{ROOT}/roll/home/.config/opute/instances"
    cred = f"{state}/platform/state/credentials/opute-mcp-host.json"
    sdb = f"{state}/platform/state/state.db"
    mint = f"{P} oauth --port 3004 --client opute-mcp-host --cred {cred}"

    def write(n: int) -> dict:
        return base.call(f"write{n}", "ensure_host_file",
                         {"path": f".config/opute-e2e/r{n}.txt", "content": f"r{n}\n", "scope": "user"}, **host)

    def read(id_: str, n: int) -> dict:
        return base.call(id_, "inspect_host_file", {"path": f".config/opute-e2e/r{n}.txt", "scope": "user"}, **host)

    steps = SETUP + [
        {"id": "old", "guest": start("roll", env, port=3004, binary=base.BIN)},
        {"id": "oldListen", "guest": f"{T} listen 3004"},
        base.mcp("oldList", "tools/list", **host),
        write(1),
        {"id": "oldMint", "guest": mint},
        {"id": "oldStop", "guest": stop("roll")},
        # upgrade: the same environment, the 0.4.0 binary
        {"id": "new", "guest": start("roll", env, port=3004)},
        {"id": "newListen", "guest": f"{T} listen 3004"},
        base.mcp("newList", "tools/list", **host),
        {"id": "newSummary", "guest": summary("roll", "${SECRET:roll}", port=3004)},
        read("newRead1", 1),
        write(2),
        {"id": "newMint", "guest": mint},
        {"id": "newState", "guest": f"ls {state}"},
        {"id": "newStop", "guest": stop("roll")},
        {"id": "newDb", "guest": f"{T} db {sdb}"},
        # rollback: the same environment, the 0.3.x binary
        {"id": "back", "guest": start("roll", env, port=3004, binary=base.BIN)},
        {"id": "backListen", "guest": f"{T} listen 3004"},
        base.mcp("backList", "tools/list", **host),
        read("backRead2", 2),
        write(3),
        {"id": "backMint", "guest": mint},
        {"id": "backStop", "guest": stop("roll")},
        {"id": "backDb", "guest": f"{T} db {sdb}"},
    ]
    served = lambda s: [a(s, "json.agentId", equals="e2e-roll")]  # noqa: E731
    wrote = lambda s: a(s, "response.body.result.isError", **{"in": [False, "<absent>"]})  # noqa: E731
    exists = lambda s: a(s, "response.body.result.structuredContent.exists", equals=True)  # noqa: E731
    same_cred = {"step": "oldMint", "path": "json.credSha256"}
    reqs = [
        req(NSPEC, "One serving profile",
            *served("old"), a("oldListen", "json.addresses", equals=["0.0.0.0:3004"]), wrote("write1"),
            *served("new"), a("newListen", "json.addresses", equals=["0.0.0.0:3004"]),
            a("newSummary", "json.profileLines", equals=["bind=0.0.0.0 port=3004 mutations=true"]),
            exists("newRead1"), wrote("write2"), a("newState", "stdout", equals="platform\n"),
            a("newDb", "json.integrity", equals="ok"),
            scenario="A host carrying both settings sets is upgraded from 0.3.x to 0.4.0 with nothing switched: "
                     "it serves at the same address with mutations admitted, on the same state"),
        req(NSPEC, "No mode setting exists",
            *served("back"), a("backListen", "json.addresses", equals=["0.0.0.0:3004"]),
            exists("backRead2"), wrote("write3"), a("backDb", "json.integrity", equals="ok"),
            scenario="Rollback to 0.3.x keeps both settings sets: the old binary reads OPUTE_AGENT_MODE, the new "
                     "one ignored it, and work done under 0.4.0 is there"),
        req(NSPEC, "Every orchestrator is an ordinary client",
            a("oldMint", "json.listStatus", equals=200),
            a("newMint", "json.listStatus", equals=200),
            a("newMint", "json.credSha256", equalsStep=same_cred),
            a("backMint", "json.listStatus", equals=200),
            a("backMint", "json.credSha256", equalsStep=same_cred),
            scenario="An orchestrator's machine client listed in OPUTE_OAUTH_CLIENTS keeps its generated "
                     "credential across the upgrade and the rollback"),
    ]
    return journey(
        "neutral.rollback",
        "Upgrade a Platform-style host from the VM's 0.3.x binary to 0.4.0 and roll it back, with one environment "
        "holding both settings sets (OPUTE_AGENT_MODE=platform next to the explicit bind, port, mutation, "
        "instance and OAuth client settings).",
        reqs, steps, clean("roll"),
        previous="the dev VM's installed 0.3.x Rust release binary (" + base.BIN + ")")


JOURNEYS = [profile, reserved, rollback]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
