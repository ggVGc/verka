package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"flag"

	"github.com/ggvgc/llaundry/internal/orchestrator"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func cmdRun(ctx context.Context, argv []string) error {
	fs := flag.NewFlagSet("run", flag.ContinueOnError)
	root := fs.String("root", ".", "project root containing .llaundry/")
	agentBin := fs.String("agent-binary", "claude", "LLM agent binary to spawn")
	promptFile := fs.String("system-prompt", "", "path to a custom system prompt (overrides embedded default)")
	userPrompt := fs.String("prompt", "", "user brief (required when no description id is given and stdin is a TTY)")
	maxTurns := fs.Int("max-turns", 50, "max agent turns (0 = agent default)")
	timeout := fs.Duration("timeout", 30*time.Minute, "overall wall-clock budget (0 = no extra timeout)")
	dryRun := fs.Bool("dry-run", false, "print the command that would be run and exit")
	if err := fs.Parse(argv); err != nil {
		return err
	}

	cfg := orchestrator.DefaultConfig()
	cfg.AgentBin = *agentBin
	cfg.MaxTurns = *maxTurns
	cfg.Timeout = *timeout
	cfg.DryRun = *dryRun

	absRoot, err := filepath.Abs(*root)
	if err != nil {
		return fmt.Errorf("resolve root: %w", err)
	}
	cfg.Cwd = absRoot

	if fs.NArg() > 0 {
		cfg.DescriptionID = fs.Arg(0)
	}

	if *promptFile != "" {
		b, err := os.ReadFile(*promptFile)
		if err != nil {
			return fmt.Errorf("read system prompt: %w", err)
		}
		cfg.SystemPromptOverride = string(b)
	}

	// User prompt: flag wins, else read from stdin if piped, else required when
	// there's no description ID.
	switch {
	case *userPrompt != "":
		cfg.UserPrompt = *userPrompt
	case !isTTY(os.Stdin):
		b, err := io.ReadAll(os.Stdin)
		if err != nil {
			return fmt.Errorf("read stdin: %w", err)
		}
		cfg.UserPrompt = strings.TrimSpace(string(b))
	}

	if cfg.DescriptionID == "" && cfg.UserPrompt == "" {
		return fmt.Errorf("either give a description id argument or provide --prompt / pipe a brief on stdin")
	}

	// Pre-flight: open the workspace so we fail fast if .llaundry/ isn't
	// initialized, and if a description ID was given, verify it exists.
	ws, err := workspace.Open(absRoot)
	if err != nil {
		return fmt.Errorf("open workspace: %w", err)
	}
	if cfg.DescriptionID != "" {
		if _, err := ws.Store().GetNode(ctx, cfg.DescriptionID); err != nil {
			ws.Close()
			return fmt.Errorf("description id %q not found: %w", cfg.DescriptionID, err)
		}
	}
	ws.Close()

	return orchestrator.Run(ctx, cfg, os.Stdout, os.Stderr)
}

// isTTY reports whether f is attached to a terminal. Rough but good enough
// for deciding "did the user pipe a prompt to us?".
func isTTY(f *os.File) bool {
	info, err := f.Stat()
	if err != nil {
		return true
	}
	return (info.Mode() & os.ModeCharDevice) != 0
}
