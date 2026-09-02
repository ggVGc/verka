//! Shared scaffolding for the rendering tests in this module.
//!
//! Two rules hold here, and both exist because the tests kept breaking on
//! changes that were not about them:
//!
//! Nothing in a rendering test may depend on a default. [`app`] names its
//! model and effort rather than letting the provider resolve them, and sets
//! the timeline filters it wants rather than toggling them relative to
//! whatever the default happens to be. A test that inherits a default is
//! really asserting that default, so changing the product's mind about it
//! breaks a test that never meant to have an opinion.
//!
//! Assertions address a region, not the whole screen. [`Screen`] hands out
//! the title row, the body, and the footer separately, because the footer
//! renders the host's working directory: a `!contains(..)` over the flattened
//! buffer can match a path component and fail for reasons that have nothing
//! to do with the view under test.

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use crate::app::App;
use styra_server::agent::Selection;

/// The model and effort every rendering test launches with, named explicitly
/// so no test inherits [`styra_server::agent::Provider`]'s defaults. Any
/// concrete profile would do; these are constants of the tests, not of the
/// product.
pub(crate) const MODEL: &str = "gpt-5.6-sol";
pub(crate) const EFFORT: &str = "high";
pub(crate) const PROFILE: &str = "codex:gpt-5.6-sol/high";

/// A session app with a pinned profile and both timeline filters off, so tool,
/// thinking, and lifecycle entries render. Tests that care about a filter set
/// it themselves; see the module note on why none of this is left to default.
pub(crate) fn app(session: &str) -> App {
    configure(App::new(Selection::parse(PROFILE).unwrap(), session))
}

/// [`app`] for a session that has not launched yet.
pub(crate) fn pending_app() -> App {
    configure(App::pending(Selection::parse(PROFILE).unwrap()))
}

/// The filter state every rendering test starts from, stated rather than
/// inherited: everything visible, so a test that pushes an event can find it.
fn configure(mut app: App) -> App {
    app.timeline.conversation_only = false;
    app.timeline.show_minor = false;
    app
}

/// Draw `app` and return the whole buffer flattened, as the per-module copies
/// of this helper used to. Sound for positive assertions; prefer [`screen`]
/// for anything negative or positional.
pub(crate) fn rendered(app: &App) -> String {
    screen(app).all()
}

/// Draw `app` at the standard test size and return its buffer for region-wise
/// assertions.
pub(crate) fn screen(app: &App) -> Screen {
    screen_sized(app, 80, 20)
}

/// [`screen`] at an explicit size, for views that need the room.
pub(crate) fn screen_sized(app: &App, width: u16, height: u16) -> Screen {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| super::render(frame, app)).unwrap();
    Screen(terminal.backend().buffer().clone())
}

/// A drawn frame, addressable by region.
pub(crate) struct Screen(Buffer);

impl Screen {
    /// The underlying buffer, for tests that assert on cell styles.
    pub(crate) fn buffer(&self) -> &Buffer {
        &self.0
    }

    /// Every cell in reading order, joined. Use for positive assertions only.
    pub(crate) fn all(&self) -> String {
        self.0.content().iter().map(|cell| cell.symbol()).collect()
    }

    /// One row's symbols, joined.
    pub(crate) fn row(&self, y: u16) -> String {
        (0..self.0.area.width)
            .map(|x| self.0.cell((x, y)).unwrap().symbol())
            .collect()
    }

    /// The top border, which carries the session title.
    pub(crate) fn title(&self) -> String {
        self.row(0)
    }

    /// The rows between the borders: the view's own content, without the title
    /// above it or the footer's working directory below.
    pub(crate) fn body(&self) -> String {
        (1..self.0.area.height.saturating_sub(2))
            .map(|y| self.row(y))
            .collect()
    }

    /// The `(x, y)` of `needle`'s first character.
    ///
    /// Column-based rather than a byte offset into a joined `String`: title
    /// rows carry multi-byte box-drawing and separator glyphs (`┌`, `·`, `●`)
    /// ahead of plain-ASCII text, so a byte offset from `str::find` would
    /// overshoot the actual column whenever the needle sits after one of those.
    pub(crate) fn find(&self, needle: &str) -> (u16, u16) {
        self.locate(needle)
            .unwrap_or_else(|| panic!("no cell contains {needle:?}"))
    }

    fn locate(&self, needle: &str) -> Option<(u16, u16)> {
        let needle: Vec<char> = needle.chars().collect();
        for y in 0..self.0.area.height {
            let symbols: Vec<&str> = (0..self.0.area.width)
                .map(|x| self.0.cell((x, y)).unwrap().symbol())
                .collect();
            let found = (0..symbols.len()).find(|&start| {
                needle.iter().enumerate().all(|(i, &ch)| {
                    symbols.get(start + i).and_then(|s| s.chars().next()) == Some(ch)
                })
            });
            if let Some(x) = found {
                return Some((x as u16, y));
            }
        }
        None
    }
}
