package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/ggvgc/llaundry/internal/model"
	_ "modernc.org/sqlite"
)

var ErrNotFound = errors.New("not found")

type SQLite struct {
	db *sql.DB
}

// Open opens (or creates) a SQLite database at dbPath, enables the pragmas we
// need for safe concurrent single-writer use, and runs migrations.
func Open(ctx context.Context, dbPath string) (*SQLite, error) {
	dsn := dbPath + "?_pragma=journal_mode(WAL)&_pragma=busy_timeout(5000)&_pragma=foreign_keys(ON)&_pragma=synchronous(NORMAL)"
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	// Single writer path for v1 — keeps `SQLITE_BUSY` out of our way entirely.
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	if err := db.PingContext(ctx); err != nil {
		db.Close()
		return nil, err
	}
	if err := runMigrations(ctx, db); err != nil {
		db.Close()
		return nil, err
	}
	return &SQLite{db: db}, nil
}

func (s *SQLite) Close() error { return s.db.Close() }

// --- nodes ---

func (s *SQLite) CreateNode(ctx context.Context, n *model.Node) error {
	if !n.Type.Valid() {
		return fmt.Errorf("invalid node type %q", n.Type)
	}
	if !n.Status.Valid() {
		return fmt.Errorf("invalid status %q", n.Status)
	}
	if n.ID == "" {
		return errors.New("node ID required")
	}
	if n.CreatedAt.IsZero() {
		n.CreatedAt = time.Now().UTC()
	}
	if n.UpdatedAt.IsZero() {
		n.UpdatedAt = n.CreatedAt
	}
	if len(n.Content) == 0 {
		n.Content = json.RawMessage("null")
	}
	if n.ContentHash == "" {
		h, err := model.ComputeContentHash(n.Content, nil)
		if err != nil {
			return err
		}
		n.ContentHash = h
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `INSERT INTO nodes(id,type,status,content_json,content_hash,created_at,updated_at) VALUES(?,?,?,?,?,?,?)`,
		n.ID, string(n.Type), string(n.Status), string(n.Content), n.ContentHash,
		n.CreatedAt.UnixNano(), n.UpdatedAt.UnixNano(),
	); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx, `INSERT INTO node_revisions(node_id,content_hash,at,cause) VALUES(?,?,?,?)`,
		n.ID, n.ContentHash, n.CreatedAt.UnixNano(), "create",
	); err != nil {
		return err
	}
	return tx.Commit()
}

func (s *SQLite) GetNode(ctx context.Context, id string) (*model.Node, error) {
	row := s.db.QueryRowContext(ctx,
		`SELECT id,type,status,content_json,content_hash,created_at,updated_at FROM nodes WHERE id=?`, id)
	n := &model.Node{}
	var content string
	var createdNs, updatedNs int64
	var typ, status string
	if err := row.Scan(&n.ID, &typ, &status, &content, &n.ContentHash, &createdNs, &updatedNs); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, ErrNotFound
		}
		return nil, err
	}
	n.Type = model.NodeType(typ)
	n.Status = model.Status(status)
	n.Content = json.RawMessage(content)
	n.CreatedAt = time.Unix(0, createdNs).UTC()
	n.UpdatedAt = time.Unix(0, updatedNs).UTC()
	return n, nil
}

func (s *SQLite) ListNodes(ctx context.Context, f NodeFilter) ([]*model.Node, error) {
	if f.Stale {
		ids, err := s.StaleNodes(ctx)
		if err != nil {
			return nil, err
		}
		implIDs, _ := s.StaleImplementations(ctx)
		seen := make(map[string]struct{}, len(ids))
		for _, id := range ids {
			seen[id] = struct{}{}
		}
		for _, id := range implIDs {
			if _, ok := seen[id]; !ok {
				ids = append(ids, id)
			}
		}
		return s.nodesByIDs(ctx, ids, f)
	}
	if f.Parent != "" {
		ids, err := s.Neighbors(ctx, f.Parent, model.EdgeChild, model.DirOutgoing)
		if err != nil {
			return nil, err
		}
		return s.nodesByIDs(ctx, ids, f)
	}
	var (
		conds []string
		args  []any
	)
	if f.Type != "" {
		conds = append(conds, "type=?")
		args = append(args, string(f.Type))
	}
	if f.Status != "" {
		conds = append(conds, "status=?")
		args = append(args, string(f.Status))
	}
	q := `SELECT id,type,status,content_json,content_hash,created_at,updated_at FROM nodes`
	if len(conds) > 0 {
		q += " WHERE " + strings.Join(conds, " AND ")
	}
	q += " ORDER BY created_at ASC"
	if f.Limit > 0 {
		q += fmt.Sprintf(" LIMIT %d OFFSET %d", f.Limit, f.Offset)
	}
	rows, err := s.db.QueryContext(ctx, q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanNodes(rows)
}

func (s *SQLite) nodesByIDs(ctx context.Context, ids []string, f NodeFilter) ([]*model.Node, error) {
	out := make([]*model.Node, 0, len(ids))
	for _, id := range ids {
		n, err := s.GetNode(ctx, id)
		if err != nil {
			if errors.Is(err, ErrNotFound) {
				continue
			}
			return nil, err
		}
		if f.Type != "" && n.Type != f.Type {
			continue
		}
		if f.Status != "" && n.Status != f.Status {
			continue
		}
		out = append(out, n)
	}
	return out, nil
}

func scanNodes(rows *sql.Rows) ([]*model.Node, error) {
	var out []*model.Node
	for rows.Next() {
		n := &model.Node{}
		var content string
		var createdNs, updatedNs int64
		var typ, status string
		if err := rows.Scan(&n.ID, &typ, &status, &content, &n.ContentHash, &createdNs, &updatedNs); err != nil {
			return nil, err
		}
		n.Type = model.NodeType(typ)
		n.Status = model.Status(status)
		n.Content = json.RawMessage(content)
		n.CreatedAt = time.Unix(0, createdNs).UTC()
		n.UpdatedAt = time.Unix(0, updatedNs).UTC()
		out = append(out, n)
	}
	return out, rows.Err()
}

func (s *SQLite) UpdateNodeContent(ctx context.Context, id string, content json.RawMessage) error {
	if len(content) == 0 {
		content = json.RawMessage("null")
	}
	files, err := s.ListFiles(ctx, id)
	if err != nil {
		return err
	}
	newHash, err := model.ComputeContentHash(content, files)
	if err != nil {
		return err
	}
	now := time.Now().UTC().UnixNano()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	res, err := tx.ExecContext(ctx,
		`UPDATE nodes SET content_json=?, content_hash=?, updated_at=? WHERE id=?`,
		string(content), newHash, now, id)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO node_revisions(node_id,content_hash,at,cause) VALUES(?,?,?,?)`,
		id, newHash, now, "content_edit"); err != nil {
		return err
	}
	return tx.Commit()
}

func (s *SQLite) SetStatus(ctx context.Context, id string, status model.Status, reason string) error {
	if !status.Valid() {
		return fmt.Errorf("invalid status %q", status)
	}
	now := time.Now().UTC().UnixNano()
	res, err := s.db.ExecContext(ctx,
		`UPDATE nodes SET status=?, updated_at=? WHERE id=?`,
		string(status), now, id)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return ErrNotFound
	}
	return nil
}

func (s *SQLite) DeleteNode(ctx context.Context, id string) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	for _, q := range []string{
		`DELETE FROM files WHERE node_id=?`,
		`DELETE FROM edges WHERE src=? OR dst=?`,
		`DELETE FROM node_revisions WHERE node_id=?`,
		`DELETE FROM input_snapshots WHERE observer_node=? OR input_node=?`,
		`DELETE FROM runs WHERE node_id=?`,
		`DELETE FROM node_packages WHERE node_id=?`,
		`DELETE FROM node_imports WHERE node_id=?`,
		`DELETE FROM code_dep_snapshots WHERE node_id=? OR dep_node_id=?`,
		`DELETE FROM nodes WHERE id=?`,
	} {
		var args []any
		switch strings.Count(q, "?") {
		case 1:
			args = []any{id}
		case 2:
			args = []any{id, id}
		}
		if _, err := tx.ExecContext(ctx, q, args...); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// --- edges ---

func (s *SQLite) Link(ctx context.Context, src, dst string, kind model.EdgeKind) error {
	if !kind.Valid() {
		return fmt.Errorf("invalid edge kind %q", kind)
	}
	if src == dst {
		return errors.New("self-edges not allowed")
	}
	_, err := s.db.ExecContext(ctx,
		`INSERT OR IGNORE INTO edges(src,dst,kind) VALUES(?,?,?)`,
		src, dst, string(kind))
	return err
}

func (s *SQLite) Unlink(ctx context.Context, src, dst string, kind model.EdgeKind) error {
	_, err := s.db.ExecContext(ctx,
		`DELETE FROM edges WHERE src=? AND dst=? AND kind=?`,
		src, dst, string(kind))
	return err
}

func (s *SQLite) Neighbors(ctx context.Context, id string, kind model.EdgeKind, dir model.Direction) ([]string, error) {
	var q string
	switch dir {
	case model.DirOutgoing:
		q = `SELECT dst FROM edges WHERE src=? AND kind=? ORDER BY dst`
	case model.DirIncoming:
		q = `SELECT src FROM edges WHERE dst=? AND kind=? ORDER BY src`
	default:
		return nil, fmt.Errorf("invalid direction %v", dir)
	}
	rows, err := s.db.QueryContext(ctx, q, id, string(kind))
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var v string
		if err := rows.Scan(&v); err != nil {
			return nil, err
		}
		out = append(out, v)
	}
	return out, rows.Err()
}

func (s *SQLite) EdgesFor(ctx context.Context, id string) ([]model.Edge, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT src,dst,kind FROM edges WHERE src=? OR dst=? ORDER BY kind,src,dst`, id, id)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []model.Edge
	for rows.Next() {
		var e model.Edge
		var kind string
		if err := rows.Scan(&e.Src, &e.Dst, &kind); err != nil {
			return nil, err
		}
		e.Kind = model.EdgeKind(kind)
		out = append(out, e)
	}
	return out, rows.Err()
}

// --- files ---

func (s *SQLite) ListFiles(ctx context.Context, nodeID string) ([]model.FileRecord, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT rel_path,sha256,size,mtime_ns,role FROM files WHERE node_id=? ORDER BY rel_path`, nodeID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []model.FileRecord
	for rows.Next() {
		var f model.FileRecord
		var role string
		if err := rows.Scan(&f.RelPath, &f.SHA256, &f.Size, &f.MtimeNs, &role); err != nil {
			return nil, err
		}
		f.Role = model.FileRole(role)
		out = append(out, f)
	}
	return out, rows.Err()
}

func (s *SQLite) ReplaceFiles(ctx context.Context, nodeID string, files []model.FileRecord) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `DELETE FROM files WHERE node_id=?`, nodeID); err != nil {
		return err
	}
	for _, f := range files {
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO files(node_id,rel_path,sha256,size,mtime_ns,role) VALUES(?,?,?,?,?,?)`,
			nodeID, f.RelPath, f.SHA256, f.Size, f.MtimeNs, string(f.Role),
		); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (s *SQLite) RecomputeAndStoreHash(ctx context.Context, nodeID string, cause string) (string, error) {
	n, err := s.GetNode(ctx, nodeID)
	if err != nil {
		return "", err
	}
	files, err := s.ListFiles(ctx, nodeID)
	if err != nil {
		return "", err
	}
	hash, err := model.ComputeContentHash(n.Content, files)
	if err != nil {
		return "", err
	}
	if hash == n.ContentHash {
		return hash, nil
	}
	now := time.Now().UTC().UnixNano()
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return "", err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx,
		`UPDATE nodes SET content_hash=?, updated_at=? WHERE id=?`, hash, now, nodeID,
	); err != nil {
		return "", err
	}
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO node_revisions(node_id,content_hash,at,cause) VALUES(?,?,?,?)`,
		nodeID, hash, now, cause,
	); err != nil {
		return "", err
	}
	if err := tx.Commit(); err != nil {
		return "", err
	}
	return hash, nil
}

// --- runs ---

func (s *SQLite) StartRun(ctx context.Context, nodeID string, kind model.RunKind) (int64, error) {
	res, err := s.db.ExecContext(ctx,
		`INSERT INTO runs(node_id,kind,started_at) VALUES(?,?,?)`,
		nodeID, string(kind), time.Now().UTC().UnixNano())
	if err != nil {
		return 0, err
	}
	return res.LastInsertId()
}

func (s *SQLite) FinishRun(ctx context.Context, runID int64, r model.RunResult) error {
	finishedNs := r.FinishedAt.UTC().UnixNano()
	if r.FinishedAt.IsZero() {
		finishedNs = time.Now().UTC().UnixNano()
	}
	_, err := s.db.ExecContext(ctx,
		`UPDATE runs SET finished_at=?, exit_code=?, stdout_path=?, stderr_path=?, artifact_rel=? WHERE id=?`,
		finishedNs, r.ExitCode, r.StdoutPath, r.StderrPath, r.ArtifactRel, runID)
	return err
}

func (s *SQLite) GetLatestRun(ctx context.Context, nodeID string) (*model.Run, error) {
	row := s.db.QueryRowContext(ctx,
		`SELECT id,node_id,kind,started_at,finished_at,exit_code,stdout_path,stderr_path,artifact_rel
		 FROM runs WHERE node_id=? ORDER BY id DESC LIMIT 1`, nodeID)
	var r model.Run
	var kind string
	var startedNs int64
	var finishedNs sql.NullInt64
	var exitCode sql.NullInt64
	var stdout, stderr, artifact sql.NullString
	if err := row.Scan(&r.ID, &r.NodeID, &kind, &startedNs, &finishedNs, &exitCode, &stdout, &stderr, &artifact); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, ErrNotFound
		}
		return nil, err
	}
	r.Kind = model.RunKind(kind)
	r.StartedAt = time.Unix(0, startedNs).UTC()
	if finishedNs.Valid {
		r.FinishedAt = time.Unix(0, finishedNs.Int64).UTC()
	}
	if exitCode.Valid {
		ec := int(exitCode.Int64)
		r.ExitCode = &ec
	}
	r.StdoutPath = stdout.String
	r.StderrPath = stderr.String
	r.ArtifactRel = artifact.String
	return &r, nil
}

func (s *SQLite) RecordInputSnapshots(ctx context.Context, runID int64, observer string, snaps []model.InputSnapshot) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	for _, sn := range snaps {
		if _, err := tx.ExecContext(ctx,
			`INSERT OR REPLACE INTO input_snapshots(run_id,observer_node,input_node,observed_hash) VALUES(?,?,?,?)`,
			runID, observer, sn.InputNode, sn.ObservedHash,
		); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (s *SQLite) StaleNodes(ctx context.Context) ([]string, error) {
	rows, err := s.db.QueryContext(ctx, `
		WITH latest_run AS (
		  SELECT node_id, MAX(id) AS run_id FROM runs GROUP BY node_id
		)
		SELECT DISTINCT n.id
		FROM nodes n
		JOIN latest_run lr ON lr.node_id = n.id
		JOIN input_snapshots s ON s.run_id = lr.run_id
		JOIN nodes i ON i.id = s.input_node
		WHERE i.content_hash <> s.observed_hash
		  AND n.type IN ('verification','build')
		ORDER BY n.id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			return nil, err
		}
		out = append(out, id)
	}
	return out, rows.Err()
}

// --- code-level dependency tracking ---

func (s *SQLite) ReplaceNodePackages(ctx context.Context, nodeID string, pkgs []model.NodePackage) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `DELETE FROM node_packages WHERE node_id=?`, nodeID); err != nil {
		return err
	}
	for _, p := range pkgs {
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO node_packages(node_id,package_path,module_path) VALUES(?,?,?)`,
			nodeID, p.PackagePath, p.ModulePath,
		); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (s *SQLite) ReplaceNodeImports(ctx context.Context, nodeID string, imports []string) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `DELETE FROM node_imports WHERE node_id=?`, nodeID); err != nil {
		return err
	}
	for _, imp := range imports {
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO node_imports(node_id,package_path) VALUES(?,?)`,
			nodeID, imp,
		); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (s *SQLite) AffectedImplementations(ctx context.Context, nodeID string) ([]AffectedNode, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT DISTINCT ni.node_id, np.package_path
		FROM node_packages np
		JOIN node_imports ni ON ni.package_path = np.package_path
		WHERE np.node_id = ?
		  AND ni.node_id <> ?
		ORDER BY ni.node_id`, nodeID, nodeID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []AffectedNode
	for rows.Next() {
		var a AffectedNode
		if err := rows.Scan(&a.ID, &a.ViaPackage); err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, rows.Err()
}

func (s *SQLite) ImplDependencies(ctx context.Context, nodeID string) ([]AffectedNode, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT DISTINCT np.node_id, ni.package_path
		FROM node_imports ni
		JOIN node_packages np ON np.package_path = ni.package_path
		WHERE ni.node_id = ?
		  AND np.node_id <> ?
		ORDER BY np.node_id`, nodeID, nodeID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []AffectedNode
	for rows.Next() {
		var a AffectedNode
		if err := rows.Scan(&a.ID, &a.ViaPackage); err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, rows.Err()
}

func (s *SQLite) NodePackages(ctx context.Context, nodeID string) ([]string, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT package_path FROM node_packages WHERE node_id=? ORDER BY package_path`, nodeID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var p string
		if err := rows.Scan(&p); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func (s *SQLite) NodeImports(ctx context.Context, nodeID string) ([]string, error) {
	rows, err := s.db.QueryContext(ctx,
		`SELECT package_path FROM node_imports WHERE node_id=? ORDER BY package_path`, nodeID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var p string
		if err := rows.Scan(&p); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func (s *SQLite) ReplaceCodeDepSnapshots(ctx context.Context, nodeID string, deps map[string]string) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `DELETE FROM code_dep_snapshots WHERE node_id=?`, nodeID); err != nil {
		return err
	}
	for depID, hash := range deps {
		if _, err := tx.ExecContext(ctx,
			`INSERT INTO code_dep_snapshots(node_id,dep_node_id,observed_hash) VALUES(?,?,?)`,
			nodeID, depID, hash,
		); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (s *SQLite) StaleImplementations(ctx context.Context) ([]string, error) {
	rows, err := s.db.QueryContext(ctx, `
		SELECT DISTINCT cs.node_id
		FROM code_dep_snapshots cs
		JOIN nodes n ON n.id = cs.dep_node_id
		WHERE n.content_hash <> cs.observed_hash
		ORDER BY cs.node_id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			return nil, err
		}
		out = append(out, id)
	}
	return out, rows.Err()
}
