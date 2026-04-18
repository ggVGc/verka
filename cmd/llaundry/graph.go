package main

import (
	"context"
	"flag"
	"os"

	"github.com/ggvgc/llaundry/internal/graph"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdGraph(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("graph", flag.ContinueOnError)
	root := fs.String("root", ".", "project root containing .llaundry/")
	if err := fs.Parse(argv); err != nil {
		return err
	}
	rootID := ""
	if fs.NArg() >= 1 {
		rootID = fs.Arg(0)
	}

	ws, err := workspace.Open(*root)
	if err != nil {
		return err
	}
	defer ws.Close()

	return graph.PrintTree(ctx, os.Stdout, ws.Store(), rootID)
}
