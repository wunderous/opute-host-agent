"""Run the go-sdk OAuth client against the Rust agent (secure-oauth-issuance 5.3).

  python3 tools/parity/oauth-sdk-client/run.py <rust-binary> <out-dir>

Builds the client offline from the pinned module cache, then for each case
starts a fresh Rust agent in its own sandbox and runs the client: `approve`
must complete an authenticated MCP session, and `deny` (the negative control)
must not. Writes receipt.json with the binary, client source and go.sum
hashes and each case's client report and exit code.
"""
import hashlib, json, os, socket, subprocess, sys, tempfile, time, urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
GO = ROOT / ".parity/setup/go/bin/go"
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def case(agent, client, deny):
    with tempfile.TemporaryDirectory(prefix="oauth-sdk-") as sandbox:
        sandbox = Path(sandbox)
        for name in ("state", "instance", "home"):
            (sandbox / name).mkdir()
        port = free_port()
        env = {"PATH": "/usr/bin:/bin", "HOME": str(sandbox / "home"), "OPUTE_REMOTE_AGENT_ID": "oauth-sdk-parity",
               "OPUTE_AGENT_MODE": "standalone", "OPUTE_STANDALONE_STATE_DIR": str(sandbox / "state"),
               "OPUTE_HOST_AGENT_INSTANCE_ROOT": str(sandbox / "instance"), "HOST_MCP_BIND_HOST": "127.0.0.1",
               "HOST_MCP_PORT": str(port), "MCP_AUTH_TOKEN": "parity-bootstrap-token"}
        server = subprocess.Popen([agent, "serve", "--mode=standalone"], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(100):
                try:
                    urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1)
                    break
                except OSError:
                    time.sleep(0.1)
            run = subprocess.run([client, "-endpoint", f"http://127.0.0.1:{port}/mcp", "-agent", agent,
                                  "-state-dir", str(sandbox / "state")] + (["-deny"] if deny else []),
                                 capture_output=True, text=True, timeout=120)
        finally:
            server.terminate()
            server.wait(timeout=30)
        report = json.loads(run.stdout)
        # The issuer and session error embed this run's port.
        report["redirect"]["iss"] = report["redirect"]["iss"].replace(str(port), "${PORT}")
        if "error" in report["session"]:
            report["session"]["error"] = report["session"]["error"].replace(str(port), "${PORT}")
        return {"exit": run.returncode, "report": report}


def main():
    agent, out = Path(sys.argv[1]).resolve(), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    client = out / "oauth-sdk-client"
    subprocess.run([GO, "build", "-o", client, "."], cwd=HERE, check=True,
                   env={**os.environ, "GOPROXY": "off", "GOFLAGS": "-mod=readonly"})
    receipt = {"rustBinarySha256": sha(agent), "clientSource": {p: sha(HERE / p) for p in ("main.go", "go.mod", "go.sum")},
               "goSdk": "github.com/modelcontextprotocol/go-sdk v1.7.0",
               "approve": case(agent, client, False), "deny": case(agent, client, True)}
    client.unlink()
    approve, deny = receipt["approve"], receipt["deny"]
    receipt["pass"] = (approve["exit"] == 0 and approve["report"]["session"]["connected"]
                       and approve["report"]["session"]["toolCount"] > 0
                       and deny["exit"] != 0 and not deny["report"]["session"]["connected"]
                       and deny["report"]["redirect"].get("error") == "access_denied")
    (out / "receipt.json").write_text(json.dumps(receipt, indent=1, sort_keys=True) + "\n")
    print("go-sdk OAuth client:", "pass" if receipt["pass"] else "FAIL")
    sys.exit(0 if receipt["pass"] else 1)


if __name__ == "__main__":
    main()
