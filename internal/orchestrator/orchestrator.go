// Package orchestrator spawns an LLM agent subprocess that drives the
// llaundry graph to completion using ONLY the llaundry MCP tools.
//
// The agent runtime is pluggable but defaults to Claude CLI (`claude`). The
// subprocess is launched with no built-in tools — the only tools it can call
// are those exposed by the llaundry MCP server we register for it.
package orchestrator

import (
	_ "embed"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"time"
)

//go:embed prompts/system.md
var embeddedSystemPrompt string

// Plan is a side-effect-free rendering of what Run would execute. Returned by
// BuildPlan; useful for --dry-run and tests.
type Plan struct {
	AgentBin     string
	Argv         []string
	MCPConfig    json.RawMessage
	SystemPrompt string
	UserPrompt   string
	LogPath      string
}

// BuildPlan returns the concrete command invocation for a given config
// without launching anything.
func BuildPlan(cfg Config) (*Plan, error) {
	if err := cfg.Validate(); err != nil {
		return nil, err
	}
	llaundryBin, err := cfg.resolveLlaundryBin()
	if err != nil {
		return nil, fmt.Errorf("resolve llaundry binary: %w", err)
	}
	mcpCfg, err := buildMCPConfig(llaundryBin, cfg.Cwd)
	if err != nil {
		return nil, err
	}
	sys := embeddedSystemPrompt
	if cfg.SystemPromptOverride != "" {
		sys = cfg.SystemPromptOverride
	}
	user := renderUserPrompt(cfg)

	logPath := filepath.Join(cfg.Cwd, ".llaundry", "logs",
		fmt.Sprintf("orchestrator-%d.ndjson", time.Now().UnixNano()))

	argv := []string{
		"--bare",
		"-p", user,
		"--mcp-config", string(mcpCfg),
		"--allowedTools", "mcp__llaundry__*",
		"--permission-mode", "dontAsk",
		"--output-format", "stream-json",
		"--verbose",
	}
	if cfg.MaxTurns > 0 {
		argv = append(argv, "--max-turns", strconv.Itoa(cfg.MaxTurns))
	}
	return &Plan{
		AgentBin:     cfg.AgentBin,
		Argv:         argv,
		MCPConfig:    mcpCfg,
		SystemPrompt: sys,
		UserPrompt:   user,
		LogPath:      logPath,
	}, nil
}

// Run spawns the agent and streams progress to stdout. If cfg.DryRun is set,
// prints the planned command and returns.
func Run(ctx context.Context, cfg Config, stdout, stderr io.Writer) error {
	plan, err := BuildPlan(cfg)
	if err != nil {
		return err
	}
	if cfg.DryRun {
		return printPlan(stdout, plan)
	}

	// Materialize the system prompt to a tempfile and pass it via
	// --append-system-prompt-file. (Claude CLI historically also accepts
	// --append-system-prompt with an inline string but the file form avoids
	// argv-length concerns for larger prompts.)
	sysFile, err := os.CreateTemp("", "llaundry-sys-*.md")
	if err != nil {
		return fmt.Errorf("create system-prompt tempfile: %w", err)
	}
	defer os.Remove(sysFile.Name())
	if _, err := sysFile.WriteString(plan.SystemPrompt); err != nil {
		sysFile.Close()
		return fmt.Errorf("write system-prompt tempfile: %w", err)
	}
	if err := sysFile.Close(); err != nil {
		return fmt.Errorf("close system-prompt tempfile: %w", err)
	}

	argv := append([]string{}, plan.Argv...)
	argv = append(argv, "--append-system-prompt-file", sysFile.Name())

	if cfg.Timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, cfg.Timeout)
		defer cancel()
	}

	if err := os.MkdirAll(filepath.Dir(plan.LogPath), 0o755); err != nil {
		return fmt.Errorf("ensure logs dir: %w", err)
	}
	logFile, err := os.Create(plan.LogPath)
	if err != nil {
		return fmt.Errorf("create orchestrator log: %w", err)
	}
	defer logFile.Close()

	cmd := exec.CommandContext(ctx, cfg.AgentBin, argv...)
	cmd.Dir = cfg.Cwd
	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("stdout pipe: %w", err)
	}
	cmd.Stderr = stderr

	fmt.Fprintf(stderr, "orchestrator: spawning %s (cwd=%s, log=%s)\n",
		cfg.AgentBin, cfg.Cwd, plan.LogPath)
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("start agent: %w", err)
	}

	// Tee the stream to both the log file (raw) and the pretty-printer.
	tee := io.TeeReader(stdoutPipe, logFile)
	if err := filterStream(tee, stdout); err != nil {
		// Keep draining so Wait() can return cleanly.
		_, _ = io.Copy(io.Discard, stdoutPipe)
	}

	if werr := cmd.Wait(); werr != nil {
		return fmt.Errorf("agent exited with error: %w", werr)
	}
	return nil
}

// buildMCPConfig returns the JSON payload for `claude --mcp-config`.
// It registers a single server named "llaundry" that spawns
// `<llaundryBin> mcp -root <cwd>` over stdio.
func buildMCPConfig(llaundryBin, cwd string) (json.RawMessage, error) {
	cfg := map[string]any{
		"mcpServers": map[string]any{
			"llaundry": map[string]any{
				"type":    "stdio",
				"command": llaundryBin,
				"args":    []string{"mcp", "-root", cwd},
			},
		},
	}
	return json.Marshal(cfg)
}

func renderUserPrompt(cfg Config) string {
	if cfg.DescriptionID != "" {
		base := fmt.Sprintf(
			"Continue driving the llaundry graph rooted at description id %q "+
				"to a state where every task has a passing verification and a "+
				"passing build. Follow the workflow in the system prompt. "+
				"Use only the llaundry MCP tools.",
			cfg.DescriptionID)
		if cfg.UserPrompt != "" {
			base += "\n\nAdditional user note:\n" + cfg.UserPrompt
		}
		return base
	}
	return "Create a new llaundry description for the following user brief, " +
		"plan tasks under it, implement, verify, and build. Follow the " +
		"workflow in the system prompt. Use only the llaundry MCP tools.\n\n" +
		"User brief:\n" + cfg.UserPrompt
}

func printPlan(w io.Writer, p *Plan) error {
	fmt.Fprintln(w, "=== llaundry run — dry run ===")
	fmt.Fprintf(w, "agent binary: %s\n", p.AgentBin)
	fmt.Fprintln(w, "argv:")
	for _, a := range p.Argv {
		fmt.Fprintf(w, "  %s\n", a)
	}
	fmt.Fprintln(w, "mcp-config:")
	var pretty any
	_ = json.Unmarshal(p.MCPConfig, &pretty)
	b, _ := json.MarshalIndent(pretty, "  ", "  ")
	fmt.Fprintf(w, "  %s\n", string(b))
	fmt.Fprintf(w, "log will be written to: %s\n", p.LogPath)
	fmt.Fprintln(w, "user prompt:")
	fmt.Fprintf(w, "  %s\n", p.UserPrompt)
	fmt.Fprintln(w, "(system prompt embedded; pass --system-prompt to override)")
	return nil
}
