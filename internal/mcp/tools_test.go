package mcp

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ggvgc/llaundry/internal/workspace"
	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

// connectTest brings up a server wired to a temp workspace and returns a
// connected client session plus a teardown.
func connectTest(t *testing.T) (*mcpsdk.ClientSession, func()) {
	t.Helper()
	dir := t.TempDir()
	if _, err := workspace.Init(dir); err != nil {
		t.Fatalf("workspace.Init: %v", err)
	}
	ws, err := workspace.Open(dir)
	if err != nil {
		t.Fatalf("workspace.Open: %v", err)
	}
	srv := mcpsdk.NewServer(&mcpsdk.Implementation{Name: "llaundry-test"}, nil)
	registerTools(srv, &handlers{ws: ws})

	t1, t2 := mcpsdk.NewInMemoryTransports()
	ctx := context.Background()
	srvSess, err := srv.Connect(ctx, t1, nil)
	if err != nil {
		t.Fatal(err)
	}
	client := mcpsdk.NewClient(&mcpsdk.Implementation{Name: "test-client"}, nil)
	cliSess, err := client.Connect(ctx, t2, nil)
	if err != nil {
		t.Fatal(err)
	}
	return cliSess, func() {
		cliSess.Close()
		srvSess.Wait()
		ws.Close()
	}
}

func call(t *testing.T, sess *mcpsdk.ClientSession, name string, args any) map[string]any {
	t.Helper()
	argsB, err := json.Marshal(args)
	if err != nil {
		t.Fatal(err)
	}
	res, err := sess.CallTool(context.Background(), &mcpsdk.CallToolParams{
		Name:      name,
		Arguments: json.RawMessage(argsB),
	})
	if err != nil {
		t.Fatalf("CallTool %s: %v", name, err)
	}
	if res.IsError {
		var msg string
		for _, c := range res.Content {
			if tc, ok := c.(*mcpsdk.TextContent); ok {
				msg += tc.Text
			}
		}
		t.Fatalf("tool %s returned error: %s", name, msg)
	}
	var text string
	for _, c := range res.Content {
		if tc, ok := c.(*mcpsdk.TextContent); ok {
			text += tc.Text
		}
	}
	var out map[string]any
	if text != "" {
		if err := json.Unmarshal([]byte(text), &out); err != nil {
			t.Fatalf("tool %s: bad response JSON: %v\npayload: %s", name, err, text)
		}
	}
	return out
}

func TestMCPHappyPath(t *testing.T) {
	sess, cleanup := connectTest(t)
	defer cleanup()

	desc := call(t, sess, "create_node", map[string]any{
		"type":    "description",
		"content": map[string]any{"text": "reverse a string"},
	})
	descID := desc["id"].(string)

	task := call(t, sess, "create_node", map[string]any{
		"type":    "task",
		"parent":  descID,
		"content": map[string]any{"title": "impl reverse", "details": "take a string, return it reversed"},
	})
	taskID := task["id"].(string)

	impl := call(t, sess, "create_node", map[string]any{
		"type":       "implementation",
		"content":    map[string]any{"note": "first cut"},
		"depends_on": []string{taskID},
	})
	implID := impl["id"].(string)

	// fetch children of description
	kids := call(t, sess, "list_nodes", map[string]any{"parent": descID})
	arr := kids["nodes"].([]any)
	if len(arr) != 1 {
		t.Fatalf("expected 1 child of description, got %d", len(arr))
	}

	// write a file via node_files
	call(t, sess, "node_files", map[string]any{
		"id":      implID,
		"op":      "write",
		"path":    "main.go",
		"content": "package main\nfunc main(){}\n",
	})

	// list files
	listed := call(t, sess, "node_files", map[string]any{"id": implID, "op": "list"})
	if files := listed["files"].([]any); len(files) != 1 {
		t.Fatalf("expected 1 file, got %d", len(files))
	}

	// get_workspace_path + direct edit + rehash
	paths := call(t, sess, "get_workspace_path", map[string]any{"id": implID})
	srcDir := paths["source_dir"].(string)
	if !strings.HasSuffix(srcDir, filepath.Join("nodes", implID, "source")) {
		t.Fatalf("unexpected source_dir %q", srcDir)
	}
	if err := os.WriteFile(filepath.Join(srcDir, "extra.txt"), []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}
	re := call(t, sess, "rehash", map[string]any{"id": implID})
	if files := re["files"].([]any); len(files) != 2 {
		t.Fatalf("expected 2 files post-rehash, got %d", len(files))
	}

	// edit task content -> verify content_hash changes (staleness wiring verified in store tests)
	before := call(t, sess, "get_node", map[string]any{"id": taskID})
	beforeHash := before["node"].(map[string]any)["content_hash"].(string)
	call(t, sess, "update_node_content", map[string]any{
		"id":      taskID,
		"content": map[string]any{"title": "impl reverse v2"},
	})
	after := call(t, sess, "get_node", map[string]any{"id": taskID})
	afterHash := after["node"].(map[string]any)["content_hash"].(string)
	if beforeHash == afterHash {
		t.Fatalf("expected task content_hash to change after update_node_content")
	}

	// set_status
	call(t, sess, "set_status", map[string]any{"id": implID, "status": "passed"})
	got := call(t, sess, "get_node", map[string]any{"id": implID})
	if s := got["node"].(map[string]any)["status"].(string); s != "passed" {
		t.Fatalf("status should be passed, got %s", s)
	}

	// link/unlink
	call(t, sess, "link", map[string]any{"src": implID, "dst": descID, "kind": "depends_on"})
	call(t, sess, "unlink", map[string]any{"src": implID, "dst": descID, "kind": "depends_on"})
}

func TestAskUser(t *testing.T) {
	// Override the TTY implementation so the test doesn't need a terminal.
	var gotQuestion string
	var gotOptions []string
	prev := promptUser
	defer func() { promptUser = prev }()
	promptUser = func(q string, opts []string) (string, error) {
		gotQuestion = q
		gotOptions = opts
		return "approve", nil
	}

	sess, cleanup := connectTest(t)
	defer cleanup()

	out := call(t, sess, "ask_user", map[string]any{
		"question": "Do you approve this plan?",
		"options":  []string{"approve", "revise"},
	})
	if out["answer"] != "approve" {
		t.Fatalf("answer = %v, want approve", out["answer"])
	}
	if gotQuestion != "Do you approve this plan?" {
		t.Errorf("promptUser question = %q, want %q", gotQuestion, "Do you approve this plan?")
	}
	if len(gotOptions) != 2 || gotOptions[0] != "approve" || gotOptions[1] != "revise" {
		t.Errorf("promptUser options = %v, want [approve revise]", gotOptions)
	}
}

func TestAskUserRejectsEmptyQuestion(t *testing.T) {
	sess, cleanup := connectTest(t)
	defer cleanup()
	argsB, _ := json.Marshal(map[string]any{"question": "   "})
	res, err := sess.CallTool(context.Background(), &mcpsdk.CallToolParams{
		Name:      "ask_user",
		Arguments: json.RawMessage(argsB),
	})
	if err != nil {
		t.Fatal(err)
	}
	if !res.IsError {
		t.Fatal("expected error on empty question")
	}
}

func TestMCPRejectsPathEscape(t *testing.T) {
	sess, cleanup := connectTest(t)
	defer cleanup()
	impl := call(t, sess, "create_node", map[string]any{
		"type":    "implementation",
		"content": map[string]any{},
	})
	implID := impl["id"].(string)
	// Direct CallTool so we can inspect the error without t.Fatal.
	argsB, _ := json.Marshal(map[string]any{
		"id":      implID,
		"op":      "write",
		"path":    "../escape.txt",
		"content": "nope",
	})
	res, err := sess.CallTool(context.Background(), &mcpsdk.CallToolParams{
		Name:      "node_files",
		Arguments: json.RawMessage(argsB),
	})
	if err != nil {
		t.Fatal(err)
	}
	if !res.IsError {
		t.Fatal("expected error on path escape")
	}
}
