//! Execution/review tests extracted from linka's ops module when the
//! projects were separated. PARKED reference material — not compiling yet.

    fn candidate(store: &Store) -> (FakeVcs, String, String) {
        let fake = FakeVcs {
            next_id: "candidate-1".into(),
            root: Some("base".into()),
            current_branch: Some("linka/candidates/attempt-1".into()),
            ..Default::default()
        };
        fake.commits.borrow_mut().insert("base".into());
        fake.refs
            .borrow_mut()
            .insert("refs/heads/main".into(), "base".into());
        let id = add(store, &fake, new_node("implement it", vec![])).unwrap();
        let branch = "linka/candidates/attempt-1".to_string();
        attempts(store)
            .write(&Attempt {
                schema: 1,
                id: "attempt-1".into(),
                work_item: id.clone(),
                worker: Author::Machine,
                force: false,
                definition: store.node_version(&id).unwrap(),
                input: git_artifact("base"),
                input_tree: "tree-base".into(),
                branch: branch.clone(),
                workspace: "/tmp/attempt-1".into(),
                backend: Some("test".into()),
                model: None,
                created_at: 0,
                prepared: true,
            })
            .unwrap();
        complete_with_execution(
            store,
            &fake,
            &id,
            &["file.txt".into()],
            &[],
            None,
            "candidate",
            Author::Machine,
            Some(ExecutionIdentity {
                node_id: id.clone(),
                attempt_id: "attempt-1".into(),
                candidate_branch: branch.clone(),
                force: false,
            }),
        )
        .unwrap();
        let (result, _) = store.read_result(&id).unwrap().unwrap();
        assert_eq!(attempt_of(&result).as_deref(), Some("attempt-1"));
        assert!(result.output.is_some());
        attempts(store)
            .finish(
                "attempt-1",
                &AttemptFinished {
                    at: 0,
                    executor_succeeded: true,
                },
            )
            .unwrap();
        fake.refs
            .borrow_mut()
            .insert(format!("refs/heads/{branch}"), "candidate-1".into());
        (fake, id, "candidate-1".into())
    }
    #[test]
    fn library_requires_execution_identity_for_machine_project_output() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            next_id: "candidate".into(),
            ..Default::default()
        };
        let id = add(&store, &fake, new_node("implement", vec![])).unwrap();
        let error = complete(
            &store,
            &fake,
            &id,
            &["file.txt".into()],
            &[],
            None,
            "",
            Author::Machine,
        )
        .unwrap_err();
        assert!(error.to_string().contains("durable execution"));
        assert!(store.read_result(&id).unwrap().is_none());
        assert!(fake.captured.borrow().is_empty());
    }
    #[test]
    fn execution_is_bound_to_machine_node_attempt_and_checked_out_branch() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            current_branch: Some("linka/candidates/run-1".into()),
            ..Default::default()
        };
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![])).unwrap();
        let execution = ExecutionIdentity {
            node_id: a.clone(),
            attempt_id: "run-1".into(),
            candidate_branch: "linka/candidates/run-1".into(),
            force: false,
        };
        assert!(fail_with_execution(
            &store,
            &fake,
            &b,
            "wrong node",
            Author::Machine,
            Some(execution.clone()),
        )
        .is_err());
        assert!(store.read_result(&b).unwrap().is_none());
        assert!(fail_with_execution(
            &store,
            &fake,
            &a,
            "wrong author",
            Author::Human,
            Some(execution.clone()),
        )
        .is_err());

        let wrong_branch = ExecutionIdentity {
            candidate_branch: "linka/candidates/not-run-1".into(),
            ..execution
        };
        assert!(fail_with_execution(
            &store,
            &fake,
            &a,
            "wrong branch",
            Author::Machine,
            Some(wrong_branch),
        )
        .is_err());
    }
    #[test]
    fn library_authorizes_execution_readiness_assignment_and_force() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let human = add(
            &store,
            &fake,
            NewNode {
                assignee: Some(Author::Human),
                ..new_node("human question", vec![])
            },
        )
        .unwrap();
        assert!(authorize_execution_start(&store, &fake, &human, Author::Machine, false).is_err());
        assert!(authorize_execution_start(&store, &fake, &human, Author::Machine, true).is_ok());

        let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
        let blocked = add(&store, &fake, new_node("blocked", vec![dependency])).unwrap();
        assert!(
            authorize_execution_start(&store, &fake, &blocked, Author::Machine, false).is_err()
        );
        assert!(authorize_execution_start(&store, &fake, &blocked, Author::Machine, true).is_ok());
    }
    #[test]
    fn library_prepares_attempt_identity_branch_base_and_worktree() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("work", vec![])).unwrap();
        let workspace = prepare_execution(
            &store,
            &fake,
            &id,
            Author::Machine,
            false,
            Some("base-commit"),
            true,
        )
        .unwrap();
        assert_eq!(workspace.identity.node_id, id);
        assert_eq!(workspace.input_commit, "base-commit");
        assert_eq!(workspace.input_tree, "tree-base-commit");
        assert_eq!(
            workspace.identity.candidate_branch,
            format!("linka/candidates/{}", workspace.identity.attempt_id)
        );
        assert!(workspace.path.is_dir());
        assert!(store
            .root()
            .join("execution")
            .join(&workspace.identity.attempt_id)
            .join("attempt.toml")
            .is_file());
        assert_eq!(
            fake.refs.borrow().get(&format!(
                "refs/heads/{}",
                workspace.identity.candidate_branch
            )),
            Some(&"base-commit".to_string())
        );
    }
    #[test]
    fn failed_workspace_creation_leaves_a_durable_unprepared_attempt() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            worktree_error: true,
            ..Default::default()
        };
        let id = add(&store, &fake, new_node("work", vec![])).unwrap();
        assert!(prepare_execution(
            &store,
            &fake,
            &id,
            Author::Machine,
            false,
            Some("base"),
            true,
        )
        .is_err());
        let attempt_ids = attempts(&store).list_ids().unwrap();
        assert_eq!(attempt_ids.len(), 1);
        assert!(!attempts(&store).read(&attempt_ids[0]).unwrap().prepared);
    }
    #[test]
    fn successful_backend_cannot_reuse_an_older_node_result() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("work", vec![])).unwrap();
        complete(&store, &fake, &id, &[], &[], None, "old", Author::Human).unwrap();
        let workspace = prepare_execution(
            &store,
            &fake,
            &id,
            Author::Machine,
            true,
            Some("base"),
            true,
        )
        .unwrap();
        assert!(finalize_execution_attempt(
            &store,
            &fake,
            &workspace.identity.attempt_id,
            WorkedBy {
                backend: "test".into(),
                model: None
            },
            0,
            &[],
            true,
        )
        .is_err());
    }
    #[test]
    fn output_result_gets_review_even_when_backend_exits_unsuccessfully() {
        let (_t, store) = temp_store();
        let (fake, implementation, _) = candidate(&store);
        let review = finalize_execution_attempt(
            &store,
            &fake,
            "attempt-1",
            WorkedBy {
                backend: "test".into(),
                model: None,
            },
            0,
            &[],
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            reviews(&store).candidate(&review).unwrap().subject,
            implementation
        );
        assert!(
            !attempts(&store)
                .read_final("attempt-1")
                .unwrap()
                .unwrap()
                .executor_succeeded
        );
    }
    #[test]
    fn rejected_review_preserves_candidate_and_reopens_implementation() {
        let (_t, store) = temp_store();
        let (fake, implementation, commit) = candidate(&store);
        let review = create_review(&store, &fake, &implementation).unwrap();
        let (meta, _) = store.read_node(&review).unwrap();
        assert_eq!(meta.assignee, Some(Author::Human));
        let pinned = reviews(&store).candidate(&review).unwrap();
        assert_eq!(pinned.artifact.id, commit);
        assert!(
            !is_dispatchable(&store, &fake, &implementation),
            "pending review gates rework"
        );

        decide_review(
            &store,
            &fake,
            &review,
            ReviewDecision::Rejected,
            "please revise",
            "main",
            None,
            None,
        )
        .unwrap();
        assert!(is_dispatchable(&store, &fake, &implementation));
        let decision = reviews(&store).decision(&review).unwrap().unwrap();
        assert_eq!(decision.kind, ReviewDecision::Rejected);
        assert_eq!(decision.notes, "please revise");
        let (_, notes) = store.read_result(&review).unwrap().unwrap();
        assert_eq!(notes, "please revise");
        assert_eq!(
            reviews(&store).candidate(&review).unwrap().artifact.id,
            commit
        );
    }
    #[test]
    fn library_prepares_review_edit_workspace_only_for_open_exact_review() {
        let (_t, store) = temp_store();
        let (fake, implementation, commit) = candidate(&store);
        let review = create_review(&store, &fake, &implementation).unwrap();
        let workspace = prepare_review_edits(&store, &fake, &review).unwrap();
        assert_eq!(workspace.branch, format!("linka/reviews/{review}"));
        assert_eq!(workspace.candidate_commit, commit);
        assert!(workspace.path.ends_with(format!("review-{review}")));
        assert_eq!(
            fake.refs
                .borrow()
                .get(&format!("refs/heads/{}", workspace.branch)),
            Some(&commit)
        );
        fake.refs.borrow_mut().insert(
            format!("refs/heads/{}", workspace.branch),
            "suggestion-commit".into(),
        );

        decide_review(
            &store,
            &fake,
            &review,
            ReviewDecision::Rejected,
            "revise",
            "main",
            None,
            None,
        )
        .unwrap();
        let decision = reviews(&store).decision(&review).unwrap().unwrap();
        assert_eq!(
            decision.suggestion.map(|artifact| artifact.id).as_deref(),
            Some("suggestion-commit")
        );
        assert!(store
            .root()
            .join("reviews")
            .join(&review)
            .join("decision.toml")
            .is_file());
        assert!(prepare_review_edits(&store, &fake, &review).is_err());
    }
    #[test]
    fn library_finalization_creates_mandatory_review_after_enrichment() {
        let (_t, store) = temp_store();
        let (fake, implementation, _) = candidate(&store);
        let review = finalize_execution_attempt(
            &store,
            &fake,
            "attempt-1",
            WorkedBy {
                backend: "test".into(),
                model: Some("model".into()),
            },
            0,
            &[],
            true,
        )
        .unwrap()
        .unwrap();
        let pinned = reviews(&store).candidate(&review).unwrap();
        assert_eq!(pinned.subject, implementation);
        assert!(store
            .root()
            .join("reviews")
            .join(&review)
            .join("candidate.toml")
            .is_file());
        assert_eq!(
            pinned.result,
            attempts(&store).result_version("attempt-1").unwrap()
        );
        // Enrichment stamped the producing backend before the review pinned it.
        let (result, _) = store.read_result(&implementation).unwrap().unwrap();
        let evidence = WorkEvidence::from_producer(result.producer.as_ref().unwrap()).unwrap();
        assert_eq!(evidence.backend.as_deref(), Some("test"));
        assert_eq!(evidence.model.as_deref(), Some("model"));
    }
    #[test]
    fn accepted_review_integrates_exact_candidate() {
        let (_t, store) = temp_store();
        let (fake, implementation, commit) = candidate(&store);
        let review = create_review(&store, &fake, &implementation).unwrap();
        decide_review(
            &store,
            &fake,
            &review,
            ReviewDecision::Accepted,
            "looks good",
            "main",
            None,
            None,
        )
        .unwrap();
        let publication = reviews(&store).read_publication(&review).unwrap().unwrap();
        assert!(publication.completed_at.is_some());
        assert_eq!(publication.candidate_commit, commit);
        assert_eq!(fake.refs.borrow().get("refs/heads/main"), Some(&commit));
        assert_eq!(node_state(&store, &implementation), NodeState::Integrated);
        assert_eq!(node_state(&store, &review), NodeState::Integrated);
    }
    #[test]
    fn publication_recovers_after_target_moved_before_store_finalization() {
        let (_t, store) = temp_store();
        let (fake, implementation, commit) = candidate(&store);
        let review = create_review(&store, &fake, &implementation).unwrap();
        reviews(&store)
            .write_publication(&PublicationIntent {
                schema: 1,
                review: review.clone(),
                implementation: implementation.clone(),
                candidate_commit: commit.clone(),
                target: "main".into(),
                target_ref: "refs/heads/main".into(),
                target_previous: "base".into(),
                notes: "approved".into(),
                prepared_at: 1,
                completed_at: None,
            })
            .unwrap();
        fake.refs
            .borrow_mut()
            .insert("refs/heads/main".into(), commit.clone());

        recover_publication(&store, &fake, &review).unwrap();
        let publication = reviews(&store).read_publication(&review).unwrap().unwrap();
        assert!(publication.completed_at.is_some());
        let decision = reviews(&store).decision(&review).unwrap().unwrap();
        assert_eq!(decision.kind, ReviewDecision::Accepted);
        assert_eq!(node_state(&store, &implementation), NodeState::Integrated);
    }
    #[test]
    fn unfinished_attempt_blocks_another_start_even_with_force() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("work", vec![])).unwrap();
        let first = prepare_execution(
            &store,
            &fake,
            &id,
            Author::Machine,
            false,
            Some("base"),
            true,
        )
        .unwrap();
        let error = prepare_execution(
            &store,
            &fake,
            &id,
            Author::Machine,
            true,
            Some("base"),
            true,
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains(&first.identity.attempt_id));
    }
    #[test]
    fn closed_clean_review_workspace_can_be_inspected_and_removed() {
        let (_t, store) = temp_store();
        let (fake, implementation, _) = candidate(&store);
        let review = create_review(&store, &fake, &implementation).unwrap();
        let workspace = prepare_review_edits(&store, &fake, &review).unwrap();
        assert_eq!(
            prepare_review_edits(&store, &fake, &review).unwrap().path,
            workspace.path
        );
        decide_review(
            &store,
            &fake,
            &review,
            ReviewDecision::Rejected,
            "revise",
            "main",
            None,
            None,
        )
        .unwrap();
        let status = review_workspace_status(&store, &fake, &review).unwrap();
        assert!(status.exists);
        assert_eq!(status.clean, Some(true));
        assert!(cleanup_review_workspace(&store, &fake, &review).unwrap());
        assert!(
            !review_workspace_status(&store, &fake, &review)
                .unwrap()
                .exists
        );
    }
    #[test]
    fn real_repository_rejection_rework_and_acceptance_lifecycle() {
        static E2E_COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = E2E_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("linka-review-e2e-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let _temp = TempDir(root.clone());
        let initialized = init_workbench(root.join(".linka"), Some("e2e".into())).unwrap();
        let store = initialized.store;
        let canonical = crate::GitVcs::for_store(&store);
        let implementation = add(
            &store,
            &canonical,
            new_node("implement reviewed content", vec![]),
        )
        .unwrap();

        let first = prepare_execution(
            &store,
            &canonical,
            &implementation,
            Author::Machine,
            false,
            None,
            true,
        )
        .unwrap();
        std::fs::write(first.path.join("feature.txt"), "first\n").unwrap();
        let first_vcs = crate::GitVcs::for_execution(&store, first.path.clone());
        complete_with_execution(
            &store,
            &first_vcs,
            &implementation,
            &["feature.txt".into()],
            &[],
            None,
            "first candidate",
            Author::Machine,
            Some(first.identity.clone()),
        )
        .unwrap();
        let first_review = finalize_execution_attempt(
            &store,
            &first_vcs,
            &first.identity.attempt_id,
            WorkedBy {
                backend: "test".into(),
                model: Some("test-model".into()),
            },
            0,
            &[],
            true,
        )
        .unwrap()
        .unwrap();
        decide_review(
            &store,
            &canonical,
            &first_review,
            ReviewDecision::Rejected,
            "make it second",
            "main",
            None,
            None,
        )
        .unwrap();

        let second = prepare_execution(
            &store,
            &canonical,
            &implementation,
            Author::Machine,
            false,
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(second.path.join("feature.txt")).unwrap(),
            "first\n"
        );
        std::fs::write(second.path.join("feature.txt"), "second\n").unwrap();
        let second_vcs = crate::GitVcs::for_execution(&store, second.path.clone());
        complete_with_execution(
            &store,
            &second_vcs,
            &implementation,
            &["feature.txt".into()],
            &[],
            None,
            "second candidate",
            Author::Machine,
            Some(second.identity.clone()),
        )
        .unwrap();
        let second_review = finalize_execution_attempt(
            &store,
            &second_vcs,
            &second.identity.attempt_id,
            WorkedBy {
                backend: "test".into(),
                model: Some("test-model".into()),
            },
            0,
            &[],
            true,
        )
        .unwrap()
        .unwrap();
        decide_review(
            &store,
            &canonical,
            &second_review,
            ReviewDecision::Accepted,
            "approved",
            "main",
            None,
            None,
        )
        .unwrap();

        assert_ne!(first_review, second_review);
        assert!(attempts(&store)
            .read_result(&first.identity.attempt_id)
            .unwrap()
            .is_some());
        assert!(attempts(&store)
            .read_result(&second.identity.attempt_id)
            .unwrap()
            .is_some());
        assert_eq!(node_state(&store, &implementation), NodeState::Integrated);
        assert_eq!(
            std::fs::read_to_string(store.project_root().join("feature.txt")).unwrap(),
            "second\n"
        );
    }
    #[test]
    fn review_refuses_a_candidate_branch_that_moved() {
        let (_t, store) = temp_store();
        let (fake, implementation, _) = candidate(&store);
        let review = create_review(&store, &fake, &implementation).unwrap();
        fake.refs.borrow_mut().insert(
            "refs/heads/linka/candidates/attempt-1".into(),
            "different".into(),
        );
        assert!(decide_review(
            &store,
            &fake,
            &review,
            ReviewDecision::Accepted,
            "",
            "main",
            None,
            None,
        )
        .is_err());
        assert!(store.read_result(&review).unwrap().is_none());
    }
    #[test]
    fn amend_worker_stamps_only_this_sessions_result() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        let engine = || WorkedBy {
            backend: "claude-code".into(),
            model: Some("opus".into()),
        };

        // No result yet (a paused unit of work): nothing to stamp.
        assert!(!amend_worker(&store, &fake, &id, engine(), 0).unwrap());

        done(&store, &fake, &id);
        let at = store.read_result(&id).unwrap().unwrap().0.at;

        // A result older than the session is someone else's — left untouched.
        assert!(!amend_worker(&store, &fake, &id, engine(), at + 1).unwrap());
        assert!(store
            .read_result(&id)
            .unwrap()
            .unwrap()
            .0
            .producer
            .is_none());

        // This session's result gets the stamp, keeping the narrative.
        assert!(amend_worker(&store, &fake, &id, engine(), at).unwrap());
        let (result, notes) = store.read_result(&id).unwrap().unwrap();
        assert_eq!(notes, "done");
        let evidence = WorkEvidence::from_producer(result.producer.as_ref().unwrap()).unwrap();
        assert_eq!(evidence.backend.as_deref(), Some("claude-code"));
        assert_eq!(evidence.model.as_deref(), Some("opus"));

        // The stamp does not reopen the node or make it stale.
        assert_eq!(current_status(&store, &id), Status::Done);
        assert!(staleness(&store, &fake, &id).is_empty());
    }
