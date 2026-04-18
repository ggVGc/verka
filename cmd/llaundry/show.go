package main

import (
	"context"
	"flag"
	"fmt"
	"os"

	"github.com/ggvgc/llaundry/internal/graph"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdShow(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("show", flag.ContinueOnError)
	root := fs.String("root", ".", "project root containing .llaundry/")
	if err := fs.Parse(argv); err != nil {
		return err
	}
	if fs.NArg() != 1 {
		return fmt.Errorf("expected exactly one node ID argument")
	}
	id := fs.Arg(0)

	ws, err := workspace.Open(*root)
	if err != nil {
		return err
	}
	defer ws.Close()

	return graph.PrintNode(ctx, os.Stdout, ws.Store(), ws, id)
}
