use super::*;
use crate::git::{Git, SystemGit};
use std::process::Command;

#[test]
fn rebase_resolution_branches_share_the_checkout_and_leave_the_source_context_untouched() {
    use super::super::native_transcripts::MemoryTranscripts;
    use genta::session::{Message, Role, Session};

    let mut fixture = Fixture::new();
    fixture.conflict();
    let transcripts = Arc::new(MemoryTranscripts::default());
    fixture.state = ServerState::with_backends(
        SystemGit::shared(),
        transcripts.clone(),
        fixture.state.store_root().to_path_buf(),
        None,
        None,
        None,
    );
    let provider = crate::agent::Provider::Codex;
    let native = genta::session::serialize(
        &Session {
            id: "original".into(),
            cwd: fixture.path().display().to_string(),
            timestamp: "2026-11-11T10:00:00Z".into(),
            messages: vec![Message {
                role: Role::User,
                text: "Implement the feature".into(),
                timestamp: Some("2026-11-11T10:00:00Z".into()),
            }],
        },
        native_session_format(provider),
    );
    transcripts
        .write(provider, "original", fixture.path(), &native)
        .unwrap();
    journal::store_provider_session_id(&fixture.session, "original").unwrap();
    let mut parent = Journal::open(&fixture.session).unwrap();
    parent.record_user_message("Implement the feature").unwrap();
    let parent_journal = parent.path().to_path_buf();
    drop(parent);
    let selection = fixture.state.stored_summary(&fixture.id).unwrap().selection;
    let source = git(&fixture.host, &["rev-parse", "main"]);
    let mut status = pending_rebase(source.clone());
    let record = fixture.session.join("rebase.json");
    let mut branches = Vec::new();

    // An unsuccessful attempt must release the checkout as well, so a
    // retry forks the original context rather than the failed resolver.
    for succeed in [false, true] {
        let result = fixture.state.run_rebase_with_resolver(
            &fixture.id,
            fixture.checkout.clone(),
            &mut status,
            &record,
            |id, path, _| {
                fixture.state.with_rebase_conversation(id, |branch| {
                    branches.push(branch.id.clone());
                    assert_ne!(branch.id, fixture.id);
                    assert_eq!(branch.selection, selection);
                    assert_eq!(branch.origin.as_ref().unwrap().session_id, fixture.id);
                    assert_eq!(
                        journal::read_session_checkout(&branch.path)?,
                        Some(fixture.checkout.clone())
                    );
                    assert_eq!(
                        journal::read_session_auto_commit(&branch.path)?,
                        Some(false)
                    );
                    let native_id = journal::read_provider_session_id(&branch.path)?.unwrap();
                    assert_ne!(native_id, "original");
                    let history = transcripts.read(provider, &native_id)?;
                    assert!(history.contains("Implement the feature"));
                    assert!(!history.contains("Resolve conflicts"));
                    Journal::open(&branch.path)?.record_user_message("Resolve conflicts")?;
                    transcripts.write(provider, &native_id, path, "resolver-only work")?;
                    if !succeed {
                        anyhow::bail!("resolver failed");
                    }
                    std::fs::write(path.join("file"), "source and feature\n")?;
                    git(path, &["add", "file"]);
                    continue_rebase(path)
                })
            },
        );
        assert_eq!(result.is_ok(), succeed);
        let branch = fixture
            .state
            .stored_summary(branches.last().unwrap())
            .unwrap();
        assert_eq!(
            branch.completed,
            if succeed {
                CompletionState::Completed
            } else {
                CompletionState::Abandoned
            }
        );
        assert_eq!(transcripts.read(provider, "original").unwrap(), native);
        let parent = std::fs::read_to_string(&parent_journal).unwrap();
        assert!(!parent.contains("Resolve conflicts"));
        assert!(!parent.contains("resolver-only work"));
        assert_eq!(
            journal::read_provider_session_id(&fixture.session)
                .unwrap()
                .as_deref(),
            Some("original")
        );
        fixture
            .state
            .ensure_branches_closed(&fixture.state.stored_summary(&fixture.id).unwrap())
            .unwrap();
        let sessions =
            journal::list_workspace_sessions(fixture.state.store_root(), &branch.workspace_id)
                .unwrap();
        assert_eq!(
            fixture
                .state
                .checkouts_in_use(&sessions, &branch.workspace_id)
                .unwrap(),
            vec![(fixture.id.clone(), fixture.path().to_path_buf())]
        );
    }
    assert_ne!(branches[0], branches[1]);
    assert_eq!(
        journal::read_session_checkout(&fixture.session)
            .unwrap()
            .unwrap()
            .branched_from
            .unwrap()
            .commit,
        source
    );
    fixture
        .state
        .run_integration(&fixture.id, fixture.progress())
        .unwrap();
    assert!(!fixture.path().exists());
}

#[test]
fn conflict_resolution_can_branch_while_its_own_automatic_action_is_applied() {
    use super::super::native_transcripts::MemoryTranscripts;
    use genta::session::{Message, Role, Session};

    // The resolver branches the interaction on the action's behalf, so the
    // lock the action holds must not refuse it.
    for action in [AutomaticAction::Rebase, AutomaticAction::Integration] {
        // The interaction branch and its source branch both change `file`,
        // so rebasing onto the source stops on a conflict.
        let mut fixture = Fixture::new();
        fixture.conflict();
        // Keep native transcripts in memory, so branching has a provider
        // transcript to copy without touching the real provider stores.
        let transcripts = Arc::new(MemoryTranscripts::default());
        fixture.state = ServerState::with_backends(
            SystemGit::shared(),
            transcripts.clone(),
            fixture.state.store_root().to_path_buf(),
            None,
            None,
            None,
        );
        // Give the interaction a stored provider transcript; the resolver
        // branches from it.
        let provider = crate::agent::Provider::Codex;
        let native = genta::session::serialize(
            &Session {
                id: "original".into(),
                cwd: fixture.path().display().to_string(),
                timestamp: "2026-11-11T10:00:00Z".into(),
                messages: vec![Message {
                    role: Role::User,
                    text: "Implement the feature".into(),
                    timestamp: Some("2026-11-11T10:00:00Z".into()),
                }],
            },
            native_session_format(provider),
        );
        transcripts
            .write(provider, "original", fixture.path(), &native)
            .unwrap();
        journal::store_provider_session_id(&fixture.session, "original").unwrap();
        // Prepare a rebase onto the source branch's current commit.
        let source = git(&fixture.host, &["rev-parse", "main"]);
        let mut status = pending_rebase(source);
        let record = fixture.session.join("rebase.json");
        // Hold the action's lock on the interaction, as `start_rebase` and
        // integration do for as long as they run.
        fixture
            .state
            .inner
            .automatic
            .begin(&fixture.id, action)
            .unwrap();
        // Rebase while the lock is held. On the conflict, the resolver
        // branches the interaction, which used to be refused by the lock,
        // then resolves the conflict in place of an agent turn and
        // continues the rebase.
        let result = fixture.state.run_rebase_with_resolver(
            &fixture.id,
            fixture.checkout.clone(),
            &mut status,
            &record,
            |id, path, _| {
                fixture.state.with_rebase_conversation(id, |_| {
                    std::fs::write(path.join("file"), "source and feature\n")?;
                    git(path, &["add", "file"]);
                    continue_rebase(path)
                })
            },
        );
        // Release the lock, then require that the branch was created and
        // the rebase finished.
        fixture.state.inner.automatic.finish(&fixture.id);
        assert!(result.is_ok(), "{action}: {:#}", result.unwrap_err());
    }
}

#[test]
fn conflict_resolution_cannot_branch_into_a_checkout_another_automatic_action_uses() {
    use super::super::native_transcripts::MemoryTranscripts;
    use genta::session::{Message, Role, Session};

    // Its own action may not refuse the resolver, but another interaction's
    // action on the same checkout still must.
    //
    // Set up a conflicting rebase, with native transcripts kept in memory.
    let mut fixture = Fixture::new();
    fixture.conflict();
    let transcripts = Arc::new(MemoryTranscripts::default());
    fixture.state = ServerState::with_backends(
        SystemGit::shared(),
        transcripts.clone(),
        fixture.state.store_root().to_path_buf(),
        None,
        None,
        None,
    );
    // Create a second interaction, `other`, in the same Workspace that
    // shares the fixture interaction's checkout.
    let session = fixture.state.stored_summary(&fixture.id).unwrap();
    let (journal, other) = create_session(
        fixture.state.store_root(),
        &session.workspace_id,
        &fixture.host,
        None,
    );
    let other_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);
    journal::store_session_checkout(&other_path, &fixture.checkout).unwrap();
    // Give `other` a stored provider transcript, so branching it would
    // succeed if the lock allowed it.
    let provider = crate::agent::Provider::Codex;
    let native = genta::session::serialize(
        &Session {
            id: "other".into(),
            cwd: fixture.path().display().to_string(),
            timestamp: "2026-11-11T10:00:00Z".into(),
            messages: vec![Message {
                role: Role::User,
                text: "Implement the feature".into(),
                timestamp: Some("2026-11-11T10:00:00Z".into()),
            }],
        },
        native_session_format(provider),
    );
    transcripts
        .write(provider, "other", fixture.path(), &native)
        .unwrap();
    journal::store_provider_session_id(&other_path, "other").unwrap();
    // Apply an integration to the fixture interaction, which needs the
    // shared checkout, and a rebase to `other`, whose own lock must not
    // refuse its resolver.
    fixture
        .state
        .inner
        .automatic
        .begin(&fixture.id, AutomaticAction::Integration)
        .unwrap();
    fixture
        .state
        .inner
        .automatic
        .begin(&other, AutomaticAction::Rebase)
        .unwrap();
    // Branch `other` for conflict resolution, recording whether the
    // resolver got to run.
    let mut resolved = false;
    let result = fixture.state.with_rebase_conversation(&other, |_| {
        resolved = true;
        Ok(())
    });
    fixture.state.inner.automatic.finish(&other);
    fixture.state.inner.automatic.finish(&fixture.id);
    // The integration's use of the checkout refuses the branch before the
    // resolver runs.
    assert!(
        !resolved,
        "the resolver ran in a checkout another action uses"
    );
    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("this checkout is participating in an automatic action"),
        "{error:#}"
    );
}

#[test]
fn an_action_without_an_integration_checkout_does_not_refuse_unrelated_interactions() {
    // An action whose interaction has no checkout to integrate, such as
    // one sealed at the end of integration or one without a recorded source
    // branch, cannot need an unrelated checkout; its state is not the
    // unrelated interaction's error.
    //
    // The fixture interaction's worktree is removed and the interaction
    // sealed, as at the end of a successful integration.
    let sealed_after_integration = |fixture: &Fixture| {
        SystemGit
            .remove_worktree(&fixture.host, fixture.path())
            .unwrap();
        journal::clear_session_worktree(&fixture.session).unwrap();
        fixture
            .state
            .set_session_completion(&fixture.id, CompletionState::Sealed)
            .unwrap();
    };
    // The fixture interaction's base records a commit but no branch name,
    // so there is no source branch to integrate onto.
    let without_source_branch = |fixture: &Fixture| {
        let mut checkout = fixture.checkout.clone();
        checkout.branched_from.as_mut().unwrap().branch = None;
        journal::store_rebased_checkout(&fixture.session, &checkout).unwrap();
    };
    let cases: [(&str, AutomaticAction, &dyn Fn(&Fixture)); 2] = [
        (
            "sealed after integration",
            AutomaticAction::Integration,
            &sealed_after_integration,
        ),
        (
            "without a source branch",
            AutomaticAction::Retarget,
            &without_source_branch,
        ),
    ];
    for (case, action, prepare) in cases {
        // Create an unrelated interaction in the same Workspace, with its
        // own worktree on its own branch.
        let fixture = Fixture::new();
        let session = fixture.state.stored_summary(&fixture.id).unwrap();
        let (journal, unrelated) = create_session(
            fixture.state.store_root(),
            &session.workspace_id,
            &fixture.host,
            None,
        );
        let unrelated_session = journal.path().parent().unwrap().to_path_buf();
        drop(journal);
        let unrelated_path = fixture.host.parent().unwrap().join("unrelated");
        git(
            &fixture.host,
            &[
                "worktree",
                "add",
                "-b",
                "unrelated",
                unrelated_path.to_str().unwrap(),
            ],
        );
        journal::store_session_checkout(
            &unrelated_session,
            &crate::worktree::Checkout {
                path: Some(unrelated_path),
                branch: "unrelated".into(),
                branched_from: None,
            },
        )
        .unwrap();
        // Put the fixture interaction in the case's state, then apply the
        // action to it.
        prepare(&fixture);
        fixture
            .state
            .inner
            .automatic
            .begin(&fixture.id, action)
            .unwrap();
        // Check whether the unrelated interaction may be acted on. Finding
        // no integration checkout for the fixture interaction must not
        // refuse it.
        let checked = fixture.state.ensure_no_automatic_action(&unrelated);
        fixture.state.inner.automatic.finish(&fixture.id);
        if let Err(error) = checked {
            panic!("{case}: {error:#}");
        }
    }
}

#[test]
fn completion_checks_older_checkout_records_without_a_base() {
    let fixture = Fixture::new();
    fixture.feature();
    let mut checkout = fixture.checkout.clone();
    checkout.branched_from = None;
    let meta_path = fixture.session.join("session.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    meta["checkout"]
        .as_object_mut()
        .unwrap()
        .remove("branched_from");
    std::fs::write(meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    let error = fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: fixture.id.clone(),
            completed: CompletionState::Completed,
        })
        .unwrap_err();
    assert!(error.to_string().contains("integrate"), "{error:#}");
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Active
    );
    git(&fixture.host, &["merge", "--ff-only", &checkout.branch]);
    fixture
        .state
        .ensure_checkout_integrated(&fixture.id)
        .unwrap();
}

#[test]
fn completion_checks_a_workspace_in_an_unrecorded_worktree() {
    let fixture = Fixture::new();
    fixture.feature();
    let workspace =
        crate::workspace::create(fixture.state.store_root(), fixture.path(), None).unwrap();

    let (journal, id) = create_session(
        fixture.state.store_root(),
        &workspace.id,
        fixture.path(),
        None,
    );
    let session = journal.path().parent().unwrap().to_path_buf();
    drop(journal);
    assert!(journal::read_session_checkout(&session).unwrap().is_none());
    let error = fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: id.clone(),
            completed: CompletionState::Completed,
        })
        .unwrap_err();
    assert!(error.to_string().contains("integrate"), "{error:#}");
    assert_eq!(
        journal::read_session_completed(&session).unwrap(),
        CompletionState::Active
    );
    git(
        &fixture.host,
        &["merge", "--ff-only", &fixture.checkout.branch],
    );
    fixture
        .state
        .handle(Request::SetSessionCompleted {
            id,
            completed: CompletionState::Completed,
        })
        .unwrap();
}

#[test]
fn completion_allows_shared_work_but_blocks_the_last_active_interaction() {
    let fixture = Fixture::new();
    fixture.feature();
    let session = fixture.state.stored_summary(&fixture.id).unwrap();

    let (journal, separate_id) = create_session(
        fixture.state.store_root(),
        &session.workspace_id,
        &fixture.host,
        None,
    );
    let separate_path = journal.path().parent().unwrap().to_path_buf();
    let workspace =
        crate::workspace::get(fixture.state.store_root(), &session.workspace_id).unwrap();
    let separate_checkout = fixture
        .state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .unwrap()
        .checkout(&separate_id, None)
        .unwrap();
    journal::store_session_checkout(&separate_path, &separate_checkout).unwrap();
    assert!(
        fixture
            .state
            .handle(Request::SetSessionCompleted {
                id: fixture.id.clone(),
                completed: CompletionState::Completed,
            })
            .is_err(),
        "an interaction in another worktree does not count"
    );
    drop(journal);
    let (journal, other_id) = create_session(
        fixture.state.store_root(),
        &session.workspace_id,
        &fixture.host,
        None,
    );
    let other_path = journal.path().parent().unwrap().to_path_buf();
    journal::store_session_checkout(&other_path, &fixture.checkout).unwrap();
    drop(journal);

    // Dirty work is safe to leave with another active interaction too.
    std::fs::write(fixture.path().join("untracked"), "work").unwrap();
    fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: fixture.id.clone(),
            completed: CompletionState::Completed,
        })
        .unwrap();
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Completed
    );
    for completed in [
        CompletionState::Completed,
        CompletionState::Abandoned,
        CompletionState::Sealed,
    ] {
        journal::store_session_completed(&fixture.session, completed).unwrap();
        let error = fixture
            .state
            .handle(Request::SetSessionCompleted {
                id: other_id.clone(),
                completed: CompletionState::Completed,
            })
            .unwrap_err();
        assert!(
            error.to_string().contains("last active interaction"),
            "{error:#}"
        );
        assert_eq!(
            journal::read_session_completed(&other_path).unwrap(),
            CompletionState::Active
        );
    }
    std::fs::remove_file(fixture.path().join("untracked")).unwrap();
    git(
        &fixture.host,
        &["merge", "--ff-only", &fixture.checkout.branch],
    );
    fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: other_id,
            completed: CompletionState::Completed,
        })
        .unwrap();
}

#[test]
fn completion_counts_active_interactions_in_an_unrecorded_worktree() {
    let fixture = Fixture::new();
    fixture.feature();
    let workspace =
        crate::workspace::create(fixture.state.store_root(), fixture.path(), None).unwrap();

    let mut ids = Vec::new();
    for _ in 0..2 {
        let (journal, id) = create_session(
            fixture.state.store_root(),
            &workspace.id,
            fixture.path(),
            None,
        );
        assert!(
            journal::read_session_checkout(journal.path().parent().unwrap())
                .unwrap()
                .is_none()
        );
        ids.push(id);
    }
    fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: ids[0].clone(),
            completed: CompletionState::Completed,
        })
        .unwrap();
    assert!(fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: ids[1].clone(),
            completed: CompletionState::Completed,
        })
        .is_err());
}

#[test]
fn completion_refuses_unmerged_changes_and_leaves_the_session_active() {
    let fixture = Fixture::new();
    let complete = || {
        fixture.state.handle(Request::SetSessionCompleted {
            id: fixture.id.clone(),
            completed: CompletionState::Completed,
        })
    };
    // Untracked, staged and unstaged work must all block completion.
    std::fs::write(fixture.path().join("feature"), "work").unwrap();
    assert!(complete().unwrap_err().to_string().contains("integrate"));
    git(fixture.path(), &["add", "feature"]);
    assert!(complete().is_err());
    SystemGit.commit_all(fixture.path(), "feature").unwrap();
    assert!(complete().is_err(), "committed work is still unmerged");
    std::fs::write(fixture.path().join("feature"), "edited").unwrap();
    assert!(complete().is_err());
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Active
    );
    // Abandonment remains available for work the operator chooses to drop.
    fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: fixture.id.clone(),
            completed: CompletionState::Abandoned,
        })
        .unwrap();
}

#[test]
fn completion_allows_a_checkout_without_changes_from_its_base() {
    let fixture = Fixture::new();
    // Even a different commit is harmless when it has identical contents.
    git(fixture.path(), &["commit", "--allow-empty", "-m", "empty"]);
    fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: fixture.id.clone(),
            completed: CompletionState::Completed,
        })
        .unwrap();
}

#[test]
fn completion_allows_changes_merged_into_the_source() {
    let fixture = Fixture::new();
    fixture.feature();
    git(
        &fixture.host,
        &["merge", "--ff-only", &fixture.checkout.branch],
    );
    fixture
        .state
        .handle(Request::SetSessionCompleted {
            id: fixture.id.clone(),
            completed: CompletionState::Completed,
        })
        .unwrap();
    // New work after that merge is no longer integrated.
    std::fs::write(fixture.path().join("later"), "later").unwrap();
    SystemGit.commit_all(fixture.path(), "later").unwrap();
    assert!(fixture
        .state
        .ensure_checkout_integrated(&fixture.id)
        .is_err());
}

#[test]
fn completion_checks_branches_whose_checkout_was_removed() {
    let fixture = Fixture::new();
    fixture.feature();
    SystemGit
        .remove_worktree(&fixture.host, fixture.path())
        .unwrap();
    journal::clear_session_worktree(&fixture.session).unwrap();
    assert!(fixture
        .state
        .ensure_checkout_integrated(&fixture.id)
        .is_err());
    git(
        &fixture.host,
        &["merge", "--ff-only", &fixture.checkout.branch],
    );
    fixture
        .state
        .ensure_checkout_integrated(&fixture.id)
        .unwrap();
}

#[test]
fn completion_recognizes_a_recorded_squash_but_rejects_later_commits() {
    let fixture = Fixture::new();
    fixture.feature();
    let mut progress = fixture.progress();
    progress.branch_commit = SystemGit.commit(fixture.path(), "HEAD").unwrap();
    progress.commit = Some(
        SystemGit
            .squash_merge(&fixture.host, &fixture.checkout.branch, "squash")
            .unwrap(),
    );
    store_progress(&fixture.session.join(journal::INTEGRATION_FILE), &progress).unwrap();
    fixture
        .state
        .ensure_checkout_integrated(&fixture.id)
        .unwrap();
    std::fs::write(fixture.path().join("later"), "later").unwrap();
    SystemGit.commit_all(fixture.path(), "later").unwrap();
    assert!(fixture
        .state
        .ensure_checkout_integrated(&fixture.id)
        .is_err());
}

/// A listing names what integration landed only once a commit has, and
/// a record from before the branch was kept in it still names one.
#[test]
fn a_landed_integration_is_listed_with_the_branch_it_landed_on() {
    let fixture = Fixture::new();
    let record = fixture.session.join(journal::INTEGRATION_FILE);
    assert!(journal::read_session_integration(&fixture.session).is_none());
    let mut progress = fixture.progress();
    store_progress(&record, &progress).unwrap();
    assert!(
        journal::read_session_integration(&fixture.session).is_none(),
        "an integration that has not committed anything has landed nothing"
    );
    progress.commit = Some("0123456789abcdef".into());
    store_progress(&record, &progress).unwrap();
    assert_eq!(
        journal::read_session_integration(&fixture.session),
        Some(crate::protocol::Integration {
            branch: Some("main".into()),
            commit: "0123456789abcdef".into(),
        })
    );
}

#[test]
fn background_integration_publishes_its_result() {
    let fixture = Fixture::new();
    fixture.feature();
    fixture
        .state
        .start_integration(&fixture.id, "Approved asynchronous integration")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if let Some(result) = fixture.state.integration_result(&fixture.id).unwrap() {
            break result;
        }
        assert!(Instant::now() < deadline, "integration did not finish");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(result.completed, "{}", result.message);
    assert_eq!(
        git(&fixture.host, &["log", "-1", "--format=%s"]),
        "Approved asynchronous integration"
    );
    assert!(!fixture.path().exists());
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Sealed
    );
}

#[test]
fn a_shared_active_checkout_is_not_integrated_or_removed() {
    let fixture = Fixture::new();
    fixture.feature();
    let session = fixture.state.stored_summary(&fixture.id).unwrap();

    let (journal, _) = create_session(
        fixture.state.store_root(),
        &session.workspace_id,
        &fixture.host,
        None,
    );
    journal::store_session_checkout(journal.path().parent().unwrap(), &fixture.checkout).unwrap();
    assert!(fixture
        .state
        .run_integration(&fixture.id, fixture.progress())
        .is_err());
    assert!(fixture.path().exists());
    assert_eq!(git(&fixture.host, &["rev-list", "--count", "main"]), "1");
}

#[test]
fn a_source_that_advanced_after_rebase_is_rejected_without_staging() {
    let fixture = Fixture::new();
    fixture.feature();
    assert!(SystemGit.rebase(fixture.path(), "main").unwrap());
    std::fs::write(fixture.host.join("upstream"), "upstream").unwrap();
    SystemGit
        .commit_all(&fixture.host, "advanced source")
        .unwrap();
    assert!(SystemGit
        .squash_merge(&fixture.host, &fixture.checkout.branch, "message")
        .is_err());
    assert!(!SystemGit.has_uncommitted_changes(&fixture.host).unwrap());
    assert!(fixture.path().exists());
}

/// Diff mode measures from where the branch would be integrated, so a
/// rebase onto an advanced source moves the base to the new source.
#[test]
fn the_diff_base_follows_the_integration_target_through_a_rebase() {
    let fixture = Fixture::new();
    let initial = git(&fixture.host, &["rev-parse", "HEAD"]);
    fixture.feature();
    std::fs::write(fixture.host.join("upstream"), "upstream").unwrap();
    SystemGit
        .commit_all(&fixture.host, "advanced source")
        .unwrap();
    let advanced = git(&fixture.host, &["rev-parse", "HEAD"]);
    let base = |fixture: &Fixture| fixture.state.diff_base(&fixture.id).unwrap().unwrap();
    assert_eq!(
        base(&fixture),
        crate::protocol::DiffBase {
            base: initial,
            branch: Some("main".into()),
        }
    );

    fixture.state.start_rebase(&fixture.id).unwrap();
    assert!(wait_for_rebase(&fixture).succeeded);
    assert_eq!(
        base(&fixture),
        crate::protocol::DiffBase {
            base: advanced,
            branch: Some("main".into()),
        }
    );
}

#[test]
fn comparison_branch_is_persisted_validated_and_can_be_reset() {
    let fixture = Fixture::new();
    let initial = fixture.state.diff_base(&fixture.id).unwrap().unwrap();
    git(fixture.path(), &["branch", "comparison"]);
    fixture.feature();
    let id = fixture.id.clone();
    fixture
        .state
        .handle(Request::SetDiffBranch {
            id: id.clone(),
            branch: Some(fixture.checkout.branch.clone()),
        })
        .unwrap();
    let chosen = fixture.state.diff_base(&id).unwrap().unwrap();
    assert_eq!(chosen.base, git(fixture.path(), &["rev-parse", "HEAD"]));
    assert_eq!(
        chosen.branch.as_deref(),
        Some(fixture.checkout.branch.as_str())
    );
    assert_eq!(
        journal::read_session_diff_branch(&fixture.session).unwrap(),
        chosen.branch
    );
    let reopened = ServerState::with_git(
        SystemGit::shared(),
        fixture.state.store_root().to_path_buf(),
    );
    assert_eq!(reopened.diff_base(&id).unwrap(), Some(chosen.clone()));
    assert_eq!(
        fixture
            .state
            .session_checkout(&fixture.session, "", &id)
            .unwrap()
            .unwrap()
            .branched_from,
        fixture.checkout.branched_from
    );
    for branch in ["missing", "main~1"] {
        assert!(fixture
            .state
            .handle(Request::SetDiffBranch {
                id: id.clone(),
                branch: Some(branch.into()),
            })
            .is_err());
        assert_eq!(fixture.state.diff_base(&id).unwrap(), Some(chosen.clone()));
    }
    fixture
        .state
        .handle(Request::SetDiffBranch {
            id: id.clone(),
            branch: None,
        })
        .unwrap();
    assert_eq!(fixture.state.diff_base(&id).unwrap(), Some(initial));
}

#[test]
fn comparison_branches_work_without_a_styra_checkout_record() {
    let fixture = Fixture::new();
    let external = fixture.host.join("external");
    git(
        &fixture.host,
        &[
            "worktree",
            "add",
            "-b",
            "external",
            external.to_str().unwrap(),
        ],
    );
    // Its HEAD differs from the main checkout, so resolving against the
    // wrong directory produces a different merge base.
    git(&external, &["commit", "--allow-empty", "-m", "external"]);
    let expected = git(&external, &["rev-parse", "HEAD"]);
    for (root, reported) in [
        (fixture.host.as_path(), None),
        (external.as_path(), None),
        (fixture.host.as_path(), Some(external.as_path())),
    ] {
        let workspace = crate::workspace::create(fixture.state.store_root(), root, None).unwrap();
        let selection = Selection::new(crate::agent::Provider::Codex);
        let profile = crate::agent::resolve_profile(&selection, &workspace_layout(root)).unwrap();
        let (mut journal, id) = Journal::create_in_workspace(
            fixture.state.store_root(),
            &workspace.id,
            &profile,
            &selection,
            None,
        )
        .unwrap();
        if let Some(directory) = reported {
            journal
                .record_agent_line(
                    &serde_json::json!({ "result": { "cwd": directory } }).to_string(),
                )
                .unwrap();
        }
        let session = journal.path().parent().unwrap().to_path_buf();
        drop(journal);
        let Response::DiffBranches(branches) = fixture
            .state
            .handle(Request::DiffBranches { id: id.clone() })
            .unwrap()
        else {
            panic!("expected branches");
        };
        assert!(branches.contains(&"main".into()));
        assert!(branches.contains(&"external".into()));
        fixture
            .state
            .handle(Request::SetDiffBranch {
                id: id.clone(),
                branch: Some("external".into()),
            })
            .unwrap();
        let expected_base = if root == external || reported.is_some() {
            expected.clone()
        } else {
            git(&fixture.host, &["rev-parse", "HEAD"])
        };
        let reopened = ServerState::with_git(
            SystemGit::shared(),
            fixture.state.store_root().to_path_buf(),
        );
        assert_eq!(
            reopened.diff_base(&id).unwrap().unwrap(),
            crate::protocol::DiffBase {
                base: expected_base,
                branch: Some("external".into()),
            }
        );
        fixture
            .state
            .handle(Request::SetDiffBranch { id, branch: None })
            .unwrap();
        assert!(journal::read_session_checkout(&session).unwrap().is_none());
    }
}

#[test]
fn comparison_branches_are_sorted_by_commit_date() {
    let fixture = Fixture::new();
    git(fixture.path(), &["branch", "z-newest"]);
    let output = Command::new("git")
        .arg("-C")
        .arg(fixture.path())
        .args(["commit", "--allow-empty", "-m", "newest"])
        .env("GIT_COMMITTER_DATE", "2030-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(output.status.success());
    git(fixture.path(), &["branch", "-f", "z-newest", "HEAD"]);
    let Response::DiffBranches(branches) = fixture
        .state
        .handle(Request::DiffBranches {
            id: fixture.id.clone(),
        })
        .unwrap()
    else {
        panic!("expected branches");
    };
    assert_eq!(
        &branches[..2],
        &[fixture.checkout.branch.clone(), "z-newest".into()]
    );
    assert_eq!(branches.last().unwrap(), "main");
}

struct Fixture {
    _directory: tempfile::TempDir,
    host: PathBuf,
    state: ServerState,
    session: PathBuf,
    id: String,
    checkout: crate::worktree::Checkout,
}

fn git(path: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let host = directory.path().join("repository");
        std::fs::create_dir(&host).unwrap();
        git(&host, &["init", "-b", "main"]);
        git(&host, &["config", "user.name", "Integration Test"]);
        git(&host, &["config", "user.email", "integration@example.test"]);
        std::fs::write(host.join("file"), "original\n").unwrap();
        SystemGit.commit_all(&host, "initial").unwrap();
        let store = directory.path().join("store");
        let state = ServerState::with_git(SystemGit::shared(), store.clone());
        let workspace = crate::workspace::create(&store, &host, None).unwrap();
        let selection = Selection::new(crate::agent::Provider::Codex);
        let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
        let (journal, id) =
            Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None)
                .unwrap();
        let session = journal.path().parent().unwrap().to_path_buf();
        drop(journal);
        let checkout = state
            .workspace_worktrees(&workspace, true)
            .unwrap()
            .unwrap()
            .checkout(&id, None)
            .unwrap();
        journal::store_session_checkout(&session, &checkout).unwrap();
        Self {
            _directory: directory,
            host,
            state,
            session,
            id,
            checkout,
        }
    }

    fn path(&self) -> &Path {
        self.checkout.path.as_deref().unwrap()
    }

    fn progress(&self) -> Progress {
        Progress {
            message: "Integrate the feature\n\nWith the approved body.".into(),
            commit: None,
            temporary_source: None,
            result: None,
            branch_commit: None,
            branch: None,
        }
    }

    fn feature(&self) {
        std::fs::write(self.path().join("feature"), "one\n").unwrap();
        SystemGit.commit_all(self.path(), "feature one").unwrap();
        std::fs::write(self.path().join("feature"), "two\n").unwrap();
        SystemGit.commit_all(self.path(), "feature two").unwrap();
    }

    fn conflict(&self) {
        std::fs::write(self.path().join("file"), "feature\n").unwrap();
        SystemGit.commit_all(self.path(), "feature").unwrap();
        std::fs::write(self.host.join("file"), "source\n").unwrap();
        SystemGit.commit_all(&self.host, "source").unwrap();
    }

    fn branch_record(&self, parent: &str, completed: CompletionState) -> (String, PathBuf) {
        let session = self.state.stored_summary(&self.id).unwrap();
        let selection = Selection::new(crate::agent::Provider::Codex);

        let (journal, id) = create_session(
            self.state.store_root(),
            &session.workspace_id,
            &self.host,
            Some("Child branch"),
        );
        let path = journal.path().parent().unwrap().to_path_buf();
        drop(journal);
        journal::store_session_origin(
            &path,
            SessionOrigin {
                session_id: parent.into(),
                provider: selection.provider,
                at_ms: None,
                history: Default::default(),
            },
        )
        .unwrap();
        journal::store_session_completed(&path, completed).unwrap();
        (id, path)
    }
}

#[test]
fn integration_requires_all_descendant_branches_to_be_closed() {
    let fixture = Fixture::new();
    fixture.feature();
    let (parent, _) = fixture.branch_record(&fixture.id, CompletionState::Completed);
    fixture.branch_record(&fixture.id, CompletionState::Sealed);
    fixture.branch_record("another-interaction", CompletionState::Active);
    let source = git(&fixture.host, &["rev-parse", "main"]);
    for (parent, completed) in [
        (&fixture.id, CompletionState::Completed),
        (&parent, CompletionState::Abandoned),
    ] {
        let (branch, path) = fixture.branch_record(parent, CompletionState::Active);
        for error in [
            fixture
                .state
                .start_integration(&fixture.id, "message")
                .unwrap_err(),
            fixture
                .state
                .run_integration(&fixture.id, fixture.progress())
                .unwrap_err(),
        ] {
            let message = error.to_string();
            assert!(message.contains("still open"), "{message}");
            assert!(message.contains(&branch), "{message}");
        }
        assert_eq!(git(&fixture.host, &["rev-parse", "main"]), source);
        assert!(fixture.path().exists());
        assert!(!fixture.session.join(journal::INTEGRATION_FILE).exists());
        assert_eq!(fixture.state.inner.automatic.applied(&fixture.id), None);
        journal::store_session_completed(&path, completed).unwrap();
    }
    fixture
        .state
        .run_integration(&fixture.id, fixture.progress())
        .unwrap();
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Sealed
    );
}

fn create_session(
    store: &Path,
    workspace_id: &str,
    directory: &Path,
    name: Option<&str>,
) -> (Journal, String) {
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(directory)).unwrap();
    Journal::create_in_workspace(
        store,
        workspace_id,
        &profile,
        &selection,
        name.map(str::to_owned),
    )
    .unwrap()
}

fn pending_rebase(source_commit: String) -> crate::protocol::RebaseStatus {
    crate::protocol::RebaseStatus {
        finished: false,
        succeeded: false,
        message: String::new(),
        output: String::new(),
        source_commit,
    }
}

fn continue_rebase(path: &Path) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .env("GIT_EDITOR", "true")
        .args(["rebase", "--continue"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "rebase continue failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn wait_for_rebase(fixture: &Fixture) -> crate::protocol::RebaseStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let status = fixture.state.rebase_status(&fixture.id).unwrap();
        if status.finished {
            return status;
        }
        assert!(std::time::Instant::now() < deadline, "rebase timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn retarget_keeps_the_branch_name_moves_only_interaction_commits_and_can_restore_copies() {
    let fixture = Fixture::new();
    let initial = git(&fixture.host, &["rev-parse", "HEAD"]);
    git(&fixture.host, &["branch", "other", &initial]);
    // Changes belonging to the old source must not follow the interaction.
    std::fs::write(fixture.host.join("old-source"), "old source\n").unwrap();
    SystemGit.commit_all(&fixture.host, "old source").unwrap();
    fixture.state.start_rebase(&fixture.id).unwrap();
    assert!(wait_for_rebase(&fixture).succeeded);
    fixture.feature();
    let old_head = git(fixture.path(), &["rev-parse", "HEAD"]);
    git(&fixture.host, &["switch", "other"]);
    std::fs::write(fixture.host.join("new-source"), "new source\n").unwrap();
    SystemGit.commit_all(&fixture.host, "new source").unwrap();
    let target = git(&fixture.host, &["rev-parse", "HEAD"]);
    fixture
        .state
        .handle(Request::RetargetSession {
            id: fixture.id.clone(),
            target: "other".into(),
        })
        .unwrap();
    let status = wait_for_rebase(&fixture);
    assert!(status.succeeded, "{}", status.message);
    let retargeted = journal::read_session_checkout(&fixture.session)
        .unwrap()
        .unwrap();
    assert_eq!(retargeted.branch, fixture.checkout.branch);
    assert_eq!(
        git(fixture.path(), &["branch", "--show-current"]),
        fixture.checkout.branch
    );
    assert_eq!(retargeted.path, fixture.checkout.path);
    assert_eq!(
        retargeted.branched_from.as_ref().unwrap(),
        &crate::protocol::BranchPoint {
            branch: Some("other".into()),
            commit: target,
        }
    );
    assert_eq!(
        git(fixture.path(), &["rev-list", "--count", "other..HEAD"]),
        "2"
    );
    assert!(!fixture.path().join("old-source").exists());
    assert!(fixture.path().join("new-source").exists());
    let retargeted_head = git(fixture.path(), &["rev-parse", "HEAD"]);
    let Response::SessionBranchHistory(history) = fixture
        .state
        .handle(Request::SessionBranchHistory {
            id: fixture.id.clone(),
        })
        .unwrap()
    else {
        panic!("expected history")
    };
    assert_eq!(history.len(), 1);
    assert_ne!(history[0].branch, fixture.checkout.branch);
    // The copy keeps the commits the branch had before retargeting.
    assert_eq!(
        git(&fixture.host, &["rev-parse", &history[0].branch]),
        old_head
    );
    assert_eq!(
        history[0].branched_from.as_ref().unwrap().branch.as_deref(),
        Some("main")
    );
    // A new server reads the same history and can restore it.
    let restarted =
        ServerState::with_git(SystemGit::shared(), fixture.state.inner.store_root.clone());
    restarted
        .handle(Request::RestoreSessionBranch {
            id: fixture.id.clone(),
            branch: history[0].branch.clone(),
        })
        .unwrap();
    assert_eq!(git(fixture.path(), &["rev-parse", "HEAD"]), old_head);
    assert_eq!(
        git(fixture.path(), &["branch", "--show-current"]),
        fixture.checkout.branch
    );
    assert!(fixture.path().join("old-source").exists());
    assert!(!fixture.path().join("new-source").exists());
    assert_eq!(
        journal::read_session_checkout(&fixture.session)
            .unwrap()
            .unwrap()
            .branched_from
            .unwrap()
            .branch
            .as_deref(),
        Some("main")
    );
    // Restoring saved the retargeted commits as a new copy in place of the restored one.
    let history = journal::read_session_branch_history(&fixture.session).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        git(&fixture.host, &["rev-parse", &history[0].branch]),
        retargeted_head
    );
    restarted
        .handle(Request::RestoreSessionBranch {
            id: fixture.id.clone(),
            branch: history[0].branch.clone(),
        })
        .unwrap();
    assert_eq!(git(fixture.path(), &["rev-parse", "HEAD"]), retargeted_head);
    assert_eq!(
        journal::read_session_checkout(&fixture.session).unwrap(),
        Some(retargeted)
    );
}

#[test]
fn retarget_history_survives_worktree_cleanup() {
    let fixture = Fixture::new();
    fixture.feature();
    git(&fixture.host, &["branch", "other"]);
    fixture
        .state
        .start_checkout_change(&fixture.id, Some("other"), None)
        .unwrap();
    assert!(wait_for_rebase(&fixture).succeeded);
    let retargeted_head = git(fixture.path(), &["rev-parse", "HEAD"]);
    let saved = journal::read_session_branch_history(&fixture.session).unwrap()[0]
        .branch
        .clone();
    SystemGit
        .remove_worktree(&fixture.host, fixture.path())
        .unwrap();
    journal::clear_session_worktree(&fixture.session).unwrap();
    fixture
        .state
        .start_checkout_change(&fixture.id, None, Some(&saved))
        .unwrap();
    let restored = journal::read_session_checkout(&fixture.session)
        .unwrap()
        .unwrap();
    assert_eq!(restored.branch, fixture.checkout.branch);
    assert_eq!(
        git(
            restored.path.as_deref().unwrap(),
            &["branch", "--show-current"]
        ),
        fixture.checkout.branch
    );
    assert_eq!(
        journal::read_session_branch_history(&fixture.session)
            .unwrap()
            .len(),
        1
    );
    let history = journal::read_session_branch_history(&fixture.session).unwrap();
    assert_ne!(history[0].branch, saved);
    assert_eq!(
        git(&fixture.host, &["rev-parse", &history[0].branch]),
        retargeted_head
    );
}

#[test]
fn retarget_rejects_invalid_targets_and_dirty_worktrees_without_history_changes() {
    let fixture = Fixture::new();
    git(&fixture.host, &["branch", "other"]);
    for target in [
        "missing",
        "HEAD",
        "main",
        "main~1",
        "main^{commit}",
        "--invalid",
        &fixture.checkout.branch,
    ] {
        assert!(fixture
            .state
            .start_checkout_change(&fixture.id, Some(target), None)
            .is_err());
    }
    std::fs::write(fixture.path().join("dirty"), "work").unwrap();
    assert!(fixture
        .state
        .start_checkout_change(&fixture.id, Some("other"), None)
        .is_err());
    assert!(fixture
        .state
        .start_checkout_change(&fixture.id, None, Some("other"))
        .is_err());
    assert!(journal::read_session_branch_history(&fixture.session)
        .unwrap()
        .is_empty());
    assert_eq!(
        journal::read_session_checkout(&fixture.session).unwrap(),
        Some(fixture.checkout.clone())
    );
    assert_eq!(
        git(fixture.path(), &["branch", "--show-current"]),
        fixture.checkout.branch
    );
    assert!(!fixture.session.join("retarget.json").exists());
}

#[test]
fn retarget_conflicts_use_the_rebase_resolver_and_retain_a_copy_of_the_branch() {
    let fixture = Fixture::new();
    std::fs::write(fixture.path().join("file"), "feature\n").unwrap();
    SystemGit.commit_all(fixture.path(), "feature").unwrap();
    let original_head = git(fixture.path(), &["rev-parse", "HEAD"]);
    git(&fixture.host, &["switch", "-c", "other"]);
    std::fs::write(fixture.host.join("file"), "target\n").unwrap();
    SystemGit.commit_all(&fixture.host, "target").unwrap();
    let target = git(&fixture.host, &["rev-parse", "HEAD"]);
    // Prepare the fork without launching a real provider; supply a resolver below.
    let mut checkout = fixture.checkout.clone();
    checkout.branched_from.as_mut().unwrap().branch = Some("other".into());
    let saved = crate::protocol::SavedBranch {
        branch: "styra/test-saved".into(),
        branched_from: fixture.checkout.branched_from.clone(),
    };
    SystemGit
        .create_branch(fixture.path(), &saved.branch, "HEAD")
        .unwrap();
    journal::store_retargeted_checkout(&fixture.session, &fixture.checkout, saved, &checkout, None)
        .unwrap();
    std::fs::write(
        fixture.session.join("retarget.json"),
        serde_json::to_vec(&serde_json::json!({
            "branch": checkout.branch,
            "upstream": fixture.checkout.branched_from.as_ref().unwrap().commit,
        }))
        .unwrap(),
    )
    .unwrap();
    let mut status = pending_rebase(target.clone());
    // A failed resolver leaves a retryable rebase and the old Git ref intact.
    assert!(fixture
        .state
        .run_rebase_with_resolver(
            &fixture.id,
            checkout.clone(),
            &mut status,
            &fixture.session.join("rebase.json"),
            |_, _, _| anyhow::bail!("resolver interrupted")
        )
        .is_err());
    assert!(SystemGit.rebase_in_progress(fixture.path()).unwrap());
    assert!(fixture
        .state
        .start_checkout_change(&fixture.id, Some("main"), None)
        .is_err());
    let mut resolved = false;
    fixture
        .state
        .run_rebase_with_resolver(
            &fixture.id,
            checkout,
            &mut status,
            &fixture.session.join("rebase.json"),
            |_, path, _| {
                resolved = true;
                std::fs::write(path.join("file"), "target and feature\n")?;
                git(path, &["add", "file"]);
                continue_rebase(path)?;
                Ok(())
            },
        )
        .unwrap();
    assert!(resolved);
    assert_eq!(
        git(&fixture.host, &["rev-parse", "styra/test-saved"]),
        original_head
    );
    assert_eq!(
        journal::read_session_checkout(&fixture.session)
            .unwrap()
            .unwrap()
            .branched_from
            .unwrap()
            .commit,
        target
    );
    assert!(!fixture.session.join("retarget.json").exists());
}

#[test]
fn standalone_rebase_updates_the_base_and_keeps_the_interaction() {
    let fixture = Fixture::new();
    fixture.feature();
    std::fs::write(fixture.host.join("upstream"), "upstream\n").unwrap();
    SystemGit.commit_all(&fixture.host, "upstream").unwrap();
    let source = git(&fixture.host, &["rev-parse", "main"]);
    fixture.state.start_rebase(&fixture.id).unwrap();
    let status = wait_for_rebase(&fixture);
    assert!(status.succeeded, "{}", status.message);
    assert_eq!(status.source_commit, source);
    let stored = journal::read_session_checkout(&fixture.session)
        .unwrap()
        .unwrap();
    assert_eq!(stored.path, fixture.checkout.path);
    assert_eq!(stored.branch, fixture.checkout.branch);
    assert_eq!(stored.branched_from.unwrap().commit, source);
    assert_eq!(git(&fixture.host, &["rev-parse", "main"]), source);
    assert_eq!(
        git(fixture.path(), &["rev-list", "--count", "main..HEAD"]),
        "2"
    );
    assert!(fixture.path().join("upstream").exists());
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Active
    );
    assert!(!fixture.session.join(journal::INTEGRATION_FILE).exists());
}

#[test]
fn standalone_rebase_rejects_dirty_or_sealed_interactions() {
    let fixture = Fixture::new();
    std::fs::write(fixture.path().join("file"), "dirty\n").unwrap();
    assert!(fixture.state.start_rebase(&fixture.id).is_err());
    assert_eq!(
        journal::read_session_checkout(&fixture.session).unwrap(),
        Some(fixture.checkout.clone())
    );
    assert!(!fixture.session.join("rebase.json").exists());
    git(fixture.path(), &["checkout", "--", "file"]);
    fixture
        .state
        .set_session_completion(&fixture.id, CompletionState::Sealed)
        .unwrap();
    assert!(fixture.state.start_rebase(&fixture.id).is_err());
}

#[test]
fn an_interaction_with_an_automatic_action_refuses_requests_that_act_on_it() {
    let fixture = Fixture::new();
    let locked = "an automatic rebase is being applied to this interaction";
    let id = fixture.id.clone();
    let acting = || {
        [
            Request::SendMessage {
                id: id.clone(),
                message: SendMessage {
                    text: "continue".into(),
                    selection: None,
                    contract: None,
                },
            },
            Request::InterruptInteraction { id: id.clone() },
            Request::StopInteraction { id: id.clone() },
            Request::ClearQueuedMessages { id: id.clone() },
            Request::SetInteractionAutoRetry {
                id: id.clone(),
                enabled: true,
            },
            Request::CloseInteraction { id: id.clone() },
        ]
    };
    fixture
        .state
        .inner
        .automatic
        .begin(&id, AutomaticAction::Rebase)
        .unwrap();
    for request in acting() {
        let error = fixture.state.handle(request).unwrap_err();
        assert!(format!("{error:#}").contains(locked), "{error:#}");
    }
    // Following the rebase reads the interaction, which stays open.
    fixture
        .state
        .handle(Request::SessionBranchHistory { id: id.clone() })
        .unwrap();
    fixture.state.inner.automatic.finish(&id);
    for request in acting() {
        if let Err(error) = fixture.state.handle(request) {
            assert!(!format!("{error:#}").contains(locked), "{error:#}");
        }
    }
}

#[test]
fn standalone_rebase_restores_a_removed_checkout() {
    let fixture = Fixture::new();
    SystemGit
        .remove_worktree(&fixture.host, fixture.path())
        .unwrap();
    journal::clear_session_worktree(&fixture.session).unwrap();
    fixture.state.start_rebase(&fixture.id).unwrap();
    let status = wait_for_rebase(&fixture);
    assert!(status.succeeded, "{}", status.message);
    assert!(fixture.path().exists());
    assert_eq!(
        journal::read_session_checkout(&fixture.session)
            .unwrap()
            .unwrap()
            .path,
        fixture.checkout.path
    );
}

#[test]
fn standalone_rebase_records_the_pinned_source_even_if_the_branch_advances() {
    let fixture = Fixture::new();
    fixture.feature();
    let source = git(&fixture.host, &["rev-parse", "main"]);
    std::fs::write(fixture.host.join("later"), "later\n").unwrap();
    SystemGit.commit_all(&fixture.host, "later").unwrap();
    let mut status = pending_rebase(source.clone());
    fixture
        .state
        .run_rebase(
            &fixture.id,
            fixture.checkout.clone(),
            &mut status,
            &fixture.session.join("rebase.json"),
        )
        .unwrap();
    assert_eq!(
        journal::read_session_checkout(&fixture.session)
            .unwrap()
            .unwrap()
            .branched_from
            .unwrap()
            .commit,
        source
    );
    assert!(!fixture.path().join("later").exists());
}

#[test]
fn standalone_rebase_resolves_conflicts_before_recording_the_base() {
    let fixture = Fixture::new();
    fixture.conflict();
    let source = git(&fixture.host, &["rev-parse", "main"]);
    let mut status = pending_rebase(source.clone());
    let record = fixture.session.join("rebase.json");
    let mut called = false;
    fixture
        .state
        .run_rebase_with_resolver(
            &fixture.id,
            fixture.checkout.clone(),
            &mut status,
            &record,
            |id, path, status| {
                called = true;
                assert_eq!(id, fixture.id);
                assert_eq!(status.source_commit, source);
                assert!(SystemGit.rebase_in_progress(path)?);
                assert_eq!(
                    journal::read_session_checkout(&fixture.session)?,
                    Some(fixture.checkout.clone())
                );
                let progress: crate::protocol::RebaseStatus =
                    serde_json::from_slice(&std::fs::read(&record)?)?;
                assert!(progress.message.contains("Conflicts found"));
                status.output = "Resolved file; continuing the rebase".into();
                std::fs::write(path.join("file"), "source and feature\n")?;
                git(path, &["add", "file"]);
                continue_rebase(path)?;
                Ok(())
            },
        )
        .unwrap();
    assert!(called);
    let progress: crate::protocol::RebaseStatus =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(progress.output, "Resolved file; continuing the rebase");
    assert_eq!(status.output, progress.output);
    assert_eq!(
        journal::read_session_checkout(&fixture.session)
            .unwrap()
            .unwrap()
            .branched_from
            .unwrap()
            .commit,
        source
    );
    assert_eq!(
        std::fs::read_to_string(fixture.path().join("file")).unwrap(),
        "source and feature\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.host.join("file")).unwrap(),
        "source\n"
    );
}

#[test]
fn standalone_rebase_keeps_the_old_base_when_conflict_resolution_fails() {
    let fixture = Fixture::new();
    fixture.conflict();
    let mut status = pending_rebase(git(&fixture.host, &["rev-parse", "main"]));
    assert!(fixture
        .state
        .run_rebase_with_resolver(
            &fixture.id,
            fixture.checkout.clone(),
            &mut status,
            &fixture.session.join("rebase.json"),
            |_, _, status| {
                status.output = "I could not resolve this conflict".into();
                anyhow::bail!("resolver stopped")
            }
        )
        .is_err());
    assert_eq!(status.output, "I could not resolve this conflict");
    assert_eq!(
        journal::read_session_checkout(&fixture.session).unwrap(),
        Some(fixture.checkout.clone())
    );
    assert!(SystemGit.rebase_in_progress(fixture.path()).unwrap());
}

#[test]
fn standalone_rebase_refuses_an_unrelated_conflicted_rebase() {
    let fixture = Fixture::new();
    fixture.conflict();
    assert!(!SystemGit.rebase(fixture.path(), "main").unwrap());
    assert!(fixture.state.start_rebase(&fixture.id).is_err());
    assert_eq!(
        journal::read_session_checkout(&fixture.session).unwrap(),
        Some(fixture.checkout.clone())
    );
    assert!(SystemGit.rebase_in_progress(fixture.path()).unwrap());
}

#[test]
fn integration_rebases_squashes_cleans_up_and_seals() {
    let fixture = Fixture::new();
    fixture.feature();
    std::fs::write(fixture.host.join("upstream"), "upstream\n").unwrap();
    SystemGit.commit_all(&fixture.host, "upstream").unwrap();
    fixture
        .state
        .run_integration(&fixture.id, fixture.progress())
        .unwrap();
    assert_eq!(git(&fixture.host, &["rev-list", "--count", "main"]), "3");
    assert_eq!(
        git(&fixture.host, &["log", "-1", "--format=%B"]),
        fixture.progress().message
    );
    assert_eq!(
        std::fs::read_to_string(fixture.host.join("feature")).unwrap(),
        "two\n"
    );
    assert!(fixture.host.join("upstream").exists());
    assert!(!fixture.path().exists());
    let integration = journal::read_session_integration(&fixture.session).unwrap();
    assert_eq!(integration.branch.as_deref(), Some("main"));
    assert_eq!(
        Some(integration.commit),
        SystemGit.commit(&fixture.host, "main").unwrap()
    );
    assert!(journal::read_session_checkout(&fixture.session)
        .unwrap()
        .unwrap()
        .path
        .is_none());
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Sealed
    );
    assert!(
        fixture
            .state
            .integration_result(&fixture.id)
            .unwrap()
            .unwrap()
            .completed
    );
    assert!(fixture
        .state
        .set_session_completion(&fixture.id, CompletionState::Active)
        .is_err());
    assert!(SystemGit
        .commit(&fixture.host, &fixture.checkout.branch)
        .unwrap()
        .is_some());
}

#[test]
fn a_source_not_checked_out_gets_a_temporary_checkout() {
    let fixture = Fixture::new();
    fixture.feature();
    git(&fixture.host, &["checkout", "-b", "other"]);
    fixture
        .state
        .run_integration(&fixture.id, fixture.progress())
        .unwrap();
    assert_eq!(git(&fixture.host, &["branch", "--show-current"]), "other");
    assert_eq!(git(&fixture.host, &["show", "main:feature"]), "two");
    assert!(!fixture.session.join("integration-source").exists());
}

#[test]
fn rebase_conflicts_are_retained_and_can_be_resolved_then_integrated() {
    let fixture = Fixture::new();
    fixture.conflict();
    assert!(!SystemGit.rebase(fixture.path(), "main").unwrap());
    assert!(SystemGit.rebase_in_progress(fixture.path()).unwrap());
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Active
    );
    std::fs::write(fixture.path().join("file"), "source and feature\n").unwrap();
    git(fixture.path(), &["add", "file"]);
    continue_rebase(fixture.path()).unwrap();
    fixture
        .state
        .run_integration(&fixture.id, fixture.progress())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.host.join("file")).unwrap(),
        "source and feature\n"
    );
    assert!(!fixture.path().exists());
}

#[test]
fn dirty_checkouts_are_rejected_before_starting_integration() {
    let fixture = Fixture::new();
    fixture.feature();
    std::fs::write(fixture.host.join("untracked"), "keep me").unwrap();
    assert!(fixture
        .state
        .start_integration(&fixture.id, "message")
        .is_err());
    assert!(fixture.path().exists());
    assert_eq!(git(&fixture.host, &["rev-list", "--count", "main"]), "1");
    std::fs::remove_file(fixture.host.join("untracked")).unwrap();
    std::fs::write(fixture.path().join("untracked"), "keep me too").unwrap();
    assert!(fixture
        .state
        .start_integration(&fixture.id, "message")
        .is_err());
    assert!(fixture.path().join("untracked").exists());
}

#[test]
fn cleanup_retry_after_worktree_removal_seals() {
    let fixture = Fixture::new();
    fixture.feature();
    let mut progress = fixture.progress();
    progress.commit = Some(
        SystemGit
            .squash_merge(&fixture.host, &fixture.checkout.branch, "approved message")
            .unwrap(),
    );
    SystemGit
        .remove_worktree(&fixture.host, fixture.path())
        .unwrap();
    journal::clear_session_worktree(&fixture.session).unwrap();
    fixture
        .state
        .run_integration(&fixture.id, progress)
        .unwrap();
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Sealed
    );
    assert_eq!(git(&fixture.host, &["rev-list", "--count", "main"]), "2");
}

#[test]
fn cleanup_retry_does_not_create_another_squash_commit() {
    let fixture = Fixture::new();
    fixture.feature();
    assert!(SystemGit.rebase(fixture.path(), "main").unwrap());
    let commit = SystemGit
        .squash_merge(&fixture.host, &fixture.checkout.branch, "approved message")
        .unwrap();
    let mut progress = fixture.progress();
    progress.commit = Some(commit);
    std::fs::write(fixture.path().join("untracked"), "keep me").unwrap();
    assert!(fixture
        .state
        .run_integration(&fixture.id, progress)
        .is_err());
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Active
    );
    std::fs::remove_file(fixture.path().join("untracked")).unwrap();
    let mut progress = fixture.progress();
    progress.commit = SystemGit.commit(&fixture.host, "HEAD").unwrap();
    fixture
        .state
        .run_integration(&fixture.id, progress)
        .unwrap();
    assert_eq!(git(&fixture.host, &["rev-list", "--count", "main"]), "2");
    assert_eq!(
        git(&fixture.host, &["log", "-1", "--format=%s"]),
        "approved message"
    );
    assert_eq!(
        journal::read_session_completed(&fixture.session).unwrap(),
        CompletionState::Sealed
    );
}
