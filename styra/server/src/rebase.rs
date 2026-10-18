//! Standalone rebase that retains the interaction and records its new base.
use super::*;
use crate::protocol::RebaseStatus;

#[derive(serde::Serialize, serde::Deserialize)]
struct Retarget {
    branch: String,
    upstream: String,
}

fn store_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let temporary = path.with_file_name(format!(".rebase-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn store_status(path: &Path, status: &RebaseStatus) -> Result<()> {
    store_json(path, status)
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
        self.start_checkout_change(id, None, None)
    }

    pub(super) fn start_checkout_change(
        &self,
        id: &str,
        target: Option<&str>,
        restore: Option<&str>,
    ) -> Result<()> {
        self.ensure_not_integrating(id)?;
        let (session, mut checkout, mut source) = if target.is_some() || restore.is_some() {
            let session = self.stored_summary(id)?;
            anyhow::ensure!(
                session.completed != CompletionState::Sealed,
                "a sealed Session cannot change branches"
            );
            let checkout = self
                .session_checkout(&session.path, &session.workspace_id, id)?
                .context("this interaction has no associated checkout")?;
            let source = checkout
                .branched_from
                .as_ref()
                .and_then(|point| point.branch.clone())
                .unwrap_or_default();
            (session, checkout, source)
        } else {
            self.integration_checkout(id)?
        };
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
            .context("the checkout is missing")?
            .to_path_buf();
        let workspace = crate::workspace::get(&self.inner.store_root, &session.workspace_id)?;
        let sessions = journal::list_workspace_sessions(&self.inner.store_root, &workspace.id)?;
        anyhow::ensure!(
            !self
                .checkouts_in_use(&sessions, &workspace.id)?
                .iter()
                .any(|(holder, held)| holder != id && held == &path),
            "another active interaction shares this checkout"
        );
        let record = session.path.join("rebase.json");
        let in_progress = self.inner.git.rebase_in_progress(&path)?;
        anyhow::ensure!(!(in_progress && (target.is_some() || restore.is_some())),
            "finish the current rebase before changing branches; retry the normal rebase action to continue");
        if !in_progress {
            anyhow::ensure!(
                self.inner.git.current_branch(&path)?.as_deref() == Some(&checkout.branch),
                "the checkout is no longer on its recorded branch"
            );
            anyhow::ensure!(
                !self.inner.git.has_uncommitted_changes(&path)?,
                "commit the interaction's changes before rebasing"
            );
        }
        let restored = restore
            .map(|branch| -> Result<_> {
                anyhow::ensure!(
                    branch != checkout.branch,
                    "the interaction is already on this branch"
                );
                journal::read_session_branch_history(&session.path)?
                    .into_iter()
                    .rev()
                    .find(|saved| saved.branch == branch)
                    .context("this branch is not in the interaction's history")
            })
            .transpose()?;
        if let Some(target) = target {
            anyhow::ensure!(
                target != checkout.branch,
                "the target and interaction branches are the same"
            );
            anyhow::ensure!(
                target != source,
                "the interaction already targets this branch; use rebase to update it"
            );
            // Require a local branch, rather than accepting arbitrary revision expressions.
            anyhow::ensure!(
                self.inner.git.local_branch_commit(&path, target)?.is_some(),
                "the target must be an existing local branch"
            );
            source = target.to_owned();
        }
        let source_commit = if restored.is_some() {
            String::new()
        } else if in_progress {
            let previous: RebaseStatus = serde_json::from_slice(&std::fs::read(&record).context(
                "an unrelated rebase is in progress; finish it before rebasing this interaction",
            )?)?;
            anyhow::ensure!(!previous.succeeded, "an unrelated rebase is in progress");
            previous.source_commit
        } else {
            self.inner
                .git
                .local_branch_commit(&path, &source)?
                .context("the source branch no longer exists")?
        };
        let status = RebaseStatus {
            finished: false,
            succeeded: false,
            message: format!("Rebasing onto {source} ({source_commit})…"),
            source_commit,
            output: String::new(),
        };
        anyhow::ensure!(
            self.inner.integrating.lock().unwrap().insert(id.into()),
            "this interaction is already being rebased or integrated"
        );
        let prepared = (|| -> Result<()> {
            let _git = self.inner.integration_git.lock().unwrap();
            if target.is_some() || restored.is_some() {
                let previous = checkout.clone();
                let mut next = checkout.clone();
                if let Some(saved) = &restored {
                    next.branch = saved.branch.clone();
                    next.branched_from = saved.branched_from.clone();
                } else {
                    // Always fork, so retargeting never rewrites the previous branch.
                    next.branch = format!("styra/retarget-{}", uuid::Uuid::new_v4());
                    let upstream = previous
                        .branched_from
                        .as_ref()
                        .context("the checkout has no base")?
                        .commit
                        .clone();
                    let retarget = Retarget {
                        branch: next.branch.clone(),
                        upstream,
                    };
                    store_json(&session.path.join("retarget.json"), &retarget)?;
                    next.branched_from.as_mut().unwrap().branch = Some(source.clone());
                }
                self.inner
                    .git
                    .switch_branch(&path, &next.branch, restored.is_none())?;
                if let Err(error) =
                    journal::store_retargeted_checkout(&session.path, &previous, &next)
                {
                    self.inner
                        .git
                        .switch_branch(&path, &previous.branch, false)
                        .context(
                            "could not restore the previous branch after a metadata write failed",
                        )?;
                    return Err(error);
                }
                checkout = next;
            }
            if restored.is_none() {
                store_status(&record, &status)?;
            }
            Ok(())
        })();
        if prepared.is_err() || restored.is_some() {
            if prepared.is_ok() {
                if let Ok(interaction) = self.interaction(id) {
                    interaction.working_tree.locate();
                    interaction.push_update(InteractionUpdate::Log(LogEntry::info(format!(
                        "Returned to {}",
                        checkout.branch
                    ))));
                }
            }
            self.inner.integrating.lock().unwrap().remove(id);
            return prepared;
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
                interaction.working_tree.locate();
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
        self.run_rebase_with_resolver(id, checkout, status, record, |id, path, status| {
            let source = status.source_commit.clone();
            self.resolve_rebase_conflicts_with_progress(id, &source, path, false, |output| {
                status.output = output;
                store_status(record, status)
            })
        })
    }

    pub(super) fn run_rebase_with_resolver(
        &self,
        id: &str,
        mut checkout: crate::worktree::Checkout,
        status: &mut RebaseStatus,
        record: &Path,
        resolve: impl FnOnce(&str, &Path, &mut RebaseStatus) -> Result<()>,
    ) -> Result<()> {
        let path = checkout
            .path
            .as_deref()
            .context("the checkout is missing")?;
        let rebased = {
            let _git = self.inner.integration_git.lock().unwrap();
            let retarget_path = self.stored_summary(id)?.path.join("retarget.json");
            let retarget = if retarget_path.exists() {
                Some(serde_json::from_slice::<Retarget>(&std::fs::read(
                    retarget_path,
                )?)?)
            } else {
                None
            };
            if let Some(retarget) = retarget.filter(|retarget| retarget.branch == checkout.branch) {
                self.inner
                    .git
                    .rebase_onto(path, &status.source_commit, &retarget.upstream)?
            } else {
                self.inner.git.rebase(path, &status.source_commit)?
            }
        };
        if !rebased {
            status.message =
                "Conflicts found. Launching the interaction's current model to resolve them…"
                    .into();
            store_status(record, status)?;
            resolve(id, path, status)?;
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
        let session = self.stored_summary(id)?.path;
        journal::store_rebased_checkout(&session, &checkout)?;
        let marker = session.join("retarget.json");
        if marker.exists() {
            std::fs::remove_file(marker)?;
        }
        Ok(())
    }
}
