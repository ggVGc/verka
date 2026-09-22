//! The one-line footer with the keyboard shortcut reference and workspace.

#[cfg(test)]
mod tests {
    use super::super::test_support;
    use super::super::test_support::rendered;

    use std::path::PathBuf;
    use styra_protocol::{DrivaOptions, InteractionActivity, InteractionSummary};

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
            last_message: None,
            events: 0,
            completed: false,
        }
    }

    #[test]
    fn footer_shows_keybinds_and_working_directory() {
        let mut app = test_support::app("s1");
        app.workspace.enter("/tmp/styra/workspace".into());
        let screen = rendered(&app);
        assert!(screen.contains("? keybinds"));
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

    #[test]
    fn working_directory_is_aligned_to_the_bottom_right() {
        let mut app = test_support::app("s1");
        app.workspace.enter("/workspace".into());
        let bottom_row = test_support::screen_sized(&app, 40, 10).row(9);
        assert!(bottom_row.ends_with("/workspace"));
    }
}
