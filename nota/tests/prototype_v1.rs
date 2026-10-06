use anyhow::{bail, Context, Result};
use nota::{
    add_note, load_review, load_review_ref, start_review, Git, GitProvider, ReviewEntryKind,
    ReviewProvider,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const ROOT: &str = "/project";

struct FakeCommit {
    parent: Option<String>,
    message: String,
    paths: Vec<String>,
}

#[derive(Default)]
struct State {
    commits: HashMap<String, FakeCommit>,
    branches: BTreeMap<String, String>,
    /// The checked-out branch; `None` is a detached HEAD.
    head: Option<String>,
    /// Contents of every file written through [`Git::commit_file`].
    files: HashMap<String, String>,
}

/// An in-memory repository at [`ROOT`] whose `main` branch holds one commit,
/// the subject under review.
struct FakeGit {
    state: RefCell<State>,
}

impl FakeGit {
    fn new() -> Self {
        let git = Self {
            state: RefCell::new(State::default()),
        };
        let subject = git.add_commit(None, "subject", &["README.md"]);
        let mut state = git.state.borrow_mut();
        state.branches.insert("main".into(), subject);
        state.head = Some("main".into());
        drop(state);
        git
    }

    fn add_commit(&self, parent: Option<String>, message: &str, paths: &[&str]) -> String {
        let mut state = self.state.borrow_mut();
        let id = format!("{:040x}", state.commits.len() + 1);
        state.commits.insert(
            id.clone(),
            FakeCommit {
                parent,
                message: message.into(),
                paths: paths.iter().map(|path| path.to_string()).collect(),
            },
        );
        id
    }

    /// Commit on the checked-out branch, as a reviewer's `git commit` would.
    fn commit(&self, message: &str, paths: &[&str]) -> String {
        let branch = self
            .state
            .borrow()
            .head
            .clone()
            .expect("a checked-out branch");
        let commit = self.add_commit(Some(self.tip(&branch)), message, paths);
        self.state
            .borrow_mut()
            .branches
            .insert(branch, commit.clone());
        commit
    }

    fn switch(&self, branch: &str) {
        assert!(self.state.borrow().branches.contains_key(branch));
        self.state.borrow_mut().head = Some(branch.into());
    }

    fn tip(&self, branch: &str) -> String {
        self.state.borrow().branches[branch].clone()
    }

    fn file(&self, path: &str) -> Option<String> {
        self.state.borrow().files.get(path).cloned()
    }

    fn with_commit<T>(&self, commit: &str, read: impl FnOnce(&FakeCommit) -> T) -> Result<T> {
        let state = self.state.borrow();
        let commit = state
            .commits
            .get(commit)
            .with_context(|| format!("no commit `{commit}`"))?;
        Ok(read(commit))
    }
}

impl Git for FakeGit {
    fn repository_root(&self, path: &Path) -> Result<PathBuf> {
        if !path.starts_with(ROOT) {
            bail!("{} is not inside a Git repository", path.display());
        }
        Ok(ROOT.into())
    }

    fn resolve_commit(&self, _repository: &Path, revision: &str) -> Result<String> {
        let state = self.state.borrow();
        let branch = match revision {
            "HEAD" => state.head.as_deref().context("detached HEAD")?,
            other => other,
        };
        if let Some(commit) = state.branches.get(branch) {
            return Ok(commit.clone());
        }
        if state.commits.contains_key(revision) {
            return Ok(revision.into());
        }
        bail!("unknown revision `{revision}`")
    }

    fn current_branch(&self, _repository: &Path) -> Result<Option<String>> {
        Ok(self.state.borrow().head.clone())
    }

    fn validate_branch_name(&self, _repository: &Path, branch: &str) -> Result<()> {
        if branch.is_empty() || branch.contains("..") || branch.contains(char::is_whitespace) {
            bail!("`{branch}` is not a valid branch name");
        }
        Ok(())
    }

    fn branch_exists(&self, _repository: &Path, branch: &str) -> Result<bool> {
        Ok(self.state.borrow().branches.contains_key(branch))
    }

    fn create_branch(&self, _repository: &Path, branch: &str, commit: &str) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if state.branches.contains_key(branch) {
            bail!("branch `{branch}` already exists");
        }
        if !state.commits.contains_key(commit) {
            bail!("no commit `{commit}`");
        }
        state.branches.insert(branch.into(), commit.into());
        Ok(())
    }

    fn commit_empty(&self, _repository: &Path, parent: &str, message: &str) -> Result<String> {
        self.with_commit(parent, |_| ())?;
        Ok(self.add_commit(Some(parent.into()), message, &[]))
    }

    fn commit_file(
        &self,
        _repository: &Path,
        path: &str,
        contents: &str,
        message: &str,
    ) -> Result<String> {
        self.state
            .borrow_mut()
            .files
            .insert(path.into(), contents.into());
        Ok(self.commit(message, &[path]))
    }

    fn first_parent_history(&self, repository: &Path, revision: &str) -> Result<Vec<String>> {
        let mut next = Some(self.resolve_commit(repository, revision)?);
        let mut history = Vec::new();
        while let Some(commit) = next {
            next = self.first_parent(repository, &commit)?;
            history.push(commit);
        }
        Ok(history)
    }

    fn commit_message(&self, _repository: &Path, commit: &str) -> Result<String> {
        self.with_commit(commit, |commit| commit.message.clone())
    }

    fn first_parent(&self, _repository: &Path, commit: &str) -> Result<Option<String>> {
        self.with_commit(commit, |commit| commit.parent.clone())
    }

    fn changed_paths(&self, _repository: &Path, commit: &str) -> Result<Vec<String>> {
        self.with_commit(commit, |commit| commit.paths.clone())
    }
}

fn root() -> PathBuf {
    PathBuf::from(ROOT)
}

/// Start a review of `main` on `branch` and check that branch out.
fn review_on(git: &FakeGit, branch: &str) {
    start_review(git, &GitProvider::new(git, root()), "HEAD", Some(branch)).unwrap();
    git.switch(branch);
}

#[test]
fn git_provider_resolves_an_exact_commit() {
    let git = FakeGit::new();
    let subject = GitProvider::new(&git, root().join("src"))
        .resolve_subject("HEAD")
        .unwrap();
    assert_eq!(subject.repository, root());
    assert_eq!(subject.revision, git.tip("main"));
}

#[test]
fn review_branch_records_notes_and_ordinary_project_commits_as_suggestions() {
    let git = FakeGit::new();
    let subject = git.tip("main");
    let started = start_review(
        &git,
        &GitProvider::new(&git, root()),
        "HEAD",
        Some("nota/review-one"),
    )
    .unwrap();

    assert_eq!(started.subject, subject);
    assert_eq!(git.tip("nota/review-one"), started.marker);
    assert_eq!(
        git.current_branch(&root()).unwrap().as_deref(),
        Some("main"),
        "starting a review must not change the checkout"
    );
    assert_eq!(
        git.first_parent(&root(), &started.marker).unwrap(),
        Some(subject.clone())
    );
    let review = load_review_ref(&git, &root(), "nota/review-one").unwrap();
    assert_eq!(review.subject, subject);
    assert!(review.entries.is_empty());

    git.switch("nota/review-one");
    let note = add_note(&git, &root(), "Please explain this behavior.").unwrap();
    assert_eq!(note.kind, ReviewEntryKind::Note);
    assert_eq!(note.message, "Please explain this behavior.");
    assert_eq!(note.paths.len(), 1);
    assert!(note.paths[0].starts_with(".nota/notes/note-"));
    assert_eq!(
        git.file(&note.paths[0]).as_deref(),
        Some("Please explain this behavior.\n")
    );

    let suggestion = git.commit("Make the behavior explicit.", &["suggested.txt"]);
    let review = load_review(&git, &root()).unwrap();
    assert_eq!(review.branch, "nota/review-one");
    assert_eq!(review.marker, started.marker);
    assert_eq!(review.subject, subject);
    assert_eq!(review.entries.len(), 2);
    assert_eq!(review.entries[0].commit, note.commit);
    assert_eq!(review.entries[1].commit, suggestion);
    assert_eq!(review.entries[1].kind, ReviewEntryKind::Suggestion);
    assert_eq!(review.entries[1].paths, vec!["suggested.txt"]);
}

#[test]
fn loading_a_review_rejects_suggestions_containing_nota_files() {
    let git = FakeGit::new();
    review_on(&git, "nota/invalid");
    git.commit("invalid suggestion", &[".nota/metadata"]);

    let error = load_review(&git, &root()).unwrap_err();
    assert!(format!("{error:#}").contains("may not contain Nota files"));
}

#[test]
fn loading_a_review_rejects_empty_suggestion_commits() {
    let git = FakeGit::new();
    review_on(&git, "nota/empty");
    git.commit("empty suggestion", &[]);

    let error = load_review(&git, &root()).unwrap_err();
    assert!(format!("{error:#}").contains("has no changed project files"));
}

#[test]
fn loading_a_review_rejects_suggestions_without_a_comment() {
    let git = FakeGit::new();
    review_on(&git, "nota/no-comment");
    git.commit("", &["suggested.txt"]);

    let error = load_review(&git, &root()).unwrap_err();
    assert!(format!("{error:#}").contains("has an empty review comment"));
}
