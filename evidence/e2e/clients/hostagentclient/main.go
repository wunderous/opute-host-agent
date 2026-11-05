// Command hostagentclient is the e2e-1 tutorial client built on the published
// Go client library, github.com/wunderous/host-agents/pkg/hostagentclient
// (pinned v0.2.4). It follows the first-success steps and prints one JSON
// document; every structured result is reported for schema validation.
//
//	HOST_AGENT_URL=http://127.0.0.1:3014/mcp HOST_AGENT_TOKEN=... hostagentclient [plan.json]
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"time"

	"github.com/modelcontextprotocol/go-sdk/mcp"
	"github.com/wunderous/host-agents/pkg/hostagentclient"
)

type result struct {
	Tool              string `json:"tool"`
	IsError           bool   `json:"isError"`
	StructuredContent any    `json:"structuredContent,omitempty"`
	Error             string `json:"error,omitempty"`
}

func main() {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()
	out := map[string]any{}
	var results []result
	fail := func(step string, err error) {
		out["failedStep"], out["error"] = step, err.Error()
		out["results"] = results
		_ = json.NewEncoder(os.Stdout).Encode(out)
		os.Exit(1)
	}
	call := func(name string, args map[string]any) *mcp.CallToolResult {
		res, err := client(ctx).Call(ctx, name, args)
		if err != nil {
			fail(name, err)
		}
		results = append(results, result{Tool: name, IsError: res.IsError, StructuredContent: res.StructuredContent})
		return res
	}
	tools, err := client(ctx).ListTools(ctx)
	if err != nil {
		fail("tools/list", err)
	}
	out["toolsList"] = len(tools)
	catalog, err := client(ctx).Catalog(ctx)
	if err != nil {
		fail("get_capability_catalog", err)
	}
	out["catalog"] = map[string]any{"revision": catalog.Revision, "tools": len(catalog.Tools)}
	call("get_capability_catalog", map[string]any{})
	call("get_host_info", map[string]any{})
	call("get_host_capacity", map[string]any{})
	if len(os.Args) > 1 {
		raw, err := os.ReadFile(os.Args[1])
		if err != nil {
			fail("plan", err)
		}
		var plan map[string]any
		if err := json.Unmarshal(raw, &plan); err != nil {
			fail("plan", err)
		}
		started := call("run_host_plan", map[string]any{"plan": plan})
		runID, _ := structured(started)["runId"].(string)
		status := ""
		for i := 0; i < 120 && runID != ""; i++ {
			status, _ = structured(call("get_host_plan_run", map[string]any{"runId": runID}))["status"].(string)
			if status == "completed" || status == "failed" || status == "cancelled" {
				break
			}
			time.Sleep(time.Second)
		}
		out["plan"] = map[string]any{"runId": runID, "status": status}
	}
	out["results"] = results
	_ = json.NewEncoder(os.Stdout).Encode(out)
}

var connected *hostagentclient.Client

func client(ctx context.Context) *hostagentclient.Client {
	if connected == nil {
		c, err := hostagentclient.Connect(ctx, os.Getenv("HOST_AGENT_URL"), os.Getenv("HOST_AGENT_TOKEN"))
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"failedStep": "connect", "error": err.Error()})
			os.Exit(1)
		}
		connected = c
	}
	return connected
}

func structured(res *mcp.CallToolResult) map[string]any {
	raw, _ := json.Marshal(res.StructuredContent)
	var m map[string]any
	_ = json.Unmarshal(raw, &m)
	return m
}
