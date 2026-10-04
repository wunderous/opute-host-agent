"""M5 state migration from the captured v0.2.2 release schema and Go's
pre-addition column contract. Each binary upgrades its own identical fixture;
all state table rows and SQL schema are compared, with preserved seed checks.
"""
from __future__ import annotations
import json
import shutil
import sqlite3
from pathlib import Path
from . import agent, canon, runner

RELEASE_FIXTURE = runner.FIXTURE_DIR / "m5/v0.2.2-state-schema.json"
CASES = ("released-v0.2.2", "pre-column-additions")
TIME = "2026-10-04T00:00:00Z"
SEEDS = {
 "operations": [{"operation_id": "op-"+status, "tool_name": "fixture", "status": status,
                 "description": "preserved", "created_at": TIME, "updated_at": TIME,
                 "result_json": '{"redacted":true}', "error_message": None, "task_snapshot_json": ""}
                for status in ["completed", "failed", "cancelled", "working"]],
 "plan_runs": [{"run_id": "run-"+status, "plan_id": "plan-"+status, "generation": 2,
                "idempotency_key": "fixture", "document_hash": "sha256:fixture", "catalog_revision": "sha256:catalog",
                "status": status, "plan_json": '{}', "recipe_json": '{}', "state_json": '{}',
                "created_at": TIME, "updated_at": TIME, "error_message": None}
               for status in ["completed", "running"]],
 "provider_generations": [{"generation_id":"g1", "provider_id":"fixture", "provider_version":"1",
                          "manifest_hash":"sha256:fixture", "endpoint":"http://127.0.0.1:1/mcp",
                          "descriptor_json":"{}", "manifest_json":"{}", "catalog_revision":"sha256:catalog",
                          "status":"candidate", "created_at":TIME, "active_at":""}],
 "capability_invocations": [{"invocation_id":"i1", "operation_id":"fixture.op", "capability_version":1,
                            "catalog_revision":"sha256:catalog", "generation_id":None, "authorization":"admitted",
                            "arguments_json":"{}", "binding_json":"{}", "result_json":"{}",
                            "observation_json":"{}", "terminal_status":"completed", "created_at":TIME}],
 "resource_registry": [{"uri":f"{kind}:local:fixture", "resource_type":kind, "tenant_id":"local",
                        "resource_id":"fixture", "coordinates_json":'{"instanceName":"fixture"}',
                        "status":"active", "created_at":TIME, "updated_at":TIME} for kind in ["vm","container"]],
 "active_runtimes": [{"capability":"llm-serving", "serving_contract":"openai-chat.v1", "runtime":"ollama",
                      "recipe_id":"r1", "recipe_version":"1", "recipe_hash":"sha256:fixture", "run_id":"run-completed",
                      "input_bindings_json":"{}", "observation_json":"{}", "activated_at":TIME}],
 "active_capabilities": [{"capability":"llm-serving", "serving_contract":"openai-chat.v1", "provider":"ollama",
                          "recipe_id":"r1", "recipe_version":"1", "recipe_hash":"sha256:fixture", "run_id":"run-completed",
                          "input_bindings_json":"{}", "observation_json":"{}", "activated_at":TIME}],
}
REMOVED = {"operations":["task_snapshot_json"], "plan_runs":["recipe_json"],
           "provider_generations":["descriptor_json","manifest_json"], "capability_invocations":["binding_json"],
           "active_runtimes":["recipe_id","recipe_version","recipe_hash","run_id","input_bindings_json","observation_json","activated_at"]}
COMPARE_SPEC = {"masks": [{"path":["rows","tables",table,key,"updated_at"],"type":"string",
                           "reason":"startup marks interrupted records unknown using SQLite datetime(now)"}
                          for table,key in [("operations","op-working"),("plan_runs","run-running")]]}


def seed(state_dir: Path, case: str) -> dict:
    release=json.loads(RELEASE_FIXTURE.read_text())
    conn=sqlite3.connect(state_dir/"state.db")
    try:
        for obj in release["schema"]["objects"]:
            if obj["type"]=="table": conn.execute(obj["sql"])
        for table, rows in SEEDS.items():
            for row in rows:
                columns=','.join('"'+key+'"' for key in row)
                conn.execute(f'INSERT INTO "{table}" ({columns}) VALUES ({",".join("?" for _ in row)})',list(row.values()))
        if case=="pre-column-additions":
            for table, columns in REMOVED.items():
                for column in columns: conn.execute(f'ALTER TABLE "{table}" DROP COLUMN "{column}"')
            conn.execute('DROP TABLE active_capabilities')
            conn.execute('DROP TABLE resource_registry')
        conn.commit()
        return {"case":case, "releaseFixtureSha256":runner.sha256_file(RELEASE_FIXTURE)}
    finally:
        conn.close()


def preservation_failures(observation: dict, case: str) -> list[str]:
    problems=[]
    tables=observation["rows"]["tables"]
    expected_tables=set(SEEDS)
    if set(tables)!=expected_tables: problems.append("state table set changed")
    for table, seed_rows in SEEDS.items():
        if case=="pre-column-additions" and table=="resource_registry":
            if tables.get(table)!={}: problems.append("unexpected legacy resources")
            continue
        for original in seed_rows:
            key_name={"operations":"operation_id","plan_runs":"run_id","provider_generations":"generation_id",
                      "capability_invocations":"invocation_id","resource_registry":"uri",
                      "active_runtimes":"capability","active_capabilities":"capability"}[table]
            key=original[key_name]
            row=tables.get(table,{}).get(key)
            if not isinstance(row,dict): problems.append(f"missing {table}/{key}"); continue
            for column,value in original.items():
                if case=="pre-column-additions" and column in REMOVED.get(table,[]): continue
                if case=="pre-column-additions" and table=="active_capabilities" and column not in ["capability","serving_contract","provider"]: continue
                if column=="status" and value in ["working","running"]: value="unknown"
                if column=="updated_at" and original.get("status") in ["working","running"]: continue
                if row.get(column)!=value: problems.append(f"changed {table}/{key}/{column}")
        if len(tables.get(table,{}))!=len(seed_rows): problems.append(f"changed {table} row count")
    return problems


def run_one(impl: agent.Impl, run_id: str, case: str) -> dict:
    sandbox=agent.Sandbox(impl.label,run_id,None)
    server=None
    try:
        fixture=seed(sandbox.root/"state",case)
        server=agent.Server(impl,sandbox,["serve","--mode=standalone"],runner.PROFILES["standalone"])
        ready=server.wait_ready(30)
        if not ready.get("ready"): raise RuntimeError(f"migration startup failed: {ready}")
        stop=server.stop()
        if stop!={"exit":0,"killed":False}: raise RuntimeError(f"migration stop failed: {stop}")
        return {"fixture":fixture,"schema":sandbox.sqlite_schemas()["state/state.db"],
                "rows":sandbox.sqlite_rows()["state/state.db"]}
    except Exception as exc:
        return {"error":f"{type(exc).__name__}: {exc}"}
    finally:
        if server: server.stop()
        shutil.rmtree(sandbox.root,ignore_errors=True)


def compare_once(go: agent.Impl, rust: agent.Impl, run_id: str, case: str) -> dict:
    observations={impl.label:run_one(impl,run_id+"-"+impl.label,case) for impl in [go,rust]}
    normalized=[]; failures=[]
    for label, observation in observations.items():
        if "error" in observation: failures.append(label+": "+observation["error"]); continue
        failures += [label+": "+p for p in preservation_failures(observation,case)]
        norm, violations=canon.normalize(observation,COMPARE_SPEC,{})
        if violations: failures.append(label+": invalid typed mask")
        normalized.append(norm)
    diff=canon.diff(*normalized) if len(normalized)==2 else []
    return {"diff":diff,"failures":failures,"raw":observations}


def run(go: agent.Impl, rust: agent.Impl, out: Path, repeats: int=3) -> dict:
    out.mkdir(parents=True,exist_ok=True)
    outcomes={f"{case}.{i}":compare_once(go,rust,f"migration-{case}-{i}",case)
              for case in CASES for i in range(repeats)}
    failed=[key for key,result in outcomes.items() if result["diff"] or result["failures"]]
    (out/"outcomes.json").write_text(json.dumps(outcomes,indent=2,sort_keys=True))
    summary={"repeats":repeats,"total":len(outcomes),"cases":list(CASES),"passed":len(outcomes)-len(failed),"failed":len(failed),
             "failedCases":failed,"files":{"outcomes.json":runner.sha256_file(out/"outcomes.json")},
             "provenance":{"sourceLock":json.loads((runner.REPO_ROOT/"baseline/source-lock.json").read_text()),
                           "goBinarySha256":go.sha256(),"rustBinarySha256":rust.sha256(),
                           "driverSha256":runner.sha256_file(Path(__file__)),"harnessSha256":runner.harness_sha256(),
                           "releaseFixtureSha256":runner.sha256_file(RELEASE_FIXTURE)}}
    (out/"summary.json").write_text(json.dumps(summary,indent=2,sort_keys=True))
    return summary
