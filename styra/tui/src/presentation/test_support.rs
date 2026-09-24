//! Shared scaffolding for TUI-to-presentation integration tests.
//!
//! These tests remain in `tui` because they construct application state and
//! verify its mapping across the `Ui` boundary. Renderer-unit tests that need
//! no [`crate::app::App`] belong to `styra_ui` instead.
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

use styra_ui::TestUi;

use crate::app::App;
use styra_protocol::agent::Selection;

/// The model and effort every rendering test launches with, named explicitly
/// so no test inherits [`styra_protocol::agent::Provider`]'s defaults. Any
/// concrete profile would do; these are constants of the tests, not of the
/// product.
pub(crate) const MODEL: &str = "gpt-5.6-sol";
pub(crate) const EFFORT: &str = "high";
pub(crate) const PROFILE: &str = "codex:gpt-5.6-sol/high";
/// A session app with a pinned profile and both timeline filters off, so tool,
/// thinking, and lifecycle entries render. Tests that care about a filter set
/// it themselves; see the module note on why none of this is left to default.
pub(crate) fn app(session: &str) -> App {
    app_with(PROFILE, session)
}

/// [`app`] on an explicitly named profile, for tests about a provider's own
/// rendering. Still a full `provider:model/effort` triple: a bare provider
/// name would put the test back at the mercy of the provider's defaults.
pub(crate) fn app_with(profile: &str, session: &str) -> App {
    configure(App::new(Selection::parse(profile).unwrap(), session))
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
    let mut ui = TestUi::new(width, height).unwrap();
    super::draw_application(&mut ui, app).unwrap();
    Screen(ui.rows())
}

/// A drawn frame, addressable by region.
pub(crate) struct Screen(Vec<String>);

impl Screen {
    /// Every cell in reading order, joined. Use for positive assertions only.
    pub(crate) fn all(&self) -> String {
        self.0.concat()
    }

    /// One row's symbols, joined.
    pub(crate) fn row(&self, y: u16) -> String {
        self.0.get(usize::from(y)).cloned().unwrap_or_default()
    }

    /// The top border, which carries the session title.
    pub(crate) fn title(&self) -> String {
        self.row(0)
    }

    /// The rows between the borders: the view's own content, without the title
    /// above it or the footer's working directory below.
    pub(crate) fn body(&self) -> String {
        (1..u16::try_from(self.0.len())
            .unwrap_or(u16::MAX)
            .saturating_sub(2))
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

    /// [`Screen::find`] without the panic, for a test asserting that
    /// something is *not* on screen.
    pub(crate) fn locate(&self, needle: &str) -> Option<(u16, u16)> {
        let needle: Vec<char> = needle.chars().collect();
        for (y, row) in self.0.iter().enumerate() {
            let symbols = row.chars().collect::<Vec<_>>();
            let found = (0..symbols.len()).find(|&start| {
                needle
                    .iter()
                    .enumerate()
                    .all(|(i, &ch)| symbols.get(start + i).copied() == Some(ch))
            });
            if let Some(x) = found {
                return Some((x as u16, y as u16));
            }
        }
        None
    }
}
