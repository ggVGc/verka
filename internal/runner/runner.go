package runner

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/ggvgc/llaundry/internal/model"
)

// Options configures a single command invocation.
type Options struct {
	Cmd        []string
	Cwd        string
	Env        []string
	LogsDir    string // directory to write <run_id>.stdout and <run_id>.stderr
	RunID      int64
	Timeout    time.Duration
}

// Run executes opts.Cmd in opts.Cwd and captures stdout/stderr to files in
// LogsDir keyed by RunID. It returns a RunResult populated with the exit
// code, log paths, and finish time. A non-zero exit code is NOT an error —
// callers interpret it based on the run kind.
func Run(ctx context.Context, opts Options) (model.RunResult, error) {
	if len(opts.Cmd) == 0 {
		return model.RunResult{}, errors.New("empty command")
	}
	if err := os.MkdirAll(opts.LogsDir, 0o755); err != nil {
		return model.RunResult{}, err
	}
	stdoutPath := filepath.Join(opts.LogsDir, fmt.Sprintf("%d.stdout", opts.RunID))
	stderrPath := filepath.Join(opts.LogsDir, fmt.Sprintf("%d.stderr", opts.RunID))
	stdout, err := os.Create(stdoutPath)
	if err != nil {
		return model.RunResult{}, err
	}
	defer stdout.Close()
	stderr, err := os.Create(stderrPath)
	if err != nil {
		return model.RunResult{}, err
	}
	defer stderr.Close()

	runCtx := ctx
	if opts.Timeout > 0 {
		var cancel context.CancelFunc
		runCtx, cancel = context.WithTimeout(ctx, opts.Timeout)
		defer cancel()
	}

	cmd := exec.CommandContext(runCtx, opts.Cmd[0], opts.Cmd[1:]...)
	cmd.Dir = opts.Cwd
	if len(opts.Env) > 0 {
		cmd.Env = opts.Env
	} else {
		cmd.Env = os.Environ()
	}
	cmd.Stdout = stdout
	cmd.Stderr = stderr

	runErr := cmd.Run()
	exit := 0
	if runErr != nil {
		var ee *exec.ExitError
		if errors.As(runErr, &ee) {
			exit = ee.ExitCode()
		} else {
			exit = -1
		}
	}
	return model.RunResult{
		ExitCode:   exit,
		StdoutPath: stdoutPath,
		StderrPath: stderrPath,
		FinishedAt: time.Now().UTC(),
	}, nil
}
