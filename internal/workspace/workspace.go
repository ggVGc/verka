package workspace

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/ggvgc/llaundry/internal/model"
	"github.com/ggvgc/llaundry/internal/store"
)

const (
	dirName     = ".llaundry"
	nodesSub    = "nodes"
	logsSub     = "logs"
	sourceRole  = "source"
	artifactRol = "artifact"
	buildSub    = "build"
)

// Workspace owns the on-disk layout and the backing store for a single project
// root. Open() returns one of these; keep it for the process lifetime.
type Workspace struct {
	root    string // absolute path to project root
	rootDir string // absolute path to .llaundry/
	store   *store.SQLite
}

// Init creates the .llaundry/ directory and database in dir if missing, and
// returns the absolute workspace directory path.
func Init(dir string) (string, error) {
	absRoot, err := filepath.Abs(dir)
	if err != nil {
		return "", err
	}
	wsDir := filepath.Join(absRoot, dirName)
	if err := os.MkdirAll(filepath.Join(wsDir, nodesSub), 0o755); err != nil {
		return "", err
	}
	if err := os.MkdirAll(filepath.Join(wsDir, logsSub), 0o755); err != nil {
		return "", err
	}
	s, err := store.Open(context.Background(), filepath.Join(wsDir, "db.sqlite"))
	if err != nil {
		return "", err
	}
	_ = s.Close()
	return wsDir, nil
}

// Open opens an existing workspace at dir (which should contain a .llaundry/
// subdirectory; `llaundry init` creates it).
func Open(dir string) (*Workspace, error) {
	absRoot, err := filepath.Abs(dir)
	if err != nil {
		return nil, err
	}
	wsDir := filepath.Join(absRoot, dirName)
	if _, err := os.Stat(wsDir); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, fmt.Errorf("no %s/ in %s — run `llaundry init` first", dirName, absRoot)
		}
		return nil, err
	}
	s, err := store.Open(context.Background(), filepath.Join(wsDir, "db.sqlite"))
	if err != nil {
		return nil, err
	}
	return &Workspace{root: absRoot, rootDir: wsDir, store: s}, nil
}

func (w *Workspace) Close() error { return w.store.Close() }

func (w *Workspace) Store() *store.SQLite { return w.store }

// RootDir returns the absolute path to the .llaundry/ directory.
func (w *Workspace) RootDir() string { return w.rootDir }

// Path returns the absolute on-disk directory for a node, creating it (and
// the source/ subdirectory) on first access.
func (w *Workspace) Path(nodeID string) string {
	return filepath.Join(w.rootDir, nodesSub, nodeID)
}

// SourceDir returns the directory where source files for a node live.
func (w *Workspace) SourceDir(nodeID string) string {
	return filepath.Join(w.Path(nodeID), sourceRole)
}

// ArtifactDir returns the directory where build artifacts for a node live.
func (w *Workspace) ArtifactDir(nodeID string) string {
	return filepath.Join(w.Path(nodeID), artifactRol)
}

// BuildDir returns the directory where assembled build workspaces for a node
// live (contains the generated go.work).
func (w *Workspace) BuildDir(nodeID string) string {
	return filepath.Join(w.Path(nodeID), buildSub)
}

// LogsDir returns the directory used for run stdout/stderr captures.
func (w *Workspace) LogsDir() string {
	return filepath.Join(w.rootDir, logsSub)
}

// Materialize ensures the per-node directory structure exists. Safe to call
// multiple times.
func (w *Workspace) Materialize(ctx context.Context, nodeID string) error {
	n, err := w.store.GetNode(ctx, nodeID)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(w.SourceDir(nodeID), 0o755); err != nil {
		return err
	}
	if n.Type == model.TypeBuild {
		if err := os.MkdirAll(w.BuildDir(nodeID), 0o755); err != nil {
			return err
		}
		if err := os.MkdirAll(w.ArtifactDir(nodeID), 0o755); err != nil {
			return err
		}
	}
	return nil
}

// Rehash walks the node's source directory, upserts file records (using the
// mtime+size fast-path), and persists the recomputed content_hash.
func (w *Workspace) Rehash(ctx context.Context, nodeID string) (string, error) {
	if err := w.Materialize(ctx, nodeID); err != nil {
		return "", err
	}
	existing, err := w.store.ListFiles(ctx, nodeID)
	if err != nil {
		return "", err
	}
	byPath := make(map[string]model.FileRecord, len(existing))
	for _, f := range existing {
		byPath[f.RelPath] = f
	}
	files, err := scanSourceDir(w.SourceDir(nodeID), byPath)
	if err != nil {
		return "", err
	}
	if err := w.store.ReplaceFiles(ctx, nodeID, files); err != nil {
		return "", err
	}
	return w.store.RecomputeAndStoreHash(ctx, nodeID, "file_change")
}
