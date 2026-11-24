#!/usr/bin/env python3
"""Write the per-client mutation authority (D55) journey as JSON.

  python3 evidence/e2e/journeys/make_mutations.py

The dev VM runs the release candidate. The journey copies the candidate's
release binary in and starts its own agents from it under /tmp/e2e-agents:
one without --allow-mutations and with three OAuth machine clients (`ro`
granted read, `rw` mutate, `plain` listed without a scope), one still
carrying the three settings earlier releases read, and one started with
--allow-mutations. Each caller calls the same mutation, and the journey
checks that it is admitted or refused with the exact text naming the grant
that would admit it. The upgrade and rollback across 0.4.0 and 0.5.0 are
neutral.rollback's; a provider callback's bound is the crate test
provider::delegation::tests::a_callback_acts_with_the_effect_of_its_parent_operation,
and e2e3a's k3s provider exercises admitted callbacks.
"""
from __future__ import annotations

import json

import make_e2e1 as base
import make_neutral as neutral
from make_e2e1 import HERE, a, ok, req
from make_neutral import BIN4, FLAG, GATE4, MSPEC, P, ROOT, SETUP, agent_effects, clean, start, units

GATE = "e2e-mutations"
TARGET = "ensure_host_file"


def target(name: str, n: str) -> dict:
    return {"path": f".config/opute-e2e/m-{n}.txt", "content": f"{n}\n", "scope": "user"}


def listed(client: str) -> str:
    return (f"Error: mutation refused: {TARGET} has effect mutation; OAuth client {client} holds the read scope. "
            f"Grant it with OPUTE_OAUTH_CLIENTS={client}:mutate and request a token with scope mutate.")


READ_TOKEN = (f"Error: mutation refused: {TARGET} has effect mutation; OAuth client rw holds a token with the read "
              "scope. Request a token with scope mutate.")
INTERACTIVE = (f"Error: mutation refused: {TARGET} has effect mutation; OAuth client host-agent-bootstrap holds a "
               "token with the read scope. Authorize it again with scope mutate and approve the request "
               "(opute-host-agent oauth approve).")


def callers() -> dict:
    env = {"OPUTE_REMOTE_AGENT_ID": "e2e-callers", "MCP_AUTH_TOKEN": "${SECRET:callers}", "HOST_MCP_PORT": "3061",
           "OPUTE_OAUTH_CLIENTS": "ro:read,rw:mutate,plain"}
    home = lambda n: f"{ROOT}/{n}/home"  # noqa: E731
    state = lambda n: f"{home(n)}/.config/opute/instances/standalone/state"  # noqa: E731
    cred = lambda c: f"{state('callers')}/credentials/{c}.json"  # noqa: E731
    file = lambda n, m: f"{home(n)}/.config/opute-e2e/m-{m}.txt"  # noqa: E731
    host = {"port": 3061, "auth": "secret:callers"}

    def client(id_: str, name: str, n: str, scope: str = "", port: int = 3061, agent: str = "callers") -> dict:
        return {"id": id_, "guest": (f"{P} oauth --port {port} --client {name} "
                                     f"--cred {state(agent)}/credentials/{name}.json"
                                     + (f" --scope {scope}" if scope else "")
                                     + f" --call {TARGET} --arguments '{json.dumps(target(agent, n))}'")}

    def interactive(id_: str, n: str, scope: str = "") -> dict:
        return {"id": id_, "guest": (f"{P} interactive --port 3061 --binary {BIN4} --state-dir {state('callers')}"
                                     + (f" --scope {scope}" if scope else "")
                                     + f" --call {TARGET} --arguments '{json.dumps(target('callers', n))}'")}

    leftover = {"OPUTE_REMOTE_AGENT_ID": "e2e-leftover", "MCP_AUTH_TOKEN": "${SECRET:leftover}",
                "HOST_MCP_PORT": "3062", "OPUTE_HOST_ALLOW_MUTATIONS": "true",
                "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true", "OPUTE_AGENT_MODE": "standalone"}
    flagged = {"OPUTE_REMOTE_AGENT_ID": "e2e-flagged", "MCP_AUTH_TOKEN": "${SECRET:flagged}",
               "HOST_MCP_PORT": "3063", "OPUTE_OAUTH_CLIENTS": "ro:read"}
    former = (f"install -d -o opute -g opute {ROOT}/former && runuser -u opute -- env -i PATH=/usr/bin:/bin "
              f"HOME={ROOT}/former OPUTE_REMOTE_AGENT_ID=e2e-former OPUTE_OAUTH_CLIENTS=ci:mcp {BIN4} serve 2>&1 "
              f"| tail -1; echo exit=${{PIPESTATUS[0]}}; find {ROOT}/former -mindepth 1 | wc -l")
    steps = SETUP + [
        {"id": "callers", "guest": start("callers", env, port=3061)},
        base.mcp("callersList", "tools/list", **host),
        base.call("bearerRead", "get_host_info", {}, **host),
        base.call("bearerMutation", TARGET, target("callers", "bearer"), **host),
        {"id": "bearerNoFile", "guest": f"test ! -e {file('callers', 'bearer')}"},
        client("ro", "ro", "ro"),
        {"id": "roAbove", "guest": f"{P} oauth --port 3061 --client ro --cred {cred('ro')} --scope mutate"},
        client("plain", "plain", "plain"),
        client("rwRead", "rw", "rw-read", scope="read"),
        {"id": "refusedNoFiles", "guest": " && ".join(f"test ! -e {file('callers', n)}"
                                                       for n in ("ro", "plain", "rw-read"))},
        client("rw", "rw", "rw"),
        {"id": "rwFile", "guest": f"cat {file('callers', 'rw')}", "confirms": ["rw"]},
        interactive("interactiveRead", "i-read"),
        {"id": "interactiveNoFile", "guest": f"test ! -e {file('callers', 'i-read')}"},
        interactive("interactiveMutate", "i-mutate", scope="mutate"),
        {"id": "interactiveFile", "guest": f"cat {file('callers', 'i-mutate')}", "confirms": ["interactiveMutate"]},
        {"id": "annotations", "guest": f"{P} annotations --port 3061 --token '${{SECRET:callers}}'"},
        {"id": "leftover", "guest": start("leftover", leftover, port=3062)},
        base.mcp("leftoverList", "tools/list", port=3062, auth="secret:leftover"),
        base.call("leftoverMutation", TARGET, target("leftover", "left"), port=3062, auth="secret:leftover"),
        {"id": "leftoverNoFile", "guest": f"test ! -e {file('leftover', 'left')}"},
        {"id": "flagged", "guest": start("flagged", flagged, port=3063, args=FLAG)},
        base.mcp("flaggedList", "tools/list", port=3063, auth="secret:flagged"),
        base.call("flaggedMutation", TARGET, target("flagged", "flag"), port=3063, auth="secret:flagged"),
        {"id": "flaggedFile", "guest": f"cat {file('flagged', 'flag')}", "confirms": ["flaggedMutation"]},
        client("flaggedRo", "ro", "flag-ro", port=3063, agent="flagged"),
        {"id": "flaggedRoNoFile", "guest": f"test ! -e {file('flagged', 'flag-ro')}"},
        {"id": "former", "guest": former},
    ]
    refused = lambda s, text: [a(s, "response.body.result.isError", equals=True),  # noqa: E731
                               a(s, "response.body.result.content[0].text", equals=text),
                               a(s, "response.body.result.structuredContent.code", equals="mutation_refused")]
    refused_call = lambda s, text: [a(s, "json.call.isError", equals=True),  # noqa: E731
                                    a(s, "json.call.text", equals=text),
                                    a(s, "json.call.code", equals="mutation_refused")]
    admitted_call = lambda s: [a(s, "json.call.isError", equals=False)]  # noqa: E731
    reqs = [
        req(MSPEC, "Mutation authority belongs to the caller",
            a("unpack04", "stdout", equals=base.RUST_VERSION + "\n"), a("callers", "json.agentId", equals="e2e-callers"),
            a("bearerRead", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            *refused("bearerMutation", GATE4(TARGET)), ok("bearerNoFile"),
            scenario="Bearer without the flag"),
        req(MSPEC, "Mutation authority belongs to the caller",
            a("flagged", "json.agentId", equals="e2e-flagged"),
            a("flaggedMutation", "response.body.result.isError", **{"in": [False, "<absent>"]}),
            a("flaggedFile", "stdout", equals="flag\n"),
            *refused_call("flaggedRo", listed("ro")), ok("flaggedRoNoFile"),
            scenario="Bearer with the flag; the flag does not raise a read client"),
        req(MSPEC, "Mutation authority belongs to the caller",
            a("ro", "json.scope", equals="read"), *refused_call("ro", listed("ro")),
            a("plain", "json.scope", equals="read"), *refused_call("plain", listed("plain")),
            a("rw", "json.scope", equals="mutate"), *admitted_call("rw"), a("rwFile", "stdout", equals="rw\n"),
            ok("refusedNoFiles"),
            scenario="Two clients on one agent: ro (read) and plain (listed without a scope) are refused, rw "
                     "(mutate) is admitted"),
        req(MSPEC, "OAuth scopes are read and mutate",
            a("roAbove", "json.tokenStatus", equals=400), a("roAbove", "json.error", equals="invalid_scope"),
            a("roAbove", "json.issued", equals=False),
            a("rwRead", "json.scope", equals="read"), *refused_call("rwRead", READ_TOKEN),
            scenario="Asking above the grant is invalid_scope; a mutate client's read token reads only"),
        req(MSPEC, "OAuth scopes are read and mutate",
            a("interactiveRead", "json.pageNamesScope", equals=True),
            a("interactiveRead", "json.pending", equals=["host-agent-bootstrap scope=read"]),
            a("interactiveRead", "json.scope", equals="read"),
            *refused_call("interactiveRead", INTERACTIVE), ok("interactiveNoFile"),
            a("interactiveMutate", "json.pageNamesScope", equals=True),
            a("interactiveMutate", "json.pending", equals=["host-agent-bootstrap scope=mutate"]),
            a("interactiveMutate", "json.approved", matches=" with scope mutate$"),
            a("interactiveMutate", "json.scope", equals="mutate"), *admitted_call("interactiveMutate"),
            a("interactiveFile", "stdout", equals="i-mutate\n"),
            scenario="Interactive approval: the request names its scope (read by default), the operator's "
                     "listing and approval show it, and the token carries it"),
        req(MSPEC, "OAuth scopes are read and mutate",
            a("former", "stdout", equals='OPUTE_OAUTH_CLIENTS: client ci asks for unknown scope "mcp"; grant ci:read '
                                         'or ci:mutate\nexit=1\n0\n'),
            scenario="Former scope: OPUTE_OAUTH_CLIENTS=ci:mcp fails the start, naming read and mutate, and writes "
                     "nothing"),
        req(MSPEC, "No mutation setting other than the flag",
            a("leftover", "json.agentId", equals="e2e-leftover"),
            *refused("leftoverMutation", GATE4(TARGET)), ok("leftoverNoFile"),
            scenario="A leftover setting"),
        req(MSPEC, "A refusal names the grant that admits the call",
            *refused("bearerMutation", GATE4(TARGET)), *refused_call("ro", listed("ro")),
            *refused_call("interactiveRead", INTERACTIVE),
            scenario="Bearer refused; read client refused; interactive client refused"),
        req(MSPEC, "Tool annotations follow the effect",
            a("annotations", "json.tools", gte=100), a("annotations", "json.disagree", equals=[]),
            a("annotations", "json.effects", equals=["credential_bearing", "destructive", "mutation", "read"]),
            scenario="Effects and hints agree"),
    ]
    agents = ("callers", "leftover", "flagged")
    return base.journey(
        "mutations.callers",
        f"Per-client mutation authority (D55) on the {base.RUST_VERSION} candidate: the bootstrap bearer with and "
        "without --allow-mutations, OAuth machine clients granted read and mutate on one agent, the interactive "
        "client approved for each scope, an agent still carrying the old settings, the former mcp scope, and "
        "the tools' annotations.",
        {"agent": neutral.AGENT04, "files": neutral.FILES}, reqs, steps,
        clean(*agents, more=f"rm -rf {ROOT}/former; rmdir {ROOT} 2>/dev/null; "),
        allow=neutral.EFFECTS + units(*(f"e2e-agent-{n}" for n in agents)) + agent_effects(*agents)
        + [f"files/{ROOT}/former*"] + [f"sockets/tcp 127.0.0.1:{p} opute-host-agen" for p in (3061, 3062, 3063)],
        minutes=10, gate=GATE)


JOURNEYS = [callers]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
