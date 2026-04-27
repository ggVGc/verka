package model

import (
	"encoding/json"
	"time"
)

type NodeType string

const (
	TypeDescription    NodeType = "description"
	TypeTask           NodeType = "task"
	TypeImplementation NodeType = "implementation"
	TypeVerification   NodeType = "verification"
	TypeBuild          NodeType = "build"
	TypeArtifact       NodeType = "artifact"
)

func (t NodeType) Valid() bool {
	switch t {
	case TypeDescription, TypeTask, TypeImplementation, TypeVerification, TypeBuild, TypeArtifact:
		return true
	}
	return false
}

type Status string

const (
	StatusDraft   Status = "draft"
	StatusReady   Status = "ready"
	StatusRunning Status = "running"
	StatusPassed  Status = "passed"
	StatusFailed  Status = "failed"
)

func (s Status) Valid() bool {
	switch s {
	case StatusDraft, StatusReady, StatusRunning, StatusPassed, StatusFailed:
		return true
	}
	return false
}

type EdgeKind string

const (
	EdgeChild            EdgeKind = "child"
	EdgeDependsOn        EdgeKind = "depends_on"
	EdgeVerifies         EdgeKind = "verifies"
	EdgeBuilds           EdgeKind = "builds"
	EdgeConsumesArtifact EdgeKind = "consumes_artifact"
	EdgeSupersedes       EdgeKind = "supersedes"
	EdgeCodeDependsOn    EdgeKind = "code_depends_on"
)

func (k EdgeKind) Valid() bool {
	switch k {
	case EdgeChild, EdgeDependsOn, EdgeVerifies, EdgeBuilds, EdgeConsumesArtifact, EdgeSupersedes, EdgeCodeDependsOn:
		return true
	}
	return false
}

type Direction int

const (
	DirOutgoing Direction = iota
	DirIncoming
)

type FileRole string

const (
	FileSource   FileRole = "source"
	FileArtifact FileRole = "artifact"
	FileLog      FileRole = "log"
)

type Node struct {
	ID          string          `json:"id"`
	Type        NodeType        `json:"type"`
	Status      Status          `json:"status"`
	Content     json.RawMessage `json:"content"`
	ContentHash string          `json:"content_hash"`
	CreatedAt   time.Time       `json:"created_at"`
	UpdatedAt   time.Time       `json:"updated_at"`
}

type Edge struct {
	Src  string   `json:"src"`
	Dst  string   `json:"dst"`
	Kind EdgeKind `json:"kind"`
}

type FileRecord struct {
	RelPath string   `json:"rel_path"`
	SHA256  string   `json:"sha256"`
	Size    int64    `json:"size"`
	MtimeNs int64    `json:"mtime_ns"`
	Role    FileRole `json:"role"`
}

type NodePackage struct {
	PackagePath string `json:"package_path"`
	ModulePath  string `json:"module_path"`
}

type InputSnapshot struct {
	InputNode    string `json:"input_node"`
	ObservedHash string `json:"observed_hash"`
}

type RunKind string

const (
	RunVerification RunKind = "verification"
	RunBuild        RunKind = "build"
)

type Run struct {
	ID          int64     `json:"id"`
	NodeID      string    `json:"node_id"`
	Kind        RunKind   `json:"kind"`
	StartedAt   time.Time `json:"started_at"`
	FinishedAt  time.Time `json:"finished_at,omitempty"`
	ExitCode    *int      `json:"exit_code,omitempty"`
	StdoutPath  string    `json:"stdout_path,omitempty"`
	StderrPath  string    `json:"stderr_path,omitempty"`
	ArtifactRel string    `json:"artifact_rel,omitempty"`
}

type RunResult struct {
	ExitCode    int
	StdoutPath  string
	StderrPath  string
	ArtifactRel string
	FinishedAt  time.Time
}
