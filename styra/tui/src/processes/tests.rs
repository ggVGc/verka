use super::*;

fn process(pid: u32, sandbox_pid: u32, parent_pid: u32) -> SandboxProcess {
    SandboxProcess {
        pid,
        sandbox_pid,
        parent_pid,
        state: "S".into(),
        command: format!("command {sandbox_pid}"),
        threads: 1,
        rss_kib: 1,
    }
}

#[test]
fn the_selection_follows_its_process_when_rows_above_it_change() {
    let mut view = ProcessView::default();
    view.replace(
        "s",
        Ok(Some(vec![
            process(101, 1, 100),
            process(102, 2, 101),
            process(103, 3, 101),
        ])),
    );
    view.select_last();
    assert_eq!(view.selected_command().as_deref(), Some("command 3"));

    assert!(view.replace(
        "s",
        Ok(Some(vec![process(101, 1, 100), process(103, 3, 101)]))
    ));
    assert_eq!(view.selected_index(), 1);
    assert_eq!(view.selected_command().as_deref(), Some("command 3"));
}

#[test]
fn an_unchanged_listing_is_not_news_and_another_sessions_is_loading() {
    let mut view = ProcessView::default();
    assert!(view.replace("s", Ok(None)));
    assert!(!view.replace("s", Ok(None)));
    assert!(matches!(view.listing("s"), ProcessListing::NotRunning));
    assert!(matches!(view.listing("other"), ProcessListing::Loading));
}

#[test]
fn process_pane_dispatches_listing_without_taking_selection() {
    let mut app = crate::presentation::test_support::app("s1");
    app.processes.replace(
        "s1",
        Ok(Some(vec![process(101, 1, 100), process(102, 2, 101)])),
    );
    app.toggle_processes_pane();
    crate::presentation::test_support::inspect(&app, |v| {
        let styra_ui::application::MainView::Events(e) = &v.main else {
            panic!()
        };
        assert!(e.entry_log.is_none());
        let processes = e.processes.expect("process pane");
        assert!(processes.selected.is_none());
        let styra_ui::processes::ProcessListing::Running(rows) = processes.listing else {
            panic!()
        };
        assert_eq!(rows.len(), 2);
    });
}

#[test]
fn a_selection_past_the_end_settles_on_the_last_process() {
    let mut view = ProcessView::default();
    view.replace(
        "s",
        Ok(Some(vec![process(101, 1, 100), process(102, 2, 101)])),
    );
    view.select_last();
    view.replace("s", Ok(Some(vec![process(101, 1, 100)])));
    assert_eq!(view.selected_index(), 0);
    view.select_prev();
    view.select_next();
    assert_eq!(view.selected_index(), 0);
}
