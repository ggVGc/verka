use super::*;

#[test]
fn one_action_at_a_time_and_refusals_name_it() {
    let actions = AutomaticActions::default();
    actions.ensure_none("a").unwrap();
    actions.begin("a", AutomaticAction::Rebase).unwrap();
    let error = actions
        .begin("a", AutomaticAction::Integration)
        .unwrap_err()
        .to_string();
    assert!(error.contains("automatic rebase"), "{error}");
    assert!(actions.ensure_none("a").is_err());
    actions.ensure_none("b").unwrap();
    assert_eq!(actions.applied("a"), Some(AutomaticAction::Rebase));
    assert_eq!(actions.interactions(), vec!["a".to_owned()]);
    actions.finish("a");
    actions.ensure_none("a").unwrap();
    actions.begin("a", AutomaticAction::Integration).unwrap();
}
