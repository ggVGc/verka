package codeanalysis

import "context"

type PackageInfo struct {
	ImportPath string
	Module     string
	Imports    []string
}

type Analyzer interface {
	Name() string
	Detect(dir string) bool
	Analyze(ctx context.Context, dir string) ([]PackageInfo, error)
}

func RunAll(ctx context.Context, analyzers []Analyzer, dir string) ([]PackageInfo, error) {
	for _, a := range analyzers {
		if a.Detect(dir) {
			return a.Analyze(ctx, dir)
		}
	}
	return nil, nil
}

func DefaultAnalyzers() []Analyzer {
	return []Analyzer{&GoAnalyzer{}}
}
