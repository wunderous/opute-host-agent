// Copied into internal/tools of a scratch worktree of the pinned Go tree by
// tools/parity (python3 -m parity catalog-source). It exports the catalog's
// declarations, never its derivations: definitions as Go's own loaders
// produce them, and the name and registration tables that are declared in
// code. The Rust crate derives descriptors, edges and the revision itself.
package tools

import "sort"

func ParityCatalogSource() (map[string]any, error) {
	host, err := HostToolDefinitionsForProvider("incus")
	if err != nil {
		return nil, err
	}
	all, err := LoadAllToolDefinitions("all")
	if err != nil {
		return nil, err
	}
	standaloneFromAll := make([]ToolDefinition, 0)
	for _, def := range all {
		if StandaloneToolNames[def.Name] {
			standaloneFromAll = append(standaloneFromAll, def)
		}
	}
	internal, err := LoadCatalogExcludedDispatchToolDefinitions()
	if err != nil {
		return nil, err
	}
	sort.Slice(internal, func(i, j int) bool { return internal[i].Name < internal[j].Name })
	if err := ValidateStandaloneToolContract(); err != nil {
		return nil, err
	}

	names := map[string]bool{}
	for _, list := range [][]ToolDefinition{host, StandaloneToolDefinitions(), standaloneFromAll, internal} {
		for _, def := range list {
			names[def.Name] = true
		}
	}
	registrations := map[string]any{}
	for name := range names {
		entry := map[string]any{}
		if effect, ok := RegisteredEffect(name); ok {
			entry["effect"] = effect
		}
		if class, ok := RegisteredAdmissionClass(name); ok {
			entry["admissionClass"] = string(class)
		}
		if cost, ok := RegisteredResourceCost(name); ok {
			entry["resourceCost"] = cost
		}
		if meta := StandaloneToolMetadata(name); meta != nil {
			entry["standaloneMetadata"] = meta
		}
		if IsStandaloneMutation(name) {
			entry["standaloneMutation"] = true
		}
		if IsTaskAware(name) {
			entry["taskAware"] = true
		}
		if len(entry) > 0 {
			registrations[name] = entry
		}
	}
	// Go appends some definitions in map order. The published catalog is
	// sorted by operation id, and no list repeats a name (the driver checks),
	// so sorting each list by name changes no outcome and makes the export
	// deterministic.
	byName := func(defs []ToolDefinition) []ToolDefinition {
		out := append([]ToolDefinition(nil), defs...)
		sort.SliceStable(out, func(i, j int) bool { return out[i].Name < out[j].Name })
		return out
	}
	return map[string]any{
		"providerId":            "incus",
		"hostDefinitions":       byName(host),
		"standaloneDefinitions": byName(StandaloneToolDefinitions()),
		"standaloneFromAll":     byName(standaloneFromAll),
		"internalDefinitions":   internal,
		"residualEffects":       capabilityEffects,
		"registrations":         registrations,
	}, nil
}
