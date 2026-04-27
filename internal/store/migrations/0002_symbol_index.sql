CREATE TABLE node_packages (
  node_id      TEXT NOT NULL,
  package_path TEXT NOT NULL,
  module_path  TEXT NOT NULL,
  PRIMARY KEY(node_id, package_path)
);
CREATE INDEX node_packages_by_pkg ON node_packages(package_path);

CREATE TABLE node_imports (
  node_id      TEXT NOT NULL,
  package_path TEXT NOT NULL,
  PRIMARY KEY(node_id, package_path)
);
CREATE INDEX node_imports_by_pkg ON node_imports(package_path);

CREATE TABLE code_dep_snapshots (
  node_id       TEXT NOT NULL,
  dep_node_id   TEXT NOT NULL,
  observed_hash TEXT NOT NULL,
  PRIMARY KEY(node_id, dep_node_id)
);

UPDATE schema_meta SET version = 2;
