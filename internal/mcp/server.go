package mcp

import (
	"context"
	"os"

	"github.com/ggvgc/llaundry/internal/workspace"
	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

// Serve starts the MCP stdio server against the given workspace. It blocks
// until stdin closes or ctx is cancelled.
func Serve(ctx context.Context, ws *workspace.Workspace) error {
	srv := mcpsdk.NewServer(&mcpsdk.Implementation{
		Name:    "llaundry",
		Version: "0.1.0",
	}, nil)

	h := &handlers{ws: ws}
	registerTools(srv, h)

	transport := &mcpsdk.LoggingTransport{
		Transport: &mcpsdk.StdioTransport{},
		Writer:    os.Stderr,
	}
	return srv.Run(ctx, transport)
}
