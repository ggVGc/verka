//! `llaundry-tui` — an interactive terminal UI over the llaundry library.
//!
//! It exposes the same operations as the `llaundry` CLI (add, link, edit,
//! complete, fail, and the browse/derive queries) but in a live, navigable view of
//! the graph. It is deliberately event-driven: the main loop *blocks* on a key
//! event, mutates state, and redraws once — there is no polling render loop.
//!
//! The screen is a two-pane dashboard: a filterable node list on the left and the
//! selected node's detail (the `show` view) on the right. Actions are triggered by
//! single keys and collect their arguments through modal prompts, each of which runs
//! its own small blocking read loop (see the `prompt` helpers).

use std::io::{self, Stdout, Write};
use std::panic;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

use llaundry::ops::{self, NewNode};
use llaundry::{title_of, Author, DepKind, GitVcs, NodeMeta, ResultMeta, Status, Store};

#[derive(Parser)]
#[command(
    name = "llaundry-tui",
    version,
    about = "Interactive terminal UI for the llaundry node graph"
)]
struct Cli {
    /// Path to the store directory.
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry")]
    store: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Open the store before touching the terminal, so the "no store yet" prompt is a
    // plain question rather than a modal we'd have to draw.
    let store = match Store::open(cli.store.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            eprint!("Initialise a new store at {}? [y/N] ", cli.store.display());
            io::stderr().flush().ok();
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim(), "y" | "Y" | "yes") {
                return Ok(());
            }
            Store::init(cli.store.clone())?
        }
    };
    let vcs = GitVcs::for_store(&store);

    let mut term = Terminal::enter()?;
    let result = run(&mut term, &store, &vcs);
    term.leave()?;
    result
}

// ---------------------------------------------------------------------------
// Terminal lifecycle
// ---------------------------------------------------------------------------

/// RAII guard for raw mode + the alternate screen, so the terminal is always
/// restored — including on a panic, via a hook installed in [`Terminal::enter`].
struct Terminal {
    out: Stdout,
    left: bool,
}

impl Terminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, cursor::Hide)?;

        // Restore the terminal even if a later panic unwinds past `leave`.
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
            default_hook(info);
        }));

        Ok(Terminal { out, left: false })
    }

    fn leave(&mut self) -> Result<()> {
        if !self.left {
            self.left = true;
            execute!(self.out, cursor::Show, LeaveAlternateScreen)?;
            terminal::disable_raw_mode()?;
        }
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// Which subset of nodes the list shows. Mirrors the CLI's browse queries.
#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Ready,
    Blocked,
    Stale,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Ready => "ready",
            Filter::Blocked => "blocked",
            Filter::Stale => "stale",
        }
    }
    fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Ready,
            Filter::Ready => Filter::Blocked,
            Filter::Blocked => Filter::Stale,
            Filter::Stale => Filter::All,
        }
    }
}

/// A snapshot of one node, precomputed once per refresh so drawing is cheap.
struct Row {
    id: String,
    version: String,
    meta: NodeMeta,
    description: String,
    result: Option<(ResultMeta, String)>,
    status: Status,
    stale: Vec<String>,
    blockers: Vec<String>,
    ready: bool,
}

struct App {
    rows: Vec<Row>,
    filter: Filter,
    /// Index into the *filtered* view.
    selected: usize,
    /// First filtered index drawn (vertical scroll of the list).
    offset: usize,
    message: String,
}

impl App {
    fn load(store: &Store, vcs: &GitVcs) -> Result<App> {
        let mut app = App {
            rows: Vec::new(),
            filter: Filter::All,
            selected: 0,
            offset: 0,
            message: String::new(),
        };
        app.refresh(store, vcs)?;
        Ok(app)
    }

    /// Recompute the node snapshot from the store, preserving the selected id.
    fn refresh(&mut self, store: &Store, vcs: &GitVcs) -> Result<()> {
        let keep = self.selected_id();
        let mut rows = Vec::new();
        for id in store.list_ids()? {
            let (meta, description) = store.read_node(&id)?;
            rows.push(Row {
                version: store.node_version(&id)?,
                result: store.read_result(&id)?,
                status: ops::current_status(store, &id),
                stale: ops::staleness(store, vcs, &id),
                blockers: ops::blockers(store, vcs, &id),
                ready: ops::is_ready(store, vcs, &id),
                id,
                meta,
                description,
            });
        }
        self.rows = rows;
        // Try to keep the cursor on the same logical node across a refresh.
        if let Some(prev) = keep {
            if let Some(pos) = self.filtered().iter().position(|&i| self.rows[i].id == prev) {
                self.selected = pos;
            }
        }
        self.clamp();
        Ok(())
    }

    /// Row indices matching the active filter, in stable (sorted) order.
    fn filtered(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| match self.filter {
                Filter::All => true,
                Filter::Ready => r.ready,
                Filter::Blocked => !r.blockers.is_empty(),
                Filter::Stale => !r.stale.is_empty(),
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn selected_row(&self) -> Option<&Row> {
        self.filtered().get(self.selected).map(|&i| &self.rows[i])
    }

    fn selected_id(&self) -> Option<String> {
        self.selected_row().map(|r| r.id.clone())
    }

    fn clamp(&mut self) {
        let len = self.filtered().len();
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }
    }

    fn move_by(&mut self, delta: isize) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        let cur = self.selected as isize;
        self.selected = (cur + delta).clamp(0, len as isize - 1) as usize;
    }
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

fn run(term: &mut Terminal, store: &Store, vcs: &GitVcs) -> Result<()> {
    let mut app = App::load(store, vcs)?;
    loop {
        draw(&mut term.out, &mut app)?;

        let key = match read_key()? {
            Some(k) => k,
            None => continue, // resize / non-key event: just redraw
        };

        app.message.clear();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Char('j') | KeyCode::Down => app.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => app.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => app.selected = 0,
            KeyCode::Char('G') | KeyCode::End => app.move_by(isize::MAX),
            KeyCode::Tab => {
                app.filter = app.filter.next();
                app.selected = 0;
                app.clamp();
            }
            KeyCode::Char('r') => {
                app.refresh(store, vcs)?;
                app.message = "refreshed".into();
            }
            KeyCode::Char('?') => help(&mut term.out)?,
            KeyCode::Char('a') => act(&mut app, store, vcs, action_add(&mut term.out, store, vcs)),
            KeyCode::Char('e') => {
                let out = &mut term.out;
                let done = app
                    .selected_row()
                    .map(|r| action_edit(out, store, vcs, r));
                if let Some(done) = done {
                    act(&mut app, store, vcs, done);
                }
            }
            KeyCode::Char('l') => {
                let out = &mut term.out;
                let done = app.selected_id().map(|id| action_link(out, store, vcs, &id));
                if let Some(done) = done {
                    act(&mut app, store, vcs, done);
                }
            }
            KeyCode::Char('c') => {
                let out = &mut term.out;
                let done = app.selected_id().map(|id| action_complete(out, store, vcs, &id));
                if let Some(done) = done {
                    act(&mut app, store, vcs, done);
                }
            }
            KeyCode::Char('f') => {
                let out = &mut term.out;
                let done = app.selected_id().map(|id| action_fail(out, store, vcs, &id));
                if let Some(done) = done {
                    act(&mut app, store, vcs, done);
                }
            }
            KeyCode::Enter => {
                if let Some(r) = app.selected_row() {
                    let lines = result_lines(r);
                    popup(&mut term.out, &format!("result: {}", r.id), &lines)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Apply the outcome of an action: refresh on success, or surface the error.
fn act(app: &mut App, store: &Store, vcs: &GitVcs, outcome: Result<Option<String>>) {
    match outcome {
        Ok(None) => {} // cancelled
        Ok(Some(msg)) => {
            let refreshed = app.refresh(store, vcs);
            app.message = match refreshed {
                Ok(()) => msg,
                Err(e) => format!("{msg}; refresh failed: {e}"),
            };
        }
        Err(e) => app.message = format!("error: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Actions (each returns Ok(None) if the user cancelled a prompt)
// ---------------------------------------------------------------------------

fn action_add(out: &mut Stdout, store: &Store, vcs: &GitVcs) -> Result<Option<String>> {
    let Some(description) =
        prompt_text(out, "description — first line is the title (Ctrl-S save, Esc cancel)", "")?
    else {
        return Ok(None);
    };
    if description.trim().is_empty() {
        return Ok(Some("add cancelled: empty description".into()));
    }
    let id = ops::add(
        store,
        vcs,
        NewNode {
            description,
            author: Author::Human,
            assignee: None,
            depends_on: Vec::new(),
            derived_from: Vec::new(),
        },
    )?;
    Ok(Some(format!("added {id}")))
}

fn action_edit(out: &mut Stdout, store: &Store, vcs: &GitVcs, row: &Row) -> Result<Option<String>> {
    let Some(description) = prompt_text(
        out,
        "description — first line is the title (Ctrl-S save, Esc cancel)",
        &row.description,
    )?
    else {
        return Ok(None);
    };
    if description == row.description {
        return Ok(Some("edit: no changes".into()));
    }
    ops::edit(store, vcs, &row.id, description)?;
    Ok(Some(format!("edited {}", row.id)))
}

fn action_link(out: &mut Stdout, store: &Store, vcs: &GitVcs, from: &str) -> Result<Option<String>> {
    // Offer the other nodes as link targets.
    let targets: Vec<String> = store
        .list_ids()?
        .into_iter()
        .filter(|id| id != from)
        .collect();
    if targets.is_empty() {
        return Ok(Some("link: no other nodes to link to".into()));
    }
    let labels: Vec<&str> = targets.iter().map(String::as_str).collect();
    let Some(ti) = pick(out, &format!("link {from} -> ?"), &labels, 0)? else {
        return Ok(None);
    };
    let rels = [DepKind::DependsOn, DepKind::DerivedFrom];
    let rel_labels: Vec<&str> = rels.iter().map(|r| r.as_str()).collect();
    let Some(ri) = pick(out, "relationship", &rel_labels, 0)? else {
        return Ok(None);
    };
    let to = &targets[ti];
    ops::link(store, vcs, from, to, rels[ri])?;
    Ok(Some(format!("linked {from} -{}-> {to}", rels[ri].as_str())))
}

fn action_complete(
    out: &mut Stdout,
    store: &Store,
    vcs: &GitVcs,
    id: &str,
) -> Result<Option<String>> {
    let Some(outputs) = prompt_line(out, "output files (space-separated, blank = none)", "")? else {
        return Ok(None);
    };
    let outputs: Vec<String> = outputs.split_whitespace().map(str::to_string).collect();
    let Some(context) = prompt_line(out, "context files (space-separated, optional)", "")? else {
        return Ok(None);
    };
    let context: Vec<String> = context.split_whitespace().map(str::to_string).collect();
    let Some(message) = prompt_line(out, "commit message (blank = default)", "")? else {
        return Ok(None);
    };
    let message = (!message.trim().is_empty()).then_some(message);
    let Some(notes) = prompt_text(out, "notes — what happened? (Ctrl-S save, Esc cancel)", "")?
    else {
        return Ok(None);
    };
    let commit = ops::complete(store, vcs, id, &outputs, &context, message, &notes, Author::Human)?;
    Ok(Some(match commit {
        Some(c) => format!("completed {id} (output {})", ops::short(&c)),
        None => format!("completed {id} (no output files)"),
    }))
}

fn action_fail(out: &mut Stdout, store: &Store, vcs: &GitVcs, id: &str) -> Result<Option<String>> {
    let Some(notes) = prompt_text(out, "notes — what went wrong? (Ctrl-S save, Esc cancel)", "")?
    else {
        return Ok(None);
    };
    ops::fail(store, vcs, id, &notes, Author::Human)?;
    Ok(Some(format!("{id} -> failed")))
}

/// The result record of a node, for the popup.
fn result_lines(r: &Row) -> Vec<String> {
    let Some((result, notes)) = &r.result else {
        return vec!["(no result yet — the node has not been worked)".into()];
    };
    let mut lines = vec![
        format!("outcome: {}", result.outcome.as_str()),
        format!("author:  {}", result.author.as_str()),
        format!("version: {}", ops::short(&result.node_version)),
    ];
    if let Some(wb) = &result.worked_by {
        lines.push(match &wb.model {
            Some(m) => format!("worked by: {} ({m})", wb.backend),
            None => format!("worked by: {}", wb.backend),
        });
    }
    if let Some(commit) = &result.output_commit {
        lines.push(format!("output:  commit {}", ops::short(commit)));
    }
    for ba in &result.built_against {
        lines.push(match &ba.output {
            Some(o) => format!(
                "built against {} @ {} (output {})",
                ba.id,
                ops::short(&ba.pin),
                ops::short(o)
            ),
            None => format!("built against {} @ {}", ba.id, ops::short(&ba.pin)),
        });
    }
    for pin in &result.context {
        let tag = if pin.observed { " (observed)" } else { "" };
        lines.push(format!("context {} @ {}{tag}", pin.path, ops::short(&pin.blob)));
    }
    let notes = notes.trim_end();
    if !notes.is_empty() {
        lines.push(String::new());
        lines.extend(notes.lines().map(String::from));
    }
    lines
}

// ---------------------------------------------------------------------------
// Rendering — the dashboard
// ---------------------------------------------------------------------------

const HEADER: u16 = 0;
const BODY_TOP: u16 = 1;

fn draw(out: &mut Stdout, app: &mut App) -> Result<()> {
    let (w, h) = terminal::size()?;
    let footer = h.saturating_sub(1);
    let body_bottom = footer.saturating_sub(1); // last body row
    let left_w = (w / 2).max(20).min(w.saturating_sub(1));
    let sep_x = left_w;
    let right_x = left_w + 1;
    let right_w = w.saturating_sub(right_x);

    queue!(out, cursor::Hide)?;

    // Header.
    let filtered = app.filtered();
    let header = format!(
        " llaundry  [{}]  {} node(s)  ({}/{})",
        app.filter.label(),
        filtered.len(),
        if filtered.is_empty() { 0 } else { app.selected + 1 },
        filtered.len(),
    );
    put(out, 0, HEADER, &header, w as usize, Color::Reset, true)?;

    // Compute list scroll so the selection stays visible.
    let list_h = body_bottom.saturating_sub(BODY_TOP) as usize + 1;
    let mut offset = app.offset.min(app.selected);
    if app.selected >= offset + list_h {
        offset = app.selected + 1 - list_h;
    }
    if app.selected < offset {
        offset = app.selected;
    }
    app.offset = offset;

    // Body rows.
    for y in BODY_TOP..=body_bottom {
        let row_i = (y - BODY_TOP) as usize + offset;
        // Left: the node list.
        if let Some(&idx) = filtered.get(row_i) {
            let r = &app.rows[idx];
            let marker = if !r.stale.is_empty() { '*' } else { ' ' };
            let text = format!("{marker}{:<7} {}", r.status.as_str(), title_of(&r.description));
            let selected = row_i == app.selected;
            let color = status_color(r.status);
            put(out, 0, y, &text, left_w as usize, color, selected)?;
        } else {
            put(out, 0, y, "", left_w as usize, Color::Reset, false)?;
        }
        // Separator.
        put(out, sep_x, y, "│", 1, Color::DarkGrey, false)?;
    }

    // Right: detail of the selected node.
    let detail = app
        .selected_row()
        .map(detail_lines)
        .unwrap_or_else(|| vec![("(no node selected)".into(), Color::DarkGrey)]);
    for (i, y) in (BODY_TOP..=body_bottom).enumerate() {
        let (text, color) = detail
            .get(i)
            .cloned()
            .unwrap_or_else(|| (String::new(), Color::Reset));
        put(out, right_x, y, &text, right_w as usize, color, false)?;
    }

    // Footer: a message if any, else the key hints.
    let hint = "a add  e edit  l link  c complete  f fail  ⏎ result  Tab filter  r refresh  ? help  q quit";
    let footer_text = if app.message.is_empty() {
        format!(" {hint}")
    } else {
        format!(" {}", app.message)
    };
    put(out, 0, footer, &footer_text, w as usize, Color::Reset, true)?;

    out.flush()?;
    Ok(())
}

/// Colour a row/label by node status.
fn status_color(status: Status) -> Color {
    match status {
        Status::Open => Color::Reset,
        Status::Done => Color::Green,
        Status::Failed => Color::Red,
    }
}

/// The right-pane detail view for a node — the `show` command, as coloured lines.
fn detail_lines(r: &Row) -> Vec<(String, Color)> {
    let mut lines: Vec<(String, Color)> = Vec::new();
    let mut plain = |s: String| lines.push((s, Color::Reset));

    plain(format!("id:      {}", r.id));
    lines.push((
        format!("status:  {}", r.status.as_str()),
        status_color(r.status),
    ));
    lines.push((format!("author:  {}", r.meta.author.as_str()), Color::Reset));
    lines.push((format!("version: {}", ops::short(&r.version)), Color::Reset));
    for dep in &r.meta.depends_on {
        lines.push((format!("depends_on:   {dep}"), Color::Reset));
    }
    for src in &r.meta.derived_from {
        lines.push((format!("derived_from: {src}"), Color::Reset));
    }
    if let Some((result, _)) = &r.result {
        lines.push((
            format!(
                "result:  {}{}",
                result.outcome.as_str(),
                result
                    .output_commit
                    .as_ref()
                    .map(|c| format!(" (output {})", ops::short(c)))
                    .unwrap_or_default()
            ),
            Color::Reset,
        ));
    }
    if !r.blockers.is_empty() {
        lines.push(("blocked by:".into(), Color::Red));
        for b in &r.blockers {
            lines.push((format!("  {b}"), Color::Red));
        }
    }
    if !r.stale.is_empty() {
        lines.push(("stale:".into(), Color::Yellow));
        for reason in &r.stale {
            for line in reason.lines() {
                lines.push((format!("  {line}"), Color::Yellow));
            }
        }
    }
    let description = r.description.trim_end();
    if !description.is_empty() {
        lines.push((String::new(), Color::Reset));
        for line in description.lines() {
            lines.push((line.to_string(), Color::Reset));
        }
    }
    lines
}

// ---------------------------------------------------------------------------
// Rendering primitives
// ---------------------------------------------------------------------------

/// Draw `text` at `(x, y)`, truncated and space-padded to exactly `width` cells.
/// `reverse` renders it as a bar (used for headers, footers, and the selection).
fn put(
    out: &mut Stdout,
    x: u16,
    y: u16,
    text: &str,
    width: usize,
    color: Color,
    reverse: bool,
) -> Result<()> {
    let mut s: String = text.chars().take(width).collect();
    let len = s.chars().count();
    if len < width {
        s.extend(std::iter::repeat_n(' ', width - len));
    }
    queue!(out, cursor::MoveTo(x, y))?;
    if reverse {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    if color != Color::Reset {
        queue!(out, SetForegroundColor(color))?;
    }
    queue!(out, Print(s))?;
    if reverse || color != Color::Reset {
        queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
    }
    Ok(())
}

/// Draw a centred bordered box and return its interior rect `(x, y, w, h)`.
fn draw_box(out: &mut Stdout, title: &str, box_w: u16, box_h: u16) -> Result<(u16, u16, u16, u16)> {
    let (tw, th) = terminal::size()?;
    // Clamp to the screen, but never below a drawable 3x3 so the borders and a row
    // of interior always exist (guards against a tiny or zero-size terminal).
    let box_w = box_w.min(tw).max(3);
    let box_h = box_h.min(th).max(3);
    let x = (tw.saturating_sub(box_w)) / 2;
    let y = (th.saturating_sub(box_h)) / 2;

    let top = format!("┌{}┐", bar(box_w.saturating_sub(2), title));
    let bottom = format!("└{}┘", "─".repeat(box_w.saturating_sub(2) as usize));
    queue!(out, cursor::MoveTo(x, y), Print(top))?;
    for row in 1..box_h - 1 {
        queue!(
            out,
            cursor::MoveTo(x, y + row),
            Print(format!("│{}│", " ".repeat(box_w.saturating_sub(2) as usize)))
        )?;
    }
    queue!(out, cursor::MoveTo(x, y + box_h - 1), Print(bottom))?;

    Ok((x + 1, y + 1, box_w.saturating_sub(2), box_h.saturating_sub(2)))
}

/// A horizontal border with an embedded ` title ` label, padded to `width`.
fn bar(width: u16, title: &str) -> String {
    let width = width as usize;
    if title.is_empty() {
        return "─".repeat(width);
    }
    let label = format!(" {title} ");
    let label: String = label.chars().take(width).collect();
    let rest = width.saturating_sub(label.chars().count());
    format!("{label}{}", "─".repeat(rest))
}

// ---------------------------------------------------------------------------
// Modal prompts — each runs its own blocking read loop
// ---------------------------------------------------------------------------

/// Read the next key press, or `None` for a non-key event (e.g. a resize).
fn read_key() -> Result<Option<KeyEvent>> {
    match event::read()? {
        Event::Key(k) if k.kind == KeyEventKind::Press => Ok(Some(k)),
        _ => Ok(None),
    }
}

/// Single-line text prompt. Returns `None` on Esc.
fn prompt_line(out: &mut Stdout, label: &str, initial: &str) -> Result<Option<String>> {
    edit(out, label, initial, false)
}

/// Multi-line text prompt (Enter inserts a newline, Ctrl-S submits). `None` on Esc.
fn prompt_text(out: &mut Stdout, label: &str, initial: &str) -> Result<Option<String>> {
    edit(out, label, initial, true)
}

/// The shared line/text editor used by both prompts.
fn edit(out: &mut Stdout, label: &str, initial: &str, multiline: bool) -> Result<Option<String>> {
    let mut buf: Vec<char> = initial.chars().collect();
    let mut cur = buf.len();
    let (tw, _) = terminal::size()?;
    let box_w = tw.saturating_sub(4).min(100);
    let box_h = if multiline { 12 } else { 4 };

    loop {
        let (ix, iy, iw, ih) = draw_box(out, label, box_w, box_h)?;
        // Render the buffer, wrapped only at explicit newlines, tracking the cursor.
        let (rows, cursor_rc) = layout(&buf, cur, iw as usize);
        let first = rows.len().saturating_sub(ih as usize); // scroll to keep the end visible
        for (r, y) in (first..rows.len()).zip(iy..iy + ih) {
            put(out, ix, y, &rows[r], iw as usize, Color::Reset, false)?;
        }
        // Position the hardware cursor.
        let (cr, cc) = cursor_rc;
        if cr >= first && cr < first + ih as usize {
            let cy = iy + (cr - first) as u16;
            let cx = ix + (cc.min(iw.saturating_sub(1) as usize)) as u16;
            queue!(out, cursor::MoveTo(cx, cy), cursor::Show)?;
        } else {
            queue!(out, cursor::Hide)?;
        }
        out.flush()?;

        let Some(key) = read_key()? else { continue };
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter if !multiline => return Ok(Some(buf.into_iter().collect())),
            KeyCode::Char('s') if multiline && key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Some(buf.into_iter().collect()));
            }
            KeyCode::Enter => {
                buf.insert(cur, '\n');
                cur += 1;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                buf.insert(cur, c);
                cur += 1;
            }
            KeyCode::Backspace if cur > 0 => {
                buf.remove(cur - 1);
                cur -= 1;
            }
            KeyCode::Delete if cur < buf.len() => {
                buf.remove(cur);
            }
            KeyCode::Left if cur > 0 => cur -= 1,
            KeyCode::Right if cur < buf.len() => cur += 1,
            KeyCode::Home => cur = 0,
            KeyCode::End => cur = buf.len(),
            _ => {}
        }
    }
}

/// Split a char buffer into display rows (at newlines) and find the cursor's
/// (row, col). Long lines are truncated visually by the caller.
fn layout(buf: &[char], cur: usize, _width: usize) -> (Vec<String>, (usize, usize)) {
    let mut rows = vec![String::new()];
    let mut cursor = (0usize, 0usize);
    for (i, &c) in buf.iter().enumerate() {
        if i == cur {
            cursor = (rows.len() - 1, rows.last().unwrap().chars().count());
        }
        if c == '\n' {
            rows.push(String::new());
        } else {
            rows.last_mut().unwrap().push(c);
        }
    }
    if cur >= buf.len() {
        cursor = (rows.len() - 1, rows.last().unwrap().chars().count());
    }
    (rows, cursor)
}

/// A single-choice picker over `items`. Returns the chosen index, or `None` on Esc.
fn pick(out: &mut Stdout, label: &str, items: &[&str], initial: usize) -> Result<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut sel = initial.min(items.len() - 1);
    let (tw, th) = terminal::size()?;
    let widest = items.iter().map(|s| s.len()).max().unwrap_or(0);
    let box_w = (widest as u16 + 4).max(label.len() as u16 + 4).min(tw.saturating_sub(2));
    let box_h = (items.len() as u16 + 2).min(th.saturating_sub(2));

    loop {
        let (ix, iy, iw, ih) = draw_box(out, label, box_w, box_h)?;
        let first = sel.saturating_sub(ih.saturating_sub(1) as usize);
        for (i, y) in (first..items.len().min(first + ih as usize)).zip(iy..iy + ih) {
            put(out, ix, y, items[i], iw as usize, Color::Reset, i == sel)?;
        }
        queue!(out, cursor::Hide)?;
        out.flush()?;

        let Some(key) = read_key()? else { continue };
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter => return Ok(Some(sel)),
            KeyCode::Char('j') | KeyCode::Down => sel = (sel + 1).min(items.len() - 1),
            KeyCode::Char('k') | KeyCode::Up => sel = sel.saturating_sub(1),
            _ => {}
        }
    }
}

/// A scrollable read-only popup (used for the result view).
fn popup(out: &mut Stdout, title: &str, lines: &[String]) -> Result<()> {
    let (tw, th) = terminal::size()?;
    let box_w = tw.saturating_sub(4).min(100);
    let box_h = th.saturating_sub(4).min(lines.len() as u16 + 3);
    let mut top = 0usize;

    loop {
        let (ix, iy, iw, ih) = draw_box(out, title, box_w, box_h)?;
        let view = ih.saturating_sub(1) as usize; // last interior row is the hint
        for (i, y) in (top..lines.len().min(top + view)).zip(iy..iy + view as u16) {
            put(out, ix, y, &lines[i], iw as usize, Color::Reset, false)?;
        }
        put(
            out,
            ix,
            iy + ih - 1,
            "(j/k scroll, any other key closes)",
            iw as usize,
            Color::DarkGrey,
            false,
        )?;
        queue!(out, cursor::Hide)?;
        out.flush()?;

        let Some(key) = read_key()? else { continue };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if top + view < lines.len() {
                    top += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => top = top.saturating_sub(1),
            _ => return Ok(()),
        }
    }
}

/// The help overlay.
fn help(out: &mut Stdout) -> Result<()> {
    let lines = [
        "Navigation",
        "  j / k, ↓ / ↑    move selection",
        "  g / G           first / last",
        "  Tab             cycle filter (all → ready → blocked → stale)",
        "  Enter           show the selected node's result record",
        "  r               refresh from the store",
        "",
        "Actions (operate on the selected node)",
        "  a               add a new node",
        "  e               edit the description (a definition change)",
        "  l               link to another node",
        "  c               complete (commit outputs, write result.md)",
        "  f               fail (write result.md with what went wrong)",
        "",
        "  ? this help      q / Esc quit",
        "",
        "A '*' marks a stale node. Colours: green done, red failed.",
        "Every action requires a clean git tree.",
    ]
    .map(String::from);
    popup(out, "llaundry-tui — help", &lines)
}
