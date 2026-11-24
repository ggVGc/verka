use super::*;
use crate::app::App;
use crate::presentation::test_support;

fn marker(direction: BranchDirection, session: &str) -> AgentEvent {
    AgentEvent::Branched {
        direction,
        session: session.into(),
        name: None,
    }
}

fn said(text: &str) -> AgentEvent {
    AgentEvent::UserMessage { text: text.into() }
}

/// A screen on `source` whose preview is open on its `branched to`
/// marker naming `branch-2`.
fn app_on_marker() -> App {
    let mut app = test_support::app("source");
    app.push_event(said("before the branch"));
    app.push_event(marker(BranchDirection::To, "branch-2"));
    app.select_last();
    app.preview.show();
    app
}

/// Give `app` the loaded log of `session`, in its own protocol.
fn load(app: &mut App, session: &str, events: Vec<AgentEvent>) {
    let protocol = app.selection.provider.protocol();
    app.branch_log = Some(BranchLog::loaded(session, events, protocol));
}

/// What the preview draws of the linked log: each entry's event, answer
/// shape and folding, and the protocol it is drawn in.
fn shown(app: &App) -> (Vec<(AgentEvent, Option<Contract>, bool)>, Protocol) {
    let view = crate::presentation::preview::view(app, false);
    let Some(styra_ui::preview::BranchLog::Ready { entries, protocol }) = view.branch_log else {
        panic!("linked log expected")
    };
    let entries = entries
        .iter()
        .map(|entry| (entry.event.clone(), entry.contract.copied(), entry.expanded))
        .collect();
    (entries, protocol)
}

fn target(app: &App) -> Option<&str> {
    app.preview_branch_target().map(|(id, _)| id)
}

/// The branch's copy of the source's history is already on this screen;
/// following the marker to it shows what the branch did after.
#[test]
fn a_branch_is_shown_from_where_its_own_history_begins() {
    let events = entries(
        [
            marker(BranchDirection::From, "source"),
            said("inherited"),
            marker(BranchDirection::From, "source"),
            said("its own"),
        ]
        .into_iter()
        .map(InteractionUpdate::Event),
    );
    assert_eq!(
        shown_from(BranchDirection::To, &events),
        &events[3..],
        "after the last `branched from` marker"
    );
    assert_eq!(shown_from(BranchDirection::From, &events), &events[..]);
}

/// The log is the linked Session's event list as this one would show it:
/// from where the branch's own history begins, conversation only, and
/// every event once all events are shown, folded as a fresh entry there
/// is. An operator message reads as it was written, with the answer shape
/// it asked for.
#[test]
fn the_preview_on_a_branch_marker_shows_the_linked_log_as_the_list_would() {
    let mut app = app_on_marker();
    let framed = styra_protocol::contract::frame("show me", Contract::Lines);
    let done = AgentEvent::AgentMessage {
        text: "done it the other way".into(),
    };
    load(
        &mut app,
        "branch-2",
        vec![
            marker(BranchDirection::From, "source"),
            said("before the branch"),
            marker(BranchDirection::From, "source"),
            AgentEvent::UserMessage { text: framed },
            AgentEvent::CommandStarted {
                command: "cargo test".into(),
            },
            done.clone(),
        ],
    );

    app.timeline.all_events = false;
    let (entries, protocol) = shown(&app);
    assert_eq!(protocol, app.selection.provider.protocol());
    assert_eq!(
        entries,
        [
            (said("show me"), Some(Contract::Lines), true),
            (done, None, true),
        ]
    );

    app.timeline.all_events = true;
    let (all, _) = shown(&app);
    assert_eq!(all.len(), 3, "the command too");
    assert!(all.iter().all(|(_, _, expanded)| !expanded));
}

/// Until the marker the cursor reached is loaded, the pane says so rather
/// than showing the log of the one it left.
#[test]
fn a_marker_not_yet_loaded_reads_as_loading() {
    let mut app = app_on_marker();
    load(&mut app, "elsewhere", vec![said("another branch entirely")]);
    assert!(matches!(
        crate::presentation::preview::view(&app, false).branch_log,
        Some(styra_ui::preview::BranchLog::Loading)
    ));
}

/// The operator opens a marker to see how the other side ended up, so
/// once its log arrives the preview starts at the bottom rather than
/// where the loading placeholder left it.
#[test]
fn a_freshly_loaded_marker_is_shown_from_its_end() {
    let root = tempfile::tempdir().unwrap();
    let client = Client::new(root.path().join("missing.sock"));
    let mut app = app_on_marker();

    assert!(sync(&mut app, &client), "a load starts");
    assert_eq!(app.preview.scroll.offset, 0, "still loading");

    app.branch_log.as_mut().unwrap().since -= SETTLE;
    assert!(sync(&mut app, &client));
    assert!(!matches!(
        app.branch_log.as_ref().unwrap().contents,
        Contents::Loading
    ));
    assert_eq!(app.preview.scroll.offset, u16::MAX, "at the end");

    app.preview.scroll.page_up();
    sync(&mut app, &client);
    assert_ne!(
        app.preview.scroll.offset,
        u16::MAX,
        "only the first load moves it"
    );
}

/// The preview loads whatever row the cursor is on: the marker, or one of
/// the branches listed beneath it as rows of their own. Off markers, or
/// with the preview closed, there is nothing to load.
#[test]
fn the_preview_loads_the_branch_the_cursor_is_on() {
    use crate::interactions::tests::branched;
    use styra_protocol::InteractionActivity::Running;

    let mut app = app_on_marker();
    app.interactions.refresh(vec![
        branched("branch-2", Running, "source"),
        branched("child-3", Running, "branch-2"),
        branched("grandchild-4", Running, "child-3"),
    ]);

    assert_eq!(target(&app), Some("branch-2"));
    app.select_next_line();
    assert_eq!(target(&app), Some("child-3"));
    app.select_next_line();
    assert_eq!(target(&app), Some("grandchild-4"));
    app.select_prev_line();
    app.select_prev_line();
    assert_eq!(target(&app), Some("branch-2"), "the marker's own row");

    app.select_first();
    assert_eq!(target(&app), None, "off the marker");
    app.select_last();
    app.preview.toggle();
    assert_eq!(target(&app), None, "the preview is closed");
}
