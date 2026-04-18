package store

import (
	"context"
	"encoding/json"
	"path/filepath"
	"testing"

	"github.com/ggvgc/llaundry/internal/model"
)

func newStore(t *testing.T) *SQLite {
	t.Helper()
	dir := t.TempDir()
	s, err := Open(context.Background(), filepath.Join(dir, "test.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func mustCreateNode(t *testing.T, s *SQLite, id string, typ model.NodeType, content string) {
	t.Helper()
	err := s.CreateNode(context.Background(), &model.Node{
		ID:      id,
		Type:    typ,
		Status:  model.StatusDraft,
		Content: json.RawMessage(content),
	})
	if err != nil {
		t.Fatalf("CreateNode(%s): %v", id, err)
	}
}

func TestCreateAndGetNode(t *testing.T) {
	s := newStore(t)
	mustCreateNode(t, s, "n1", model.TypeDescription, `{"text":"hi"}`)
	got, err := s.GetNode(context.Background(), "n1")
	if err != nil {
		t.Fatal(err)
	}
	if got.Type != model.TypeDescription || string(got.Content) != `{"text":"hi"}` {
		t.Fatalf("unexpected node: %+v", got)
	}
	if got.ContentHash == "" {
		t.Fatal("content_hash not populated")
	}
}

func TestLinkAndNeighbors(t *testing.T) {
	s := newStore(t)
	ctx := context.Background()
	mustCreateNode(t, s, "d", model.TypeDescription, `{}`)
	mustCreateNode(t, s, "t1", model.TypeTask, `{}`)
	mustCreateNode(t, s, "t2", model.TypeTask, `{}`)
	if err := s.Link(ctx, "d", "t1", model.EdgeChild); err != nil {
		t.Fatal(err)
	}
	if err := s.Link(ctx, "d", "t2", model.EdgeChild); err != nil {
		t.Fatal(err)
	}
	kids, err := s.Neighbors(ctx, "d", model.EdgeChild, model.DirOutgoing)
	if err != nil {
		t.Fatal(err)
	}
	if len(kids) != 2 {
		t.Fatalf("expected 2 children, got %v", kids)
	}
}

func TestUpdateNodeContentChangesHash(t *testing.T) {
	s := newStore(t)
	ctx := context.Background()
	mustCreateNode(t, s, "n", model.TypeTask, `{"a":1}`)
	before, _ := s.GetNode(ctx, "n")
	if err := s.UpdateNodeContent(ctx, "n", json.RawMessage(`{"a":2}`)); err != nil {
		t.Fatal(err)
	}
	after, _ := s.GetNode(ctx, "n")
	if before.ContentHash == after.ContentHash {
		t.Fatal("content_hash should have changed")
	}
}

func TestReplaceFilesAndRehash(t *testing.T) {
	s := newStore(t)
	ctx := context.Background()
	mustCreateNode(t, s, "i", model.TypeImplementation, `{}`)
	before, _ := s.GetNode(ctx, "i")
	files := []model.FileRecord{
		{RelPath: "a.go", SHA256: "h1", Size: 10, MtimeNs: 1, Role: model.FileSource},
		{RelPath: "b.go", SHA256: "h2", Size: 20, MtimeNs: 2, Role: model.FileSource},
	}
	if err := s.ReplaceFiles(ctx, "i", files); err != nil {
		t.Fatal(err)
	}
	if _, err := s.RecomputeAndStoreHash(ctx, "i", "file_change"); err != nil {
		t.Fatal(err)
	}
	after, _ := s.GetNode(ctx, "i")
	if before.ContentHash == after.ContentHash {
		t.Fatal("expected hash to change after adding files")
	}
	listed, err := s.ListFiles(ctx, "i")
	if err != nil || len(listed) != 2 {
		t.Fatalf("expected 2 files, got %v err=%v", listed, err)
	}
}

func TestStaleDetectionAfterInputChange(t *testing.T) {
	s := newStore(t)
	ctx := context.Background()
	mustCreateNode(t, s, "impl", model.TypeImplementation, `{"v":1}`)
	mustCreateNode(t, s, "ver", model.TypeVerification, `{}`)
	if err := s.Link(ctx, "ver", "impl", model.EdgeDependsOn); err != nil {
		t.Fatal(err)
	}

	impl, _ := s.GetNode(ctx, "impl")
	runID, err := s.StartRun(ctx, "ver", model.RunVerification)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.RecordInputSnapshots(ctx, runID, "ver", []model.InputSnapshot{
		{InputNode: "impl", ObservedHash: impl.ContentHash},
	}); err != nil {
		t.Fatal(err)
	}
	if err := s.FinishRun(ctx, runID, model.RunResult{ExitCode: 0}); err != nil {
		t.Fatal(err)
	}

	stale, _ := s.StaleNodes(ctx)
	if len(stale) != 0 {
		t.Fatalf("expected no stale nodes initially, got %v", stale)
	}

	if err := s.UpdateNodeContent(ctx, "impl", json.RawMessage(`{"v":2}`)); err != nil {
		t.Fatal(err)
	}
	stale, _ = s.StaleNodes(ctx)
	if len(stale) != 1 || stale[0] != "ver" {
		t.Fatalf("expected ver to be stale, got %v", stale)
	}
}
