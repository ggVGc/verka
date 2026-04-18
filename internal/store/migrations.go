package store

import (
	"context"
	"database/sql"
	"embed"
	"fmt"
	"io/fs"
	"sort"
	"strings"
)

//go:embed migrations/*.sql
var migrationsFS embed.FS

type migration struct {
	version int
	name    string
	body    string
}

func loadMigrations() ([]migration, error) {
	entries, err := fs.ReadDir(migrationsFS, "migrations")
	if err != nil {
		return nil, err
	}
	var migs []migration
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".sql") {
			continue
		}
		var v int
		if _, err := fmt.Sscanf(e.Name(), "%04d_", &v); err != nil {
			return nil, fmt.Errorf("migration %s: cannot parse version: %w", e.Name(), err)
		}
		body, err := fs.ReadFile(migrationsFS, "migrations/"+e.Name())
		if err != nil {
			return nil, err
		}
		migs = append(migs, migration{version: v, name: e.Name(), body: string(body)})
	}
	sort.Slice(migs, func(i, j int) bool { return migs[i].version < migs[j].version })
	return migs, nil
}

func currentSchemaVersion(ctx context.Context, db *sql.DB) (int, error) {
	var exists int
	if err := db.QueryRowContext(ctx,
		`SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_meta'`,
	).Scan(&exists); err != nil {
		return 0, err
	}
	if exists == 0 {
		return 0, nil
	}
	var v int
	if err := db.QueryRowContext(ctx, `SELECT version FROM schema_meta LIMIT 1`).Scan(&v); err != nil {
		if err == sql.ErrNoRows {
			return 0, nil
		}
		return 0, err
	}
	return v, nil
}

func runMigrations(ctx context.Context, db *sql.DB) error {
	migs, err := loadMigrations()
	if err != nil {
		return err
	}
	cur, err := currentSchemaVersion(ctx, db)
	if err != nil {
		return err
	}
	for _, m := range migs {
		if m.version <= cur {
			continue
		}
		if _, err := db.ExecContext(ctx, m.body); err != nil {
			return fmt.Errorf("migration %s failed: %w", m.name, err)
		}
	}
	return nil
}
