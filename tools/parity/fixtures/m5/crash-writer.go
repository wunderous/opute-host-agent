// Isolated M5 fixture process. It calls the pinned Go store's actual methods;
// it is compiled in a separate source copy and never installed as the agent.
package main

import (
    "bufio"
    "fmt"
    "os"
    "strings"
    "github.com/wunderous/host-agents/internal/state"
    "github.com/wunderous/host-agents/internal/tasks"
)

func must(err error) { if err != nil { panic(err) } }

func main() {
    store, err := state.Open(os.Getenv("PARITY_STATE_DIR")); must(err)
    defer store.Close()
    task := os.Getenv("PARITY_TASK_ID")
    for _, id := range []string{"old", "new"} {
        status := "running"; if id == "old" { status = "completed" }
        _, _, err = store.CreatePlan(state.PlanRecord{RunID:id, PlanID:id, Generation:1, IdempotencyKey:id,
            DocumentHash:"sha256:fixture", CatalogRevision:"sha256:fixture", Status:status,
            PlanJSON:"{}", RecipeJSON:"{}", StateJSON:`{"checkpoint":"original"}`})
        must(err)
    }
    active := func(id string) state.ActiveCapabilityRecord { return state.ActiveCapabilityRecord{
        Capability:"fixture", ServingContract:"fixture.v1", Provider:"fixture", RecipeID:id,
        RecipeVersion:"1", RecipeHash:"sha256:fixture", RunID:id, InputBindingsJSON:"{}", ObservationJSON:"{}"} }
    must(store.CompletePlanWithActiveCapability("old", `{"checkpoint":"original"}`, active("old")))
    fmt.Println("PARITY_SETUP")
    line, err := bufio.NewReader(os.Stdin).ReadString('\n'); must(err)
    if strings.TrimSpace(line) != "GO" { panic("missing isolated fixture command") }
    switch os.Getenv("PARITY_STATE_ACTION") {
    case "operation-create": err = store.Create(task, "fixture", "replacement")
    case "operation-complete": err = store.Complete(task, map[string]any{"checkpoint":"changed"})
    case "operation-fail": err = store.Fail(task, "changed")
    case "operation-cancel": err = store.Cancel(task)
    case "task-snapshot": err = store.SaveTaskSnapshot(task, "request_task_input", "changed", map[string]any{
        "taskId":task, "status":tasks.Status("completed"), "resultType":"complete", "toolName":"request_task_input",
        "createdAt":"2026-10-04T00:00:00Z", "lastUpdatedAt":"2026-10-04T00:00:00Z",
        "result":map[string]any{"checkpoint":"changed"}, "toolArgs":map[string]any{}})
    case "plan-update": err = store.UpdatePlan("new", "completed", `{"checkpoint":"changed"}`, "")
    case "plan-complete": err = store.CompletePlanWithActiveCapability("new", `{"checkpoint":"changed"}`, active("new"))
    default: panic("unknown fixture action")
    }
    must(err)
    fmt.Println("PARITY_COMMITTED")
}
