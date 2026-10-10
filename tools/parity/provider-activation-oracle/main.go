package main

import (
	"encoding/json"
	"os"

	"github.com/wunderous/host-agents/internal/hostmcp"
)

func main() {
	var rows []struct {
		Activation json.RawMessage
	}
	if err := json.NewDecoder(os.Stdin).Decode(&rows); err != nil {
		panic(err)
	}
	outputs := make([]any, 0, len(rows))
	for _, row := range rows {
		outputs = append(outputs, hostmcp.ParityProviderActivation(row.Activation))
	}
	if err := json.NewEncoder(os.Stdout).Encode(outputs); err != nil {
		panic(err)
	}
}
