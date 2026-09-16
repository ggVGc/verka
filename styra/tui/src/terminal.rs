use anyhow::{Context, Result};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::ffi::OsString;
use std::io::{Stdout, Write};
use std::process::{Command, Stdio};
use styra_server::Client;

use crate::config::Configuration;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_described_as_it_would_have_been_typed() {
        let mut command = Command::new("urxvt");
        command.args(["-e", "nvim", "/workspace/src/main.rs"]);

        assert_eq!(describe(&command), "urxvt -e nvim /workspace/src/main.rs");
    }
}
