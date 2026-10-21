#!/usr/bin/env python3
"""Generate the M8e postgres-domain T1 matrix: scenarios/postgres-domain.json
and its shim fixtures (fixtures/shims/postgres-domain-*.json).

Run from tools/parity: python3 postgres_domain_scenarios.py

Every kubectl call of the domain goes through the pinned Go k3s provider
(installed exactly as provider-real.k3s installs it), whose `incus exec k1 --
k3s kubectl ...` calls the recording shim answers; the relay's loopback
forward is the shimmed `incus config device`. The trace holds the provider
argv and the stdin hash on both sides, so the manifests the agent applies
(the CNPG HelmChart and Cluster, the consumer Secret with the database URLs)
and the pgpass lines it feeds psql are compared byte for byte.

- **sqlite:** databases under an explicit root and under the default one
  (the instance root), created, found, refused and removed. The file's
  SQLite library stamp is the engine's (Go links SQLite 3.53, Rust 3.46), so
  the comparison reads the database itself (its schema and journal mode)
  and the root's tree by type and mode.
- **sql connector:** the reference-counted host TCP relay, its listen
  address as Go's listener reports it, and each refusal.
- **platform service:** a ready CNPG estate (status, reconcile's steady
  path, consumer Secret and restarts, storage admission), a fresh cluster
  (the ordered install: HelmChart, CRD and webhook wait, Cluster, database
  creation), and removal under each retention policy.
- **relay:** the authenticated relay, persisted, restored after a restart
  on the domain's first call, and released only with its token.
- **standalone:** install, status, SQL and delete in a namespace.

Ports a listener picks (`listenPort: 0`) and relay expiry times are this
run's own and masked by type.
"""
import base64
import copy
import json
import sys
from pathlib import Path

import cluster_domain_scenarios as cds

HERE = Path(__file__).resolve().parent
K1 = cds.K1
call, task_call, flat, rule, kubectl, meta, items = (cds.call, cds.task_call, cds.flat, cds.rule, cds.kubectl,
                                                     cds.meta, cds.items)
STOP = {"as": "stop", "stop": {}}
SQLITE_ROOT = "${SANDBOX}/home/sqlite"


def serve(as_="start", **env):
    s = copy.deepcopy(cds.SERVE)
    s["as"] = as_
    s["serve"]["env"].update({"OPUTE_HOST_AGENT_SQLITE_ROOT": SQLITE_ROOT, **env})
    return s


TS = "wall-clock timestamp of this side's run"
PORT = "a port the listener picked for this run"
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")]
RESULT = ["body", "result", "structuredContent"]
TASK_RESULT = ["body", "result", "result", "structuredContent"]


def port_masks(label, *path, task=False):
    """The listen port a listener picked, in the result and its text."""
    base = ["steps", label, *(TASK_RESULT if task else RESULT)]
    text = ["steps", label, "body", "result", *(["result"] if task else []), "content", 0, "text"]
    return [{"path": base + list(path), "type": "int", "reason": PORT},
            {"path": text, "type": "string", "reason": PORT + " (named in the text)", "optional": True}]


def scenario(id_, steps, anchors, fixture, masks=(), sets=(), cluster=False, collect=()):
    compare = copy.deepcopy(cds.REAL_K3S["compare"]) if cluster else {"masks": []}
    have = {json.dumps(m["path"]) for m in compare["masks"]}
    compare["masks"] += [m for m in MASKS + list(masks) if json.dumps(m["path"]) not in have]
    compare["sets"] = compare.get("sets", []) + [list(p) for p in sets]
    compare.pop("divergences", None)
    if cluster:
        compare["divergences"] = ["D19.neutral-host-tool-names"]
    return {"id": "postgres-domain." + id_, "surface": "postgres-domain", "owner": "postgres",
            "anchors": anchors, "fixture": fixture, "collect": ["trace", "orphans", *collect],
            "steps": flat(steps), "compare": compare}


# --- the CNPG estate ----------------------------------------------------------------------

NS, CLUSTER, POD = "platform", "pg", "pg-1"
USER, PASSWORD = "app", "s3cr3t/Pass:word+@x"
CLUSTER_IP = "10.43.12.34"
DATABASES = ["app", "ledger"]
KEYS = {"app": "DATABASE_URL", "ledger": "LEDGER_URL"}
LABEL = "opute.io/pg-consumer"
SERVICE = {"uri": K1, "clusterName": CLUSTER, "namespace": NS, "databases": DATABASES,
           "consumerSecretName": "pg-consumer", "consumerSecretLabel": LABEL, "serviceOwner": "app",
           "servicePartOf": "opute-platform", "consumerDatabaseKeys": KEYS}
VALUES = """monitoring:
  enabled: false
config:
  maxConcurrentReconciles: 1
webhook:
  livenessProbe:
    initialDelaySeconds: 30
  readinessProbe:
    initialDelaySeconds: 30
  startupProbe:
    failureThreshold: 24
    periodSeconds: 5
resources:
  requests:
    cpu: 25m
    memory: 32Mi
  limits:
    cpu: 500m
    memory: 128Mi
"""


def b64(text):
    return base64.b64encode(text.encode()).decode()


def shell_quote(value):
    return "'" + value.replace("'", "'\\''") + "'"


def service_script(database, sql):
    """postgresqlServiceSQLScript, encoded as kubectlShellScriptArgument."""
    script = (
        'set -eu\n'
        'pgpass_dir="${TMPDIR:-/controller/tmp}"\n'
        'if [ ! -d "$pgpass_dir" ] || [ ! -w "$pgpass_dir" ]; then\n'
        '  pgpass_dir="/tmp"\n'
        'fi\n'
        'pgpass="$(mktemp "$pgpass_dir/opute-pgpass.XXXXXX")"\n'
        "trap 'rm -f \"$pgpass\"' EXIT\n"
        'cat >"$pgpass"\n'
        'chmod 600 "$pgpass"\n'
        f'PGPASSFILE="$pgpass" psql -h {shell_quote(CLUSTER + "-rw." + NS + ".svc")} -p 5432 '
        f'-U {shell_quote(USER)} -d {shell_quote(database)} -v ON_ERROR_STOP=1 -Atqc {shell_quote(sql)}\n')
    return "eval \"$(printf '%s' '" + b64(script) + "' | base64 -d)\""


def exec_sql(database, sql, stdout=None, **extra):
    return kubectl(["exec", "-i", POD, "-n", NS, "--", "sh", "-ceu", service_script(database, sql)],
                   stdout, **extra)


def check(database):
    return f"SELECT 1 FROM pg_database WHERE datname = {shell_quote(database)}"


def cluster_object(size="10Gi", ready=True, storage_class="local-path", profile="constrained-2GiB-v3"):
    storage = {"size": size}
    if storage_class:
        storage["storageClass"] = storage_class
    return {"apiVersion": "postgresql.cnpg.io/v1", "kind": "Cluster",
            "metadata": {**meta(CLUSTER, NS, "c-1"), "annotations": {"host-agent.io/resource-profile": profile}},
            "spec": {"instances": 1, "storage": storage},
            "status": {"phase": "Cluster in healthy state" if ready else "Setting up primary",
                       "readyInstances": 1 if ready else 0}}


PRIMARY = items({"metadata": meta(POD, NS, "p-1"), "status": {
    "phase": "Running", "conditions": [{"type": "Ready", "status": "True"}],
    "containerStatuses": [{"name": "postgres", "ready": True}]}})
OPERATOR = items({"metadata": meta("cloudnativepg", "cnpg-system", "d-cnpg"),
                  "spec": {"replicas": 1}, "status": {"availableReplicas": 1}})
APP_SECRET = {"metadata": meta(CLUSTER + "-app", NS), "data": {"username": b64(USER), "password": b64(PASSWORD)}}
CONSUMER_SECRET = {"metadata": meta("pg-consumer", NS), "data": {"DATABASE_URL": "eA==", "LEDGER_URL": "eQ=="}}
CRD = "customresourcedefinition.apiextensions.k8s.io/clusters.postgresql.cnpg.io\n"
DEVICE_SHOW = ("root:\n  path: /\n  pool: default\n  type: disk\n"
               f"opute-pg-rw:\n  connect: tcp:{CLUSTER_IP}:5432\n  listen: tcp:127.0.0.1:25432\n  type: proxy\n")
STANDALONE = items({"metadata": meta("postgres-1", "opute-local-db"), "status": {"phase": "Running"}})


def get(*args):
    return ["get", *args, "-o", "json"]


def estate(operator=True, cluster=True, missing=()):
    """A CNPG estate. operator: the CRD, the webhook and the operator
    Deployment exist from the start; otherwise the first `apply -f -` (the
    HelmChart) installs them. cluster: the tenant Cluster is healthy from the
    start; otherwise the next apply (the Cluster) creates it. missing: the
    databases whose check is empty until their CREATE DATABASE runs."""
    op_ready = None if operator else "markers/operator"
    cl_ready = None if cluster else "markers/cluster"

    def when(marker, **extra):
        return {"ifExists": marker, **extra} if marker else extra

    rules = []
    if not operator:
        rules += [
            kubectl(["get", "crd", "clusters.postgresql.cnpg.io", "--ignore-not-found=true", "-o", "name"], "",
                    unlessExists=op_ready),
            kubectl(["apply", "-f", "-"], "helmchart.helm.cattle.io/cloudnativepg created\n",
                    unlessExists=op_ready, creates=[op_ready]),
        ]
    if not cluster:
        rules += [
            kubectl(get("cluster.postgresql.cnpg.io", CLUSTER, "-n", NS), exit_=1, unlessExists=cl_ready,
                    stderr=f'Error from server (NotFound): clusters.postgresql.cnpg.io "{CLUSTER}" not found\n'),
            kubectl(["apply", "-f", "-"], f"cluster.postgresql.cnpg.io/{CLUSTER} created\n",
                    unlessExists=cl_ready, creates=[cl_ready]),
        ]
    for database in missing:
        created = f"markers/db-{database}"
        rules += [
            exec_sql("postgres", check(database), "", unlessExists=created),
            exec_sql("postgres", f'CREATE DATABASE "{database}"', "", creates=[created]),
            exec_sql(database, check(database), exit_=2, unlessExists=created,
                     stderr=f'psql: error: FATAL:  database "{database}" does not exist\n'),
        ]
    rules += [
        kubectl(["get", "crd", "clusters.postgresql.cnpg.io", "--ignore-not-found=true", "-o", "name"], CRD,
                **when(op_ready)),
        kubectl(["get", "crd", "clusters.postgresql.cnpg.io"], "NAME   CREATED AT\n", **when(op_ready)),
        kubectl(get("deployments", "-n", "cnpg-system", "-l", "app.kubernetes.io/name=cloudnative-pg"), OPERATOR,
                **when(op_ready)),
        kubectl(get("endpoints", "cnpg-webhook-service", "-n", "cnpg-system"),
                {"subsets": [{"addresses": [{"ip": "10.42.0.5"}]}]}, **when(op_ready)),
        kubectl(get("helmchart.helm.cattle.io", "cloudnativepg", "-n", "kube-system"),
                {"metadata": meta("cloudnativepg", "kube-system"), "spec": {"valuesContent": VALUES}},
                **when(op_ready)),
        kubectl(get("cluster.postgresql.cnpg.io", CLUSTER, "-n", NS), cluster_object(), **when(cl_ready)),
        kubectl(get("service", CLUSTER + "-rw", "-n", NS), {"spec": {"clusterIP": CLUSTER_IP}}, **when(cl_ready)),
        kubectl(get("endpoints", CLUSTER + "-rw", "-n", NS), {"subsets": [{"addresses": [{"ip": "10.42.0.9"}]}]},
                **when(cl_ready)),
        kubectl(get("secret", CLUSTER + "-app", "-n", NS), APP_SECRET, **when(cl_ready)),
        kubectl(get("pods", "-n", NS, "-l", f"cnpg.io/cluster={CLUSTER},role=primary"), PRIMARY, **when(cl_ready)),
        kubectl(["exec", "-i", POD, "-n", NS, "--", "sh", "-ceu", "..."], "1\n"),
        kubectl(get("cluster.postgresql.cnpg.io", "grow", "-n", NS), {**cluster_object(storage_class=""),
                                                                     "metadata": meta("grow", NS)}),
        kubectl(get("cluster.postgresql.cnpg.io", "broken", "-n", NS), exit_=1,
                stderr="The connection to the server 127.0.0.1:6443 was refused\n"),
        kubectl(get("storageclass", "local-path"), {"metadata": meta("local-path"),
                                                    "provisioner": "rancher.io/local-path"}),
        kubectl(get("storageclass", "expandable"), {"metadata": meta("expandable"), "allowVolumeExpansion": True}),
        kubectl(["get", "namespace", NS], f"{NS}   Active   1d\n"),
        kubectl(["get", "namespace", "cnpg-system"], "cnpg-system   Active   1d\n"),
        kubectl(["delete", "secret", "pg-consumer", "-n", NS, "--ignore-not-found=true"],
                'secret "pg-consumer" deleted\n'),
        kubectl(get("secret", "pg-consumer", "-n", NS), CONSUMER_SECRET),
        kubectl(get("deployments", "-n", NS, "-l", LABEL), items({"metadata": meta("ledger-api", NS)})),
        kubectl(["rollout", "restart", "deployment", "ledger-api", "-n", NS],
                "deployment.apps/ledger-api restarted\n"),
        kubectl(["rollout", "status", "deployment", "ledger-api", "-n", NS, "--timeout=2m"],
                'deployment "ledger-api" successfully rolled out\n'),
        kubectl(["delete", "cluster.postgresql.cnpg.io", CLUSTER, "-n", NS, "--ignore-not-found=true",
                 "--wait=true"], f'cluster.postgresql.cnpg.io "{CLUSTER}" deleted\n'),
        kubectl(["delete", "secret", CLUSTER + "-app", "-n", NS, "--ignore-not-found=true"],
                f'secret "{CLUSTER}-app" deleted\n'),
        kubectl(["delete", "pvc", "-l", f"cnpg.io/cluster={CLUSTER}", "-n", NS, "--ignore-not-found=true"],
                'persistentvolumeclaim "pg-1" deleted\n'),
        # Standalone PostgreSQL in its default namespace.
        kubectl(get("cluster.postgresql.cnpg.io", "postgres", "-n", "opute-local-db"),
                {"metadata": meta("postgres", "opute-local-db"), "spec": {"instances": 1},
                 "status": {"phase": "Cluster in healthy state", "readyInstances": 1}}),
        kubectl(get("cluster.postgresql.cnpg.io", "postgres", "-n", "absent"), exit_=1,
                stderr='Error from server (NotFound): clusters.postgresql.cnpg.io "postgres" not found\n'),
        kubectl(get("pods", "-n", "opute-local-db", "-l", "cnpg.io/cluster=postgres,role=primary"), STANDALONE),
        kubectl(get("secret", "postgres-app", "-n", "opute-local-db"),
                {"data": {"username": b64("app"), "password": b64("standalone-pass")}}),
        kubectl(["exec", "-i", "postgres-1", "-n", "opute-local-db", "--", "sh", "-ceu", "..."], "42\n"),
        kubectl(["delete", "namespace", "opute-local-db", "--ignore-not-found=true"],
                'namespace "opute-local-db" deleted\n'),
    ]
    return rules


DEVICE = [
    rule(["config", "device", "show", "k1"], DEVICE_SHOW, unlessExists="markers/device-moved"),
    rule(["config", "device", "show", "k1"], "root:\n  path: /\n  type: disk\n"),
    rule(["config", "device", "remove", "k1", "opute-pg-rw"], "Device opute-pg-rw removed from k1\n"),
    rule(["config", "device", "add", "k1", "opute-pg-rw", "proxy", "..."], "Device opute-pg-rw added to k1\n"),
]


def fixture(description, **kw):
    commands = {**cds.FIXTURES["cluster-domain-estate"]["commands"],
                "incus": DEVICE + estate(**kw) + cds.estate_rules()}
    return {"description": "M8e: " + description + " The cluster-domain estate (k1 with k3s) answers the provider.",
            "commands": commands, "default": cds.DEFAULT}


FIXTURES = {
    "postgres-domain-ready": fixture(
        "a ready CNPG estate: operator, webhook, a healthy tenant Cluster 'pg' with its read/write Service, "
        "application Secret and primary, both databases present, one consumer Deployment, and an Incus "
        "proxy device already aimed at the Service; a standalone PostgreSQL in opute-local-db."),
    "postgres-domain-fresh": fixture(
        "a fresh cluster: no CNPG API until the HelmChart is applied, no tenant Cluster until it is applied, "
        "and the ledger database missing until it is created.", operator=False, cluster=False,
        missing=("ledger",)),
}

# --- scenarios ----------------------------------------------------------------------------

PG = "internal/domain/postgres/"
SQLITE = [PG + "sqlite_database.go", "internal/tools/dispatch_postgres.go", "internal/config/config.go"]
CONNECTOR = [PG + "sql_connector.go", "internal/tcprelay/tcprelay.go", "internal/tools/dispatch_unassigned.go"]
PLATFORM = [PG + "platform_postgres.go", PG + "platform_postgres_storage.go", PG + "generic_postgresql_service.go",
            "internal/tools/dispatch_postgres.go"] + cds.DOMAIN
RELAY = [PG + "platform_postgres_relay.go", "internal/tools/dispatch_postgres.go"]
STANDALONE_ANCHORS = [PG + "standalone_postgres.go", "internal/tools/dispatch_unassigned.go"] + cds.DOMAIN
TOKEN = "relay-token-0123456789abcdef0123456789"
OTHER_TOKEN = "other-token-0123456789abcdef0123456789"
DB = SQLITE_ROOT + "/consumer-a/main.sqlite"


def sqlite(as_, tool, consumer="consumer-a", name="main", **extra):
    return call(as_, tool, {"consumerId": consumer, "databaseName": name, **extra})


def journal(as_, path=DB):
    return {"as": as_, "query": {"db": path, "sql": {"journal": "PRAGMA journal_mode",
                                                      "tables": "SELECT name FROM sqlite_master ORDER BY name"}}}


def relay(session="s1", token=TOKEN, **extra):
    return {"sessionId": session, "relayToken": token, **extra}


def service(**extra):
    return {**SERVICE, **extra}


RELAY_DIR = "${SANDBOX}/relays"
RELAY_FILE = RELAY_DIR + "/postgresql-service-relays/e8bc163c82eee18733288c7d4ac636db3a6deb013ef2d37b68322be20edc45cc.json"  # sha256("s1")

cases = [
    # SQLite under an explicit root: created once, found, refused names,
    # removed only with confirm, and gone afterwards.
    scenario("sqlite.lifecycle", [
        serve(),
        sqlite("ensure", "ensure_sqlite_database"),
        sqlite("again", "ensure_sqlite_database"),
        journal("journal"),
        sqlite("status", "get_sqlite_database_status"),
        sqlite("statusOther", "get_sqlite_database_status", name="other"),
        sqlite("dotted", "ensure_sqlite_database", consumer="team.b-1", name="v1.2_db"),
        sqlite("badConsumer", "ensure_sqlite_database", consumer="../escape"),
        sqlite("badName", "ensure_sqlite_database", name=".hidden"),
        sqlite("longName", "ensure_sqlite_database", name="x" * 65),
        sqlite("blank", "ensure_sqlite_database", consumer="  "),
        sqlite("removeUnconfirmed", "remove_sqlite_database", confirm=False),
        sqlite("remove", "remove_sqlite_database", confirm=True),
        sqlite("statusAfter", "get_sqlite_database_status"),
        sqlite("removeAgain", "remove_sqlite_database", confirm=True),
        STOP], SQLITE, None, collect=("homeTree",)),
    # The default root (the instance root's databases directory) and a
    # relative root, resolved against the agent's working directory.
    scenario("sqlite.roots", [
        serve(OPUTE_HOST_AGENT_SQLITE_ROOT=None),
        sqlite("default", "ensure_sqlite_database"),
        STOP,
        serve("startRelative", OPUTE_HOST_AGENT_SQLITE_ROOT="relative-dbs"),
        sqlite("relative", "ensure_sqlite_database"),
        {"as": "stopRelative", "stop": {}},
        serve("startFile", OPUTE_HOST_AGENT_SQLITE_ROOT="${SANDBOX}/file-root"),
        {"as": "fileRoot", "write": {"path": "${SANDBOX}/file-root", "content": "not a directory"}},
        sqlite("underFile", "ensure_sqlite_database"),
        {"as": "stopFile", "stop": {}},
        ], SQLITE, None, collect=("homeTree",)),
    # The connector: one relay per database, reference counted; the last
    # release drains after an idle period and force drains now.
    scenario("connector", [
        serve(),
        call("ensure", "ensure_sql_connector", {"databaseId": "db-1", "targetHost": "127.0.0.1", "targetPort": 5432,
                                                "listenHost": "127.0.0.1", "listenPort": 0}),
        call("ensureAgain", "ensure_sql_connector", {"databaseId": "db-1", "targetHost": "10.0.0.9",
                                                     "targetPort": 6543}),
        call("status", "get_sql_connector_status", {"databaseId": " db-1 "}),
        call("release", "release_sql_connector", {"databaseId": "db-1"}),
        call("releaseLast", "release_sql_connector", {"databaseId": "db-1"}),
        call("statusIdle", "get_sql_connector_status", {"databaseId": "db-1"}),
        call("revive", "ensure_sql_connector", {"databaseId": "db-1", "targetHost": "127.0.0.1", "targetPort": 5432}),
        call("releaseForce", "release_sql_connector", {"databaseId": "db-1", "force": True}),
        call("statusGone", "get_sql_connector_status", {"databaseId": "db-1"}),
        call("releaseMissing", "release_sql_connector", {"databaseId": "db-1"}),
        call("wildcard", "ensure_sql_connector", {"databaseId": "db-2", "targetHost": "127.0.0.1",
                                                  "targetPort": 5432}),
        call("loopbackV6", "ensure_sql_connector", {"databaseId": "db-3", "targetHost": "127.0.0.1",
                                                    "targetPort": 5432, "listenHost": "::1"}),
        call("bracketedV6", "ensure_sql_connector", {"databaseId": "db-4", "targetHost": "127.0.0.1",
                                                     "targetPort": 5432, "listenHost": "[::1]"}),
        call("noTarget", "ensure_sql_connector", {"databaseId": "db-5", "targetHost": " ", "targetPort": 5432}),
        call("badPort", "ensure_sql_connector", {"databaseId": "db-6", "targetHost": "127.0.0.1",
                                                 "targetPort": 5432, "listenPort": 70000}),
        call("blankId", "ensure_sql_connector", {"databaseId": "  ", "targetHost": "127.0.0.1", "targetPort": 1}),
        cds.REGISTRY,
        call("cleanup2", "release_sql_connector", {"databaseId": "db-2", "force": True}),
        call("cleanup4", "release_sql_connector", {"databaseId": "db-4", "force": True}),
        STOP], CONNECTOR, None,
        masks=port_masks("ensure", "listenPort") + port_masks("ensureAgain", "listenPort")
        + port_masks("status", "listenPort") + port_masks("statusIdle", "listenPort")
        + port_masks("revive", "listenPort") + port_masks("wildcard", "listenPort")
        + port_masks("bracketedV6", "listenPort")),
    # A ready service: status (with and without a relay), the steady-state
    # reconcile (no reinstall; the consumer Secret is replaced and its
    # Deployment restarted), and the relay it reuses.
    scenario("service.ready", [
        serve(), *cds.k3s_install(),
        call("status", "get_postgresql_service_status", service()),
        task_call("reconcile", "reconcile_postgresql_service", service(), timeout=120),
        task_call("reconcileQuiet", "reconcile_postgresql_service", service(restartConsumers=False, instances=1),
                  timeout=120),
        call("statusRelay", "get_postgresql_service_status",
             service(relayDeviceName="opute-pg-rw", localRelay=relay(persistent=True))),
        call("statusRelayAgain", "get_postgresql_service_status",
             service(relayDeviceName="opute-pg-rw", localRelay=relay(persistent=True))),
        call("statusRelayConflict", "get_postgresql_service_status",
             service(relayDeviceName="opute-pg-rw", localRelay=relay(token=OTHER_TOKEN, persistent=True))),
        call("statusNoDevice", "get_postgresql_service_status", service(localRelay=relay("s2"))),
        call("releaseWrong", "release_postgresql_service_relay", relay(token=OTHER_TOKEN)),
        call("release", "release_postgresql_service_relay", relay()),
        call("releaseAgain", "release_postgresql_service_relay", relay()),
        STOP], PLATFORM + RELAY, "postgres-domain-ready", cluster=True,
        masks=port_masks("statusRelay", "localRelay", "listenPort")
        + port_masks("statusRelayAgain", "localRelay", "listenPort")),
    # The relay outlives the agent: persisted, restored on the domain's
    # first call after a restart, then released with its token.
    scenario("relay.restart", [
        serve(OPUTE_HOST_AGENT_RELAY_DIR=RELAY_DIR), *cds.k3s_install(),
        call("ensure", "get_postgresql_service_status",
             service(relayDeviceName="opute-pg-rw", localRelay=relay(persistent=True))),
        call("ephemeral", "get_postgresql_service_status",
             service(relayDeviceName="opute-pg-rw", localRelay=relay("eph", ttlSeconds=60))),
        {"as": "state", "readFile": {"path": RELAY_FILE, "json": True}},
        {"as": "stopFirst", "stop": {}},
        serve("restart", OPUTE_HOST_AGENT_RELAY_DIR=RELAY_DIR),
        {"as": "stateRestored", "readFile": {"path": RELAY_FILE, "json": True}},
        call("releaseAfterRestart", "release_postgresql_service_relay", relay()),
        call("releaseEphemeral", "release_postgresql_service_relay", relay("eph")),
        {"as": "stateReleased", "readFile": {"path": RELAY_FILE, "json": True}},
        STOP], RELAY, "postgres-domain-ready", cluster=True,
        masks=port_masks("ensure", "localRelay", "listenPort")
        + [{"path": ["steps", s, "content", "listenPort"], "type": "int", "reason": PORT}
           for s in ("state", "stateRestored")] + port_masks("ephemeral", "localRelay", "listenPort")
        + [{"path": ["steps", "ephemeral", *RESULT, "localRelay", "expiresAt"], "type": "rfc3339",
            "reason": TS}]),
    # A fresh cluster: the ordered install (k3s ready, HelmChart, CRD and
    # webhook, Cluster) and the missing database created.
    scenario("service.fresh", [
        serve(), *cds.k3s_install(),
        call("statusBefore", "get_postgresql_service_status", service()),
        task_call("reconcile", "reconcile_postgresql_service", service(storageSize="10Gi"), timeout=240),
        call("statusAfter", "get_postgresql_service_status", service()),
        STOP], PLATFORM, "postgres-domain-fresh", cluster=True),
    # Storage admission and argument refusals, each before anything changes.
    scenario("service.admission", [
        serve(), *cds.k3s_install(),
        task_call("shrink", "reconcile_postgresql_service", service(storageSize="5Gi")),
        task_call("growFixed", "reconcile_postgresql_service", service(storageSize="20Gi")),
        task_call("unreadable", "reconcile_postgresql_service", service(clusterName="broken")),
        task_call("badSize", "reconcile_postgresql_service", service(storageSize="10G")),
        task_call("tooMany", "reconcile_postgresql_service", service(instances=6)),
        task_call("badRetention", "reconcile_postgresql_service", service(retentionPolicy="keep")),
        task_call("duplicate", "reconcile_postgresql_service", service(databases=["app", "app"])),
        task_call("badDatabase", "reconcile_postgresql_service", service(databases=["App"])),
        task_call("missingKey", "reconcile_postgresql_service", service(consumerDatabaseKeys={"app": "URL"})),
        task_call("noKeys", "reconcile_postgresql_service", service(consumerDatabaseKeys={"app": " "})),
        task_call("badName", "reconcile_postgresql_service", service(clusterName="Pg")),
        task_call("badOwner", "reconcile_postgresql_service", service(serviceOwner="-app")),
        task_call("noDevice", "reconcile_postgresql_service", service(localRelay=relay())),
        task_call("noBinding", "reconcile_postgresql_service", {k: v for k, v in SERVICE.items() if k != "uri"}),
        call("statusBadName", "get_postgresql_service_status", service(namespace="Platform")),
        STOP], PLATFORM, "postgres-domain-ready", cluster=True),
    # Removal: refused without confirm, retain keeps Secrets and volumes,
    # delete removes them; the operator stays.
    scenario("service.remove", [
        serve(), *cds.k3s_install(),
        task_call("unconfirmed", "remove_postgresql_service", service()),
        task_call("retain", "remove_postgresql_service", service(confirm=True, retentionPolicy="retain")),
        task_call("delete", "remove_postgresql_service", service(confirm=True)),
        STOP], PLATFORM, "postgres-domain-ready", cluster=True),
    # Standalone PostgreSQL: the operator and Cluster applied, status, SQL
    # through the primary, and the namespace deleted. Go takes the VM from
    # the cluster binding's providerInstanceName, so a vmName alone (which the schema requires) is refused.
    scenario("standalone", [
        serve(), *cds.k3s_install(),
        task_call("install", "install_postgresql", {"uri": K1, "vmName": "k1"}, timeout=120),
        task_call("installVmName", "install_postgresql", {"vmName": "k1"}),
        task_call("installBadNs", "install_postgresql", {"uri": K1, "vmName": "k1", "namespace": "Bad_NS"}),
        call("status", "get_postgresql_status", {"uri": K1, "vmName": "k1"}),
        call("statusAbsent", "get_postgresql_status", {"uri": K1, "vmName": "k1", "namespace": "absent"}),
        call("sql", "run_sql", {"uri": K1, "vmName": "k1", "sql": "SELECT 42"}),
        call("sqlBadDb", "run_sql", {"uri": K1, "vmName": "k1", "sql": "SELECT 1", "database": "app_db"}),
        call("sqlBlank", "run_sql", {"uri": K1, "vmName": "k1", "sql": "  "}),
        call("sqlLarge", "run_sql", {"uri": K1, "vmName": "k1", "sql": "SELECT '" + "x" * 65536 + "'"}),
        task_call("delete", "delete_postgresql", {"uri": K1, "vmName": "k1"}),
        STOP], STANDALONE_ANCHORS, "postgres-domain-ready", cluster=True),
]

if __name__ == "__main__":
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "scenarios" / "postgres-domain.json"
    for name, value in FIXTURES.items():
        (HERE / "fixtures" / "shims" / f"{name}.json").write_text(json.dumps(value, indent=1) + "\n")
    out.write_text(json.dumps(cases, indent=1) + "\n")
    print(f"wrote {len(cases)} scenarios to {out} and {len(FIXTURES)} fixtures")
