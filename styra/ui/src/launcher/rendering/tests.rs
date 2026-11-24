use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn rows() -> Vec<String> {
    [
        "codex:gpt-5.6-sol/minimal",
        "claude:claude-opus-5/max",
        "claude:claude-haiku-4-5-20251001",
    ]
    .iter()
    .map(|row| (*row).to_owned())
    .collect()
}

fn view() -> LauncherView {
    let rows = rows();
    LauncherView {
        title: "launch".into(),
        hint: " type to narrow · ? keys ".into(),
        selection: "codex:gpt-5.6-sol/minimal".into(),
        provider_locked: false,
        list: FuzzyList::at(&rows, 0),
        effort: None,
        rows,
    }
}

fn rendered(view: &LauncherView) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| render_launcher(frame, view, frame.area()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// Every row is a whole triple, so what is on screen is what would launch
/// — no assembling a selection out of three columns.
#[test]
fn shows_whole_triples_and_the_one_it_would_confirm() {
    let screen = rendered(&view());
    for text in [
        "styra · launch",
        "type to narrow",
        "codex:gpt-5.6-sol/minimal",
        "claude:claude-opus-5/max",
        "claude:claude-haiku-4-5-20251001",
    ] {
        assert!(screen.contains(text), "missing {text}: {screen}");
    }
}

#[test]
fn typing_narrows_the_list_and_shows_the_query() {
    let mut view = view();
    for character in "haiku".chars() {
        view.list.push(character);
    }
    let screen = rendered(&view);

    assert!(screen.contains("/haiku"), "the query is drawn: {screen}");
    assert!(screen.contains("claude-haiku"), "{screen}");
    assert!(!screen.contains("opus"), "the rows it excluded are gone");
}

#[test]
fn a_query_matching_nothing_says_so() {
    let mut view = view();
    for character in "zzz".chars() {
        view.list.push(character);
    }
    assert!(rendered(&view).contains("no match for zzz"));
}

#[test]
fn marks_a_locked_provider_as_fixed() {
    let mut view = view();
    view.provider_locked = true;
    assert!(rendered(&view).contains("agent fixed"));
}

#[test]
fn shows_separate_model_and_effort_lists_with_independent_filters() {
    let mut view = view();
    view.title = "model / effort".into();
    view.rows = vec!["model-sol".into(), "model-astra".into()];
    view.list = FuzzyList::at(&view.rows, 0);
    view.list.push('s');
    view.list.push('o');
    view.list.push('l');
    let rows = vec!["low".into(), "high".into()];
    let mut list = FuzzyList::at(&rows, 1);
    for ch in "high".chars() {
        list.push(ch);
    }
    view.effort = Some(EffortView {
        rows,
        list,
        focused: true,
    });
    let screen = rendered(&view);
    for expected in ["Models", "Effort", "model-sol", "high", "/sol", "/high"] {
        assert!(screen.contains(expected), "missing {expected}: {screen}");
    }
    assert!(!screen.contains("model-astra"));
    assert!(!screen.contains("low"));
}

#[test]
fn renders_as_a_centered_modal() {
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| render_launcher(frame, &view(), frame.area()))
        .unwrap();

    // Three rows need a five-row modal, 60 columns wide; centering that in
    // an 80x20 screen puts its top-left corner at (10, 7) rather than at
    // the top of the screen.
    assert_eq!(terminal.backend().buffer()[(10, 7)].symbol(), "┌");
}

/// A catalog longer than the screen is what the list is for, so the modal
/// stops growing and scrolls instead.
#[test]
fn a_long_catalog_does_not_grow_the_modal_past_its_limit() {
    let rows: Vec<String> = (0..60)
        .map(|index| format!("claude:model-{index}"))
        .collect();
    let mut view = view();
    view.list = FuzzyList::at(&rows, 59);
    view.rows = rows;

    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    terminal
        .draw(|frame| render_launcher(frame, &view, frame.area()))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        screen.contains("claude:model-59"),
        "the list scrolls to the cursor: {screen}"
    );
    assert!(
        !screen.contains("claude:model-0 "),
        "and leaves the top off"
    );
}
