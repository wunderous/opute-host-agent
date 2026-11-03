"""M9 provider evidence is re-derived from receipts and fails closed."""
import gzip
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from parity import provider_verify

GO, RUST, CORE = "a" * 64, "b" * 64, "c" * 64


def recording(sha, steps):
    return {"tools": ["t"], "stdout": "", "stderr": [], "executableSha256": sha, "steps": steps}


def step(name, response, requests=None):
    out = {"as": name, "response": response}
    if requests is not None:
        out["fakeRequests"] = requests
    return out


class ProviderVerifyTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def pin(self, name, data):
        path = self.root / name
        raw = json.dumps(data).encode()
        path.write_bytes(gzip.compress(raw) if name.endswith(".gz") else raw)
        return {"path": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

    def manifest(self, rust_steps, order_only=None, right_provider=RUST, release=RUST):
        go_steps = [step("call x", {"r": 1}, [{"a": 1}, {"b": 2}]), step("call y", {"r": 2})]
        swap = {"provenance": {"left": {"binarySha256": CORE}, "changedDuringRun": False,
                               "right": {"binarySha256": CORE,
                                         "providerSha256s": {"opute-provider-k3s": right_provider}}},
                "items": {"provider-real.k3s": {"divergences": [], "failingIterations": [], "iterations": 1}}}
        return {
            "rustCandidate": {"binarySha256": CORE},
            "providerEvidence": {
                "releases": self.pin("releases.json", {"rust": {"text": f"{release}  opute-provider-k3s-linux-x64\n"}}),
                "providers": {"k3s": {
                    "goSha256": GO, "rustSha256": RUST, "artifact": "opute-provider-k3s-linux-x64",
                    "wire": [{"calls": "k3s.json", "orderOnly": order_only or {},
                              "go": self.pin("go.json.gz", recording(GO, go_steps)),
                              "goAgain": self.pin("go2.json.gz", recording(GO, go_steps)),
                              "rust": self.pin("rust.json.gz", recording(RUST, rust_steps))}],
                    "swap": {**self.pin("swap.json", swap), "scenarios": ["provider-real.k3s"]},
                }},
            },
        }

    def run_check(self, manifest):
        problems = []
        return provider_verify.check(self.root, manifest, ["k3s"], problems.append), problems

    def test_equal_evidence_passes(self):
        rust = [step("call x", {"r": 1}, [{"a": 1}, {"b": 2}]), step("call y", {"r": 2})]
        out, problems = self.run_check(self.manifest(rust))
        self.assertEqual(set(out.values()), {"pass"}, problems)

    def test_a_differing_step_fails(self):
        rust = [step("call x", {"r": 1}, [{"a": 1}, {"b": 2}]), step("call y", {"r": 3})]
        out, _ = self.run_check(self.manifest(rust))
        self.assertEqual(out["k3s:wire"], "fail")

    def test_order_only_passes_only_where_declared(self):
        rust = [step("call x", {"r": 1}, [{"b": 2}, {"a": 1}]), step("call y", {"r": 2})]
        self.assertEqual(self.run_check(self.manifest(rust))[0]["k3s:wire"], "fail")
        out, _ = self.run_check(self.manifest(rust, order_only={"call x": "Go ranges over a map"}))
        self.assertEqual(out["k3s:wire"], "pass")

    def test_another_executable_fails(self):
        rust = [step("call x", {"r": 1}, [{"a": 1}, {"b": 2}]), step("call y", {"r": 2})]
        out, _ = self.run_check(self.manifest(rust, right_provider=GO, release=GO))
        self.assertEqual((out["k3s:swap"], out["k3s:release"]), ("fail", "fail"))

    def test_a_changed_receipt_or_missing_evidence_fails_closed(self):
        rust = [step("call x", {"r": 1}, [{"a": 1}, {"b": 2}]), step("call y", {"r": 2})]
        manifest = self.manifest(rust)
        (self.root / "rust.json.gz").write_bytes(b"changed")
        self.assertEqual(self.run_check(manifest)[0]["k3s:wire"], "unverified")
        out = provider_verify.check(self.root, {}, ["k3s"], lambda _: None)
        self.assertEqual(out, {"k3s:evidence": "unverified"})


if __name__ == "__main__":
    unittest.main()
