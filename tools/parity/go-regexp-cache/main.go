// Copyright 2009 The Go Authors. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause; see licenses/Go-BSD.txt.
// Diagnostic only: the owning exported AST and uncached parse.go size equations.
package main

import (
	"encoding/json"
	"os"
	"regexp"
	"regexp/syntax"
	"strings"
)

func size(r *syntax.Regexp) int64 {
	var n int64
	switch r.Op {
	case syntax.OpLiteral:
		n = int64(len(r.Rune))
	case syntax.OpCapture, syntax.OpStar:
		n = 2 + size(r.Sub[0])
	case syntax.OpPlus, syntax.OpQuest:
		n = 1 + size(r.Sub[0])
	case syntax.OpConcat, syntax.OpAlternate:
		for _, c := range r.Sub {
			n += size(c)
		}
		if r.Op == syntax.OpAlternate && len(r.Sub) > 1 {
			n += int64(len(r.Sub) - 1)
		}
	case syntax.OpRepeat:
		s := size(r.Sub[0])
		if r.Max == -1 {
			if r.Min == 0 {
				n = 2 + s
			} else {
				n = 1 + int64(r.Min)*s
			}
		} else {
			n = int64(r.Max)*s + int64(r.Max-r.Min)
		}
	}
	return max(1, n)
}
func main() {
	p := strings.Repeat("(?:abcd|abce|abcf){1000}", 1000)
	r, e := syntax.Parse(p, syntax.Perl)
	if e != nil {
		panic(e)
	}
	_, ce := regexp.Compile(p)
	err := ""
	if ce != nil {
		err = ce.Error()
	}
	json.NewEncoder(os.Stdout).Encode(map[string]any{"goCompileError": err, "trueSize": size(r), "limit": 128 << 20 / (5 * 8), "rootChildren": len(r.Sub), "firstChildSize": size(r.Sub[0]), "firstChild": r.Sub[0].String()})
}
