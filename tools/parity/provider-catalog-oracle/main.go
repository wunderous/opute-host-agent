package main

import (
	"encoding/json"
	"fmt"
	provider "github.com/wunderous/host-agents/contracts/provider"
	"github.com/wunderous/host-agents/internal/hostmcp"
	"github.com/wunderous/host-agents/internal/tools"
	"os"
)

func main() {
	var rows []struct {
		Manifest   json.RawMessage           `json:"manifest"`
		Previous   *provider.InstallManifest `json:"previous"`
		Generation string                    `json:"generation"`
		Standalone bool                      `json:"standalone"`
		Registry   json.RawMessage           `json:"registry"`
	}
	if err := json.NewDecoder(os.Stdin).Decode(&rows); err != nil {
		panic(err)
	}
	results := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		if len(row.Registry) > 0 {
			results = append(results, map[string]any{"observations": registryScenario(row.Registry)})
			continue
		}
		var manifest provider.InstallManifest
		if err := json.Unmarshal(row.Manifest, &manifest); err != nil {
			results = append(results, map[string]any{"error": err.Error()})
			continue
		}
		definitions, err := tools.HostToolDefinitionsForProvider("incus")
		if err != nil {
			panic(err)
		}
		if row.Standalone {
			definitions = append(definitions, tools.StandaloneToolDefinitions()...)
			all, err := tools.LoadAllToolDefinitions("all")
			if err != nil {
				panic(err)
			}
			for _, def := range all {
				if tools.StandaloneToolNames[def.Name] {
					definitions = append(definitions, def)
				}
			}
		}
		active := tools.BuildCapabilityCatalog("incus", tools.CanonicalizeToolDefinitions(definitions))
		if row.Previous != nil {
			_, active, _ = hostmcp.ParityProviderCatalog(active, *row.Previous, "old-generation")
		}
		descriptors, candidate, ok := hostmcp.ParityProviderCatalog(active, manifest, row.Generation)
		if !ok {
			panic("fixture generation must identify a candidate")
		}
		encoded := func(value any) string {
			raw, err := json.Marshal(value)
			if err != nil {
				panic(err)
			}
			return string(raw)
		}
		results = append(results, map[string]any{"descriptors": encoded(descriptors), "active": encoded(active), "candidate": encoded(candidate)})
	}
	if err := json.NewEncoder(os.Stdout).Encode(results); err != nil {
		panic(fmt.Errorf("encode: %w", err))
	}
}
