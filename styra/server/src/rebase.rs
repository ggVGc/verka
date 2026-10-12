//! Standalone rebase that retains the interaction and records its new base.
use super::*;
use crate::protocol::RebaseStatus;

fn store_status(path: &Path, status: &RebaseStatus) -> Result<()> {
    let temporary = path.with_file_name(format!(".rebase-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec(status)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

impl ServerState {
    pub(super) fn rebase_status(&self, id: &str) -> Result<RebaseStatus> {
        let session = self.stored_summary(id)?;
        let mut status: RebaseStatus =
            serde_json::from_slice(&std::fs::read(session.path.join("rebase.json"))?)?;
        if !status.finished && !self.inner.integrating.lock().unwrap().contains(id) {
            status.finished = true;
            status.message = "rebase was interrupted; retry rebase to continue".into();
        }
        Ok(status)
    }

    pub(super) fn start_rebase(&self, id: &str) -> Result<()> {
        self.ensure_not_integrating(id)?;
        let (session, mut checkout, source) = self.integration_checkout(id)?;
        anyhow::ensure!(
            journal::read_queued_messages(&session.path)?.is_empty(),
            "send or discard queued messages before rebasing"
        );
        if let Ok(interaction) = self.interaction(id) {
            anyhow::ensure!(
                matches!(
                    interaction.activity.activity(),
                    InteractionActivity::Pending | InteractionActivity::Stopped
                ),
                "wait for the interaction to become idle before rebasing"
            );
            anyhow::ensure!(
                interaction.queued_messages().is_empty(),
                "send or discard queued messages before rebasing"
            );
        }
        if checkout.path.is_none() {
            let workspace = crate::workspace::get(&self.inner.store_root, &session.workspace_id)?;
            let worktrees = self
                .workspace_worktrees(&workspace, true)?
                .context("the repository is missing")?;
            checkout.path = Some(worktrees.restore(&checkout.branch)?);
            journal::store_session_checkout(&session.path, &checkout)?;
        }
        let path = checkout
            .path
            .as_deref()
            .context("the checkout is missing")?;
        let workspace = crate::workspace::get(&self.inner.store_root, &session.workspace_id)?;
        let sessions = journal::list_workspace_sessions(&self.inner.store_root, &workspace.id)?;
        anyhow::ensure!(
            !self
                .checkouts_in_use(&sessions, &workspace.id)?
                .iter()
                .any(|(holder, held)| holder != id && held == path),
            "another active interaction shares this checkout"
        );
        let record = session.path.join("rebase.json");
        let source_commit = if self.inner.git.rebase_in_progress(path)? {
            let previous: RebaseStatus = serde_json::from_slice(&std::fs::read(&record).context(
                "an unrelated rebase is in progress; finish it before rebasing this interaction",
            )?)?;
            anyhow::ensure!(!previous.succeeded, "an unrelated rebase is in progress");
            previous.source_commit
        } else {
            anyhow::ensure!(
                self.inner.git.current_branch(path)?.as_deref() == Some(&checkout.branch),
                "the checkout is no longer on its recorded branch"
            );
            anyhow::ensure!(
                !self.inner.git.has_uncommitted_changes(path)?,
                "commit the interaction's changes before rebasing"
            );
            self.inner
                .git
                .commit(path, &source)?
                .context("the source branch no longer exists")?
        };
        let status = RebaseStatus {
            finished: false,
            succeeded: false,
            message: format!("Rebasing onto {source} ({source_commit})…"),
            source_commit,
        };
        anyhow::ensure!(
            self.inner.integrating.lock().unwrap().insert(id.into()),
            "this interaction is already being rebased or integrated"
        );
        if let Err(error) = store_status(&record, &status) {
            self.inner.integrating.lock().unwrap().remove(id);
            return Err(error);
        }
        let state = self.clone();
        let id = id.to_owned();
        std::thread::spawn(move || {
            let mut status = status;
            let result = state.run_rebase(&id, checkout, &mut status, &record);
            status.finished = true;
            status.succeeded = result.is_ok();
            status.message = match result {
                Ok(()) => format!("Rebase completed. New base: {}", status.source_commit),
                Err(error) => format!("Rebase stopped: {error:#}; retry rebase to continue"),
            };
            if let Err(error) = store_status(&record, &status) {
                tracing::warn!(
                    session_id = id,
                    "could not persist rebase result: {error:#}"
                );
            }
            if let Ok(interaction) = state.interaction(&id) {
                interaction.push_update(InteractionUpdate::Log(if status.succeeded {
                    LogEntry::info(status.message)
                } else {
                    LogEntry::warn(status.message)
                }));
            }
            state.inner.integrating.lock().unwrap().remove(&id);
        });
        Ok(())
    }

    pub(super) fn run_rebase(
        &self,
        id: &str,
        checkout: crate::worktree::Checkout,
        status: &mut RebaseStatus,
        record: &Path,
    ) -> Result<()> {
        self.run_rebase_with_resolver(id, checkout, status, record, |id, source, path| {
            self.resolve_rebase_conflicts(id, source, path, false)
        })
    }

    pub(super) fn run_rebase_with_resolver(
        &self,
        id: &str,
        mut checkout: crate::worktree::Checkout,
        status: &mut RebaseStatus,
        record: &Path,
        resolve: impl FnOnce(&str, &str, &Path) -> Result<()>,
    ) -> Result<()> {
        let path = checkout
            .path
            .as_deref()
            .context("the checkout is missing")?;
        let rebased = {
            let _git = self.inner.integration_git.lock().unwrap();
            self.inner.git.rebase(path, &status.source_commit)?
        };
        if !rebased {
            status.message =
                "Conflicts found. Launching the interaction's current model to resolve them…"
                    .into();
            store_status(record, status)?;
            resolve(id, &status.source_commit, path)?;
        }
        status.message = "Verifying the rebase and recording the new source commit…".into();
        store_status(record, status)?;
        let _git = self.inner.integration_git.lock().unwrap();
        anyhow::ensure!(
            !self.inner.git.rebase_in_progress(path)?,
            "the rebase is still in progress"
        );
        anyhow::ensure!(
            self.inner.git.current_branch(path)?.as_deref() == Some(&checkout.branch),
            "the rebase did not return to the interaction branch"
        );
        anyhow::ensure!(
            !self.inner.git.has_uncommitted_changes(path)?,
            "the rebased checkout has uncommitted changes"
        );
        anyhow::ensure!(
            self.inner
                .git
                .is_ancestor(path, &status.source_commit, "HEAD")?,
            "the checkout does not contain the new source commit"
        );
        checkout
            .branched_from
            .as_mut()
            .context("the checkout has no source")?
            .commit = status.source_commit.clone();
        journal::store_rebased_checkout(&self.stored_summary(id)?.path, &checkout)
    }
}
