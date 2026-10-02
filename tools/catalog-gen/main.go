// Copied into cmd/parity-catalog-source of a scratch worktree of the pinned
// Go tree. Prints the catalog declarations as JSON (see export_tools.go).
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"sort"

	"github.com/wunderous/host-agents/internal/tasks"
	"github.com/wunderous/host-agents/internal/tools"
)

func main() {
	source, err := tools.ParityCatalogSource()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	// tasks.TaskAwareTools also routes a call through the task boundary.
	taskAware := make([]string, 0, len(tasks.TaskAwareTools))
	for name, aware := range tasks.TaskAwareTools {
		if aware {
			taskAware = append(taskAware, name)
		}
	}
	sort.Strings(taskAware)
	source["taskAwareTools"] = taskAware
	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", " ")
	if err := encoder.Encode(source); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
