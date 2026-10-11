#!/usr/bin/env python3
"""Generate scenarios/provider-lifecycle.json: the M7 fake-provider lifecycle matrix.

Run from tools/parity: python3 provider_lifecycle_scenarios.py scenarios/provider-lifecycle.json
"""
import json,sys,hashlib
SERVE={"as":"start","serve":{"argv":["serve","--mode=standalone"],"profile":"standalone","env":{"OPUTE_STANDALONE_ALLOW_MUTATIONS":"true","OPUTE_HOST_RESOURCE_ENFORCEMENT":"advisory","OPUTE_HOST_MAX_NORMAL_OPERATIONS":"64","OPUTE_HOST_MAX_HEAVY_OPERATIONS":"64","OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES":"64","OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES":"68719476736","XDG_RUNTIME_DIR":"${SANDBOX}/run","DBUS_SESSION_BUS_ADDRESS":"unix:path=${SANDBOX}/run/bus"}}}
PID="com.opute.fixture"
CAP="opute.capability.llm-serving.v1"
RECIPE="""contractVersion: runtime-recipe.v1
recipeId: com.opute.fixture.activate
recipeVersion: 1.0.0
runtime:
  id: provider-llm-serving
  servingContract: llm-serving.v1
  capabilities: [llm-serving]
activation:
  capability: llm-serving
  servingContract: llm-serving.v1
  inputBindings:
    providerId: providerId
compatibility:
  requiredTools: [inspect_host_file]
inputs:
  providerId:
    default: com.opute.fixture
    schema: {type: string, const: com.opute.fixture}
  activationNonce:
    default: default
    schema: {type: string, minLength: 1, maxLength: 128}
plan:
  contractVersion: host-plan.v1
  planId: com.opute.fixture.activate
  generation: 1
  idempotencyKey: com.opute.fixture.activate-${vars.inputs.providerId}-${vars.inputs.activationNonce}
  nodes:
    - id: host
      action:
        tool: inspect_host_file
        args:
          path: ~/.fixture-marker
"""
def op(id,**kw):
    d={"id":id,"version":1,"inputSchema":{"type":"object"},"outputSchema":{"type":"object"},"effect":"read","idempotent":True,"supportsReadiness":False,"taskSupport":"sync_only"}
    d.update(kw);return d
def manifest(version="1.0.0",ops=None,**kw):
    m={"schema":"opute-provider-install-manifest.v1","provider":{"id":PID,"version":version},"provides":[{"id":CAP,"version":1}],"hostAgent":{},
       "recipes":[{"id":"fixture.activate","source":{"uri":"recipes/activate.yaml","revision":"working-tree","sha256":"sha256:"+hashlib.sha256(RECIPE.encode()).hexdigest()},"mode":"activate"}],
       "services":[{"id":"opute.capability.fixture","capabilityId":CAP,"version":1,"operations":ops or [op("opute.capability.fixture.probe")]}],
       "teardown":op("opute.provider.teardown",inputSchema={"type":"object","required":["inputs"],"properties":{"inputs":{"type":"object"}}},effect="destructive",resourceKinds=["service"]),
       "validation":{"capability":CAP,"operation":"opute.capability.fixture.probe"}}
    m.update(kw);return m
def descriptor(version="1.0.0"):
    return {"schema":"opute-provider-plugin.v1","pluginId":PID,"version":version,"capabilities":[{"id":CAP,"version":1}],"server":{"transport":"streamable_http","endpoint":"http://127.0.0.1:${PROVIDER_PORT}/mcp"}}
def call(as_,name,args,capture=None,until=None,timeout=None):
    s={"as":as_,"mcp":{"method":"tools/call","name":name,"params":{"name":name,"arguments":args},"expand":True}}
    if capture:s["mcp"]["capture"]=capture
    if until:s["mcp"]["until"]=until;s["mcp"]["timeout"]=timeout or 15
    return s
def task_done(as_,var):
    # The run reports completed before its task does; a client waits on the task.
    return {"as":as_,"requires":[var],"mcp":{"method":"tasks/get","name":"${%s}"%var,"params":{"taskId":"${%s}"%var},"expand":True,
        "until":{"path":["body","result","status"],"in":["completed","failed","cancelled"]},"timeout":20}}
def wait_run(as_,var):
    return call(as_,"get_runtime_recipe_run",{"runId":"${%s}"%var},until={"path":["body","result","structuredContent","status"],"in":["completed","failed","unknown","cancelled"]})
def install(as_,extra=None,cap="RUN_ID"):
    a={"descriptor":descriptor(),"recipeSource":"${SANDBOX}/recipes/activate.yaml","activate":True}
    a.update(extra or {})
    return call(as_,"opute.provider.install",a,capture={cap:["body","result","structuredContent","runId"]} if cap else None)
STATE={"as":"state","query":{"db":"${SANDBOX}/state/state.db","sql":{
  "generations":"SELECT generation_id, provider_id, provider_version, status, endpoint, manifest_hash, active_at IS NOT NULL AS activated FROM provider_generations ORDER BY generation_id",
  "planRuns":"SELECT plan_id, status, error_message FROM plan_runs ORDER BY plan_id, created_at",
  # get_host_capacity rows come only from the IDLE synchronization poll, whose
  # count depends on timing.
  "invocations":"SELECT operation_id, generation_id, terminal_status, authorization FROM capability_invocations WHERE operation_id <> 'get_host_capacity' ORDER BY created_at, operation_id",
  "activeCapabilities":"SELECT * FROM active_capabilities ORDER BY 1",
  "activeRuntimes":"SELECT * FROM active_runtimes ORDER BY 1",
  "operations":"SELECT tool_name, status, error_message FROM operations ORDER BY created_at"}}}
SC=["steps","*","body","result","structuredContent"]
SETS=[SC+["nodes","*","output","supportedTools"],SC+["nodes","*","output","services"],SC+["state","nodes","*","output","services"]]
def m(path,kind,reason):return {"path":path,"type":kind,"reason":reason,"optional":True}
TS="wall-clock timestamp of this side's run"
MASKS=[m(SC+[k],"rfc3339",TS) for k in ("createdAt","updatedAt")]+[
  m(SC+[w,"nodes","*",k],"rfc3339",TS) for w in ("state",) for k in ("startedAt","completedAt")]+[
  m(SC+["nodes","*",k],"rfc3339",TS) for k in ("startedAt","completedAt")]+[
  m(SC+["activeRuntime","activatedAt"],"rfc3339",TS),m(SC+["activeCapability","activatedAt"],"rfc3339",TS)]+[
  m(SC+["generation",k],"rfc3339",TS) for k in ("CreatedAt","ActiveAt")]+[
  m(["steps","*",t,"*","activated_at"],"rfc3339",TS) for t in ("activeCapabilities","activeRuntimes")]+[
  m(["steps","*","body","result",k],"rfc3339",TS) for k in ("createdAt","lastUpdatedAt")]+[
  m(["steps","*","body","result","result","structuredContent","nodes","*",k],"rfc3339",TS) for k in ("startedAt","completedAt")]
# A provider callback's own task handle: its wall-clock times and the task id
# each host allocates, both where the provider echoes it and where the stub
# records it.
CB=[["steps","*","body","result","result","structuredContent","callback"],["steps","*","requests","*","callback"]]
for base in CB:
    for sub in (["body","result"],["acknowledged","body","result"]):
        MASKS+=[m(base+sub+[k],"rfc3339",TS) for k in ("createdAt","lastUpdatedAt")]
        MASKS.append(m(base+sub+["taskId"],"uuid","task id the host allocates for the callback-created task"))
def scenario(id,steps,collect=()):
    return {"id":"provider-lifecycle."+id,"surface":"provider-lifecycle","owner":"provider","anchors":["internal/hostmcp/provider_install.go:handleProviderInstallContext"],"fixture":None,"collect":list(collect),"steps":steps,"compare":{"sets":[["steps","*","body","result","structuredContent","nodes","*","output","supportedTools"],["steps","*","body","result","structuredContent",w,"nodes","*","output","services"]] if False else SETS,"notes":"Go emits supportedTools in map-iteration order (inventory finding F-2). After teardown Go removes and re-adds every MCP tool, so a call racing that refresh sees unknown tool; post-teardown reads poll until the status answers. Go also reports a provider teardown run completed while its finalize hook is still running, then records it failed when finalize fails; that case waits for the post-hook terminal state.","masks":MASKS}}
def stub(m=None,tools=None):
    s={"manifest":m or manifest()}
    if tools:s["tools"]=tools
    return {"as":"provider","providerStub":s}
WRITE={"as":"recipe","write":{"path":"${SANDBOX}/recipes/activate.yaml","content":RECIPE}}
happy=scenario("install-call-teardown",[SERVE,{"as":"list0","mcp":{"method":"tools/list","params":{}}},stub(tools={"opute.capability.fixture.probe":{"default":{"result":{"content":[],"structuredContent":{"ready":True}}}},
    "opute.provider.teardown":{"default":{"result":{"content":[],"structuredContent":{"contractVersion":"host-plan.v1","plan":{"contractVersion":"host-plan.v1","planId":"fixture.teardown","generation":1,"idempotencyKey":"fixture.teardown","nodes":[{"id":"host","action":{"tool":"inspect_host_file","args":{"path":"~/.fixture-marker"}}}]}}}}}}),WRITE,
    install("install"),wait_run("installed","RUN_ID"),
    call("status","opute.provider.status",{"provider":PID}),
    {"as":"list","mcp":{"method":"tools/list","params":{}}},
    call("probe","opute.capability.fixture.probe",{}),
    call("validate","opute.provider.validate",{"provider":PID}),
    {"as":"requests1","providerRequests":{"stub":"provider","clear":True}},
    call("teardown","opute.provider.teardown",{"provider":PID,"confirm":True,"inputs":{"hostService":"none"}},capture={"TEARDOWN_RUN":["body","result","structuredContent","runId"]}),
    call("tornDown","get_host_plan_run",{"runId":"${TEARDOWN_RUN}"},until={"path":["body","result","structuredContent","status"],"in":["completed","failed","unknown","cancelled"]}),
    task_done("teardownTask","TEARDOWN_RUN"),
    call("statusAfter","opute.provider.status",{"provider":PID},until={"path":["body","result","structuredContent","active"],"in":[False]}),
    {"as":"listAfter","mcp":{"method":"tools/list","params":{},"until":{"path":["body","result","tools","$len"],"in":[142]},"timeout":15}},
    {"as":"requests2","providerRequests":{"stub":"provider","clear":True}},
    STATE,
    {"as":"stop","stop":{}}])

def ok(v):return {"result":{"content":[],"structuredContent":v}}
TEARDOWN_PLAN={"contractVersion":"host-plan.v1","plan":{"contractVersion":"host-plan.v1","planId":"fixture.teardown","generation":1,"idempotencyKey":"fixture.teardown","nodes":[{"id":"host","action":{"tool":"inspect_host_file","args":{"path":"~/.fixture-marker"}}}]}}
TOOLS={"opute.capability.fixture.probe":{"default":ok({"ready":True})},"opute.provider.teardown":{"default":ok(TEARDOWN_PLAN)}}
def tools(**over):
    t=dict(TOOLS);t.update(over);return t
def wait(as_,var="RUN_ID",tool="get_runtime_recipe_run"):
    s=call(as_,tool,{"runId":"${%s}"%var},until={"path":["body","result","structuredContent","status"],"in":["completed","failed","unknown","cancelled"]})
    s["requires"]=[var];return s
LIST_WHOLE=lambda as_,n=142:{"as":as_,"mcp":{"method":"tools/list","params":{},"until":{"path":["body","result","tools","$len"],"in":[n]},"timeout":15}}
STATUS=lambda as_:call(as_,"opute.provider.status",{"provider":PID})
def teardown(as_,var="TEARDOWN_RUN",**extra):
    a={"provider":PID,"confirm":True,"inputs":{"hostService":"none"}};a.update(extra)
    return call(as_,"opute.provider.teardown",a,capture={var:["body","result","structuredContent","runId"]})
def fault(id,manifest_=None,tools_=None,extra=None,after=(),stub_extra=None,recipe=None,whole=142):
    st={"manifest":manifest_ or manifest(),"tools":tools_ or TOOLS}
    st.update(stub_extra or {})
    steps=[SERVE,{"as":"provider","providerStub":st},
        {"as":"recipe","write":{"path":"${SANDBOX}/recipes/activate.yaml","content":recipe or RECIPE}},
        install("install",extra),wait("installed"),STATUS("status")]+list(after)+[
        {"as":"requests","providerRequests":{"stub":"provider"}},LIST_WHOLE("catalog",whole),STATE,{"as":"stop","stop":{}}]
    return scenario(id,steps)
cases=[happy]
# Bad manifests: install must refuse with the prior catalog unchanged.
m=manifest();m["recipes"][0]["source"]={"uri":"recipes/activate.yaml"}
cases.append(fault("bad-manifest.unpinned-recipe",m))
m=manifest();m["provider"]["id"]="com.opute.other"
cases.append(fault("bad-manifest.provider-mismatch",m))
cases.append(fault("bad-manifest.schema",manifest(schema="opute-provider-install-manifest.v0")))
cases.append(fault("bad-manifest.no-recipe-for-mode",extra={"mode":"managed"}))
cases.append(fault("bad-manifest.tool-error",tools_=tools(**{"opute.provider.get_install_manifest":{"default":{"result":{"content":[{"type":"text","text":"manifest unavailable"}],"isError":True}}}})))
cases.append(fault("bad-manifest.rpc-error",tools_=tools(**{"opute.provider.get_install_manifest":{"default":{"error":{"code":-32000,"message":"manifest refused"}}}})))
m=manifest();m["validation"]={"capability":CAP,"operation":"opute.capability.fixture.missing"}
cases.append(fault("bad-manifest.validation-operation-undeclared",m,whole=143))
m=manifest();m["services"][0]["capabilityId"]="opute.capability.kubernetes.v1"
cases.append(fault("bad-manifest.service-capability-not-provided",m))
# Schema drift between the manifest and the provider's tools/list.
cases.append(fault("schema-drift.operation-not-listed",stub_extra={"listTools":["opute.provider.get_install_manifest","opute.provider.teardown"]},whole=143))
m=manifest(ops=[op("opute.capability.fixture.probe",inputSchema={"type":5})])
cases.append(fault("schema-drift.invalid-input-schema",m))
m=manifest(ops=[op("opute.capability.fixture.probe",outputSchema={"type":"object","required":["ready"],"properties":{"ready":{"type":"boolean"}}})])
cases.append(fault("schema-drift.result-violates-output-schema",m,tools(**{"opute.capability.fixture.probe":{"default":ok({"ready":"yes"})}}),
    after=[call("probe","opute.capability.fixture.probe",{})],whole=143))
# Missing dependency.
cases.append(fault("missing-dependency.requires",manifest(requires=[{"id":"opute.capability.kubernetes.v1","version":1}]),whole=143))
d=descriptor();d["requires"]=[{"id":"opute.capability.kubernetes.v1","version":1}]
cases.append(fault("missing-dependency.descriptor-requires",extra={"descriptor":d},whole=143))
cases.append(fault("missing-dependency.recipe-tool",recipe=RECIPE.replace("requiredTools: [inspect_host_file]","requiredTools: [inspect_host_file, not_a_host_tool]")))
# Readiness: activation probe failure, plan failure, slow provider.
cases.append(fault("readiness.activation-probe-error",tools_=tools(**{"opute.provider.get_install_manifest":{"responses":[ok(manifest())],"default":{"error":{"code":-32000,"message":"not ready"}}}})))
cases.append(fault("readiness.activation-probe-is-error",tools_=tools(**{"opute.provider.get_install_manifest":{"responses":[ok(manifest())],"default":{"result":{"content":[],"isError":True}}}})))
cases.append(fault("readiness.plan-assert-fails",recipe=RECIPE.replace("""        args:
          path: ~/.fixture-marker
""","""        args:
          path: ~/.fixture-marker
      validate:
        tool: inspect_host_file
        args:
          path: ~/.fixture-marker
        assert:
          - {path: /exists, op: eq, value: true}
""")))
cases.append(fault("readiness.discover-unavailable",stub_extra={"discover":None},tools_=TOOLS))
# A flapping provider: the activation probe fails once, so the first install
# leaves the prior (empty) catalog; a second install then activates.
cases.append(fault("readiness.flapping",tools_=tools(**{"opute.provider.get_install_manifest":{"responses":[ok(manifest()),{"error":{"code":-32000,"message":"not ready"}}],"default":ok(manifest())}}),
    after=[LIST_WHOLE("catalogBetween"),install("install2",cap="RUN2"),wait("installed2","RUN2"),STATUS("status2")],whole=143))
cases.append(fault("readiness.slow-probe",tools_=tools(**{"opute.provider.get_install_manifest":{"responses":[ok(manifest()),{"hang":1,**ok(manifest())}]}}),whole=143))
# Calls: provider errors map to the same typed results.
cases.append(fault("call.rpc-error",tools_=tools(**{"opute.capability.fixture.probe":{"default":{"error":{"code":-32000,"message":"probe refused"}}}}),
    after=[call("probe","opute.capability.fixture.probe",{})],whole=143))
cases.append(fault("call.is-error",tools_=tools(**{"opute.capability.fixture.probe":{"default":{"result":{"content":[{"type":"text","text":"probe failed"}],"isError":True}}}}),
    after=[call("probe","opute.capability.fixture.probe",{})],whole=143))
cases.append(fault("call.task-result-sync-only",tools_=tools(**{"opute.capability.fixture.probe":{"default":{"result":{"resultType":"task","taskId":"t1"}}}}),
    after=[call("probe","opute.capability.fixture.probe",{})],whole=143))
# Crash: the provider dies after activation.
cases.append(fault("crash.after-activation",after=[{"as":"crash","providerStop":{"stub":"provider"}},
    call("probe","opute.capability.fixture.probe",{}),call("validate","opute.provider.validate",{"provider":PID}),STATUS("statusCrashed"),
    call("teardownBlocked","opute.provider.teardown",{"provider":PID,"confirm":True,"inputs":{"hostService":"none"}}),
    teardown("teardownForced","FORCED_RUN",force=True),wait("forced","FORCED_RUN","get_host_plan_run"),
    call("statusAfter","opute.provider.status",{"provider":PID},until={"path":["body","result","structuredContent","active"],"in":[False]},timeout=10)]))
# Publish failure: operation names the host cannot publish.
cases.append(fault("publish.conflicting-host-tool",manifest(ops=[op("get_host_info")],validation={"capability":CAP,"operation":"get_host_info"})))
cases.append(fault("publish.invalid-operation-name",manifest(ops=[op("Bad Name")],validation={"capability":CAP,"operation":"Bad Name"})))
# Teardown: refusals and idempotency.
cases.append(fault("teardown.idempotent",after=[
    call("teardownUnconfirmed","opute.provider.teardown",{"provider":PID,"inputs":{"hostService":"none"}}),
    call("teardownNoTarget","opute.provider.teardown",{"provider":PID,"confirm":True}),
    teardown("teardown1","TD1"),wait("tornDown1","TD1","get_host_plan_run"),
    call("statusAfter","opute.provider.status",{"provider":PID},until={"path":["body","result","structuredContent","active"],"in":[False]},timeout=10),
    # Go refreshes every MCP tool after a retire; wait for the whole catalog
    # before calling a lifecycle tool again.
    LIST_WHOLE("catalogAfter",142),
    call("teardown2","opute.provider.teardown",{"provider":PID,"confirm":True,"inputs":{"hostService":"none"}}),
    {"as":"teardownRequests","providerRequests":{"stub":"provider","clear":True}}]))
cases.append(fault("teardown.prepare-error",tools_=tools(**{"opute.provider.teardown":{"default":{"error":{"code":-32000,"message":"cannot prepare"}}}}),
    after=[call("teardownBlocked","opute.provider.teardown",{"provider":PID,"confirm":True,"inputs":{"hostService":"none"}}),STATUS("statusAfter")],whole=143))
cases.append(fault("teardown.finalize-error",tools_=tools(**{"opute.provider.teardown":{"responses":[ok(TEARDOWN_PLAN)],"default":{"error":{"code":-32000,"message":"cannot finalize"}}}}),
    after=[teardown("teardown1","TD1"),
        # Go publishes a transient "completed" while the finalize hook still
        # runs, then records "failed"; wait for the post-hook terminal state.
        dict(call("tornDown1","get_host_plan_run",{"runId":"${TD1}"},until={"path":["body","result","structuredContent","status"],"in":["failed"]}),requires=["TD1"]),
        STATUS("statusAfter")],whole=143))
# Reinstall of an unchanged active provider is reused; reload forces a new generation.
cases.append(fault("install.reuse-and-reload",after=[install("reinstall",cap=None),
    call("reload","opute.provider.reload",{"descriptor":descriptor(),"recipeSource":"${SANDBOX}/recipes/activate.yaml","activate":True},capture={"RELOAD_RUN":["body","result","structuredContent","runId"]}),
    wait("reloaded","RELOAD_RUN"),STATUS("statusReloaded"),call("probe","opute.capability.fixture.probe",{})],whole=143))
# In-flight replacement: a g1 call held in the provider finishes on g1 while
# g2 activates; new calls reach g2, and g1 drains and stops.
d2=descriptor("2.0.0");d2["server"]["endpoint"]="http://127.0.0.1:${PROVIDER2_PORT}/mcp"
def stateq(as_,until=None):
    q=json.loads(json.dumps(STATE));q["as"]=as_
    if until:q["query"]["until"]=until
    return q
replace=scenario("replacement.in-flight",[SERVE,
    {"as":"provider","providerStub":{"manifest":manifest(),"tools":tools(**{"opute.capability.fixture.probe":{"responses":[{"gate":"held",**ok({"generation":"g1"})}],"default":ok({"generation":"g1"})}})}},
    {"as":"provider2","providerStub":{"portVar":"PROVIDER2_PORT","manifest":manifest("2.0.0"),"tools":tools(**{"opute.capability.fixture.probe":{"default":ok({"generation":"g2"})}})}},
    WRITE,install("install1",cap="RUN1"),wait("installed1","RUN1"),STATUS("status1"),
    {"as":"held","mcp":{"method":"tools/call","name":"opute.capability.fixture.probe","params":{"name":"opute.capability.fixture.probe","arguments":{}},"background":"g1call"}},
    {"as":"heldReached","providerGate":{"stub":"provider","await":"held"}},
    install("install2",{"descriptor":d2},cap="RUN2"),wait("installed2","RUN2"),STATUS("status2"),
    call("probeNew","opute.capability.fixture.probe",{}),
    stateq("stateWhileHeld"),
    {"as":"release","providerGate":{"stub":"provider","release":"held"}},
    {"as":"heldResult","join":"g1call"},
    stateq("stateAfter"),
    STATUS("status3"),
    {"as":"requests1","providerRequests":{"stub":"provider"}},
    {"as":"requests2","providerRequests":{"stub":"provider2"}},
    LIST_WHOLE("catalog",143),{"as":"stop","stop":{}}])
cases.append(replace)
# Restart: generations, catalog revision and provider reconnection restored.
RESTART=json.loads(json.dumps(SERVE));RESTART["as"]="restart"
def restart_case(id,between=(),whole=143):
    return scenario(id,[SERVE,{"as":"provider","providerStub":{"manifest":manifest(),"tools":TOOLS}},WRITE,
        install("install"),wait("installed"),STATUS("statusBefore"),
        {"as":"catalogBefore","mcp":{"method":"tools/call","name":"get_capability_catalog","params":{"name":"get_capability_catalog","arguments":{}}}},
        {"as":"stop1","stop":{}}]+list(between)+[RESTART,
        STATUS("statusRestored"),
        {"as":"catalogRestored","mcp":{"method":"tools/call","name":"get_capability_catalog","params":{"name":"get_capability_catalog","arguments":{}}}},
        call("probe","opute.capability.fixture.probe",{}),
        call("validate","opute.provider.validate",{"provider":PID}),
        {"as":"requests","providerRequests":{"stub":"provider","clear":True}},
        LIST_WHOLE("catalog",whole),stateq("state"),{"as":"stop2","stop":{}}])
cases.append(restart_case("restart.active-provider"))
cases.append(restart_case("restart.provider-down",[{"as":"crash","providerStop":{"stub":"provider"}}],whole=142))
def restart_after(id,steps,whole):
    return scenario(id,[SERVE,{"as":"provider","providerStub":{"manifest":manifest(),"tools":TOOLS}},
        {"as":"provider2","providerStub":{"portVar":"PROVIDER2_PORT","manifest":manifest("2.0.0"),"tools":tools(**{"opute.capability.fixture.probe":{"default":ok({"generation":"g2"})}})}},
        WRITE,install("install",cap="RUN1"),wait("installed","RUN1")]+steps+[{"as":"stop1","stop":{}},RESTART,
        STATUS("statusRestored"),call("probe","opute.capability.fixture.probe",{}),
        {"as":"requests1","providerRequests":{"stub":"provider","clear":True}},
        {"as":"requests2","providerRequests":{"stub":"provider2","clear":True}},
        LIST_WHOLE("catalog",whole),stateq("state"),{"as":"stop2","stop":{}}])
cases.append(restart_after("restart.after-teardown",[teardown("teardown","TD1"),wait("tornDown","TD1","get_host_plan_run"),task_done("teardownTask","TD1"),
    call("statusAfter","opute.provider.status",{"provider":PID},until={"path":["body","result","structuredContent","active"],"in":[False]},timeout=10)],142))
cases.append(restart_after("restart.after-replacement",[install("install2",{"descriptor":d2},cap="RUN2"),wait("installed2","RUN2"),STATUS("status2")],143))
# Callbacks: a bridged operation runs under a host task, so its provider call
# carries a signed delegation; the provider calls back into the host with it.
HOST={"url":"http://127.0.0.1:${PORT}/mcp","token":"${TOKEN}"}
BRIDGE="opute.capability.fixture.bridge"
def bridged_manifest():
    return manifest(ops=[op("opute.capability.fixture.probe"),op(BRIDGE,taskSupport="bridged",resourceCost={"class":"normal","cpuCores":0.25,"memoryBytes":268435456,"tasks":1})])
# Go releases the install run's heavy reservation after its task reports
# completion, and refuses normal-class work while a heavy one is held, so a
# bridged call first waits for the coordinator to hold no reservation.
IDLE={"as":"idle","mcp":{"method":"tools/call","name":"get_host_capacity","params":{"name":"get_host_capacity","arguments":{}},
    "until":{"path":["body","result","structuredContent","reservations","count"],"in":[0]},"timeout":15,
    "observe":["body","result","structuredContent","reservations","count"]}}
def bridge_call(as_,var):
    return {"as":as_,"mcp":{"method":"tools/call","name":BRIDGE,"params":{"name":BRIDGE,"arguments":{}},
        "capture":{var:["body","result","taskId"]}}}
def callback_case(id,callbacks,extra=()):
    replies=[{"callback":cb,"echoCallback":True} for cb in callbacks]
    return scenario(id,[SERVE,{"as":"provider","providerStub":{"host":HOST,"manifest":bridged_manifest(),
            "tools":tools(**{BRIDGE:{"responses":replies,"default":ok({"bridged":True})}})}},
        WRITE,install("install"),wait("installed"),task_done("installTask","RUN_ID")]+
        [step for i in range(len(callbacks)) for step in (dict(IDLE,**{"as":"idle%d"%i}),bridge_call("bridge%d"%i,"BRIDGE%d"%i),task_done("bridged%d"%i,"BRIDGE%d"%i))]+
        list(extra)+[{"as":"requests","providerRequests":{"stub":"provider"}},stateq("state"),{"as":"stop","stop":{}}])
READ={"tool":"inspect_host_file","arguments":{"path":"~/.fixture-marker"}}
cases.append(callback_case("callback.host-tool",[READ]))
cases.append(callback_case("callback.no-delegation",[dict(READ,token="none")]))
cases.append(callback_case("callback.tampered",[dict(READ,token="tampered")]))
cases.append(callback_case("callback.malformed",[dict(READ,token="malformed")]))
cases.append(callback_case("callback.stale",[READ,dict(READ,token="previous")]))
cases.append(callback_case("callback.lifecycle-tool",[{"tool":"opute.provider.status","arguments":{"provider":PID}}]))
cases.append(callback_case("callback.provider-tool",[{"tool":"opute.capability.fixture.probe","arguments":{}}]))
cases.append(callback_case("callback.standalone-tool",[{"tool":"list_operations","arguments":{}}]))
cases.append(callback_case("callback.unknown-tool",[{"tool":"not_a_host_tool","arguments":{}}]))
NIC={"tool":"configure_network","arguments":{"uri":"vm:local:fixture-vm"}}
ack=callback_case("callback.child-task-acknowledged",[dict(NIC,ack=True)])
# D12: the refused child's operation row exists only on Go; isolate it so the
# declared divergence drops exactly that row and nothing else.
DB="${SANDBOX}/state/state.db"
for step in ack["steps"]:
    if step.get("as")=="state":
        step["query"]["sql"]=dict(step["query"]["sql"],operations="SELECT tool_name, status, error_message FROM operations WHERE tool_name <> 'configure_network' ORDER BY created_at")
ack["steps"].insert(-1,{"as":"refusedChild","query":{"db":DB,"sql":{"operations":"SELECT tool_name, status FROM operations WHERE tool_name = 'configure_network'"}}})
ack["compare"]=dict(ack["compare"],divergences=["D12.refused-callback-child-operation"])
cases.append(ack)
# A delegated child the provider never acknowledges is still pending when the
# host shuts down; after restart a client reads what the host kept of it.
pending=callback_case("callback.child-task-unacknowledged-at-shutdown",[NIC])
pending["steps"][-4]["mcp"]["capture"]={"CHILD":["body","result","result","structuredContent","callback","body","result","taskId"]}
pending["steps"]+=[RESTART,{"as":"childRestored","requires":["CHILD"],"mcp":{"method":"tasks/get","name":"${CHILD}","params":{"taskId":"${CHILD}"},"expand":True}},
    {"as":"stateRestored","query":{"db":"${SANDBOX}/state/state.db","sql":{"operations":"SELECT tool_name, status, error_message FROM operations ORDER BY created_at"}}},{"as":"stop2","stop":{}}]
# Open owner decision (openspec/changes/unadmitted-callback-task-record): Go
# persists this child before admission, Rust (D12) does not. The case is
# kept out of the parity suite and captured on its own.
# CHILD is captured, so its task id is already a variable, not a uuid.
pending["compare"]=dict(pending["compare"],masks=[x for x in MASKS if not (x["type"]=="uuid" and x["path"][-1]=="taskId")])
open_cases=[pending]
# A bridged task still running when the host shuts down is cancelled, and
# the restarted host reports what it kept of it.
cases.append(scenario("callback.restart-during-bridged-task",[SERVE,{"as":"provider","providerStub":{"host":HOST,"manifest":bridged_manifest(),
        "tools":tools(**{BRIDGE:{"responses":[dict(ok({"bridged":True}),gate="held")]}})}},
    WRITE,install("install"),wait("installed"),task_done("installTask","RUN_ID"),IDLE,bridge_call("bridge","BRIDGE"),
    {"as":"held","providerGate":{"stub":"provider","await":"held"}},{"as":"stop1","stop":{}},
    {"as":"release","providerGate":{"stub":"provider","release":"held"}},RESTART,
    {"as":"restored","requires":["BRIDGE"],"mcp":{"method":"tasks/get","name":"${BRIDGE}","params":{"taskId":"${BRIDGE}"},"expand":True}},
    stateq("state"),{"as":"stop2","stop":{}}]))
# Go refuses tasks/cancel for a bridged task; the held call then finishes
# normally and records its invocation.
cases.append(scenario("callback.cancel-bridged-task-refused",[SERVE,{"as":"provider","providerStub":{"host":HOST,"manifest":bridged_manifest(),
        "tools":tools(**{BRIDGE:{"responses":[dict(ok({"bridged":True}),gate="held")]}})}},
    WRITE,install("install"),wait("installed"),task_done("installTask","RUN_ID"),IDLE,bridge_call("bridge","BRIDGE"),
    {"as":"held","providerGate":{"stub":"provider","await":"held"}},
    {"as":"cancel","requires":["BRIDGE"],"mcp":{"method":"tasks/cancel","name":"${BRIDGE}","params":{"taskId":"${BRIDGE}"}}},
    stateq("stateHeld"),{"as":"release","providerGate":{"stub":"provider","release":"held"}},
    task_done("bridged","BRIDGE"),stateq("state"),{"as":"stop","stop":{}}]))
cases.append(scenario("callback.bridged-without-task-extension",[SERVE,{"as":"provider","providerStub":{"host":HOST,"manifest":bridged_manifest(),"tools":TOOLS}},
    WRITE,install("install"),wait("installed"),task_done("installTask","RUN_ID"),IDLE,
    {"as":"bridge","mcp":{"method":"tools/call","name":BRIDGE,"modern":True,"meta":{"io.modelcontextprotocol/clientCapabilities":{}},"params":{"name":BRIDGE,"arguments":{}}}},
    {"as":"requests","providerRequests":{"stub":"provider"}},{"as":"stop","stop":{}}]))
if __name__=="__main__":
    json.dump(cases,open(sys.argv[1],'w'),indent=1)
    if len(sys.argv)>2:
        json.dump(open_cases,open(sys.argv[2],'w'),indent=1)
