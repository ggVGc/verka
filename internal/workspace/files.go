package workspace

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"

	"github.com/ggvgc/llaundry/internal/model"
)

// scanSourceDir walks the directory rooted at dir and returns a FileRecord for
// each regular file found. The prior map lets the scan skip re-hashing files
// whose (size, mtime) are unchanged.
func scanSourceDir(dir string, prior map[string]model.FileRecord) ([]model.FileRecord, error) {
	var out []model.FileRecord
	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			if errors.Is(walkErr, fs.ErrNotExist) && path == dir {
				return nil
			}
			return walkErr
		}
		if d.IsDir() {
			return nil
		}
		info, err := d.Info()
		if err != nil {
			return err
		}
		if !info.Mode().IsRegular() {
			return nil
		}
		rel, err := filepath.Rel(dir, path)
		if err != nil {
			return err
		}
		rel = filepath.ToSlash(rel)

		mtimeNs := info.ModTime().UnixNano()
		size := info.Size()
		if p, ok := prior[rel]; ok && p.Size == size && p.MtimeNs == mtimeNs && p.SHA256 != "" {
			out = append(out, p)
			return nil
		}
		h, err := hashFile(path)
		if err != nil {
			return err
		}
		out = append(out, model.FileRecord{
			RelPath: rel,
			SHA256:  h,
			Size:    size,
			MtimeNs: mtimeNs,
			Role:    model.FileSource,
		})
		return nil
	})
	if err != nil {
		return nil, err
	}
	sort.Slice(out, func(i, j int) bool { return out[i].RelPath < out[j].RelPath })
	return out, nil
}

func hashFile(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
