"""Read-only native admission smoke test with isolated state and no shims.

This is not a disposable-runtime/T2 substitute. It reads the actual host
and a test-owned file, using the installed product's existing typed tools.
"""
import argparse
import hashlib
import json
import platform
from pathlib import Path

from parity import agent, runner


def run(impl: agent.Impl, env_file: Path, memory_bytes: int) -> dict:
    results = {}
    for policy in ["default", "advisory"]:
        sandbox = agent.Sandbox(impl.label, "native-admission-" + policy, None)
        path = sandbox.root / "home" / "native-admission.txt"
        contents = b"native admission control\n"
        path.write_bytes(contents)
        env = dict(runner.PROFILES["standalone"])
        if policy == "advisory":
            env["OPUTE_HOST_RESOURCE_ENFORCEMENT"] = "advisory"
        server = agent.Server(impl, sandbox, ["serve", "--env-file", str(env_file)], env)
        try:
            ready = server.wait_ready()
            assert ready.get("ready") is True, ready
            capacity = agent.mcp_call(sandbox, "tools/call", {
                "name": "get_host_capacity", "arguments": {}}, name="get_host_capacity")
            observed = capacity["body"]["result"]["structuredContent"]
            assert observed["effectiveLimits"]["memoryBytes"] == memory_bytes
            inspected = agent.mcp_call(sandbox, "tools/call", {
                "name": "inspect_host_file", "arguments": {"path": str(path)}}, name="inspect_host_file")
            result = inspected["body"]["result"]
            projected = result["structuredContent"]
            refused = policy == "default" and observed["enforcement"] != "enforced"
            if refused:
                assert result["isError"] is True
                assert projected["owner"] == "admission"
                assert projected["code"] == "host_resource_enforcement_unknown"
            else:
                assert result.get("isError", False) is False
                assert projected["exists"] is True
                assert projected["regular"] is True
                assert projected["sha256"] == "sha256:" + hashlib.sha256(contents).hexdigest()
            assert path.read_bytes() == contents
            results[policy] = {"ready": ready, "capacity": capacity, "inspection": inspected,
                               "expectedRefusal": refused, "fileUnchanged": True}
        finally:
            server.stop()
    return {"binarySha256": impl.sha256(), "policies": results}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--go", required=True, type=Path)
    parser.add_argument("--rust", required=True, type=Path)
    parser.add_argument("--env-file", required=True, type=Path)
    parser.add_argument("--memory-bytes", required=True, type=int)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    results = {label: run(agent.Impl(label, binary.resolve()), args.env_file.resolve(), args.memory_bytes)
               for label, binary in [("go", args.go), ("rust", args.rust)]}
    output = {"scope": "native read-only admission; not T2 external-effect evidence",
              "platform": platform.platform(), "harnessSha256": runner.harness_sha256(),
              "driverSha256": runner.sha256_file(Path(__file__)), "expectedMemoryBytes": args.memory_bytes,
              "sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
              "results": results}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(output, indent=2) + "\n")
    print("Native read-only admission: Go and Rust default/advisory checks passed")


if __name__ == "__main__":
    main()
