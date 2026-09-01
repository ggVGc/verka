use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use linka_tui::{app::App, ui};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version, about = "A terminal interface for the Linka graph")]
struct Args {
    /// Path to the Linka store.
    #[arg(long, env = "LINKA_DIR", default_value = ".linka")]
    store: PathBuf,
    /// Initialize a workbench before opening it.
    #[arg(long)]
    init: bool,
    /// Descriptive project name used with --init.
    #[arg(long, requires = "init")]
    name: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.init {
        linka::ops::init_workbench(args.store.clone(), args.name)?;
    }

    let mut app = App::open(args.store)?;
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
        terminal.draw(|frame| ui::draw(frame, app))?;
        if app.should_quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == event::KeyEventKind::Press => app.on_key(key),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}
