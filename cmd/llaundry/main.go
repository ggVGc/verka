package main

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"syscall"
)

const usage = `llaundry — hierarchical LLM coding state server

Usage:
  llaundry init                  Initialize .llaundry/ workspace in cwd
  llaundry mcp                   Run MCP server over stdio
  llaundry show <id>             Print a node's details
  llaundry graph [root-id]       Print an ASCII graph rooted at a description
  llaundry run [desc-id]         Drive the graph autonomously via an LLM agent

Run "llaundry <command> -h" for command-specific flags.
`

func main() {
	if len(os.Args) < 2 {
		fmt.Fprint(os.Stderr, usage)
		os.Exit(2)
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()

	cmd, args := os.Args[1], os.Args[2:]
	var err error
	switch cmd {
	case "init":
		err = cmdInit(ctx, args)
	case "mcp":
		err = cmdMCP(ctx, args)
	case "show":
		err = cmdShow(ctx, args)
	case "graph":
		err = cmdGraph(ctx, args)
	case "run":
		err = cmdRun(ctx, args)
	case "-h", "--help", "help":
		fmt.Print(usage)
		return
	default:
		fmt.Fprintf(os.Stderr, "unknown command: %s\n\n%s", cmd, usage)
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "llaundry %s: %v\n", cmd, err)
		os.Exit(1)
	}
}
