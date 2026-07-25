mod app;
mod ui;

use anyhow::{Context, Result};
use app::App;
use clap::Parser;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version, about = "A terminal control panel for an Orka workbench")]
struct Args {
    /// Workbench root (the directory containing .linka/ and project/).
    /// Defaults to the nearest ancestor containing .linka/.
    #[arg(long, env = "ORKA_WORKBENCH")]
    workbench: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut app = App::open(args.workbench)?;

    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let result = run(&mut terminal, &mut app);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        app.poll_worker();
        terminal.draw(|frame| ui::draw(frame, app))?;
        if app.should_quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == event::KeyEventKind::Press => app.on_key(key),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}
