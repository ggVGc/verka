package mcp

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/runner"
	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

// --- 11. run_verification ---

type runVerificationArgs struct {
	ID      string `json:"id"`
	Timeout int    `json:"timeout_seconds,omitempty" jsonschema:"hard time limit for the run in seconds (default: 300)"`
}

type runResultView struct {
	RunID      int64  `json:"run_id"`
	ExitCode   int    `json:"exit_code"`
	Status     string `json:"status"`
	StdoutPath string `json:"stdout_path"`
	StderrPath string `json:"stderr_path"`
	StdoutTail string `json:"stdout_tail"`
	StderrTail string `json:"stderr_tail"`
	ArtifactRel string `json:"artifact_rel,omitempty"`
}

func (h *handlers) runVerification(ctx context.Context, req *mcpsdk.CallToolRequest, in runVerificationArgs) (*mcpsdk.CallToolResult, runResultView, error) {
	s := h.store()
	n, err := s.GetNode(ctx, in.ID)
	if err != nil {
		return errf("run_verification: %v", err), runResultView{}, nil
	}
	if n.Type != model.TypeVerification {
		return errf("node %s is %s, not verification", in.ID, n.Type), runResultView{}, nil
	}
	deps, err := s.Neighbors(ctx, in.ID, model.EdgeDependsOn, model.DirOutgoing)
	if err != nil {
		return errf("neighbors: %v", err), runResultView{}, nil
	}
	if len(deps) == 0 {
		return errf("verification %s has no depends_on inputs", in.ID), runResultView{}, nil
	}
	// v1: run in the source dir of the first dependency that's an implementation
	// or build. If the dep has produced artifacts we just run against the impl's
	// source dir; artifact-driven verifications are a later refinement.
	cwd := ""
	for _, dep := range deps {
		dn, err := s.GetNode(ctx, dep)
		if err != nil {
			continue
		}
		if dn.Type == model.TypeImplementation {
			cwd = h.ws.SourceDir(dep)
			break
		}
		if dn.Type == model.TypeBuild {
			cwd = h.ws.BuildDir(dep)
			break
		}
	}
	if cwd == "" {
		return errf("verification %s: no implementation or build input found among dependencies", in.ID), runResultView{}, nil
	}

	cmd := runner.DefaultVerifyCmd()

	_ = s.SetStatus(ctx, in.ID, model.StatusRunning, "")
	runID, err := s.StartRun(ctx, in.ID, model.RunVerification)
	if err != nil {
		return errf("start_run: %v", err), runResultView{}, nil
	}

	// Snapshot inputs at start — we compare against current hashes later for
	// staleness.
	snaps, err := collectInputSnapshots(ctx, h, in.ID, deps)
	if err != nil {
		return errf("input snapshots: %v", err), runResultView{}, nil
	}
	if err := s.RecordInputSnapshots(ctx, runID, in.ID, snaps); err != nil {
		return errf("record snapshots: %v", err), runResultView{}, nil
	}

	timeout := time.Duration(in.Timeout) * time.Second
	if timeout <= 0 {
		timeout = 5 * time.Minute
	}
	result, err := runner.Run(ctx, runner.Options{
		Cmd:     cmd,
		Cwd:     cwd,
		LogsDir: h.ws.LogsDir(),
		RunID:   runID,
		Timeout: timeout,
	})
	if err != nil {
		_ = s.SetStatus(ctx, in.ID, model.StatusFailed, err.Error())
		return errf("run: %v", err), runResultView{}, nil
	}
	if err := s.FinishRun(ctx, runID, result); err != nil {
		return errf("finish_run: %v", err), runResultView{}, nil
	}
	status := model.StatusPassed
	if result.ExitCode != 0 {
		status = model.StatusFailed
	}
	_ = s.SetStatus(ctx, in.ID, status, "")

	view := runResultView{
		RunID:      runID,
		ExitCode:   result.ExitCode,
		Status:     string(status),
		StdoutPath: result.StdoutPath,
		StderrPath: result.StderrPath,
		StdoutTail: readTail(result.StdoutPath),
		StderrTail: readTail(result.StderrPath),
	}
	return ok(view), view, nil
}

// --- 12. run_build ---

type runBuildArgs struct {
	ID      string `json:"id"`
	Timeout int    `json:"timeout_seconds,omitempty" jsonschema:"hard time limit for the run in seconds (default: 600)"`
}

func (h *handlers) runBuild(ctx context.Context, req *mcpsdk.CallToolRequest, in runBuildArgs) (*mcpsdk.CallToolResult, runResultView, error) {
	s := h.store()
	n, err := s.GetNode(ctx, in.ID)
	if err != nil {
		return errf("run_build: %v", err), runResultView{}, nil
	}
	if n.Type != model.TypeBuild {
		return errf("node %s is %s, not build", in.ID, n.Type), runResultView{}, nil
	}
	// AssembleBuild generates a go.work for provenance, but `go build` runs
	// inside the single implementation's source directory.
	if _, err := h.ws.AssembleBuild(ctx, in.ID); err != nil {
		return errf("assemble: %v", err), runResultView{}, nil
	}
	artifactDir := h.ws.ArtifactDir(in.ID)
	if err := os.MkdirAll(artifactDir, 0o755); err != nil {
		return errf("artifact dir: %v", err), runResultView{}, nil
	}

	deps, err := s.Neighbors(ctx, in.ID, model.EdgeDependsOn, model.DirOutgoing)
	if err != nil {
		return errf("neighbors: %v", err), runResultView{}, nil
	}
	var implDeps []string
	for _, d := range deps {
		dn, err := s.GetNode(ctx, d)
		if err == nil && dn.Type == model.TypeImplementation {
			implDeps = append(implDeps, d)
		}
	}
	if len(implDeps) != 1 {
		return errf("build %s: exactly one implementation dependency required (got %d)", in.ID, len(implDeps)), runResultView{}, nil
	}
	cwd := h.ws.SourceDir(implDeps[0])
	cmd := runner.DefaultBuildCmd(artifactDir)

	_ = s.SetStatus(ctx, in.ID, model.StatusRunning, "")
	runID, err := s.StartRun(ctx, in.ID, model.RunBuild)
	if err != nil {
		return errf("start_run: %v", err), runResultView{}, nil
	}
	snaps, err := collectInputSnapshots(ctx, h, in.ID, deps)
	if err != nil {
		return errf("input snapshots: %v", err), runResultView{}, nil
	}
	if err := s.RecordInputSnapshots(ctx, runID, in.ID, snaps); err != nil {
		return errf("record snapshots: %v", err), runResultView{}, nil
	}

	timeout := time.Duration(in.Timeout) * time.Second
	if timeout <= 0 {
		timeout = 10 * time.Minute
	}
	result, err := runner.Run(ctx, runner.Options{
		Cmd:     cmd,
		Cwd:     cwd,
		LogsDir: h.ws.LogsDir(),
		RunID:   runID,
		Timeout: timeout,
	})
	if err != nil {
		_ = s.SetStatus(ctx, in.ID, model.StatusFailed, err.Error())
		return errf("run: %v", err), runResultView{}, nil
	}

	// Pick a primary artifact if any were produced.
	if result.ExitCode == 0 {
		if art, err := firstArtifact(artifactDir); err == nil && art != "" {
			result.ArtifactRel = art
		}
	}
	if err := s.FinishRun(ctx, runID, result); err != nil {
		return errf("finish_run: %v", err), runResultView{}, nil
	}
	status := model.StatusPassed
	if result.ExitCode != 0 {
		status = model.StatusFailed
	}
	_ = s.SetStatus(ctx, in.ID, status, "")

	view := runResultView{
		RunID:       runID,
		ExitCode:    result.ExitCode,
		Status:      string(status),
		StdoutPath:  result.StdoutPath,
		StderrPath:  result.StderrPath,
		StdoutTail:  readTail(result.StdoutPath),
		StderrTail:  readTail(result.StderrPath),
		ArtifactRel: result.ArtifactRel,
	}
	return ok(view), view, nil
}

// --- 13. attach_run_result ---

type attachRunResultArgs struct {
	ID          string `json:"id"`
	Kind        string `json:"kind" jsonschema:"one of: verification, build"`
	ExitCode    int    `json:"exit_code"`
	Stdout      string `json:"stdout,omitempty"`
	Stderr      string `json:"stderr,omitempty"`
	ArtifactRel string `json:"artifact_rel,omitempty"`
}

func (h *handlers) attachRunResult(ctx context.Context, req *mcpsdk.CallToolRequest, in attachRunResultArgs) (*mcpsdk.CallToolResult, runResultView, error) {
	s := h.store()
	n, err := s.GetNode(ctx, in.ID)
	if err != nil {
		return errf("attach: %v", err), runResultView{}, nil
	}
	kind := model.RunKind(in.Kind)
	if kind != model.RunVerification && kind != model.RunBuild {
		return errf("invalid kind %q", in.Kind), runResultView{}, nil
	}
	if kind == model.RunVerification && n.Type != model.TypeVerification {
		return errf("node %s is %s, not verification", in.ID, n.Type), runResultView{}, nil
	}
	if kind == model.RunBuild && n.Type != model.TypeBuild {
		return errf("node %s is %s, not build", in.ID, n.Type), runResultView{}, nil
	}

	runID, err := s.StartRun(ctx, in.ID, kind)
	if err != nil {
		return errf("start_run: %v", err), runResultView{}, nil
	}

	deps, _ := s.Neighbors(ctx, in.ID, model.EdgeDependsOn, model.DirOutgoing)
	snaps, err := collectInputSnapshots(ctx, h, in.ID, deps)
	if err != nil {
		return errf("input snapshots: %v", err), runResultView{}, nil
	}
	if err := s.RecordInputSnapshots(ctx, runID, in.ID, snaps); err != nil {
		return errf("record snapshots: %v", err), runResultView{}, nil
	}

	stdoutPath := filepath.Join(h.ws.LogsDir(), fmt.Sprintf("%d.stdout", runID))
	stderrPath := filepath.Join(h.ws.LogsDir(), fmt.Sprintf("%d.stderr", runID))
	_ = os.MkdirAll(h.ws.LogsDir(), 0o755)
	_ = os.WriteFile(stdoutPath, []byte(in.Stdout), 0o644)
	_ = os.WriteFile(stderrPath, []byte(in.Stderr), 0o644)

	res := model.RunResult{
		ExitCode:    in.ExitCode,
		StdoutPath:  stdoutPath,
		StderrPath:  stderrPath,
		ArtifactRel: in.ArtifactRel,
		FinishedAt:  time.Now().UTC(),
	}
	if err := s.FinishRun(ctx, runID, res); err != nil {
		return errf("finish_run: %v", err), runResultView{}, nil
	}
	status := model.StatusPassed
	if in.ExitCode != 0 {
		status = model.StatusFailed
	}
	_ = s.SetStatus(ctx, in.ID, status, "")

	view := runResultView{
		RunID:       runID,
		ExitCode:    in.ExitCode,
		Status:      string(status),
		StdoutPath:  stdoutPath,
		StderrPath:  stderrPath,
		StdoutTail:  truncate(in.Stdout, 10*1024),
		StderrTail:  truncate(in.Stderr, 10*1024),
		ArtifactRel: in.ArtifactRel,
	}
	return ok(view), view, nil
}

// --- helpers ---

// collectInputSnapshots records the current content_hash of every dependency
// of id, so a later UpdateNodeContent (or rehash) on any input will surface
// the observer as stale.
func collectInputSnapshots(ctx context.Context, h *handlers, id string, deps []string) ([]model.InputSnapshot, error) {
	s := h.store()
	out := make([]model.InputSnapshot, 0, len(deps))
	for _, d := range deps {
		dn, err := s.GetNode(ctx, d)
		if err != nil {
			continue
		}
		out = append(out, model.InputSnapshot{InputNode: d, ObservedHash: dn.ContentHash})
	}
	return out, nil
}

// readTail returns the last 10 KB of the file at path; empty string on error.
func readTail(path string) string {
	const limit = 10 * 1024
	b, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	if len(b) <= limit {
		return string(b)
	}
	return "…[truncated]\n" + string(b[len(b)-limit:])
}

// firstArtifact returns the relative path (under artifactDir) of the first
// regular file found; empty string if none.
func firstArtifact(artifactDir string) (string, error) {
	entries, err := os.ReadDir(artifactDir)
	if err != nil {
		return "", err
	}
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		info, err := e.Info()
		if err != nil || !info.Mode().IsRegular() {
			continue
		}
		return e.Name(), nil
	}
	return "", nil
}

// keep strings import satisfied if no other handler uses it
var _ = strings.TrimSpace
