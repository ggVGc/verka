CREATE TABLE nodes (
  id           TEXT PRIMARY KEY,
  type         TEXT NOT NULL,
  status       TEXT NOT NULL,
  content_json TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX nodes_by_type ON nodes(type);
CREATE INDEX nodes_by_status ON nodes(status);

CREATE TABLE edges (
  src  TEXT NOT NULL,
  dst  TEXT NOT NULL,
  kind TEXT NOT NULL,
  PRIMARY KEY(src, dst, kind)
);
CREATE INDEX edges_by_dst ON edges(dst, kind);

CREATE TABLE files (
  node_id  TEXT NOT NULL,
  rel_path TEXT NOT NULL,
  sha256   TEXT NOT NULL,
  size     INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL,
  role     TEXT NOT NULL,
  PRIMARY KEY(node_id, rel_path)
);

CREATE TABLE node_revisions (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  node_id      TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  at           INTEGER NOT NULL,
  cause        TEXT NOT NULL
);
CREATE INDEX node_revisions_by_node ON node_revisions(node_id, id);

CREATE TABLE runs (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  node_id      TEXT NOT NULL,
  kind         TEXT NOT NULL,
  started_at   INTEGER NOT NULL,
  finished_at  INTEGER,
  exit_code    INTEGER,
  stdout_path  TEXT,
  stderr_path  TEXT,
  artifact_rel TEXT
);
CREATE INDEX runs_by_node ON runs(node_id, id);

CREATE TABLE input_snapshots (
  run_id        INTEGER NOT NULL,
  observer_node TEXT NOT NULL,
  input_node    TEXT NOT NULL,
  observed_hash TEXT NOT NULL,
  PRIMARY KEY(run_id, input_node)
);
CREATE INDEX input_snapshots_by_observer ON input_snapshots(observer_node);

CREATE TABLE schema_meta (
  version INTEGER NOT NULL
);
INSERT INTO schema_meta(version) VALUES (1);
