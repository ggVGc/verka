package graph

import (
	"bytes"
	"context"
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/workspace"
)

func TestPrintTreeAndShowSmoke(t *testing.T) {
	dir := t.TempDir()
	if _, err := workspace.Init(dir); err != nil {
		t.Fatal(err)
	}
	ws, err := workspace.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer ws.Close()

	ctx := context.Background()
	s := ws.Store()
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	must(s.CreateNode(ctx, &model.Node{
		ID: "desc", Type: model.TypeDescription, Status: model.StatusDraft,
		Content: json.RawMessage(`{"text":"root"}`),
	}))
	must(s.CreateNode(ctx, &model.Node{
		ID: "task1", Type: model.TypeTask, Status: model.StatusReady,
		Content: json.RawMessage(`{"title":"do thing"}`),
	}))
	must(s.CreateNode(ctx, &model.Node{
		ID: "impl", Type: model.TypeImplementation, Status: model.StatusReady,
		Content: json.RawMessage(`{}`),
	}))
	must(s.Link(ctx, "desc", "task1", model.EdgeChild))
	must(s.Link(ctx, "impl", "task1", model.EdgeDependsOn))

	var out bytes.Buffer
	if err := PrintTree(ctx, &out, s, "desc"); err != nil {
		t.Fatal(err)
	}
	got := out.String()
	if !strings.Contains(got, "desc ") || !strings.Contains(got, "task1") || !strings.Contains(got, "impl") {
		t.Fatalf("tree missing nodes:\n%s", got)
	}

	out.Reset()
	if err := PrintNode(ctx, &out, s, ws, "impl"); err != nil {
		t.Fatal(err)
	}
	node := out.String()
	if !strings.Contains(node, "impl") || !strings.Contains(node, "workspace:") {
		t.Fatalf("show missing fields:\n%s", node)
	}
	if !strings.Contains(node, filepath.Base(ws.Path("impl"))) {
		t.Fatalf("show missing node dir:\n%s", node)
	}
}
