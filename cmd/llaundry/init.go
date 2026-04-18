package main

import (
	"context"
	"flag"
	"fmt"

	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdInit(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("init", flag.ContinueOnError)
	root := fs.String("root", ".", "project root in which to create .llaundry/")
	if err := fs.Parse(argv); err != nil {
		return err
	}
	created, err := workspace.Init(*root)
	if err != nil {
		return err
	}
	fmt.Printf("initialized llaundry workspace at %s\n", created)
	return nil
}
