package mcp

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/store"
	"github.com/ggvgc/llaundry/internal/workspace"
	"github.com/oklog/ulid/v2"
	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

type handlers struct {
	ws *workspace.Workspace
}

func (h *handlers) store() *store.SQLite { return h.ws.Store() }

func registerTools(srv *mcpsdk.Server, h *handlers) {
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "create_node",
		Description: "Create a new node (description|task|implementation|verification|build|artifact). Optional parent links a child edge; optional depends_on creates dependency edges.",
	}, h.createNode)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "get_node",
		Description: "Fetch a node by ID, optionally including its file list and edges.",
	}, h.getNode)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "list_nodes",
		Description: "List nodes matching a filter (type, status, parent, stale).",
	}, h.listNodes)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "update_node_content",
		Description: "Replace the content JSON of a node. Recomputes the node's content_hash.",
	}, h.updateNodeContent)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "set_status",
		Description: "Explicitly transition a node's status. Does not affect content_hash.",
	}, h.setStatus)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "link",
		Description: "Add a graph edge from src to dst with a given kind (child|depends_on|verifies|builds|consumes_artifact|supersedes|code_depends_on).",
	}, h.link)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "unlink",
		Description: "Remove a graph edge.",
	}, h.unlink)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "node_files",
		Description: "Operate on files under a node's source directory. op ∈ {list, read, write, delete}. Writes are capped at 256 KB; for larger files use get_workspace_path and edit directly, then call rehash.",
	}, h.nodeFiles)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "get_workspace_path",
		Description: "Return absolute paths to a node's on-disk directory and its source/ subdirectory. Use these with native filesystem tools; call rehash when done.",
	}, h.getWorkspacePath)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "rehash",
		Description: "Re-scan a node's source directory, update file records (with mtime fast-path), and recompute content_hash.",
	}, h.rehash)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "run_verification",
		Description: "Run `go test ./...` inside the verification's first implementation-dependency source dir. Captures stdout/stderr to logs, records input snapshots, updates node status.",
	}, h.runVerification)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "run_build",
		Description: "Run `go build -o <artifact>/ ./...` after assembling a go.work over its depends_on implementations. Captures artifacts into the artifact/ directory.",
	}, h.runBuild)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "attach_run_result",
		Description: "Record the result of an externally executed run (e.g., from CI) against a verification or build node. Creates input snapshots from the node's current dependencies.",
	}, h.attachRunResult)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "impact_analysis",
		Description: "Analyze code-level dependencies for an implementation node. Returns: what Go packages it provides, what it imports, which other implementations would be affected if it changes, and which implementations it depends on. Triggers a rehash to ensure the symbol index is current.",
	}, h.impactAnalysis)
	mcpsdk.AddTool(srv, &mcpsdk.Tool{
		Name:        "ask_user",
		Description: "Ask the human user a question and block until they answer. Use this to clarify an ambiguous brief and to get explicit approval before creating task nodes. Returns the user's free-form text answer.",
	}, h.askUser)
}

// --- 1. create_node ---

type createNodeArgs struct {
	Type      string   `json:"type" jsonschema:"node type: description|task|implementation|verification|build|artifact"`
	Parent    string   `json:"parent,omitempty" jsonschema:"optional parent node ID; a child edge is added from parent to the new node"`
	DependsOn []string `json:"depends_on,omitempty" jsonschema:"optional list of node IDs to add depends_on edges to"`
	Content   any      `json:"content,omitempty" jsonschema:"JSON content payload; interpretation is node-type specific"`
	Status    string   `json:"status,omitempty" jsonschema:"optional starting status (default: draft for description, ready otherwise)"`
}

type createNodeResult struct {
	ID          string `json:"id"`
	ContentHash string `json:"content_hash"`
}

func (h *handlers) createNode(ctx context.Context, req *mcpsdk.CallToolRequest, in createNodeArgs) (*mcpsdk.CallToolResult, createNodeResult, error) {
	typ := model.NodeType(in.Type)
	if !typ.Valid() {
		return errf("invalid type %q", in.Type), createNodeResult{}, nil
	}
	status := model.Status(in.Status)
	if status == "" {
		if typ == model.TypeDescription {
			status = model.StatusDraft
		} else {
			status = model.StatusReady
		}
	}
	if !status.Valid() {
		return errf("invalid status %q", in.Status), createNodeResult{}, nil
	}
	contentRaw, err := toRawJSON(in.Content)
	if err != nil {
		return errf("content: %v", err), createNodeResult{}, nil
	}
	id := newULID()
	n := &model.Node{
		ID:      id,
		Type:    typ,
		Status:  status,
		Content: contentRaw,
	}
	if err := h.store().CreateNode(ctx, n); err != nil {
		return errf("create_node: %v", err), createNodeResult{}, nil
	}
	if in.Parent != "" {
		if err := h.store().Link(ctx, in.Parent, id, model.EdgeChild); err != nil {
			return errf("link parent: %v", err), createNodeResult{}, nil
		}
	}
	for _, dep := range in.DependsOn {
		if err := h.store().Link(ctx, id, dep, model.EdgeDependsOn); err != nil {
			return errf("link depends_on %s: %v", dep, err), createNodeResult{}, nil
		}
	}
	if err := h.ws.Materialize(ctx, id); err != nil {
		return errf("materialize: %v", err), createNodeResult{}, nil
	}
	out := createNodeResult{ID: id, ContentHash: n.ContentHash}
	return ok(out), out, nil
}

// --- 2. get_node ---

type getNodeArgs struct {
	ID           string `json:"id"`
	IncludeFiles bool   `json:"include_files,omitempty"`
	IncludeEdges bool   `json:"include_edges,omitempty"`
}

type getNodeResult struct {
	Node  nodeView            `json:"node"`
	Files []model.FileRecord  `json:"files,omitempty"`
	Edges []edgeView          `json:"edges,omitempty"`
	Run   *runView            `json:"latest_run,omitempty"`
}

type nodeView struct {
	ID          string         `json:"id"`
	Type        model.NodeType `json:"type"`
	Status      model.Status   `json:"status"`
	ContentHash string         `json:"content_hash"`
	Content     any            `json:"content"`
	CreatedAt   time.Time      `json:"created_at"`
	UpdatedAt   time.Time      `json:"updated_at"`
	Stale       bool           `json:"stale,omitempty"`
}

type edgeView struct {
	Src  string         `json:"src"`
	Dst  string         `json:"dst"`
	Kind model.EdgeKind `json:"kind"`
}

type runView struct {
	ID          int64     `json:"id"`
	Kind        string    `json:"kind"`
	StartedAt   time.Time `json:"started_at"`
	FinishedAt  time.Time `json:"finished_at,omitempty"`
	ExitCode    *int      `json:"exit_code,omitempty"`
	StdoutPath  string    `json:"stdout_path,omitempty"`
	StderrPath  string    `json:"stderr_path,omitempty"`
	ArtifactRel string    `json:"artifact_rel,omitempty"`
}

func (h *handlers) getNode(ctx context.Context, req *mcpsdk.CallToolRequest, in getNodeArgs) (*mcpsdk.CallToolResult, getNodeResult, error) {
	n, err := h.store().GetNode(ctx, in.ID)
	if err != nil {
		return errf("get_node %s: %v", in.ID, err), getNodeResult{}, nil
	}
	stale, _ := h.store().StaleNodes(ctx)
	isStale := false
	for _, s := range stale {
		if s == n.ID {
			isStale = true
			break
		}
	}
	if !isStale {
		staleImpls, _ := h.store().StaleImplementations(ctx)
		for _, s := range staleImpls {
			if s == n.ID {
				isStale = true
				break
			}
		}
	}
	var contentAny any
	if err := json.Unmarshal(n.Content, &contentAny); err != nil {
		contentAny = string(n.Content)
	}
	view := getNodeResult{
		Node: nodeView{
			ID:          n.ID,
			Type:        n.Type,
			Status:      n.Status,
			ContentHash: n.ContentHash,
			Content:     contentAny,
			CreatedAt:   n.CreatedAt,
			UpdatedAt:   n.UpdatedAt,
			Stale:       isStale,
		},
	}
	if in.IncludeFiles {
		files, err := h.store().ListFiles(ctx, in.ID)
		if err != nil {
			return errf("list files: %v", err), getNodeResult{}, nil
		}
		view.Files = files
	}
	if in.IncludeEdges {
		edges, err := h.store().EdgesFor(ctx, in.ID)
		if err != nil {
			return errf("edges: %v", err), getNodeResult{}, nil
		}
		for _, e := range edges {
			view.Edges = append(view.Edges, edgeView{Src: e.Src, Dst: e.Dst, Kind: e.Kind})
		}
	}
	if run, err := h.store().GetLatestRun(ctx, in.ID); err == nil && run != nil {
		view.Run = &runView{
			ID:          run.ID,
			Kind:        string(run.Kind),
			StartedAt:   run.StartedAt,
			FinishedAt:  run.FinishedAt,
			ExitCode:    run.ExitCode,
			StdoutPath:  run.StdoutPath,
			StderrPath:  run.StderrPath,
			ArtifactRel: run.ArtifactRel,
		}
	}
	return ok(view), view, nil
}

// --- 3. list_nodes ---

type listNodesArgs struct {
	Type   string `json:"type,omitempty"`
	Status string `json:"status,omitempty"`
	Parent string `json:"parent,omitempty"`
	Stale  bool   `json:"stale,omitempty"`
	Limit  int    `json:"limit,omitempty"`
	Offset int    `json:"offset,omitempty"`
}

type listNodesResult struct {
	Nodes []nodeSummary `json:"nodes"`
}

type nodeSummary struct {
	ID          string         `json:"id"`
	Type        model.NodeType `json:"type"`
	Status      model.Status   `json:"status"`
	ContentHash string         `json:"content_hash"`
	Preview     string         `json:"preview"`
}

func (h *handlers) listNodes(ctx context.Context, req *mcpsdk.CallToolRequest, in listNodesArgs) (*mcpsdk.CallToolResult, listNodesResult, error) {
	nodes, err := h.store().ListNodes(ctx, store.NodeFilter{
		Type:   model.NodeType(in.Type),
		Status: model.Status(in.Status),
		Parent: in.Parent,
		Stale:  in.Stale,
		Limit:  in.Limit,
		Offset: in.Offset,
	})
	if err != nil {
		return errf("list_nodes: %v", err), listNodesResult{}, nil
	}
	out := listNodesResult{Nodes: make([]nodeSummary, 0, len(nodes))}
	for _, n := range nodes {
		out.Nodes = append(out.Nodes, nodeSummary{
			ID:          n.ID,
			Type:        n.Type,
			Status:      n.Status,
			ContentHash: n.ContentHash,
			Preview:     preview(string(n.Content)),
		})
	}
	return ok(out), out, nil
}

// --- 4. update_node_content ---

type updateNodeContentArgs struct {
	ID      string `json:"id"`
	Content any    `json:"content" jsonschema:"new JSON content payload for the node"`
}
type updateNodeContentResult struct {
	ContentHash string `json:"content_hash"`
}

func (h *handlers) updateNodeContent(ctx context.Context, req *mcpsdk.CallToolRequest, in updateNodeContentArgs) (*mcpsdk.CallToolResult, updateNodeContentResult, error) {
	raw, err := toRawJSON(in.Content)
	if err != nil {
		return errf("content: %v", err), updateNodeContentResult{}, nil
	}
	if err := h.store().UpdateNodeContent(ctx, in.ID, raw); err != nil {
		return errf("update_node_content: %v", err), updateNodeContentResult{}, nil
	}
	n, err := h.store().GetNode(ctx, in.ID)
	if err != nil {
		return errf("get_node: %v", err), updateNodeContentResult{}, nil
	}
	out := updateNodeContentResult{ContentHash: n.ContentHash}
	return ok(out), out, nil
}

// --- 5. set_status ---

type setStatusArgs struct {
	ID     string `json:"id"`
	Status string `json:"status"`
	Reason string `json:"reason,omitempty"`
}
type emptyResult struct{}

func (h *handlers) setStatus(ctx context.Context, req *mcpsdk.CallToolRequest, in setStatusArgs) (*mcpsdk.CallToolResult, emptyResult, error) {
	if err := h.store().SetStatus(ctx, in.ID, model.Status(in.Status), in.Reason); err != nil {
		return errf("set_status: %v", err), emptyResult{}, nil
	}
	return ok(map[string]string{"status": in.Status}), emptyResult{}, nil
}

// --- 6/7. link/unlink ---

type linkArgs struct {
	Src  string `json:"src"`
	Dst  string `json:"dst"`
	Kind string `json:"kind"`
}

func (h *handlers) link(ctx context.Context, req *mcpsdk.CallToolRequest, in linkArgs) (*mcpsdk.CallToolResult, emptyResult, error) {
	if err := h.store().Link(ctx, in.Src, in.Dst, model.EdgeKind(in.Kind)); err != nil {
		return errf("link: %v", err), emptyResult{}, nil
	}
	return ok(in), emptyResult{}, nil
}

func (h *handlers) unlink(ctx context.Context, req *mcpsdk.CallToolRequest, in linkArgs) (*mcpsdk.CallToolResult, emptyResult, error) {
	if err := h.store().Unlink(ctx, in.Src, in.Dst, model.EdgeKind(in.Kind)); err != nil {
		return errf("unlink: %v", err), emptyResult{}, nil
	}
	return ok(in), emptyResult{}, nil
}

// --- 8. node_files ---

const maxWriteBytes = 256 * 1024

type nodeFilesArgs struct {
	ID      string `json:"id"`
	Op      string `json:"op" jsonschema:"one of: list, read, write, delete"`
	Path    string `json:"path,omitempty" jsonschema:"relative path within the node's source/ directory"`
	Content string `json:"content,omitempty" jsonschema:"UTF-8 content to write (op=write only); max 256 KB"`
}

type nodeFilesResult struct {
	Op      string             `json:"op"`
	Path    string             `json:"path,omitempty"`
	Files   []model.FileRecord `json:"files,omitempty"`
	Content string             `json:"content,omitempty"`
	Bytes   int                `json:"bytes,omitempty"`
}

func (h *handlers) nodeFiles(ctx context.Context, req *mcpsdk.CallToolRequest, in nodeFilesArgs) (*mcpsdk.CallToolResult, nodeFilesResult, error) {
	if _, err := h.store().GetNode(ctx, in.ID); err != nil {
		return errf("node_files: %v", err), nodeFilesResult{}, nil
	}
	if err := h.ws.Materialize(ctx, in.ID); err != nil {
		return errf("materialize: %v", err), nodeFilesResult{}, nil
	}
	srcDir := h.ws.SourceDir(in.ID)

	switch in.Op {
	case "list":
		files, err := h.store().ListFiles(ctx, in.ID)
		if err != nil {
			return errf("list files: %v", err), nodeFilesResult{}, nil
		}
		res := nodeFilesResult{Op: "list", Files: files}
		return ok(res), res, nil

	case "read":
		rel, err := safeRel(in.Path)
		if err != nil {
			return errf("read: %v", err), nodeFilesResult{}, nil
		}
		b, err := os.ReadFile(filepath.Join(srcDir, rel))
		if err != nil {
			return errf("read %s: %v", rel, err), nodeFilesResult{}, nil
		}
		res := nodeFilesResult{Op: "read", Path: rel, Content: string(b), Bytes: len(b)}
		return ok(res), res, nil

	case "write":
		rel, err := safeRel(in.Path)
		if err != nil {
			return errf("write: %v", err), nodeFilesResult{}, nil
		}
		if len(in.Content) > maxWriteBytes {
			return errf("write: content exceeds %d bytes (use get_workspace_path + native tools)", maxWriteBytes), nodeFilesResult{}, nil
		}
		dst := filepath.Join(srcDir, rel)
		if err := os.MkdirAll(filepath.Dir(dst), 0o755); err != nil {
			return errf("mkdir: %v", err), nodeFilesResult{}, nil
		}
		if err := os.WriteFile(dst, []byte(in.Content), 0o644); err != nil {
			return errf("write: %v", err), nodeFilesResult{}, nil
		}
		if _, err := h.ws.Rehash(ctx, in.ID); err != nil {
			return errf("rehash: %v", err), nodeFilesResult{}, nil
		}
		res := nodeFilesResult{Op: "write", Path: rel, Bytes: len(in.Content)}
		return ok(res), res, nil

	case "delete":
		rel, err := safeRel(in.Path)
		if err != nil {
			return errf("delete: %v", err), nodeFilesResult{}, nil
		}
		if err := os.Remove(filepath.Join(srcDir, rel)); err != nil && !errors.Is(err, os.ErrNotExist) {
			return errf("delete: %v", err), nodeFilesResult{}, nil
		}
		if _, err := h.ws.Rehash(ctx, in.ID); err != nil {
			return errf("rehash: %v", err), nodeFilesResult{}, nil
		}
		res := nodeFilesResult{Op: "delete", Path: rel}
		return ok(res), res, nil
	}
	return errf("unknown op %q (want list|read|write|delete)", in.Op), nodeFilesResult{}, nil
}

// --- 9. get_workspace_path ---

type getWorkspacePathArgs struct {
	ID string `json:"id"`
}
type getWorkspacePathResult struct {
	NodeDir   string `json:"node_dir"`
	SourceDir string `json:"source_dir"`
	BuildDir  string `json:"build_dir,omitempty"`
	ArtifactDir string `json:"artifact_dir,omitempty"`
}

func (h *handlers) getWorkspacePath(ctx context.Context, req *mcpsdk.CallToolRequest, in getWorkspacePathArgs) (*mcpsdk.CallToolResult, getWorkspacePathResult, error) {
	n, err := h.store().GetNode(ctx, in.ID)
	if err != nil {
		return errf("get_workspace_path: %v", err), getWorkspacePathResult{}, nil
	}
	if err := h.ws.Materialize(ctx, in.ID); err != nil {
		return errf("materialize: %v", err), getWorkspacePathResult{}, nil
	}
	res := getWorkspacePathResult{
		NodeDir:   h.ws.Path(in.ID),
		SourceDir: h.ws.SourceDir(in.ID),
	}
	if n.Type == model.TypeBuild {
		res.BuildDir = h.ws.BuildDir(in.ID)
		res.ArtifactDir = h.ws.ArtifactDir(in.ID)
	}
	return ok(res), res, nil
}

// --- 10. rehash ---

type rehashArgs struct {
	ID string `json:"id"`
}
type rehashResult struct {
	ContentHash string             `json:"content_hash"`
	Files       []model.FileRecord `json:"files"`
	Affected    []string           `json:"affected,omitempty"`
}

func (h *handlers) rehash(ctx context.Context, req *mcpsdk.CallToolRequest, in rehashArgs) (*mcpsdk.CallToolResult, rehashResult, error) {
	hash, err := h.ws.Rehash(ctx, in.ID)
	if err != nil {
		return errf("rehash: %v", err), rehashResult{}, nil
	}
	files, _ := h.store().ListFiles(ctx, in.ID)
	res := rehashResult{ContentHash: hash, Files: files}
	if affected, err := h.store().AffectedImplementations(ctx, in.ID); err == nil {
		for _, a := range affected {
			res.Affected = append(res.Affected, a.ID)
		}
	}
	return ok(res), res, nil
}

// --- helpers ---

func newULID() string {
	return ulid.Make().String()
}

func safeRel(p string) (string, error) {
	if p == "" {
		return "", errors.New("path required")
	}
	p = filepath.ToSlash(p)
	if strings.HasPrefix(p, "/") {
		return "", errors.New("absolute paths not allowed")
	}
	clean := filepath.ToSlash(filepath.Clean(p))
	if clean == ".." || strings.HasPrefix(clean, "../") {
		return "", errors.New("escapes source directory")
	}
	return clean, nil
}

func preview(s string) string {
	s = strings.TrimSpace(s)
	s = strings.ReplaceAll(s, "\n", " ")
	const max = 80
	if len(s) <= max {
		return s
	}
	return s[:max-1] + "…"
}

func toRawJSON(v any) (json.RawMessage, error) {
	if v == nil {
		return json.RawMessage("null"), nil
	}
	b, err := json.Marshal(v)
	if err != nil {
		return nil, fmt.Errorf("marshal: %w", err)
	}
	return json.RawMessage(b), nil
}
