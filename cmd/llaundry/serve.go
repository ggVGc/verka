package main

import (
	"context"
	"flag"

	"github.com/ggvgc/llaundry/internal/web"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdServe(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("serve", flag.ContinueOnError)
	root := fs.String("root", ".", "project root containing .llaundry/")
	addr := fs.String("addr", "127.0.0.1:7777", "HTTP listen address")
	if err := fs.Parse(argv); err != nil {
		return err
	}
	ws, err := workspace.Open(*root)
	if err != nil {
		return err
	}
	defer ws.Close()
	return web.Serve(ctx, ws, *addr)
}
