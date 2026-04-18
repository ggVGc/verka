package main

import (
	"context"
	"flag"
	"fmt"

	"github.com/ggvgc/llaundry/internal/mcp"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdMCP(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("mcp", flag.ContinueOnError)
	root := fs.String("root", ".", "project root containing .llaundry/")
	if err := fs.Parse(argv); err != nil {
		return err
	}
	ws, err := workspace.Open(*root)
	if err != nil {
		return fmt.Errorf("open workspace: %w", err)
	}
	defer ws.Close()

	return mcp.Serve(ctx, ws)
}
