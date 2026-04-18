package workspace

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/ggvgc/llaundry/internal/model"
)

func writeFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestScanSourceDir(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, filepath.Join(dir, "a.txt"), "hello")
	writeFile(t, filepath.Join(dir, "sub", "b.txt"), "world")

	files, err := scanSourceDir(dir, nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(files) != 2 {
		t.Fatalf("expected 2 files, got %d", len(files))
	}
	paths := []string{files[0].RelPath, files[1].RelPath}
	if paths[0] != "a.txt" || paths[1] != "sub/b.txt" {
		t.Fatalf("unexpected paths %v", paths)
	}
	for _, f := range files {
		if f.SHA256 == "" {
			t.Fatalf("missing hash on %s", f.RelPath)
		}
	}
}

func TestScanSourceDirMtimeFastPath(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "a.txt")
	writeFile(t, p, "hello")

	files, err := scanSourceDir(dir, nil)
	if err != nil {
		t.Fatal(err)
	}
	firstHash := files[0].SHA256

	// Return a prior map with a SENTINEL hash so we can verify the fast path is
	// hit — if it is, the returned record will carry the sentinel, not a real
	// hash of the file.
	prior := map[string]model.FileRecord{
		files[0].RelPath: {
			RelPath: files[0].RelPath,
			SHA256:  "SENTINEL",
			Size:    files[0].Size,
			MtimeNs: files[0].MtimeNs,
			Role:    model.FileSource,
		},
	}
	files2, err := scanSourceDir(dir, prior)
	if err != nil {
		t.Fatal(err)
	}
	if files2[0].SHA256 != "SENTINEL" {
		t.Fatalf("expected mtime fast-path to reuse prior hash, got %q (orig %q)", files2[0].SHA256, firstHash)
	}

	// Modify content (and touch mtime) — fast path must re-hash.
	writeFile(t, p, "changed")
	files3, err := scanSourceDir(dir, prior)
	if err != nil {
		t.Fatal(err)
	}
	if files3[0].SHA256 == "SENTINEL" {
		t.Fatalf("fast path fired even though file changed")
	}
}

func TestScanSourceDirMissing(t *testing.T) {
	files, err := scanSourceDir(filepath.Join(t.TempDir(), "nope"), nil)
	if err != nil {
		t.Fatalf("missing dir should not error: %v", err)
	}
	if len(files) != 0 {
		t.Fatalf("expected 0 files, got %d", len(files))
	}
}
