//! The one-line footer with the workspace and standing fleet state.

#[cfg(test)]
mod tests {
    use super::super::test_support;
    use super::super::test_support::rendered;

    use std::path::PathBuf;
    use styra_protocol::{CompletionState, DrivaOptions, InteractionActivity, InteractionSummary};

    fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            auto_retry: false,
            id: id.into(),
            name: None,
            tags: Vec::new(),
            workspace_id: "workspace".into(),
            selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            driva: DrivaOptions {
                isolation_backend: "none".into(),
                command: vec![],
                working_directory: PathBuf::from("/workspace"),
                network: false,
                base: vec![],
                mounts: vec![],
                ..Default::default()
            },
            activity,
            activity_reason: None,
            activity_since_ms: 0,
            idle_unseen: false,
            uncommitted_changes: false,
            checkout: None,
            last_message: None,
            events: 0,
            completed: CompletionState::Active,
        }
    }

    #[test]
    fn footer_shows_the_working_directory() {
        let mut app = test_support::app("s1");
        app.workspace.enter("/tmp/styra/workspace".into());
        let screen = rendered(&app);
        assert!(screen.contains("/tmp/styra/workspace"));
        assert!(!screen.contains("j/k next/prev"));
    }

    /// An armed rate-limit retry has to be visible from the interaction the
    /// operator is watching: it was set once, in a view they have since left,
    /// and it changes what happens hours later.
    #[test]
    fn footer_reports_an_armed_rate_limit_retry() {
        let mut app = test_support::app("s1");
        assert!(
            !rendered(&app).contains("rate-limit retry"),
            "off is the default and takes no footer space"
        );

        app.auto_retry = true;

        assert!(rendered(&app).contains("R rate-limit retry: on"));
    }

    #[test]
    fn footer_reports_interactions_that_became_idle_while_unseen() {
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

        assert!(rendered(&app).contains("^a 1 interaction idle"));
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
        assert!(!rendered(&app).contains("uncommitted changes"));

        let mut current = interaction("current", InteractionActivity::Pending);
        current.uncommitted_changes = true;
        app.interactions.refresh(vec![
            current,
            interaction("other", InteractionActivity::Pending),
        ]);

        let screen = test_support::screen(&app);
        // 20 rows tall: the footer is the last, the panel's bottom border the
        // one above it.
        let bottom_border = screen.row(18);
        assert!(
            bottom_border.contains("uncommitted changes"),
            "{bottom_border}"
        );
    }

    /// The fleet tally rides the panel's bottom border rather than the footer
    /// row below it, so the footer can give its whole left edge to the path.
    #[test]
    fn panel_border_opens_with_the_fleet_tally() {
        let mut app = test_support::app("current");
        app.interactions.refresh(vec![
            interaction("current", InteractionActivity::Running),
            interaction("other", InteractionActivity::Pending),
            interaction("done", InteractionActivity::Stopped),
        ]);

        let bottom_border = test_support::screen(&app).row(18);
        assert!(bottom_border.starts_with("└ 1/1/1 "), "{bottom_border}");
    }

    #[test]
    fn working_directory_opens_the_bottom_row() {
        let mut app = test_support::app("s1");
        app.workspace.enter("/workspace".into());
        let bottom_row = test_support::screen_sized(&app, 40, 10).row(9);
        assert!(bottom_row.starts_with("/workspace"), "{bottom_row}");
    }
}
