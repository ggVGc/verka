use anyhow::{Context, Result};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{Stdout, Write};

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
