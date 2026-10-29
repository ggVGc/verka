//! Fixtures and assertions for data passed across the Ui boundary.
use crate::test_ui::TestUi;

use crate::app::App;
use styra_protocol::agent::Selection;

/// The model and effort every presentation test launches with, named explicitly
/// so no test inherits [`styra_protocol::agent::Provider`]'s defaults. Any
/// concrete profile would do; these are constants of the tests, not of the
/// product.
pub(crate) const MODEL: &str = "gpt-5.6-sol";
pub(crate) const EFFORT: &str = "high";
pub(crate) const PROFILE: &str = "codex:gpt-5.6-sol/high";
/// A session with a pinned profile, all events enabled, and minor events hidden.
/// Tests set any different filters they need.
pub(crate) fn app(session: &str) -> App {
    app_with(PROFILE, session)
}

/// [`app`] on an explicitly named profile, for tests about a provider's own
/// presentation data. Still a full `provider:model/effort` triple: a bare provider
/// name would put the test back at the mercy of the provider's defaults.
pub(crate) fn app_with(profile: &str, session: &str) -> App {
    configure(App::new(Selection::parse(profile).unwrap(), session))
}

/// The filter state every presentation test starts from, stated rather than
/// inherited: include tool events while hiding minor lifecycle noise.
fn configure(mut app: App) -> App {
    app.timeline.all_events = true;
    app.timeline.show_minor = false;
    app
}

/// Inspect the actual view dispatched by the presentation adapter, before its borrows expire.
pub(crate) fn inspect(
    app: &App,
    mut check: impl FnMut(&styra_ui::application::ApplicationView<'_>),
) {
    let mut ui = TestUi::default();
    ui.application = Box::new(|view| {
        check(view);
        Ok(Default::default())
    });
    super::draw_application(&mut ui, app).unwrap();
    assert_eq!(ui.applications, 1);
}
