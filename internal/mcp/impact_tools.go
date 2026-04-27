package mcp

import (
	"context"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/store"
	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

type impactAnalysisArgs struct {
	ID string `json:"id"`
}

type impactAnalysisResult struct {
	Provides  []string             `json:"provides"`
	Imports   []string             `json:"imports"`
	Affected  []store.AffectedNode `json:"affected"`
	DependsOn []store.AffectedNode `json:"depends_on"`
}

func (h *handlers) impactAnalysis(ctx context.Context, req *mcpsdk.CallToolRequest, in impactAnalysisArgs) (*mcpsdk.CallToolResult, impactAnalysisResult, error) {
	s := h.store()
	n, err := s.GetNode(ctx, in.ID)
	if err != nil {
		return errf("impact_analysis: %v", err), impactAnalysisResult{}, nil
	}
	if n.Type != model.TypeImplementation {
		return errf("impact_analysis: node %s is %s, not implementation", in.ID, n.Type), impactAnalysisResult{}, nil
	}

	if _, err := h.ws.Rehash(ctx, in.ID); err != nil {
		return errf("rehash: %v", err), impactAnalysisResult{}, nil
	}

	affected, err := s.AffectedImplementations(ctx, in.ID)
	if err != nil {
		return errf("affected: %v", err), impactAnalysisResult{}, nil
	}
	deps, err := s.ImplDependencies(ctx, in.ID)
	if err != nil {
		return errf("dependencies: %v", err), impactAnalysisResult{}, nil
	}

	provides, _ := s.NodePackages(ctx, in.ID)
	imports, _ := s.NodeImports(ctx, in.ID)

	out := impactAnalysisResult{
		Provides:  provides,
		Imports:   imports,
		Affected:  affected,
		DependsOn: deps,
	}
	return ok(out), out, nil
}
