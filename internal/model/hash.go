package model

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
)

// Canonicalize decodes the JSON bytes and re-encodes them with stable key
// ordering and no extraneous whitespace. It returns an error if the input is
// not valid JSON. nil or empty input is treated as a JSON null.
func Canonicalize(raw []byte) ([]byte, error) {
	if len(raw) == 0 {
		return []byte("null"), nil
	}
	var v any
	dec := json.NewDecoder(strings.NewReader(string(raw)))
	dec.UseNumber()
	if err := dec.Decode(&v); err != nil {
		return nil, fmt.Errorf("canonicalize: %w", err)
	}
	return canonicalMarshal(v)
}

func canonicalMarshal(v any) ([]byte, error) {
	var buf strings.Builder
	if err := writeCanonical(&buf, v); err != nil {
		return nil, err
	}
	return []byte(buf.String()), nil
}

func writeCanonical(buf *strings.Builder, v any) error {
	switch x := v.(type) {
	case nil:
		buf.WriteString("null")
	case bool:
		if x {
			buf.WriteString("true")
		} else {
			buf.WriteString("false")
		}
	case json.Number:
		buf.WriteString(x.String())
	case string:
		b, err := json.Marshal(x)
		if err != nil {
			return err
		}
		buf.Write(b)
	case []any:
		buf.WriteByte('[')
		for i, elem := range x {
			if i > 0 {
				buf.WriteByte(',')
			}
			if err := writeCanonical(buf, elem); err != nil {
				return err
			}
		}
		buf.WriteByte(']')
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		buf.WriteByte('{')
		for i, k := range keys {
			if i > 0 {
				buf.WriteByte(',')
			}
			kb, err := json.Marshal(k)
			if err != nil {
				return err
			}
			buf.Write(kb)
			buf.WriteByte(':')
			if err := writeCanonical(buf, x[k]); err != nil {
				return err
			}
		}
		buf.WriteByte('}')
	default:
		return fmt.Errorf("canonicalize: unsupported value of type %T", v)
	}
	return nil
}

// ComputeContentHash derives the node's content_hash from its canonical
// content_json and the set of files it contains. Status is intentionally not
// part of the hash: status transitions are bookkeeping, not substance, and
// mixing them in would invalidate every downstream node on every run.
func ComputeContentHash(contentJSON []byte, files []FileRecord) (string, error) {
	canon, err := Canonicalize(contentJSON)
	if err != nil {
		return "", err
	}
	h := sha256.New()
	h.Write(canon)
	h.Write([]byte{'\n'})

	sorted := make([]FileRecord, len(files))
	copy(sorted, files)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].RelPath < sorted[j].RelPath })
	for _, f := range sorted {
		h.Write([]byte(f.RelPath))
		h.Write([]byte{':'})
		h.Write([]byte(f.SHA256))
		h.Write([]byte{'\n'})
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
