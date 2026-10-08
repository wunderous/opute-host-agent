// Extract Go 1.25 Unicode name facts for the native admission parser.
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"regexp"
	"regexp/syntax"
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
	folds := map[rune]rune{}
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if f := unicode.SimpleFold(r); f != r {
			folds[r] = f
		}
	}
	ranges := func(pattern string, fold bool) []rune {
		flags := syntax.Perl
		if fold {
			flags |= syntax.FoldCase
		}
		r, err := syntax.Parse(pattern, flags)
		if err != nil {
			panic(err)
		}
		switch r.Op {
		case syntax.OpAnyChar:
			return []rune{0, unicode.MaxRune}
		case syntax.OpAnyCharNotNL:
			return []rune{0, 9, 11, unicode.MaxRune}
		case syntax.OpLiteral:
			points := map[rune]bool{}
			for _, c := range r.Rune {
				points[c] = true
				if r.Flags&syntax.FoldCase != 0 {
					for f := unicode.SimpleFold(c); f != c; f = unicode.SimpleFold(f) {
						points[f] = true
					}
				}
			}
			p := []int{}
			for c := range points {
				p = append(p, int(c))
			}
			sort.Ints(p)
			out := []rune{}
			for _, c := range p {
				n := len(out)
				if n > 0 && out[n-1]+1 == rune(c) {
					out[n-1] = rune(c)
				} else {
					out = append(out, rune(c), rune(c))
				}
			}
			return out
		case syntax.OpCharClass:
			return r.Rune
		default:
			panic(r.Op)
		}
	}
	classes := map[string]any{}
	for _, name := range keys {
		classes[name] = map[string]any{"plain": ranges(`\p{`+name+`}`, false), "fold": ranges(`\p{`+name+`}`, true)}
	}
	posix := map[string]any{}
	for _, name := range []string{"alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space", "upper", "word", "xdigit"} {
		posix[name] = map[string]any{"plain": ranges(`[[:`+name+`:]]`, false), "fold": ranges(`[[:`+name+`:]]`, true)}
	}
	if err := json.NewEncoder(os.Stdout).Encode(map[string]any{"goVersion": runtime.Version(), "unicodeVersion": unicode.Version, "names": keys, "candidates": candidates, "classes": classes, "posix": posix, "folds": folds}); err != nil {
		panic(fmt.Errorf("encode: %w", err))
	}
}
