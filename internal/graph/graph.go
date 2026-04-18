package graph

import (
	"context"
	"fmt"
	"io"
	"sort"
	"strings"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/store"
	"github.com/ggvgc/llaundry/internal/workspace"
)

// PrintNode writes a human-readable summary of a node: header, edges, files,
// latest run.
func PrintNode(ctx context.Context, w io.Writer, s *store.SQLite, ws *workspace.Workspace, id string) error {
	n, err := s.GetNode(ctx, id)
	if err != nil {
		return err
	}
	stale, err := s.StaleNodes(ctx)
	if err != nil {
		return err
	}
	isStale := false
	for _, sid := range stale {
		if sid == id {
			isStale = true
			break
		}
	}
	fmt.Fprintf(w, "%s\n", n.ID)
	fmt.Fprintf(w, "  type:    %s\n", n.Type)
	fmt.Fprintf(w, "  status:  %s%s\n", n.Status, staleFlag(isStale))
	fmt.Fprintf(w, "  hash:    %s\n", shortHash(n.ContentHash))
	fmt.Fprintf(w, "  created: %s\n", n.CreatedAt.Format("2006-01-02 15:04:05"))
	fmt.Fprintf(w, "  content: %s\n", truncate(string(n.Content), 200))

	edges, err := s.EdgesFor(ctx, id)
	if err == nil && len(edges) > 0 {
		fmt.Fprintln(w, "  edges:")
		for _, e := range edges {
			dir := "->"
			peer := e.Dst
			if e.Dst == id {
				dir = "<-"
				peer = e.Src
			}
			fmt.Fprintf(w, "    %s %s %s\n", dir, e.Kind, peer)
		}
	}

	files, err := s.ListFiles(ctx, id)
	if err == nil && len(files) > 0 {
		fmt.Fprintln(w, "  files:")
		for _, f := range files {
			fmt.Fprintf(w, "    %s  %d bytes  %s  %s\n", shortHash(f.SHA256), f.Size, f.Role, f.RelPath)
		}
	}

	run, err := s.GetLatestRun(ctx, id)
	if err == nil && run != nil {
		exit := "?"
		if run.ExitCode != nil {
			exit = fmt.Sprintf("%d", *run.ExitCode)
		}
		fmt.Fprintf(w, "  latest run: id=%d kind=%s exit=%s stdout=%s\n", run.ID, run.Kind, exit, run.StdoutPath)
	}

	if ws != nil {
		fmt.Fprintf(w, "  workspace: %s\n", ws.Path(id))
	}
	return nil
}

// PrintTree prints an ASCII tree rooted at rootID. If rootID is empty, every
// description in the store is printed in creation order.
func PrintTree(ctx context.Context, w io.Writer, s *store.SQLite, rootID string) error {
	if rootID == "" {
		descs, err := s.ListNodes(ctx, store.NodeFilter{Type: model.TypeDescription})
		if err != nil {
			return err
		}
		if len(descs) == 0 {
			fmt.Fprintln(w, "(no descriptions)")
			return nil
		}
		for _, d := range descs {
			if err := printSubtree(ctx, w, s, d.ID, "", "", map[string]bool{}); err != nil {
				return err
			}
		}
		return nil
	}
	return printSubtree(ctx, w, s, rootID, "", "", map[string]bool{})
}

// printSubtree prints the subtree rooted at id.
//   - linePrefix is prepended to this node's own line (branch marker + indent).
//   - childIndent is prepended to the recursive calls' linePrefix (the
//     continuation column: "│   " or "    ").
func printSubtree(ctx context.Context, w io.Writer, s *store.SQLite, id, linePrefix, childIndent string, seen map[string]bool) error {
	if seen[id] {
		fmt.Fprintf(w, "%s%s (cycle)\n", linePrefix, id)
		return nil
	}
	seen[id] = true
	n, err := s.GetNode(ctx, id)
	if err != nil {
		fmt.Fprintf(w, "%s%s (missing)\n", linePrefix, id)
		return nil
	}
	fmt.Fprintf(w, "%s%s [%s/%s] %s\n", linePrefix, n.ID, n.Type, n.Status, previewContent(n.Content))

	var children []string
	for _, kind := range []model.EdgeKind{model.EdgeChild, model.EdgeDependsOn, model.EdgeVerifies, model.EdgeBuilds} {
		ids, err := s.Neighbors(ctx, id, kind, model.DirIncoming)
		if err == nil {
			children = append(children, ids...)
		}
		if kind == model.EdgeChild {
			ids, err = s.Neighbors(ctx, id, kind, model.DirOutgoing)
			if err == nil {
				children = append(children, ids...)
			}
		}
	}
	children = unique(children)
	sort.Strings(children)
	for i, c := range children {
		last := i == len(children)-1
		branch := "├── "
		nextIndent := childIndent + "│   "
		if last {
			branch = "└── "
			nextIndent = childIndent + "    "
		}
		if err := printSubtree(ctx, w, s, c, childIndent+branch, nextIndent, seen); err != nil {
			return err
		}
	}
	return nil
}

func unique(in []string) []string {
	seen := map[string]struct{}{}
	out := in[:0]
	for _, s := range in {
		if _, ok := seen[s]; ok {
			continue
		}
		seen[s] = struct{}{}
		out = append(out, s)
	}
	return out
}

func previewContent(b []byte) string {
	s := strings.TrimSpace(string(b))
	s = strings.ReplaceAll(s, "\n", " ")
	return truncate(s, 80)
}

func truncate(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max-1] + "…"
}

func shortHash(h string) string {
	if len(h) <= 12 {
		return h
	}
	return h[:12]
}

func staleFlag(s bool) string {
	if s {
		return " (STALE)"
	}
	return ""
}
