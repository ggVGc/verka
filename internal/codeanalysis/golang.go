package codeanalysis

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

type GoAnalyzer struct{}

func (g *GoAnalyzer) Name() string { return "go" }

func (g *GoAnalyzer) Detect(dir string) bool {
	_, err := os.Stat(filepath.Join(dir, "go.mod"))
	return err == nil
}

func (g *GoAnalyzer) Analyze(ctx context.Context, dir string) ([]PackageInfo, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	cmd := exec.CommandContext(ctx, "go", "list", "-json", "./...")
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		return nil, nil
	}

	dec := json.NewDecoder(strings.NewReader(string(out)))
	var pkgs []PackageInfo
	for dec.More() {
		var p goListPackage
		if err := dec.Decode(&p); err != nil {
			return pkgs, nil
		}
		modPath := ""
		if p.Module != nil {
			modPath = p.Module.Path
		}
		var filtered []string
		for _, imp := range p.Imports {
			if !isStdlib(imp) {
				filtered = append(filtered, imp)
			}
		}
		pkgs = append(pkgs, PackageInfo{
			ImportPath: p.ImportPath,
			Module:     modPath,
			Imports:    filtered,
		})
	}
	return pkgs, nil
}

type goListPackage struct {
	ImportPath string    `json:"ImportPath"`
	Module     *goModule `json:"Module"`
	Imports    []string  `json:"Imports"`
}

type goModule struct {
	Path string `json:"Path"`
}

func isStdlib(importPath string) bool {
	first := importPath
	if i := strings.IndexByte(importPath, '/'); i >= 0 {
		first = importPath[:i]
	}
	return !strings.Contains(first, ".")
}
