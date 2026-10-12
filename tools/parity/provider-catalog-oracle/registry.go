package main

import (
	"context"
	"encoding/json"
	sdk "github.com/modelcontextprotocol/go-sdk/mcp"
	provider "github.com/wunderous/host-agents/contracts/provider"
	"github.com/wunderous/host-agents/internal/capability"
	registry "github.com/wunderous/host-agents/internal/catalog"
	"github.com/wunderous/host-agents/internal/hostmcp"
	"github.com/wunderous/host-agents/internal/tools"
)

type registryValue struct {
	Manifest   provider.InstallManifest   `json:"manifest"`
	Patch      map[string]json.RawMessage `json:"patch"`
	Generation string                     `json:"generation"`
	Marker     string                     `json:"marker"`
}

func registryDescriptor(v *registryValue) tools.CapabilityDescriptor {
	descriptors, _, _ := hostmcp.ParityProviderCatalog(tools.CapabilityCatalogSnapshot{ProviderID: "incus"}, v.Manifest, "fixture-generation")
	d := descriptors[0]
	d.GenerationID = v.Generation
	raw, err := json.Marshal(d)
	if err != nil {
		panic(err)
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(raw, &fields); err != nil {
		panic(err)
	}
	for key, value := range v.Patch {
		fields[key] = value
	}
	raw, err = json.Marshal(fields)
	if err != nil {
		panic(err)
	}
	if err = json.Unmarshal(raw, &d); err != nil {
		panic(err)
	}
	return d
}
func registryExecutable(v *registryValue) capability.Capability {
	if v == nil {
		return nil
	}
	d := registryDescriptor(v)
	return capability.NewLegacyAdapter(d, func(context.Context, capability.RawArguments, tools.ExecutionBinding, capability.ExecutionSink) (*sdk.CallToolResult, error) {
		return &sdk.CallToolResult{StructuredContent: map[string]any{"marker": v.Marker, "generation": d.GenerationID}}, nil
	})
}
func registryScenario(raw json.RawMessage) []map[string]any {
	var input struct {
		Options registry.Options `json:"options"`
		Base    []*registryValue `json:"base"`
		Steps   []struct {
			Action          string                     `json:"action"`
			ID              string                     `json:"id"`
			Provider        string                     `json:"provider"`
			Implementation  string                     `json:"implementation"`
			Generation      string                     `json:"generation"`
			Revision        string                     `json:"revision"`
			Handle          string                     `json:"handle"`
			Value           *registryValue             `json:"value"`
			Values          []*registryValue           `json:"values"`
			DescriptorPatch map[string]json.RawMessage `json:"descriptorPatch"`
			NilExecutable   bool                       `json:"nilExecutable"`
			Families        []string                   `json:"families"`
		} `json:"steps"`
	}
	if err := json.Unmarshal(raw, &input); err != nil {
		panic(err)
	}
	descriptors := []tools.CapabilityDescriptor{}
	for _, v := range input.Base {
		descriptors = append(descriptors, registryDescriptor(v))
	}
	r := registry.NewRegistry(tools.BuildCapabilityCatalogFromDescriptors("incus", descriptors), input.Options)
	handles := map[string]capability.Capability{}
	observations := []map[string]any{}
	for _, step := range input.Steps {
		var err error
		observation := map[string]any{}
		var values []capability.Capability
		for _, v := range step.Values {
			values = append(values, registryExecutable(v))
		}
		switch step.Action {
		case "register":
			err = r.Register(registryExecutable(step.Value))
		case "registerCapability":
			err = r.RegisterCapability(registryExecutable(step.Value), step.Provider, step.Implementation)
		case "upsertCapability":
			err = r.UpsertCapability(registryExecutable(step.Value), step.Provider, step.Implementation)
		case "registration", "upsert":
			d := registryDescriptor(step.Value)
			raw, _ := json.Marshal(d)
			var fields map[string]json.RawMessage
			_ = json.Unmarshal(raw, &fields)
			for key, value := range step.DescriptorPatch {
				fields[key] = value
			}
			raw, _ = json.Marshal(fields)
			_ = json.Unmarshal(raw, &d)
			registration := registry.Registration{Descriptor: d, ProviderID: step.Provider, Implementation: step.Implementation, Capability: registryExecutable(step.Value)}
			if step.NilExecutable {
				registration.Capability = nil
			}
			if step.Action == "upsert" {
				err = r.Upsert(registration)
			} else {
				err = r.RegisterRegistration(registration)
			}
		case "replaceGeneration":
			err = r.ReplaceGeneration(step.Generation, values)
		case "replaceProvider":
			err = r.ReplaceProvider(step.Provider, values)
		case "authorize":
			err = r.AuthorizeProvider(step.Provider)
		case "unregister":
			err = r.Unregister(step.ID)
		case "displace":
			observation["removed"] = r.DisplaceCapabilityFamilies(step.Provider, step.Families)
		case "validateBase":
			err = r.ValidateBase()
		case "resolve":
			revision := step.Revision
			if revision == "$current" {
				revision = r.Snapshot().Revision
			}
			var value capability.Capability
			value, err = r.Resolve(step.ID, revision)
			if err == nil {
				handles[step.Handle] = value
				observation["resolved"] = value.Definition()
			}
		case "lookup":
			value, ok := r.ResolveCapability(step.ID)
			observation["found"] = ok
			if ok {
				observation["resolved"] = value.Definition()
			}
		case "invoke":
			value := handles[step.Handle]
			if value == nil {
				panic("fixture handle missing")
			}
			result, e := value.Invoke(context.Background(), capability.RawArguments{}, tools.ExecutionBinding{}, nil)
			err = e
			if err == nil {
				observation["result"] = result.StructuredContent
			}
		default:
			panic(step.Action)
		}
		observation["error"] = ""
		if err != nil {
			observation["error"] = err.Error()
		}
		snapshot, err := json.Marshal(r.Snapshot())
		if err != nil {
			panic(err)
		}
		observation["snapshot"] = string(snapshot)
		observations = append(observations, observation)
	}
	return observations
}
