//! The seam the in-memory fake abstracts: the same operations against a real
//! temporary workbench and the `git` binary.

use std::path::{Path, PathBuf};
use std::process::Command;

use linka::graph::Graph;
use linka::model::{
    Author, Conclusion, IntegrationStatus, NewCandidate, StalenessReason, Submission, Workability,
};
use linka::ops::{self, NewNode};
use linka::{check, GitVcs, NodeId, Store};

struct Workbench {
    root: PathBuf,
    store: Store,
    vcs: GitVcs,
}

impl Drop for Workbench {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("running git {args:?}: {error}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with an identity configured, so committing needs nothing from
/// the environment running the test.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--initial-branch=main"]);
    git(dir, &["config", "user.name", "linka test"]);
    git(dir, &["config", "user.email", "test@linka.invalid"]);
}

impl Workbench {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("linka-workbench-{}", ulid::Ulid::new()));
        init_repo(&root);
        let store = Store::init(root.join(".linka")).unwrap();
        init_repo(&store.project_root());
        git(
            &store.project_root(),
            &["commit", "--allow-empty", "-m", "root"],
        );
        // Git tracks no empty directories, so a fresh store has nothing to
        // commit yet — it is already clean. The workbench still needs a root
        // commit for history to be countable.
        git(&root, &["commit", "--allow-empty", "-m", "workbench"]);
        let vcs = GitVcs::for_store(&store);
        let bench = Self { root, store, vcs };
        ops::pair(&bench.store, &bench.vcs, Some("test".into()), false).unwrap();
        bench
    }

    fn project(&self) -> PathBuf {
        self.store.project_root()
    }

    fn graph(&self) -> Graph<'_> {
        Graph::load(&self.store, &self.vcs).unwrap()
    }

    fn add(&self, description: &str) -> NodeId {
        ops::add(
            &self.store,
            &self.vcs,
            NewNode {
                description: description.into(),
                author: Author::Human,
                assignee: None,
                depends_on: Vec::new(),
                derived_from: Vec::new(),
            },
            None,
        )
        .unwrap()
    }

    fn write(&self, path: &str, content: &str) {
        let file = self.project().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
}

#[test]
fn work_is_completed_captured_and_retained_in_the_project_repository() {
    let bench = Workbench::new();
    let id = bench.add("write the parser");
    bench.write("src/parser.rs", "fn parse() {}\n");

    let commit = ops::complete(
        &bench.store,
        &bench.vcs,
        &id,
        &["src/parser.rs".into()],
        &[],
        Some("add the parser".into()),
        "wrote it",
        Author::Machine,
    )
    .unwrap()
    .expect("declared outputs produce a commit");

    // The commit is real, carries its trailers, and is retained by a ref.
    let message = git(&bench.project(), &["show", "-s", "--format=%B", &commit]);
    assert!(message.contains(&format!("Linka-Node: {id}")), "{message}");
    assert!(message.contains("Linka-Input: "), "{message}");
    assert_eq!(
        git(
            &bench.project(),
            &["rev-parse", &format!("refs/linka/outputs/{id}")]
        ),
        commit
    );

    let graph = bench.graph();
    assert_eq!(graph.state(&id).workability(), Workability::Complete);
    assert_eq!(graph.origin(&commit), Some(&id));
    assert!(check::check_artifacts(&bench.store, &bench.vcs)
        .unwrap()
        .is_empty());

    // A direct result drifts with the project working tree.
    bench.write("src/parser.rs", "fn parse() { todo!() }\n");
    let graph = bench.graph();
    assert!(
        matches!(
            graph.state(&id).staleness(),
            [StalenessReason::OutputDrifted { .. }]
        ),
        "{:?}",
        graph.state(&id).staleness()
    );
    assert_eq!(graph.state(&id).workability(), Workability::Ready);
}

#[test]
fn a_candidate_is_reviewed_and_published_by_fast_forward() {
    let bench = Workbench::new();
    let id = bench.add("write the parser");
    bench.write("src/parser.rs", "fn parse() {}\n");
    // Work on its own branch, so publishing is a real fast-forward of main.
    git(&bench.project(), &["checkout", "-q", "-b", "work/parser"]);
    let commit = ops::complete(
        &bench.store,
        &bench.vcs,
        &id,
        &["src/parser.rs".into()],
        &[],
        None,
        "wrote it",
        Author::Machine,
    )
    .unwrap()
    .unwrap();

    let candidate = ops::register_candidate(
        &bench.store,
        &bench.vcs,
        NewCandidate {
            node: id.clone(),
            branch: "work/parser".into(),
            target: "main".into(),
            external: None,
        },
    )
    .unwrap();
    assert_eq!(
        bench.graph().state(&id).integration(),
        Some(IntegrationStatus::Pending)
    );

    let review = ops::add(
        &bench.store,
        &bench.vcs,
        NewNode {
            description: "review the parser".into(),
            author: Author::Human,
            assignee: None,
            depends_on: Vec::new(),
            derived_from: Vec::new(),
        },
        Some(candidate.id.clone()),
    )
    .unwrap();
    let snapshot = ops::snapshot(&bench.store, &bench.vcs, &review, &[]).unwrap();
    ops::submit(
        &bench.store,
        &bench.vcs,
        Submission {
            snapshot,
            conclusion: Conclusion::Accepted,
            notes: "looks right".into(),
            author: Author::Human,
            producer: None,
            attachments: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        bench.graph().state(&id).integration(),
        Some(IntegrationStatus::Accepted)
    );

    ops::publish(&bench.vcs, &candidate).unwrap();

    assert_eq!(git(&bench.project(), &["rev-parse", "main"]), commit);
    let graph = bench.graph();
    assert_eq!(
        graph.state(&id).integration(),
        Some(IntegrationStatus::Published)
    );
    assert_eq!(graph.state(&id).workability(), Workability::Complete);
    // A published artifact is not drift: the candidate's commit is immutable.
    assert_eq!(graph.state(&id).staleness(), []);
    // Publication is re-derived from ancestry, so repeating it is a no-op.
    ops::publish(&bench.vcs, &candidate).unwrap();
    assert!(check::check_artifacts(&bench.store, &bench.vcs)
        .unwrap()
        .is_empty());
}

#[test]
fn every_mutation_is_one_commit_and_leaves_the_store_clean() {
    let bench = Workbench::new();
    let before: usize = git(&bench.root, &["rev-list", "--count", "HEAD"])
        .parse()
        .unwrap();

    let first = bench.add("first");
    let second = bench.add("second");
    ops::link(
        &bench.store,
        &bench.vcs,
        &second,
        &first,
        linka::DepKind::DependsOn,
    )
    .unwrap();
    ops::edit(&bench.store, &bench.vcs, &first, "first, restated".into()).unwrap();

    let after: usize = git(&bench.root, &["rev-list", "--count", "HEAD"])
        .parse()
        .unwrap();
    assert_eq!(after - before, 4, "one commit per mutation");
    assert!(check::check_workbench(&bench.store, &bench.vcs)
        .unwrap()
        .is_empty());

    // An uncommitted hand edit blocks the next mutation until it is resolved.
    std::fs::write(
        bench.store.node_dir(&second).join("description.md"),
        "hand edit",
    )
    .unwrap();
    let error = ops::edit(&bench.store, &bench.vcs, &first, "again".into()).unwrap_err();
    assert!(
        format!("{error:#}").contains("uncommitted store changes"),
        "{error:#}"
    );
    assert!(!check::check_workbench(&bench.store, &bench.vcs)
        .unwrap()
        .is_empty());
}

#[test]
fn an_interrupted_completion_is_refused_rather_than_built_upon() {
    let bench = Workbench::new();
    let id = bench.add("write the parser");
    bench.write("src/parser.rs", "fn parse() {}\n");
    // A project commit that claims to be a node's output, which the store has
    // never recorded — exactly what an interruption between the two commits
    // leaves behind.
    git(&bench.project(), &["add", "src/parser.rs"]);
    git(
        &bench.project(),
        &[
            "commit",
            "-m",
            &format!("orphaned output\n\nLinka-Node: {id}"),
        ],
    );

    let error = ops::require_consistent_project_head(&bench.store, &bench.vcs).unwrap_err();
    assert!(
        format!("{error:#}").contains("never recorded that output"),
        "{error:#}"
    );
    let error = ops::complete(
        &bench.store,
        &bench.vcs,
        &id,
        &[],
        &[],
        None,
        "",
        Author::Machine,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("never recorded that output"),
        "{error:#}"
    );
}
