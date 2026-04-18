package workspace

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/ggvgc/llaundry/internal/model"
)

// AssembleBuild writes a go.work file in the build node's build/ directory
// that `use`s the source/ directory of every implementation the build depends
// on. It returns the absolute build dir path; callers run `go build` there.
func (w *Workspace) AssembleBuild(ctx context.Context, buildNodeID string) (string, error) {
	n, err := w.store.GetNode(ctx, buildNodeID)
	if err != nil {
		return "", err
	}
	if n.Type != model.TypeBuild {
		return "", fmt.Errorf("AssembleBuild: node %s is %s, not build", buildNodeID, n.Type)
	}
	if err := w.Materialize(ctx, buildNodeID); err != nil {
		return "", err
	}
	buildDir := w.BuildDir(buildNodeID)
	if err := os.MkdirAll(buildDir, 0o755); err != nil {
		return "", err
	}

	impls, err := w.store.Neighbors(ctx, buildNodeID, model.EdgeDependsOn, model.DirOutgoing)
	if err != nil {
		return "", err
	}
	if len(impls) == 0 {
		return "", fmt.Errorf("build node %s has no depends_on implementations", buildNodeID)
	}

	var uses []string
	for _, impl := range impls {
		src := w.SourceDir(impl)
		rel, err := filepath.Rel(buildDir, src)
		if err != nil {
			return "", err
		}
		uses = append(uses, rel)
	}

	// Emit go.work. Go 1.21+ accepts `go 1.21` as the minimum version directive.
	var b strings.Builder
	b.WriteString("go 1.22\n\n")
	b.WriteString("use (\n")
	for _, u := range uses {
		fmt.Fprintf(&b, "\t%s\n", u)
	}
	b.WriteString(")\n")
	if err := os.WriteFile(filepath.Join(buildDir, "go.work"), []byte(b.String()), 0o644); err != nil {
		return "", err
	}
	return buildDir, nil
}
