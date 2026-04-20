package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"syscall"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdExec(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("exec", flag.ContinueOnError)
	root := fs.String("root", ".", "project root containing .llaundry/")
	if err := fs.Parse(argv); err != nil {
		return err
	}
	if fs.NArg() < 1 {
		return fmt.Errorf("expected build node ID as first argument")
	}
	buildID := fs.Arg(0)
	childArgs := fs.Args()[1:]

	ws, err := workspace.Open(*root)
	if err != nil {
		return err
	}
	s := ws.Store()

	n, err := s.GetNode(ctx, buildID)
	if err != nil {
		ws.Close()
		return err
	}
	if n.Type != model.TypeBuild {
		ws.Close()
		return fmt.Errorf("node %s is %s, not build", buildID, n.Type)
	}
	if n.Status != model.StatusPassed {
		ws.Close()
		return fmt.Errorf("build %s has status %s; only passed builds can be executed", buildID, n.Status)
	}
	run, err := s.GetLatestRun(ctx, buildID)
	if err != nil {
		ws.Close()
		return err
	}
	if run.ArtifactRel == "" {
		ws.Close()
		return fmt.Errorf("build %s has no recorded artifact", buildID)
	}
	abs := filepath.Join(ws.ArtifactDir(buildID), run.ArtifactRel)
	if _, err := os.Stat(abs); err != nil {
		ws.Close()
		return fmt.Errorf("artifact missing on disk: %w", err)
	}

	ws.Close()
	return syscall.Exec(abs, append([]string{abs}, childArgs...), os.Environ())
}
