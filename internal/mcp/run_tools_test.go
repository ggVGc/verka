package mcp

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	mcpsdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

func hasGo(t *testing.T) {
	t.Helper()
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("go toolchain not available")
	}
}

const (
	goMod = `module example.com/reverse

go 1.22
`
	goSrc = `package reverse

func Reverse(s string) string {
	r := []rune(s)
	for i, j := 0, len(r)-1; i < j; i, j = i+1, j-1 {
		r[i], r[j] = r[j], r[i]
	}
	return string(r)
}
`
	goTest = `package reverse

import "testing"

func TestReverse(t *testing.T) {
	if got := Reverse("abc"); got != "cba" {
		t.Fatalf("Reverse(abc) = %q, want cba", got)
	}
}
`
	goMain = `package main

import (
	"fmt"
	"os"

	"example.com/reverse"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: reverse <string>")
		os.Exit(2)
	}
	fmt.Println(reverse.Reverse(os.Args[1]))
}
`
)

func TestRunVerificationAndBuildEndToEnd(t *testing.T) {
	hasGo(t)
	sess, cleanup := connectTest(t)
	defer cleanup()

	desc := call(t, sess, "create_node", map[string]any{
		"type":    "description",
		"content": map[string]any{"text": "reverse a string CLI"},
	})
	descID := desc["id"].(string)

	task := call(t, sess, "create_node", map[string]any{
		"type":    "task",
		"parent":  descID,
		"content": map[string]any{"title": "impl reverse"},
	})
	taskID := task["id"].(string)

	impl := call(t, sess, "create_node", map[string]any{
		"type":       "implementation",
		"content":    map[string]any{},
		"depends_on": []string{taskID},
	})
	implID := impl["id"].(string)

	// Materialize + write sources directly.
	paths := call(t, sess, "get_workspace_path", map[string]any{"id": implID})
	src := paths["source_dir"].(string)
	writeFile(t, filepath.Join(src, "go.mod"), goMod)
	writeFile(t, filepath.Join(src, "reverse.go"), goSrc)
	writeFile(t, filepath.Join(src, "reverse_test.go"), goTest)
	writeFile(t, filepath.Join(src, "cmd", "reverse", "main.go"), goMain)
	call(t, sess, "rehash", map[string]any{"id": implID})

	// Verification.
	ver := call(t, sess, "create_node", map[string]any{
		"type":       "verification",
		"content":    map[string]any{},
		"depends_on": []string{implID},
	})
	verID := ver["id"].(string)
	verRes := call(t, sess, "run_verification", map[string]any{"id": verID})
	if int(verRes["exit_code"].(float64)) != 0 {
		t.Fatalf("verification failed: %+v", verRes)
	}
	if verRes["status"].(string) != "passed" {
		t.Fatalf("expected passed, got %v", verRes["status"])
	}

	// Build.
	build := call(t, sess, "create_node", map[string]any{
		"type":       "build",
		"content":    map[string]any{},
		"depends_on": []string{implID},
	})
	buildID := build["id"].(string)
	buildRes := call(t, sess, "run_build", map[string]any{"id": buildID})
	if int(buildRes["exit_code"].(float64)) != 0 {
		t.Fatalf("build failed: %+v", buildRes)
	}
	artifact, _ := buildRes["artifact_rel"].(string)
	if artifact == "" {
		t.Fatalf("no artifact recorded: %+v", buildRes)
	}

	// Staleness: editing the task content should mark ver+build stale.
	stale := call(t, sess, "list_nodes", map[string]any{"stale": true})
	if arr := stale["nodes"].([]any); len(arr) != 0 {
		t.Fatalf("expected no stale nodes before change, got %v", arr)
	}
	call(t, sess, "update_node_content", map[string]any{
		"id":      implID, // change impl (a direct input of both ver and build)
		"content": map[string]any{"note": "changed"},
	})
	stale = call(t, sess, "list_nodes", map[string]any{"stale": true})
	ids := idsIn(stale["nodes"].([]any))
	if !containsAll(ids, verID, buildID) {
		t.Fatalf("expected ver+build to be stale, got %v", ids)
	}
}

func idsIn(list []any) []string {
	out := make([]string, 0, len(list))
	for _, item := range list {
		m := item.(map[string]any)
		out = append(out, m["id"].(string))
	}
	return out
}

func containsAll(haystack []string, needles ...string) bool {
	set := make(map[string]struct{}, len(haystack))
	for _, h := range haystack {
		set[h] = struct{}{}
	}
	for _, n := range needles {
		if _, ok := set[n]; !ok {
			return false
		}
	}
	return true
}

// writeFile used by tests; lives here rather than adding a test-helper pkg.
func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

// silence unused import warning when run subset
var (
	_ = context.Background
	_ = json.Marshal
	_ = mcpsdk.NewServer
)
