use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use styra_protocol::ProjectAction;
use styra_server::Client;

use crate::config::Configuration;

/// Start a configured command and let it live on its own.
///
/// Every stream is closed: the command shares this process's terminal, which
/// Styra is drawing in, so anything it writes there would land in the middle
/// of the interface.
pub fn spawn_detached(command: &mut Command) -> Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", describe(command)))?;
    Ok(())
}

/// A command as the operator would have typed it, for the message that says
/// what was run. Lossy on purpose: this is prose, not something to re-execute.
pub fn describe(command: &Command) -> String {
    let mut described = command.get_program().to_string_lossy().into_owned();
    for arg in command.get_args() {
        described.push(' ');
        described.push_str(&arg.to_string_lossy());
    }
    described
}

/// Open a live session's persistent sandbox shell in a terminal window of its
/// own, and report the emulator it was opened in.
///
/// The shell itself is a `tmux` session the server owns, so what is run is
/// fixed; the window it runs in is the operator's to configure
/// ([`Configuration::open_terminal`]).
pub fn open_shell(client: &Client, session: &str, config: &dyn Configuration) -> Result<String> {
    let shell = client.shell(session)?;
    let argv = [
        shell.tmux.into_os_string(),
        OsString::from("-S"),
        shell.socket.into_os_string(),
        OsString::from("attach-session"),
        OsString::from("-t"),
        OsString::from("shell"),
    ];
    let mut command = config.open_terminal(&argv);
    let program = command.get_program().to_string_lossy().into_owned();
    spawn_detached(&mut command)?;
    Ok(program)
}

/// Resolve a project command from the interaction, Workspace, and templates.
pub fn project_command(
    directory: &Path,
    policy: &styra_protocol::LaunchPolicy,
    action: ProjectAction,
) -> Result<String> {
    if let Some(command) = policy
        .command(action)
        .filter(|command| !command.trim().is_empty())
    {
        return Ok(command.to_owned());
    }
    let path = directory.join("driva.toml");
    let config = if path.exists() {
        driva::Config::load(&path)?
    } else {
        driva::Config::default()
    };
    let mut merged = driva::TemplateConfig::default();
    for name in &policy.templates {
        merged.overlay(
            config
                .template(name)
                .with_context(|| format!("unknown driva template {name:?}"))?,
        );
    }
    let command = match action {
        ProjectAction::Run => merged.run,
        ProjectAction::Test => merged.test,
    };
    command
        .filter(|command| !command.trim().is_empty())
        .with_context(|| {
            format!(
                "no {} command is set; set one in the details view",
                action.name()
            )
        })
}

/// Open the configured command in a terminal at the selected project directory.
pub fn open_project_action(
    directory: &Path,
    command: &str,
    config: &dyn Configuration,
) -> Result<String> {
    let mut terminal = project_action_in(directory, command, config);
    let program = terminal.get_program().to_string_lossy().into_owned();
    spawn_detached(&mut terminal)?;
    Ok(program)
}

fn project_action_in(directory: &Path, command: &str, config: &dyn Configuration) -> Command {
    let argv = [
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(PROJECT_ACTION_SCRIPT),
        OsString::from("styra-action"),
        OsString::from(command),
    ];
    let mut terminal = config.open_terminal(&argv);
    terminal.current_dir(directory);
    terminal
}

const PROJECT_ACTION_SCRIPT: &str = r#"/bin/sh -c "$1"
status=$?
printf '\n[exited with status %s — press Enter to close]' "$status"
read -r _"#;

/// Open an ordinary host shell in `directory`, and report the emulator it was
/// opened in.
///
/// The sibling of [`open_shell`], and deliberately not the same thing: that one
/// attaches to the agent's sandbox, while this is the operator's own shell on
/// the host, standing where the interaction is working. Neither the emulator
/// nor the shell is guessed at — both are configured
/// ([`Configuration::open_terminal`] and [`Configuration::shell`]).
pub fn open_directory(directory: &Path, config: &dyn Configuration) -> Result<String> {
    let mut command = shell_in(directory, config);
    let program = command.get_program().to_string_lossy().into_owned();
    spawn_detached(&mut command)?;
    Ok(program)
}

/// The command [`open_directory`] runs, built apart from running it so what it
/// asks for can be examined.
fn shell_in(directory: &Path, config: &dyn Configuration) -> Command {
    let mut command = config.open_terminal(&config.shell());
    // The emulator is spawned in the directory, and the shell it runs inherits
    // that, so the operator lands where the agent is rather than wherever Styra
    // was started.
    command.current_dir(directory);
    command
}

#[cfg(test)]
mod tests {
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
}
