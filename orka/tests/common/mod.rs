//! Shared fixture for Orka integration tests: a real workbench (two git
//! repositories and a real Linka store), plus helpers to shape the graph
//! through Linka's public operations.
//!
//! Each integration-test binary includes this module and uses a different
//! subset of it, so unused helpers are expected per binary.
#![allow(dead_code)]

use linka::{Author, GitVcs, Store};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct TempDir(pub PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("running git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.name", "orka test"]);
    git(dir, &["config", "user.email", "test@orka.invalid"]);
}

/// A real workbench: outer repo holding `.linka/`, inner `project/` repo.
pub fn workbench() -> (TempDir, PathBuf) {
    let root = std::env::temp_dir().join(format!("orka-it-{}", ulid::Ulid::new()));
    init_repo(&root);
    init_repo(&root.join("project"));
    linka::ops::init_workbench(root.join(".linka"), None).unwrap();
    (TempDir(root.clone()), root)
}

pub fn store_at(root: &Path) -> Store {
    Store::open(root.join(".linka")).unwrap()
}

pub fn add_node(root: &Path, description: &str, depends_on: Vec<String>) -> String {
    add_related(root, description, depends_on, vec![])
}

pub fn add_related(
    root: &Path,
    description: &str,
    depends_on: Vec<String>,
    derived_from: Vec<String>,
) -> String {
    let store = store_at(root);
    let vcs = GitVcs::for_store(&store);
    linka::ops::add(
        &store,
        &vcs,
        linka::ops::NewNode {
            description: description.into(),
            author: Author::Human,
            assignee: None,
            depends_on,
            derived_from,
        },
    )
    .unwrap()
}

/// Add a node assigned to a human, so machine selection must skip it.
pub fn add_human_node(root: &Path, description: &str) -> String {
    let store = store_at(root);
    let vcs = GitVcs::for_store(&store);
    linka::ops::add(
        &store,
        &vcs,
        linka::ops::NewNode {
            description: description.into(),
            author: Author::Human,
            assignee: Some(Author::Human),
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap()
}

pub fn complete_node(root: &Path, id: &str, outputs: &[String], notes: &str) {
    let store = store_at(root);
    let vcs = GitVcs::for_store(&store);
    linka::ops::complete(&store, &vcs, id, outputs, &[], None, notes, Author::Human).unwrap();
}

pub fn edit_node(root: &Path, id: &str, description: &str) {
    let store = store_at(root);
    let vcs = GitVcs::for_store(&store);
    linka::ops::edit(&store, &vcs, id, description.into()).unwrap();
}
