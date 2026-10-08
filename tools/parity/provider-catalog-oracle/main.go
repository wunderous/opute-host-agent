package main

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	provider "github.com/wunderous/host-agents/contracts/provider"
	registry "github.com/wunderous/host-agents/internal/catalog"
	"github.com/wunderous/host-agents/internal/hostmcp"
	"github.com/wunderous/host-agents/internal/tools"
	"net/url"
	"os"
	"regexp"
)

func main() {
	var rows []struct {
		Regexp *string `json:"regexp"`
		URI    *struct {
			Base string  `json:"base"`
			Ref  *string `json:"ref"`
		} `json:"uri"`
		Manifest       json.RawMessage           `json:"manifest"`
		Previous       *provider.InstallManifest `json:"previous"`
		Generation     string                    `json:"generation"`
		Standalone     bool                      `json:"standalone"`
		Registry       json.RawMessage           `json:"registry"`
		Schema         json.RawMessage           `json:"schema"`
		Group          string                    `json:"group"`
		DecodeOnly     bool                      `json:"decodeOnly"`
		ResolverAction string                    `json:"resolverAction"`
		BaseURI        string                    `json:"baseURI"`
		Pointer        string                    `json:"pointer"`
	}
	if err := json.NewDecoder(os.Stdin).Decode(&rows); err != nil {
		panic(err)
	}
	results := make([]map[string]any, 0, len(rows))
	for _, row := range rows {
		if row.Regexp != nil {
			_, err := regexp.Compile(*row.Regexp)
			message := ""
			if err != nil {
				message = err.Error()
			}
			results = append(results, map[string]any{"error": message})
			continue
		}
		if row.URI != nil {
			u, err := url.Parse(row.URI.Base)
			if err == nil && row.URI.Ref != nil {
				var ref *url.URL
				ref, err = url.Parse(*row.URI.Ref)
				if err == nil {
					u = u.ResolveReference(ref)
				}
			}
			if err != nil {
				results = append(results, map[string]any{"error": err.Error()})
				continue
			}
			var user any
			if u.User != nil {
				password, present := u.User.Password()
				var p any
				if present {
					p = hex.EncodeToString([]byte(password))
				}
				user = map[string]any{"name": hex.EncodeToString([]byte(u.User.Username())), "password": p}
			}
			results = append(results, map[string]any{"error": "", "scheme": u.Scheme, "opaque": u.Opaque, "user": user, "host": hex.EncodeToString([]byte(u.Host)), "path": hex.EncodeToString([]byte(u.Path)), "rawPath": u.RawPath, "omitHost": u.OmitHost, "forceQuery": u.ForceQuery, "query": u.RawQuery, "fragment": hex.EncodeToString([]byte(u.Fragment)), "rawFragment": u.RawFragment, "escapedPath": u.EscapedPath(), "escapedFragment": u.EscapedFragment(), "string": u.String()})
			continue
		}
		if len(row.Schema) > 0 {
			var value map[string]any
			if err := json.Unmarshal(row.Schema, &value); err != nil {
				panic(err)
			}
			if row.ResolverAction != "" {
				results = append(results, registry.ParitySchemaResolver(value, row.ResolverAction, row.Pointer, row.BaseURI))
				continue
			}
			if row.DecodeOnly {
				results = append(results, registry.ParityDecodeSchema(value))
				continue
			}
			err := registry.ParityCheckSchema("fixture", value)
			message := ""
			if err != nil {
				message = err.Error()
			}
			observation := map[string]any{"error": message}
			if row.Group == "integer" {
				observation["value"] = nil
				if err == nil {
					observation["value"] = registry.ParitySchemaInteger(value)
				}
			}
			results = append(results, observation)
			continue
		}
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
