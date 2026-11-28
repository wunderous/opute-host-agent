"""m35: the public sites on the site VM, by typed MCP calls and the two site recipes only.

Shared by site-restore.py (bring all three sites up) and site-deploy.py (deploy a new docs image).
Every command runs against the site VM (E2E_INSTANCE=site) under its lease. The Cloudflare
credentials file is copied into the VM, never read.
"""
import json
import os
import stat
import subprocess
import sys
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen

os.environ.setdefault("E2E_INSTANCE", "site")
REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "evidence/e2e"), str(REPO / "evidence/e2e/devvm")]
import devvm  # noqa: E402
import e2elib  # noqa: E402
import mcpwire  # noqa: E402

if e2elib.INSTANCE != "site":
    raise SystemExit("refused: the sites run on the site VM only (E2E_INSTANCE=site)")

CONFIG = e2elib.PARITY / "minimal-site"
GUEST_CONFIG = "/home/opute/.config/opute-m20"
INPUTS = REPO / "evidence/e2e/journeys/inputs"
MINIMAL = REPO / "evidence/m20/minimal-site.recipe.yaml"
DOCS = REPO / "evidence/m34/opute-io-site.recipe.yaml"
CLUSTER = "minimal-k1"
IMAGE = "ghcr.io/wunderous/host-agent-www"
DEPLOYED = e2elib.DEV / "site-deployed.json"
TASKS = {"io.modelcontextprotocol/clientCapabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}}}}


def raw(method, params=None, meta=None, timeout=600):
    return mcpwire.mcp(devvm.MCP_PORT, devvm.token(), method, params, meta=meta, timeout=timeout)["body"]


def call(name, args=None):
    body = raw("tools/call", {"name": name, "arguments": args or {}})
    if "error" in body:
        raise SystemExit(f"{name} failed: {json.dumps(body['error'])[:400]}")
    result = body["result"]
    if result.get("isError"):
        text = "".join(c.get("text", "") for c in result.get("content") or [])
        raise SystemExit(f"{name} failed: {text[:400]}")
    return result.get("structuredContent") or {}


def task(name, args=None, timeout=900):
    body = raw("tools/call", {"name": name, "arguments": args or {}}, meta=TASKS)
    result = body.get("result") or {}
    task_id = result.get("taskId")
    if not task_id:
        if "error" in body or result.get("isError"):
            raise SystemExit(f"{name} failed: {json.dumps(body)[:400]}")
        return result.get("structuredContent") or {}
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        state = raw("tasks/get", {"taskId": task_id}, meta=TASKS).get("result") or {}
        if state.get("status") == "completed" and not (state.get("result") or {}).get("isError"):
            return (state.get("result") or {}).get("structuredContent") or {}
        if state.get("status") in ("completed", "failed", "cancelled"):
            raise SystemExit(f"{name} {state.get('status')}: {json.dumps(state)[:400]}")
        time.sleep(3)
    raise SystemExit(f"{name} still running after {timeout}s")


def release_dir() -> str:
    version = json.loads((e2elib.DEV / "build.json").read_text())["agent"]["version"]
    return f"/opt/opute-e2e/releases/rust/v{version}"


def copy_credentials() -> None:
    env = CONFIG / "cloudflare.env"
    if not env.is_file() or stat.S_IMODE(env.stat().st_mode) & 0o077:
        raise SystemExit("refused: the credentials file must exist and be 0600 or stricter")
    devvm.guest(f"install -d -m 0700 -o opute -g opute {GUEST_CONFIG}", check=True)
    devvm.scp([env], f"{GUEST_CONFIG}/cloudflare.env")
    devvm.guest(f"chmod 0600 {GUEST_CONFIG}/cloudflare.env && chown opute:opute {GUEST_CONFIG}/cloudflare.env",
                check=True)


# The providers run as user units beside the agent; a recipe cannot install a provider (D44).
PROVIDERS = [
    {"id": "k3s", "port": 4320, "env": [], "activate": INPUTS / "k3s-activate.yaml",
     "descriptor": {"schema": "opute-provider-plugin.v1", "pluginId": "com.opute.k3s", "version": "1.0.2",
                    "capabilities": [{"id": "opute.capability.kubernetes.v1", "version": 1}]},
     "probe": "opute.capability.kubernetes.provision"},
    {"id": "cloudflare", "port": 4330,
     "env": ["Environment=OPUTE_HOST_AGENT_ENDPOINT=http://127.0.0.1:3014/mcp",
             "EnvironmentFile=/etc/opute/host-agent.env", f"EnvironmentFile=-{GUEST_CONFIG}/cloudflare.env"],
     "activate": INPUTS / "cloudflare-activate-tunneling.yaml",
     "descriptor": {"schema": "opute-provider-plugin.v1", "pluginId": "com.opute.cloudflare", "version": "1.0.1",
                    "capabilities": [{"id": f"opute.capability.{c}.v1", "version": 1}
                                     for c in ("tunneling", "mesh-runtime", "mesh-membership", "public-ingress")]},
     "probe": "ensure_cloudflared_tunnel"},
]


def prerequisites() -> None:
    call("ensure_host_service_supervisor")
    task("ensure_oci_builder")
    for p in PROVIDERS:
        name = f"opute-provider-{p['id']}"
        unit = "\n".join(["[Unit]", f"Description=Opute {p['id']} provider (site VM)", "", "[Service]",
                          f"Environment=OPUTE_PROVIDER_PORT={p['port']}", *p["env"],
                          f"ExecStart={release_dir()}/opute-provider-{p['id']}-linux-x64", "Restart=on-failure", "",
                          "[Install]", "WantedBy=default.target", ""])
        call("ensure_host_file", {"path": f"~/.config/systemd/user/{name}.service", "content": unit,
                                  "mode": 420, "scope": "user"})
        call("set_host_service_state", {"uri": f"host-service:local:user/{name}", "state": "restart",
                                        "scope": "user"})
        activate = f"{GUEST_CONFIG}/{p['id']}-activate.yaml"
        call("ensure_host_file", {"path": activate, "content": p["activate"].read_text(), "mode": 420,
                                  "scope": "user"})
        time.sleep(2)
        descriptor = {**p["descriptor"],
                      "server": {"transport": "streamable_http", "endpoint": f"http://127.0.0.1:{p['port']}/mcp"}}
        call("opute.provider.install", {"descriptor": descriptor, "activate": True, "recipeSource": activate})
        for _ in range(120):
            if p["probe"] in [t["name"] for t in raw("tools/list")["result"]["tools"]]:
                break
            time.sleep(1)
        else:
            raise SystemExit(f"{p['id']}: {p['probe']} never listed")
        print(f"provider {p['id']} listed", flush=True)


def wait_for_cluster(minutes=10):
    """After the VM boots, wait until the guest's K3s serves every Deployment again (it restarts by itself).
    The recipes run only then: run earlier, the minimal site recipe re-pushes its image to a registry that
    is not up yet, and fails (m35)."""
    deadline = time.monotonic() + minutes * 60
    while time.monotonic() < deadline:
        body = raw("tools/call", {"name": "exec_cluster_command", "arguments": {
            "uri": f"cluster:local:{CLUSTER}", "command": "wait",
            "args": ["--for=condition=Available", "deployment", "--all", "--all-namespaces", "--timeout=60s"]}})
        result = body.get("result") or {}
        if not result.get("isError") and (result.get("structuredContent") or {}).get("exitCode") == 0:
            print("the cluster's Deployments are available", flush=True)
            return
        time.sleep(10)
    raise SystemExit(f"the cluster's Deployments were not available within {minutes} minutes")


def run_recipe(label, source, inputs, minutes):
    reply = raw("tools/call", {"name": "run_host_local_recipe", "arguments": {"source": source, "inputs": inputs}},
                meta=TASKS)
    started = reply.get("result") or {}
    run = (started.get("structuredContent") or {}).get("runId")
    if not run or started.get("isError"):
        raise SystemExit(f"the {label} recipe could not start: {json.dumps(reply)[:600]}")
    t0 = time.monotonic()
    while time.monotonic() - t0 < minutes * 60:
        state = call("get_host_plan_run", {"runId": run})
        if state["status"] == "completed":
            nodes = {k: v.get("status") for k, v in state.get("nodes", {}).items()}
            print(f"{label}: run {run} completed in {round(time.monotonic() - t0)} s {nodes}", flush=True)
            return run, state
        if state["status"] in ("failed", "cancelled"):
            print(json.dumps(state, indent=1)[:4000], flush=True)
            raise SystemExit(f"the {label} recipe {state['status']}")
        time.sleep(3)
    raise SystemExit(f"the {label} recipe exceeded its {minutes}-minute wait")


def push(path, content):
    call("ensure_host_file", {"path": path, "content": content, "mode": 420, "scope": "user"})


def minimal_site():
    source = f"{GUEST_CONFIG}/minimal-site.recipe.yaml"
    push(source, MINIMAL.read_text())
    root = CONFIG / "context"
    files = {str(p.relative_to(root)): p.read_text() for p in sorted(root.rglob("*")) if p.is_file()}
    return run_recipe("minimal site", source, {
        "hostname": "minimal.opute.io", "tunnelName": "minimal-opute-io", "name": CLUSTER,
        "guestImage": "parity-noble", "contextDir": f"/home/opute/.opute/build-contexts/{CLUSTER}-v1",
        "contextFiles": files, "siteVersion": "v1", "runNonce": f"restore-{time.time_ns()}"}, 20)


def docs_site(digest, sha):
    """Deploy one site image, then wait until the public install page names its commit's catalog version."""
    if not digest.startswith("sha256:") or len(digest) != 71:
        raise SystemExit("refused: the image digest must be sha256:<64 hex>")
    source = f"{GUEST_CONFIG}/opute-io-site.recipe.yaml"
    push(source, DOCS.read_text())
    # Every deploy is a new siteVersion, so going back to an earlier image moves the tunnel too (recipe 1.1.0).
    site_version = f"{digest.split(':', 1)[1][:12]}-{time.strftime('%Y%m%dt%H%M%S', time.gmtime())}"
    retired = (deployed() or {}).get("siteVersion", "none")
    run, state = run_recipe("www.opute.io", source, {
        "clusterUri": f"cluster:local:{CLUSTER}", "image": f"{IMAGE}@{digest}",
        "siteVersion": site_version, "retiredVersion": retired}, 25)
    catalog = catalog_version(sha)
    for _ in range(30):
        if served_version() == catalog:
            break
        time.sleep(5)
    else:
        raise SystemExit(f"www.opute.io serves {served_version()}, not the deployed {catalog}")
    DEPLOYED.write_text(json.dumps({"digest": digest, "main": sha, "catalog": catalog, "siteVersion": site_version,
                                    "run": run, "at": e2elib.now()}, indent=1) + "\n")
    print(f"www.opute.io serves the {catalog} catalog", flush=True)
    return run, state


def catalog_version(sha):
    url = f"https://raw.githubusercontent.com/wunderous/host-agents/{sha}/site/context/release-catalog.json"
    with urlopen(url, timeout=30) as reply:
        return json.load(reply)["packageVersion"]


def served_version():
    """The one release the public install page pins (@opute/host-agent@X; None when it pins none or several)."""
    import re
    _, body = page(f"https://www.opute.io/docs/install/?v={time.time_ns()}")
    found = set(re.findall(r"@opute/host-agent@(\d+\.\d+\.\d+)", body))
    return found.pop() if len(found) == 1 else None


def latest_digest(sha=None):
    """The digest of the site image built from host-agents main (its sha-<commit> tag), by anonymous pull."""
    if sha is None:
        sha = subprocess.run(["git", "ls-remote", "https://github.com/wunderous/host-agents", "refs/heads/main"],
                             capture_output=True, text=True, check=True).stdout.split()[0]
    scope = "repository:wunderous/host-agent-www:pull"
    with urlopen(f"https://ghcr.io/token?scope={scope}", timeout=30) as reply:
        pull = json.load(reply)["token"]
    accept = ", ".join(["application/vnd.oci.image.index.v1+json", "application/vnd.oci.image.manifest.v1+json",
                        "application/vnd.docker.distribution.manifest.v2+json"])
    request = Request(f"https://ghcr.io/v2/wunderous/host-agent-www/manifests/sha-{sha}", method="HEAD",
                      headers={"Authorization": f"Bearer {pull}", "Accept": accept})
    try:
        with urlopen(request, timeout=30) as reply:
            return sha, reply.headers["Docker-Content-Digest"]
    except HTTPError as error:
        if error.code == 404:  # the image of a just-merged commit is still building
            return sha, None
        raise


def deployed():
    """What the last deploy recorded: digest, host-agents commit, catalog version and siteVersion."""
    try:
        return json.loads(DEPLOYED.read_text())
    except (FileNotFoundError, ValueError):
        return None


def page(url):
    # Cloudflare refuses Python-urllib's agent
    with urlopen(Request(url, headers={"User-Agent": "curl/8"}), timeout=30) as reply:
        return reply.status, reply.read().decode()


def mail_records():
    def dig(kind, server):
        out = subprocess.run(["dig", "+short", f"@{server}", "opute.io", kind], capture_output=True, text=True)
        return sorted(out.stdout.split("\n"))
    return {s: {"MX": dig("MX", s), "TXT": dig("TXT", s)} for s in ("1.1.1.1", "8.8.8.8")}


def check_sites(mail_before=None):
    """All three URLs answer 200 with their sites; the apex mail records are what they were."""
    status, body = page("https://minimal.opute.io/")
    expected = (CONFIG / "context/index.html").read_text().split("</body>", 1)[0].strip()
    if status != 200 or expected not in body:
        raise SystemExit("minimal.opute.io does not serve the owner's page")
    for url in ("https://www.opute.io/", "https://opute.io/"):
        status, body = page(url)
        if status != 200 or "Opute" not in body:
            raise SystemExit(f"{url} answered {status} without the site")
    if mail_before is not None and mail_records() != mail_before:
        raise SystemExit("the apex mail records changed")
    print("minimal.opute.io, www.opute.io and opute.io answer 200; apex mail records unchanged", flush=True)
