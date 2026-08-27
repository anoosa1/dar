//go:build ignore

package main

import (
	"bufio"
	"flag"
	"fmt"
	"os"
	"path"
	"sort"
	"strconv"
	"strings"
)

type block struct {
	file                        string
	startLine, startCol         int
	endLine, endCol, statements int
	count                       int64
}

func main() {
	globalMinimum := flag.Float64("global", 90, "minimum global statement coverage")
	packageMinimum := flag.Float64("package", 80, "minimum per-package statement coverage")
	output := flag.String("out", "", "optional merged profile path")
	flag.Parse()
	if flag.NArg() == 0 {
		fatalf("usage: go run test/coverage_gate.go [flags] profile...")
	}
	merged := map[string]block{}
	mode := "atomic"
	for _, profile := range flag.Args() {
		loadedMode, err := mergeProfile(profile, merged)
		if err != nil {
			fatalf("%s: %v", profile, err)
		}
		if loadedMode != "" {
			mode = loadedMode
		}
	}
	if *output != "" {
		if err := writeProfile(*output, mode, merged); err != nil {
			fatalf("write merged profile: %v", err)
		}
	}

	type totals struct{ covered, statements int }
	packages := map[string]totals{}
	global := totals{}
	for _, item := range merged {
		pkg := packageName(item.file)
		if pkg == "" || strings.HasSuffix(pkg, "/internal/testutil") || strings.Contains(pkg, "/test/") {
			continue
		}
		value := packages[pkg]
		value.statements += item.statements
		global.statements += item.statements
		if item.count > 0 {
			value.covered += item.statements
			global.covered += item.statements
		}
		packages[pkg] = value
	}
	names := make([]string, 0, len(packages))
	for name := range packages {
		names = append(names, name)
	}
	sort.Strings(names)
	failed := false
	for _, name := range names {
		value := packages[name]
		percent := percentage(value)
		fmt.Printf("%6.1f%%  %s\n", percent, name)
		if percent < *packageMinimum {
			failed = true
		}
	}
	globalPercent := percentage(global)
	fmt.Printf("%6.1f%%  global production coverage\n", globalPercent)
	if globalPercent < *globalMinimum {
		failed = true
	}
	if failed {
		fatalf("coverage gate failed: require global >= %.1f%% and every production package >= %.1f%%", *globalMinimum, *packageMinimum)
	}
}

func mergeProfile(filename string, merged map[string]block) (string, error) {
	file, err := os.Open(filename)
	if err != nil {
		return "", err
	}
	defer file.Close()
	scanner := bufio.NewScanner(file)
	mode := ""
	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, "mode: ") {
			mode = strings.TrimSpace(strings.TrimPrefix(line, "mode: "))
			continue
		}
		item, err := parseBlock(line)
		if err != nil {
			return "", err
		}
		key := fmt.Sprintf("%s:%d.%d,%d.%d %d", item.file, item.startLine, item.startCol, item.endLine, item.endCol, item.statements)
		if previous, ok := merged[key]; !ok || item.count > previous.count {
			merged[key] = item
		}
	}
	return mode, scanner.Err()
}

func parseBlock(line string) (block, error) {
	var item block
	var count string
	n, err := fmt.Sscanf(line, "%s %d %s", &item.file, &item.statements, &count)
	if err != nil || n != 3 {
		return item, fmt.Errorf("invalid coverage line %q", line)
	}
	location := item.file
	colon := strings.LastIndex(location, ":")
	if colon < 0 {
		return item, fmt.Errorf("invalid coverage location %q", location)
	}
	item.file = location[:colon]
	var end string
	if _, err := fmt.Sscanf(location[colon+1:], "%d.%d,%s", &item.startLine, &item.startCol, &end); err != nil {
		return item, err
	}
	if _, err := fmt.Sscanf(end, "%d.%d", &item.endLine, &item.endCol); err != nil {
		return item, err
	}
	item.count, err = strconv.ParseInt(count, 10, 64)
	return item, err
}

func packageName(filename string) string {
	const module = "github.com/anoosa/dar/"
	if !strings.HasPrefix(filename, module) {
		return ""
	}
	dir := path.Dir(filename)
	if dir == "." {
		return ""
	}
	return dir
}

func percentage(value struct{ covered, statements int }) float64 {
	if value.statements == 0 {
		return 0
	}
	return float64(value.covered) * 100 / float64(value.statements)
}

func writeProfile(filename, mode string, merged map[string]block) error {
	keys := make([]string, 0, len(merged))
	for key := range merged {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	var output strings.Builder
	fmt.Fprintf(&output, "mode: %s\n", mode)
	for _, key := range keys {
		item := merged[key]
		fmt.Fprintf(&output, "%s:%d.%d,%d.%d %d %d\n", item.file, item.startLine, item.startCol, item.endLine, item.endCol, item.statements, item.count)
	}
	return os.WriteFile(filename, []byte(output.String()), 0o600)
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}
