use super::*;
use crate::config::Defaults;

#[test]
fn project_actions_resolve_without_a_live_interaction() {
    use styra_protocol::{LaunchPolicy, ProjectAction};
    let directory = tempfile::tempdir().unwrap();
    let policy = LaunchPolicy {
        templates: vec!["rust".into()],
        ..Default::default()
    };
    assert_eq!(
        project_command(directory.path(), &policy, ProjectAction::Run).unwrap(),
        "cargo run"
    );
    assert_eq!(
        project_command(directory.path(), &policy, ProjectAction::Test).unwrap(),
        "cargo test"
    );
    let workspace = LaunchPolicy {
        run: Some("cargo run --release".into()),
        ..policy
    };
    let interaction = LaunchPolicy {
        run: Some("printf '%s' '$literal'".into()),
        ..Default::default()
    };
    let effective = LaunchPolicy::merge(&workspace, &interaction);
    let text = project_command(directory.path(), &effective, ProjectAction::Run).unwrap();
    let terminal = project_action_in(directory.path(), &text, &Defaults);
    assert_eq!(terminal.get_current_dir(), Some(directory.path()));
    assert_eq!(terminal.get_args().last().unwrap(), text.as_str());
    let output = Command::new("/bin/sh")
        .args(["-c", PROJECT_ACTION_SCRIPT, "styra-action", &text])
        .current_dir(directory.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("$literal"));
}

#[test]
fn a_shell_opens_standing_in_the_directory_it_was_asked_for() {
    let command = shell_in(Path::new("/home/me/project"), &Defaults);

    assert_eq!(
        command.get_current_dir(),
        Some(Path::new("/home/me/project")),
        "the window starts where the interaction is working"
    );
    assert_eq!(
        command.get_args().last(),
        Defaults.shell().last().map(OsString::as_os_str),
        "the configured shell, not this process's environment"
    );
}

#[test]
fn a_command_is_described_as_it_would_have_been_typed() {
    let mut command = Command::new("urxvt");
    command.args(["-e", "nvim", "/workspace/src/main.rs"]);

    assert_eq!(describe(&command), "urxvt -e nvim /workspace/src/main.rs");
}
