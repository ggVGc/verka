use super::*;

fn effects_for_test() -> (LaunchEffects, Sender<LaunchEffectResult>) {
    let (send, _jobs) = mpsc::channel();
    let (results, receive) = mpsc::channel();
    (
        LaunchEffects {
            send: Some(send),
            receive,
            worker: None,
            next_template_request: AtomicU64::new(1),
        },
        results,
    )
}

#[test]
fn workspace_acknowledgement_updates_the_snapshot_and_clears_pending() {
    let (effects, results) = effects_for_test();
    let mut app =
        App::pending(styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap());
    app.workspace_launch_pending = 1;
    let policy = LaunchPolicy {
        templates: vec!["rust".into()],
        ..LaunchPolicy::default()
    };
    results
        .send(LaunchEffectResult::WorkspaceChanged {
            workspace_id: "workspace".into(),
            clear_interaction: false,
            result: Ok(policy.clone()),
        })
        .unwrap();

    effects.apply_ready(&mut app, "workspace");

    assert_eq!(app.workspace_launch_pending, 0);
    assert_eq!(app.launch.workspace, policy);
    assert!(app
        .notices
        .iter()
        .any(|notice| notice.text == "Workspace launch policy saved"));
}

#[test]
fn workspace_edit_failure_is_visible_without_opening_the_log() {
    let (effects, results) = effects_for_test();
    let mut app =
        App::pending(styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap());
    app.workspace_launch_pending = 1;
    results
        .send(LaunchEffectResult::WorkspaceChanged {
            workspace_id: "workspace".into(),
            clear_interaction: false,
            result: Err("disk full".into()),
        })
        .unwrap();

    effects.apply_ready(&mut app, "workspace");

    assert_eq!(app.workspace_launch_pending, 0);
    assert!(app
        .notices
        .iter()
        .any(|notice| notice.text.contains("disk full")));
}
