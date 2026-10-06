// Drives the pinned lifecycle implementation without changing its source tree.
package main

import (
	"bytes"
	"context"
	"encoding/json"
	sdkmcp "github.com/modelcontextprotocol/go-sdk/mcp"
	provider "github.com/wunderous/host-agents/contracts/provider"
	"github.com/wunderous/host-agents/internal/cordis"
	provideradapter "github.com/wunderous/host-agents/internal/cordis/mcp"
	"github.com/wunderous/host-agents/internal/mcphttp"
	"io"
	"os"
	"sort"
	"time"
)

type Step struct {
	Action     string                    `json:"action"`
	Provider   provider.ProviderRef      `json:"provider"`
	ID         string                    `json:"id"`
	ProviderID string                    `json:"providerId"`
	Session    string                    `json:"session"`
	Previous   string                    `json:"previous"`
	Hash       string                    `json:"hash"`
	Endpoint   string                    `json:"endpoint"`
	Revision   string                    `json:"revision"`
	Generation cordis.ProviderGeneration `json:"generation"`
	Cancelled  bool                      `json:"cancelled"`
}

func main() {
	raw, err := io.ReadAll(os.Stdin)
	if err != nil {
		panic(err)
	}
	if bytes.HasPrefix(bytes.TrimSpace(raw), []byte("[")) {
		var rows []struct {
			Kind     string               `json:"kind"`
			Value    json.RawMessage      `json:"value"`
			Expected provider.ProviderRef `json:"expected"`
		}
		if err := json.Unmarshal(raw, &rows); err != nil {
			panic(err)
		}
		results := make([]map[string]any, 0, len(rows))
		for _, row := range rows {
			if row.Kind == "result" {
				var result sdkmcp.CallToolResult
				err := json.Unmarshal(row.Value, &result)
				value := map[string]any{"result": result}
				if err != nil {
					value = map[string]any{"error": err.Error()}
				}
				results = append(results, value)
				continue
			}
			if row.Kind == "adapter" {
				var input struct {
					Descriptor provider.PluginDescriptor `json:"descriptor"`
					Token      string                    `json:"token"`
					Name       string                    `json:"name"`
					Version    string                    `json:"version"`
					Action     string                    `json:"action"`
					Operation  string                    `json:"operation"`
					Arguments  map[string]any            `json:"arguments"`
					Delegation string                    `json:"delegation"`
				}
				if err := json.Unmarshal(row.Value, &input); err != nil {
					panic(err)
				}
				adapter, err := provideradapter.Connect(context.Background(), input.Descriptor, provideradapter.Options{BearerToken: input.Token, ClientName: input.Name, ClientVersion: input.Version})
				result := map[string]any{}
				if err != nil {
					result["error"] = err.Error()
				} else {
					if input.Action != "" {
						ctx := mcphttp.WithResourceDelegation(context.Background(), input.Delegation)
						if input.Action == "closed" {
							adapter.Close()
							adapter.Close()
						}
						if input.Operation == "" {
							input.Operation = "test.operation"
						}
						var value any
						key := "result"
						if input.Action == "manifest" {
							key = "manifest"
							value, err = adapter.InstallManifest(ctx)
						} else if input.Action == "sync" {
							value, err = adapter.CallSynchronousOnly(ctx, input.Operation, input.Arguments)
						} else {
							value, err = adapter.Call(ctx, input.Operation, input.Arguments)
						}
						if err != nil {
							result["error"] = err.Error()
						} else {
							result[key] = value
						}
						adapter.Close()
						results = append(results, result)
						continue
					}
					names := adapter.ToolNames()
					sort.Strings(names)
					result["tools"] = names
					adapter.Close()
				}
				results = append(results, result)
				continue
			}
			if row.Kind == "wire-descriptor" || row.Kind == "wire-manifest" {
				var raw string
				if err := json.Unmarshal(row.Value, &raw); err != nil {
					panic(err)
				}
				var value any
				var err error
				if row.Kind == "wire-descriptor" {
					var d provider.PluginDescriptor
					err = json.Unmarshal([]byte(raw), &d)
					value = d
				} else {
					var m provider.InstallManifest
					err = json.Unmarshal([]byte(raw), &m)
					value = m
				}
				encoded, marshalErr := json.Marshal(value)
				if marshalErr != nil && err == nil {
					err = marshalErr
				}
				result := map[string]any{"decoded": value, "encoded": string(encoded)}
				if err != nil {
					result = map[string]any{"error": err.Error()}
				}
				results = append(results, result)
				continue
			}
			var err error
			if row.Kind == "descriptor" {
				var value provider.PluginDescriptor
				if err = json.Unmarshal(row.Value, &value); err == nil {
					err = provider.ValidateDescriptor(value)
				}
			} else {
				var value provider.InstallManifest
				if err = json.Unmarshal(row.Value, &value); err == nil {
					err = provider.ValidateInstallManifest(value, row.Expected)
				}
			}
			result := map[string]any{"valid": true}
			if err != nil {
				result = map[string]any{"error": err.Error()}
			}
			results = append(results, result)
		}
		if err := json.NewEncoder(os.Stdout).Encode(results); err != nil {
			panic(err)
		}
		return
	}

	var input struct {
		TimeoutMs       int    `json:"timeoutMs"`
		CancelOnTimeout bool   `json:"cancelOnTimeout"`
		Steps           []Step `json:"steps"`
	}
	if err := json.Unmarshal(raw, &input); err != nil {
		panic(err)
	}
	timeout := input.TimeoutMs
	if timeout == 0 {
		timeout = 20
	}
	manager := cordis.NewProviderLifecycleManager(cordis.DrainPolicy{Timeout: time.Duration(timeout) * time.Millisecond, CancelOnTimeout: input.CancelOnTimeout})
	sessions := map[string]*cordis.GenerationSession{}
	results := make([]map[string]any, 0, len(input.Steps))
	for _, step := range input.Steps {
		var value any
		var err error
		switch step.Action {
		case "create":
			value, err = manager.CreateCandidate(step.Provider, step.Hash, step.Endpoint, step.Revision)
		case "restore":
			err = manager.Restore(step.Generation)
		case "ready":
			err = manager.MarkReady(step.ID)
		case "activate":
			var previous *cordis.ProviderGeneration
			var active cordis.ProviderGeneration
			previous, active, err = manager.Activate(step.ID)
			value = map[string]any{"previous": previous, "active": active}
		case "rollback":
			err = manager.RollbackActivation(step.ID, step.Previous)
		case "refresh":
			value, err = manager.Refresh(step.ID, step.Hash, step.Revision)
		case "get":
			if generation, ok := manager.Get(step.ID); ok {
				value = generation
			}
		case "active":
			if generation, ok := manager.Active(step.ProviderID); ok {
				value = generation
			}
		case "open", "open-explicit":
			var session *cordis.GenerationSession
			if step.Action == "open" {
				session, err = manager.OpenSession(step.ProviderID)
			} else {
				session, err = manager.OpenSessionForGeneration(step.ProviderID, step.ID)
			}
			if err == nil {
				value = session.GenerationID()
				sessions[step.Session] = session
			}
		case "close":
			sessions[step.Session].Close()
		case "fail":
			err = manager.Fail(step.ID, "")
		case "dispose":
			err = manager.Dispose(step.ID)
		case "drain":
			ctx := context.Background()
			if step.Cancelled {
				child, cancel := context.WithCancel(ctx)
				cancel()
				ctx = child
			}
			err = manager.Drain(ctx, step.ID)
		default:
			panic("unknown fixture action " + step.Action)
		}
		result := map[string]any{"value": value}
		if err != nil {
			result = map[string]any{"error": err.Error()}
		}
		results = append(results, result)
	}
	if err := json.NewEncoder(os.Stdout).Encode(results); err != nil {
		panic(err)
	}
}
