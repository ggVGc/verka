//! Styra's terminal entry point: CLI, terminal lifecycle, and the event loop
//! that wires the session threads to the application state and renderer.

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use styra::agent::{MountSpec, Profile, SandboxLayout};
use styra::app::{App, Focus};
use styra::journal::Journal;
use styra::session::{Session, SessionSpec, SessionUpdate};
use styra::ui;

/// Run an interactive, isolated agent session in a terminal interface.
#[derive(Parser)]
#[command(name = "styra", about, version)]
struct Cli {
    /// Agent profile to launch.
    #[arg(long, default_value = "codex")]
    profile: String,
    /// Host directory mounted writable as the agent workspace (default: cwd).
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Permit agent networking (profiles may default this on).
    #[arg(long)]
    network: bool,
    /// Open a captured journal read-only instead of launching an agent.
    #[arg(long, value_name = "SESSION")]
    attach: Option<PathBuf>,
    /// Optional first message, sent to seed the opening turn.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let layout = SandboxLayout::default();
    let profile = Profile::builtin(&cli.profile, &layout)?;

    // Build the application and, unless attaching, a live session up front so a
    // setup failure is reported plainly before the terminal is taken over.
    let mut app;
    let mut session: Option<Session> = None;
    let mut updates: Option<Receiver<SessionUpdate>> = None;

    if let Some(attach) = &cli.attach {
        let events = styra::journal::replay(attach, profile.protocol)
            .with_context(|| format!("attaching to journal {}", attach.display()))?;
        app = App::new(profile.name.clone(), attach.display().to_string());
        for event in events {
            app.push_event(event);
        }
        // A replayed session has no live agent to end; mark it stopped.
        app.on_ended(styra::session::SessionEnd { exit_code: None, error: None });
    } else {
        let workspace = resolve_workspace(cli.workspace.as_deref())?;
        let store_root = std::env::current_dir()?.join(".styra");
        let (journal, session_id) = Journal::create_in_store(&store_root)?;
        let diagnostics = journal
            .path()
            .parent()
            .unwrap_or(&store_root)
            .join("diagnostics.log");

        let mut profile = profile;
        profile.network = profile.network || cli.network;

        let spec = SessionSpec {
            profile: profile.clone(),
            working_directory: layout.workspace.clone(),
            workspace: MountSpec {
                source: workspace,
                destination: layout.workspace.clone(),
                writable: true,
            },
            temporary_mounts: vec![PathBuf::from("/root")],
        };
        let backend = Box::new(driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: Some(PathBuf::from("/")),
        });

        let (spawned, receiver) =
            Session::spawn(spec, backend, journal, session_id.clone(), diagnostics)?;
        app = App::new(profile.name.clone(), session_id);

        let prompt = cli.prompt.join(" ");
        if !prompt.trim().is_empty() {
            spawned.send(prompt.trim())?;
        }
        session = Some(spawned);
        updates = Some(receiver);
    }

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app, session.as_ref(), updates.as_ref());
    restore_terminal(&mut terminal)?;
    // Dropping the session here (after the terminal is restored) closes stdin
    // and joins the worker threads.
    drop(session);
    result
}

/// The event loop: apply pending session updates, render, and handle input
/// until the operator quits.
fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    session: Option<&Session>,
    updates: Option<&Receiver<SessionUpdate>>,
) -> Result<()> {
    let mut pending_fold = false;
    loop {
        if let Some(updates) = updates {
            while let Ok(update) = updates.try_recv() {
                match update {
                    SessionUpdate::Event(event) => app.push_event(event),
                    SessionUpdate::Ended(end) => app.on_ended(end),
                }
            }
        }

        terminal.draw(|frame| ui::render(frame, app))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match app.focus {
            Focus::List => handle_list_key(app, session, key, &mut pending_fold),
            Focus::Input => handle_input_key(app, session, key),
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_list_key(app: &mut App, session: Option<&Session>, key: KeyEvent, pending_fold: &mut bool) {
    // Vim-style fold chord: `z` then `R` (expand all) or `M` (collapse all).
    if std::mem::take(pending_fold) {
        match key.code {
            KeyCode::Char('R') => app.expand_all(),
            KeyCode::Char('M') => app.collapse_all(),
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Char('q') => app.request_quit(),
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
        KeyCode::Char(' ') | KeyCode::Enter => app.toggle_expand(),
        KeyCode::Char('o') => app.expand_selected(),
        KeyCode::Char('c') => app.collapse_selected(),
        KeyCode::Char('g') => app.select_first(),
        KeyCode::Char('G') => app.select_last(),
        KeyCode::Char('z') => *pending_fold = true,
        KeyCode::Char('i') => app.enter_input(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Char('s') => {
            if let Some(session) = session {
                session.stop();
            }
        }
        _ => {}
    }
}

fn handle_input_key(app: &mut App, session: Option<&Session>, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.enter_list(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => app.input_newline(),
        KeyCode::Enter => {
            if let Some(message) = app.take_message() {
                if let Some(session) = session {
                    if app.can_send() {
                        // The sent message returns as a UserMessage event, so
                        // it is not pushed here.
                        let _ = session.send(&message);
                    }
                }
            }
        }
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Char(ch) => app.input_char(ch),
        _ => {}
    }
}

fn resolve_workspace(workspace: Option<&std::path::Path>) -> Result<PathBuf> {
    let raw = match workspace {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("determining the current directory")?,
    };
    raw.canonicalize()
        .with_context(|| format!("workspace directory {} must exist", raw.display()))
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("entering the alternate screen")?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout)).context("initialising terminal")?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().ok();
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    terminal.backend_mut().flush().ok();
    Ok(())
}
