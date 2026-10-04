// Compiled only in the owned M5 source copy. These calls exercise the pinned
// projection methods and SQLite sinks; no capability effects are executed.
package hostmcp

import (
    "bufio"
    "encoding/json"
    "fmt"
    "os"
    "strings"
    "testing"
    "time"
    "github.com/modelcontextprotocol/go-sdk/mcp"
    hostcapability "github.com/wunderous/host-agents/internal/capability"
    "github.com/wunderous/host-agents/internal/hostagent"
    "github.com/wunderous/host-agents/internal/plan"
    "github.com/wunderous/host-agents/internal/state"
    "github.com/wunderous/host-agents/internal/tasks"
    "github.com/wunderous/host-agents/internal/tools"
)

func TestParityDurableProjection(t *testing.T) {
    path := os.Getenv("PARITY_EVIDENCE_SPEC")
    if path == "" { t.Skip("isolated M5 fixture only") }
    var spec struct { Output string `json:"output"`; Cases []struct {
        ID string `json:"id"`; Tool string `json:"tool"`; Arguments map[string]any `json:"arguments"`
        Document map[string]any `json:"document"`; State plan.RunState `json:"state"`
    } `json:"cases"` }
    data, err := os.ReadFile(path); if err != nil { t.Fatal(err) }
    if err = json.Unmarshal(data, &spec); err != nil { t.Fatal(err) }
    svc := hostagent.New(hostagent.Options{ProviderID:"incus", ToolsForProvider: func(provider string) []string {
        names, _ := tools.HostToolNamesForProvider(provider); return names
    }})
    server, err := NewServer(Options{ProviderID:"incus", Ops:svc, Standalone:os.Getenv("OPUTE_AGENT_MODE")=="standalone",
        AllowMutations:true, StateDir:os.Getenv("OPUTE_STANDALONE_STATE_DIR")})
    if err != nil { t.Fatal(err) }; defer server.Close()
    snapshot := server.CatalogSnapshot()
    projected := map[string]any{"_catalog":snapshot}
    for _, value := range spec.Cases {
        descriptor, found := server.taskCapabilityDescriptor(value.Tool)
        if !found { t.Fatalf("fixture capability absent: %s", value.Tool) }
        args := server.redactTaskArgs(value.Tool, value.Arguments)
        task := server.tasks.Create(value.Tool, args, time.Hour, "fixture", nil)
        if err = server.state.Create(task.TaskID, value.Tool, "fixture"); err != nil { t.Fatal(err) }
        server.persistTask(task)
        server.tasks.Complete(task.TaskID, tasks.ToolResult{StructuredContent:map[string]any{"marker":"safe"}})
        task, _ = server.tasks.Get(task.TaskID); server.persistTask(task)
        capability := hostcapability.NewLegacyAdapter(descriptor, nil)
        binding := tools.ExecutionBinding{SchemaVersion:tools.ExecutionBindingSchemaVersion, TenantID:"local",
            Admission:"tenant-resource-registry", Authorization:"admitted", CatalogRevision:snapshot.Revision,
            ReservationID:"control", ResourcePolicyRevision:"opute-host-resource-policy.v2"}
        result := &mcp.CallToolResult{StructuredContent:map[string]any{"marker":"safe"}}
        observation := server.normalizeCapabilityObservation(capability, hostcapability.CapabilityObservation{
            Status:"success", Structured:json.RawMessage(`{"marker":"safe"}`)}, snapshot.Revision)
        server.recordCapabilityInvocation(capability, value.Arguments, binding, result, observation, nil)
        document := server.redactPlanEvidence(value.Document, map[string]struct{}{"recipeSecret":{}}, planCapabilitiesFromSnapshot(snapshot))
        raw, _ := json.Marshal(value.Document); var doc plan.Document
        if err = json.Unmarshal(raw, &doc); err != nil { t.Fatal(err) }
        runState := server.redactPlanRunState(value.State, &doc)
        encodedDocument, _ := json.Marshal(document); encodedState, _ := json.Marshal(runState); encodedArgs, _ := json.Marshal(args)
        _, _, err = server.state.CreatePlan(state.PlanRecord{RunID:value.ID, PlanID:value.ID, Generation:1,
            IdempotencyKey:value.ID, DocumentHash:"sha256:fixture", CatalogRevision:snapshot.Revision, Status:"completed",
            PlanJSON:string(encodedDocument), RecipeJSON:string(encodedDocument), StateJSON:string(encodedState)})
        if err != nil { t.Fatal(err) }
        if err = server.state.CompletePlanWithActiveCapability(value.ID, string(encodedState), state.ActiveCapabilityRecord{
            Capability:value.ID, ServingContract:"fixture.v1", Provider:"fixture", RecipeID:value.ID, RecipeVersion:"1",
            RecipeHash:"sha256:fixture", RunID:value.ID, InputBindingsJSON:string(encodedArgs), ObservationJSON:string(encodedState)}); err != nil { t.Fatal(err) }
        projected[value.ID] = map[string]any{"taskId":task.TaskID,"arguments":args,"document":document,"state":runState}
    }
    data, err = json.Marshal(projected); if err != nil { t.Fatal(err) }
    if err = os.WriteFile(spec.Output, data, 0600); err != nil { t.Fatal(err) }
    fmt.Println("PARITY_EVIDENCE_READY")
    line, err := bufio.NewReader(os.Stdin).ReadString('\n'); if err != nil { t.Fatal(err) }
    if strings.TrimSpace(line)!="GO" { t.Fatal("missing fixture completion command") }
}
