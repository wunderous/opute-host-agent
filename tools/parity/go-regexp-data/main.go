// Extract Go 1.25 Unicode name facts for the native admission parser.
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"runtime"
	"sort"
	"unicode"
)

func main() {
	if runtime.Version() != "go1.25.4" {
		panic(runtime.Version())
	}
	names := map[string]bool{"Any": true, "Assigned": true, "Ascii": true, "Lc": true}
	for key := range unicode.Categories {
		names[key] = true
	}
	for key := range unicode.Scripts {
		names[key] = true
	}
	for key := range unicode.CategoryAliases {
		names[key] = true
	}
	candidates := []string{}
	for key := range names {
		candidates = append(candidates, key)
	}
	sort.Strings(candidates)
	keys := []string{}
	for key := range names {
		if _, err := regexp.Compile(`\p{` + key + `}`); err == nil {
			keys = append(keys, key)
		}
	}
	sort.Strings(keys)
	if err := json.NewEncoder(os.Stdout).Encode(map[string]any{"goVersion": runtime.Version(), "unicodeVersion": unicode.Version, "names": keys, "candidates": candidates}); err != nil {
		panic(fmt.Errorf("encode: %w", err))
	}
}
