use anyhow::{Context, Result};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::ffi::{OsStr, OsString};
use std::io::{Stdout, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use styra_server::Client;

pub fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("entering the alternate screen")?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout)).context("initialising terminal")?;
    Ok(terminal)
}

pub fn restore(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().ok();
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    terminal.backend_mut().flush().ok();
    Ok(())
}

/// Open a file in the configured editor in a new terminal window.
pub fn open_editor(terminal: &str, editor: &str, path: &Path) -> Result<()> {
    let mut command = editor_terminal_command(OsStr::new(terminal), editor, path);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting terminal {terminal}"))?;
    Ok(())
}

/// Open a live session's persistent sandbox shell in a new terminal window.
///
/// `$TERM` describes capabilities rather than the emulator executable, so the
/// de-facto `$TERMINAL` override and `TERM_PROGRAM` identity take precedence.
pub fn open_shell(client: &Client, session: &str) -> Result<String> {
    let shell = client.shell(session)?;
    launch_candidates(
        std::env::var_os("TERMINAL"),
        std::env::var_os("TERM_PROGRAM"),
        std::env::var_os("TERM"),
    )
    .into_iter()
    .find_map(|program| {
        let mut command = terminal_command(&program, &shell.tmux, &shell.socket);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(_) => Some(Ok(program.to_string_lossy().into_owned())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(Err(error.into())),
        }
    })
    .unwrap_or_else(|| {
        Err(anyhow::anyhow!(
            "no terminal emulator found; set $TERMINAL to one such as foot, kitty, or alacritty"
        ))
    })
}

fn launch_candidates(
    terminal: Option<OsString>,
    term_program: Option<OsString>,
    term: Option<OsString>,
) -> Vec<OsString> {
    let mut candidates = Vec::new();
    if let Some(value) = terminal.filter(|value| !value.is_empty()) {
        candidates.push(value);
    }
    if let Some(value) = term_program.and_then(term_program_executable) {
        candidates.push(value);
    }
    if let Some(value) = term.and_then(term_executable) {
        candidates.push(value);
    }
    for fallback in [
        "x-terminal-emulator",
        "foot",
        "kitty",
        "wezterm",
        "alacritty",
        "gnome-terminal",
        "konsole",
        "xterm",
    ] {
        let fallback = OsString::from(fallback);
        if !candidates.contains(&fallback) {
            candidates.push(fallback);
        }
    }
    candidates
}

fn term_program_executable(value: OsString) -> Option<OsString> {
    let normalized = value.to_string_lossy().to_ascii_lowercase();
    Some(
        match normalized.as_str() {
            "wezterm" => "wezterm",
            "kitty" => "kitty",
            "alacritty" => "alacritty",
            "gnome-terminal" => "gnome-terminal",
            "konsole" => "konsole",
            "apple_terminal" => "open",
            _ => return None,
        }
        .into(),
    )
}

fn term_executable(value: OsString) -> Option<OsString> {
    let normalized = value.to_string_lossy().to_ascii_lowercase();
    ["foot", "kitty", "wezterm", "alacritty", "xterm"]
        .into_iter()
        .find(|name| normalized == *name || normalized.starts_with(&format!("{name}-")))
        .map(Into::into)
}

fn terminal_command(program: &OsStr, tmux: &Path, socket: &Path) -> Command {
    let mut command = Command::new(program);
    let name = Path::new(program)
        .file_name()
        .unwrap_or(program)
        .to_string_lossy()
        .to_ascii_lowercase();
    if name == "open" {
        command.args(["-a", "Terminal", "--args"]);
    } else if name == "wezterm" {
        command.args(["start", "--"]);
    } else if matches!(name.as_str(), "gnome-terminal" | "kitty" | "foot") {
        command.arg("--");
    } else {
        command.arg("-e");
    }
    command
        .arg(tmux)
        .arg("-S")
        .arg(socket)
        .args(["attach-session", "-t", "shell"]);
    command
}

fn editor_terminal_command(program: &OsStr, editor: &str, path: &Path) -> Command {
    let mut command = Command::new(program);
    let name = Path::new(program)
        .file_name()
        .unwrap_or(program)
        .to_string_lossy()
        .to_ascii_lowercase();
    if name == "open" {
        command.args(["-a", "Terminal", "--args"]);
    } else if name == "wezterm" {
        command.args(["start", "--"]);
    } else if matches!(name.as_str(), "gnome-terminal" | "kitty" | "foot") {
        command.arg("--");
    } else {
        command.arg("-e");
    }
    command.arg(editor).arg(path);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_override_precedes_detected_and_fallback_emulators() {
        let candidates = launch_candidates(
            Some("my-terminal".into()),
            Some("WezTerm".into()),
            Some("xterm-256color".into()),
        );
        assert_eq!(
            &candidates[..3],
            &[
                OsString::from("my-terminal"),
                OsString::from("wezterm"),
                OsString::from("xterm")
            ]
        );
    }

    #[test]
    fn wezterm_uses_its_start_subcommand() {
        let command = terminal_command(
            OsStr::new("wezterm"),
            Path::new("/usr/bin/tmux"),
            Path::new("/tmp/tmux.sock"),
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "start",
                "--",
                "/usr/bin/tmux",
                "-S",
                "/tmp/tmux.sock",
                "attach-session",
                "-t",
                "shell"
            ]
        );
    }

    #[test]
    fn editor_is_launched_in_a_new_terminal() {
        let command = editor_terminal_command(
            OsStr::new("urxvt"),
            "nvim",
            Path::new("/workspace/src/main.rs"),
        );
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                OsStr::new("-e"),
                OsStr::new("nvim"),
                OsStr::new("/workspace/src/main.rs"),
            ]
        );
    }

    #[test]
    fn kitty_separates_its_options_from_tmux_without_xterms_e_flag() {
        let command = terminal_command(
            OsStr::new("/usr/bin/kitty"),
            Path::new("/usr/bin/tmux"),
            Path::new("/tmp/tmux.sock"),
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args.first().map(String::as_str), Some("--"));
        assert_eq!(args.get(1).map(String::as_str), Some("/usr/bin/tmux"));
    }
}
