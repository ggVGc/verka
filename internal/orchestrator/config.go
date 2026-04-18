package orchestrator

import (
	"errors"
	"os"
	"time"
)

// Config describes one invocation of the llaundry orchestrator agent.
//
// The zero-value is not usable; use DefaultConfig() and override fields.
type Config struct {
	// Cwd is the directory containing .llaundry/. The spawned MCP server is
	// pointed at this root so the agent operates on the caller's workspace.
	Cwd string

	// LlaundryBin is the absolute path to the llaundry binary used for the
	// child MCP server process. Defaults to os.Executable() at runtime.
	LlaundryBin string

	// AgentBin is the LLM agent binary to spawn. Default: "claude".
	AgentBin string

	// DescriptionID, if non-empty, pins the run to an existing description
	// node; if empty, UserPrompt is passed as the bootstrapping instruction.
	DescriptionID string

	// UserPrompt is the initial instruction to the agent. Required when
	// DescriptionID is empty.
	UserPrompt string

	// SystemPromptOverride, if non-empty, replaces the embedded system prompt.
	SystemPromptOverride string

	// MaxTurns caps the number of agent turns. 0 means leave to the agent's
	// default.
	MaxTurns int

	// Timeout is the overall wall-clock budget for the agent run. 0 means no
	// additional timeout beyond the caller's context.
	Timeout time.Duration

	// DryRun prints the command that would be executed and returns.
	DryRun bool
}

// DefaultConfig returns a Config with documented defaults filled in.
func DefaultConfig() Config {
	return Config{
		AgentBin: "claude",
		MaxTurns: 50,
		Timeout:  30 * time.Minute,
	}
}

// Validate reports whether the config is usable.
func (c *Config) Validate() error {
	if c.Cwd == "" {
		return errors.New("Cwd is required")
	}
	if c.AgentBin == "" {
		return errors.New("AgentBin is required")
	}
	if c.DescriptionID == "" && c.UserPrompt == "" {
		return errors.New("either DescriptionID or UserPrompt must be set")
	}
	return nil
}

// resolveLlaundryBin returns c.LlaundryBin or falls back to os.Executable().
func (c *Config) resolveLlaundryBin() (string, error) {
	if c.LlaundryBin != "" {
		return c.LlaundryBin, nil
	}
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	return exe, nil
}
