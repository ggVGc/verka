package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/store"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdArtifacts(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("artifacts", flag.ContinueOnError)
	root := fs.String("root", ".", "project root containing .llaundry/")
	if err := fs.Parse(argv); err != nil {
		return err
	}

	ws, err := workspace.Open(*root)
	if err != nil {
		return err
	}
	defer ws.Close()
	s := ws.Store()

	builds, err := s.ListNodes(ctx, store.NodeFilter{
		Type:   model.TypeBuild,
		Status: model.StatusPassed,
	})
	if err != nil {
		return err
	}
	if len(builds) == 0 {
		fmt.Fprintln(os.Stdout, "(no passed builds)")
		return nil
	}
	for _, b := range builds {
		run, err := s.GetLatestRun(ctx, b.ID)
		if err != nil && !errors.Is(err, store.ErrNotFound) {
			return err
		}
		if run == nil || run.ArtifactRel == "" {
			fmt.Printf("%s\t(no artifact recorded)\n", b.ID)
			continue
		}
		abs := filepath.Join(ws.ArtifactDir(b.ID), run.ArtifactRel)
		fmt.Printf("%s\t%s\n", b.ID, abs)
	}
	return nil
}
