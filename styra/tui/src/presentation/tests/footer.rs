//! The one-line footer with the workspace and standing fleet state.

#[cfg(test)]
mod tests {
    use super::super::test_support;
    use super::super::test_support::inspect;
    use styra_ui::application::MainView;

    use std::path::PathBuf;
    use styra_protocol::{CompletionState, InteractionActivity, InteractionSummary};

    fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            auto_retry: false,
            auto_commit: false,
            id: id.into(),
            name: None,
            tags: Vec::new(),
            workspace_id: "workspace".into(),
            selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            activity,
            activity_reason: None,
            activity_since_ms: 0,
            idle_unseen: false,
            interacted_at_ms: 0,
            last_user_message_at_ms: 0,
            uncommitted_changes: false,
            expected_branch: None,
            checkout: None,
            last_message: None,
            recent_messages: Vec::new(),
            events: 0,
            branched_from: None,
            completed: CompletionState::Active,
            focus_requested: false,
            origin: None,
            integration: None,
            restored: false,
        }
    }

    #[test]
    fn footer_shows_the_working_directory() {
        let mut app = test_support::app("s1");
        app.workspace.enter("/tmp/styra/workspace".into());
        inspect(&app, |v| {
            assert_eq!(v.footer.working_directory, "/tmp/styra/workspace")
        });
    }

    /// An armed rate-limit retry has to be visible from the interaction the
    /// operator is watching: it was set once, in a view they have since left,
    /// and it changes what happens hours later.
    #[test]
    fn footer_reports_an_armed_rate_limit_retry() {
        let mut app = test_support::app("s1");
        inspect(&app, |v| assert!(!v.footer.auto_retry));
        app.auto_retry = true;
        inspect(&app, |v| assert!(v.footer.auto_retry));
    }

    /// An edit report takes a while and is asked for from the diff, which the
    /// operator may well leave; the footer keeps saying it is being written.
    #[test]
    fn footer_reports_edits_being_described() {
        let mut app = test_support::app("s1");
        for (count, expected) in [
            (0, None),
            (1, Some("describing edits")),
            (2, Some("describing edits of 2 interactions")),
        ] {
            app.describing_edits = count;
            inspect(&app, |v| assert_eq!(v.footer.background, expected));
        }
    }

    /// Committing turns is on by default in a worktree, so the footer has to
    /// say so for the operator to know that branch's history is being written
    /// to.
    #[test]
    fn footer_reports_that_turns_are_committed() {
        let mut app = test_support::app("s1");
        inspect(&app, |v| assert!(!v.footer.auto_commit));
        app.auto_commit = true;
        inspect(&app, |v| assert!(v.footer.auto_commit));
        app.session_id.clear();
        inspect(&app, |v| assert!(!v.footer.auto_commit));
    }

    /// Beside the fleet's tally on the panel's bottom border, not in the
    /// footer: the count reads as part of that tally.
    #[test]
    fn panel_border_reports_interactions_that_became_idle_while_unseen() {
        let mut app = test_support::app("current");
        app.interactions.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("other", InteractionActivity::Running),
            ],
            vec![],
        );
        app.interactions.close();
        let mut other = interaction("other", InteractionActivity::Pending);
        other.idle_unseen = true;
        app.interactions.refresh(vec![
            interaction("current", InteractionActivity::Running),
            other,
        ]);

        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!(
                (
                    e.list.activity.running,
                    e.list.activity.idle,
                    e.list.activity.stopped,
                    e.list.activity.newly_idle
                ),
                (1, 1, 0, 1)
            );
        });
    }

    /// The attached interaction's own checkout, not any other idle one's: the
    /// marker describes the pane it rides the border of.
    #[test]
    fn panel_border_reports_uncommitted_work_in_the_attached_interaction() {
        let mut app = test_support::app("current");
        let mut elsewhere = interaction("other", InteractionActivity::Pending);
        elsewhere.uncommitted_changes = true;
        app.interactions.refresh(vec![
            interaction("current", InteractionActivity::Pending),
            elsewhere,
        ]);
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(!e.list.uncommitted_changes);
        });

        let mut current = interaction("current", InteractionActivity::Pending);
        current.uncommitted_changes = true;
        app.interactions.refresh(vec![
            current,
            interaction("other", InteractionActivity::Pending),
        ]);

        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(e.list.uncommitted_changes);
        });
    }

    #[test]
    fn footer_data_uses_the_entered_workspace() {
        let mut app = test_support::app("s1");
        app.workspace.enter("/workspace".into());
        inspect(&app, |v| {
            assert_eq!(v.footer.working_directory, "/workspace")
        });
    }
}
