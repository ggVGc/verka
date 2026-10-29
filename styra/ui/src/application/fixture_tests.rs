//! Renderer regressions use presentation fixtures, with no application state.
use super::*;
use crate::event_list::*;
use crate::{
    chrome::{PanelChrome, StatusTone},
    markdown::LinkDisplay,
    search::SearchView,
    TestUi,
};
use styra_protocol::event::{AgentEvent, Protocol};

fn version() -> EntryVersion {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1 << 63);
    EntryVersion {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
        revision: 0,
    }
}
fn list(events: &[AgentEvent]) -> EventListView<'_> {
    let selected = 0;
    let requested_offset = 0;
    let anchor_selection = true;
    let scroll_delta = 0;
    let entries = events
        .iter()
        .enumerate()
        .map(|(index, event)| EventEntry {
            event,
            version: version(),
            expanded: false,
            has_detail: false,
            contract: None,
            selected: index == selected,
            link_highlight: None,
            branch: Default::default(),
            branch_provider_switch: None,
            inherited: false,
        })
        .collect();
    EventListView {
        chrome: PanelChrome {
            focused: true,
            workspace: None,
            worktree: None,
            source_branch: None,
            agent: "codex".into(),
            model: "gpt-5.6-sol".into(),
            model_reported: true,
            effort: None,
            effort_reported: true,
            status: "running".into(),
            status_tone: StatusTone::Running,
            elapsed: None,
            suffix: None,
            session: None,
        },
        entries,
        activity: ActivityCounts::default(),
        all_events: false,
        show_minor: false,
        uncommitted_changes: false,
        usage: None,
        can_configure_launch: false,
        selection_name: "codex".into(),
        requested_offset,
        requested_row_offset: 0,
        scroll_delta,
        anchor_selection,
        keep_viewport: false,
        moved_backward: false,
        max_lines_above_selection: None,
        protocol: Protocol::default(),
        links: LinkDisplay::Compact,
        search: SearchView {
            query: None,
            typing: false,
        },
        status: EventListStatus::Idle {
            reason: None,
            failed: false,
        },
        queued: &[],
    }
}
fn preview() -> preview::PreviewView<'static> {
    preview::PreviewView {
        entry: None,
        entry_change: None,
        workspace_roots: vec!["/workspace".into()],
        protocol: Protocol::default(),
        target: preview::PreviewTarget::Selection,
        links: LinkDisplay::Compact,
        link_highlight: None,
        branch_log: None,
        file_target: None,
        answer_target: None,
        requested_scroll: 0,
        fullscreen: false,
    }
}
fn footer() -> footer::FooterView<'static> {
    footer::FooterView {
        working_directory: "/workspace",
        quota: &[],
        auto_retry: false,
        auto_commit: false,
        background: None,
    }
}
fn application<'a>(
    list: &'a EventListView<'a>,
    footer: &'a footer::FooterView<'a>,
) -> ApplicationView<'a> {
    ApplicationView {
        session_id: "fixture",
        main: MainView::Events(EventView {
            list,
            navigator: None,
            entry_log: None,
            processes: None,
            preview: None,
            link_mode: false,
        }),
        notices: &[],
        footer,
        overlays: ApplicationOverlays::default(),
    }
}
fn draw(view: &ApplicationView<'_>, width: u16, height: u16) -> (Vec<String>, RenderFeedback) {
    let mut ui = TestUi::new(width, height).unwrap();
    let feedback = crate::Ui::render_application(&mut ui, view).unwrap();
    (ui.rows(), feedback)
}
fn locate(rows: &[String], text: &str) -> (usize, usize) {
    rows.iter()
        .enumerate()
        .find_map(|(y, row)| row.find(text).map(|byte| (row[..byte].chars().count(), y)))
        .unwrap_or_else(|| panic!("missing {text:?}: {rows:#?}"))
}

#[test]
fn header_formats_workspace_session_model_and_source_branch() {
    let mut list = list(&[]);
    list.chrome.focused = false;
    list.chrome.workspace = Some("payments".into());
    list.chrome.session = Some("Fix retries".into());
    list.chrome.effort = Some("high".into());
    let footer = footer();
    let (rows, _) = draw(&application(&list, &footer), 160, 20);
    assert!(
        rows[0].starts_with("┌ payments · Fix retries · codex · gpt-5.6-sol · high"),
        "{}",
        rows[0]
    );
    assert!(rows[0].contains("● running"));
    list.chrome.worktree = Some("fix-retries".into());
    list.chrome.source_branch = Some("main".into());
    let (rows, _) = draw(&application(&list, &footer), 160, 20);
    assert!(rows[0].contains("fix-retries"));
    assert!(rows[0].contains("from main"));
}

#[test]
fn focus_and_fleet_markers_belong_to_the_panel_border() {
    let mut list = list(&[]);
    let footer = footer();
    list.activity = ActivityCounts {
        running: 1,
        idle: 1,
        stopped: 0,
        newly_idle: 1,
    };
    list.uncommitted_changes = true;
    list.all_events = true;
    for minor in [false, true] {
        list.show_minor = minor;
        let (rows, _) = draw(&application(&list, &footer), 160, 20);
        assert!(rows[0].starts_with('┏'));
        assert!(rows[18].contains("1/1/0 ^a 1 interaction idle"));
        assert!(rows[18].contains("uncommitted changes"));
        assert!(rows[18].contains(if minor {
            "all events · minor shown"
        } else {
            "all events · minor hidden"
        }));
        assert!(!rows[19].contains("interaction idle"));
    }
    list.chrome.focused = false;
    assert!(draw(&application(&list, &footer), 160, 20).0[0].starts_with('┌'));
}

#[test]
fn footer_stays_on_the_bottom_row_and_formats_standing_state() {
    let list = list(&[]);
    let mut footer = footer();
    let (rows, _) = draw(&application(&list, &footer), 240, 20);
    assert!(rows[19].contains("/workspace"));
    assert!(!rows[19].contains("rate-limit retry"));
    assert!(!rows[19].contains("auto-commit"));
    footer.auto_retry = true;
    footer.auto_commit = true;
    footer.background = Some("describing edits of 2 interactions");
    let (rows, _) = draw(&application(&list, &footer), 240, 20);
    assert!(rows[19].contains("R rate-limit retry: on"));
    assert!(rows[19].contains("^G auto-commit"));
    assert!(rows[19].contains("⟳ describing edits of 2 interactions"));
}

#[test]
fn queued_messages_follow_status_and_a_long_queue_is_truncated() {
    let events = [AgentEvent::UserMessage {
        text: "rename the loader".into(),
    }];
    let mut list = list(&events);
    let footer = footer();
    list.status = EventListStatus::Running {
        elapsed: std::time::Duration::ZERO,
        quiet: None,
        events: 1,
    };
    let queue = [
        styra_protocol::QueuedMessage::new("then run tests"),
        styra_protocol::QueuedMessage::new("and commit it\nwith a short message"),
    ];
    list.queued = &queue;
    let (rows, _) = draw(&application(&list, &footer), 120, 30);
    assert!(locate(&rows, "working").1 < locate(&rows, "queued » then run tests").1);
    assert!(locate(&rows, "queued » then run tests").1 < locate(&rows, "queued » and commit it").1);
    assert!(!rows.concat().contains("with a short message"));
    let queue: Vec<_> = (0..8)
        .map(|n| styra_protocol::QueuedMessage::new(format!("follow-up {n}")))
        .collect();
    list.queued = &queue;
    let (rows, _) = draw(&application(&list, &footer), 120, 30);
    assert!(rows.concat().contains("follow-up 4"));
    assert!(!rows.concat().contains("follow-up 5"));
    assert!(rows.concat().contains("+3 more queued"));
}

#[test]
fn search_prompt_displays_query_and_cursor() {
    let mut list = list(&[]);
    let footer = footer();
    list.search = SearchView {
        query: Some("que"),
        typing: true,
    };
    assert!(draw(&application(&list, &footer), 80, 20)
        .0
        .concat()
        .contains("/que▌"));
}

#[test]
fn scoped_log_is_below_the_list_and_preview_spans_the_right_pane() {
    let events = [
        AgentEvent::UserMessage {
            text: "run tests".into(),
        },
        AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "ok".into(),
            exit_code: Some(0),
            output: "hidden command output".into(),
        },
    ];
    let list = list(&events[..1]);
    let scoped = list_entries(&events);
    let footer = footer();
    let preview = preview();
    let mut view = application(&list, &footer);
    let MainView::Events(e) = &mut view.main else {
        unreachable!()
    };
    e.entry_log = Some(&scoped);
    e.preview = Some(SidePreview::Entry(&preview));
    let (rows, _) = draw(&view, 120, 30);
    assert!(locate(&rows, "entry log · follows selection").1 > 15);
    assert!(locate(&rows, "cargo test").1 > 15);
    assert!(!rows.concat().contains("hidden command output"));
    let (x, y) = locate(&rows, "preview · C: command");
    assert!(x > 60);
    assert_eq!(y, 0);
    assert!(rows[28].chars().skip(60).any(|c| c == '└'));
}
fn list_entries(events: &[AgentEvent]) -> EntryLogView<'_> {
    EntryLogView {
        entries: list(events).entries,
        focused: false,
        requested_scroll: 0,
        protocol: Protocol::default(),
        links: LinkDisplay::Compact,
        focus_key: "Tab".into(),
        close_key: "L".into(),
    }
}

#[test]
fn scoped_log_scrolls_to_its_cursor_and_reports_focus() {
    let events: Vec<_> = (0..40)
        .map(|n| AgentEvent::CommandStarted {
            command: format!("cargo test case-{n}"),
        })
        .collect();
    let list = list(&[]);
    let footer = footer();
    let mut scoped = list_entries(&events);
    for last in [false, true] {
        scoped.focused = last;
        for (i, e) in scoped.entries.iter_mut().enumerate() {
            e.selected = i == if last { 39 } else { 0 };
        }
        let mut view = application(&list, &footer);
        let MainView::Events(e) = &mut view.main else {
            unreachable!()
        };
        e.entry_log = Some(&scoped);
        let (rows, _) = draw(&view, 120, 30);
        let text = rows.concat();
        assert!(text.contains(if last {
            "cargo test case-39"
        } else {
            "cargo test case-0"
        }));
        assert!(!text.contains(if last {
            "cargo test case-0"
        } else {
            "cargo test case-39"
        }));
        assert!(text.contains(if last {
            "Tab: back to the list"
        } else {
            "follows selection"
        }));
    }
}

fn input(parts: &[String]) -> modal_input::ModalInput<'_> {
    modal_input::ModalInput {
        title: "message".into(),
        note: None,
        model: None,
        model_reported: false,
        effort: None,
        effort_reported: false,
        preceding: vec![],
        notice: None,
        parts,
        focused: 0,
        choosing: false,
        source: None,
        placeholder: "type a message, Enter to send",
        cursor: true,
    }
}
#[test]
fn message_box_is_centered_and_recording_or_busy_takes_its_place() {
    let list = list(&[]);
    let footer = footer();
    let parts = vec![String::new()];
    let input = input(&parts);
    let mut view = application(&list, &footer);
    view.overlays.input = Some(&input);
    let (rows, _) = draw(&view, 80, 20);
    assert_eq!(locate(&rows, "type a message, Enter to send").1, 9);
    let parts = vec!["half a sentence".into()];
    let input = self::input(&parts);
    view.overlays.input = Some(&input);
    let capture = recording::RecordingView {
        level: 0.4,
        loudest: 0.6,
        gain: 2.0,
        captured: Some("1s".into()),
    };
    view.overlays.recording = Some(&capture);
    let (rows, _) = draw(&view, 80, 20);
    let text = rows.concat();
    assert!(text.contains("recording"));
    assert!(text.contains("boost ×2"));
    assert!(!text.contains("half a sentence"));
    view.overlays.busy = Some(busy::BusyView {
        message: "creating a new Git workspace…",
    });
    let (rows, _) = draw(&view, 80, 20);
    let text = rows.concat();
    assert!(text.contains("creating a new Git workspace…"));
    assert!(!text.contains("half a sentence"));
}

#[test]
fn integration_target_is_at_the_right_of_the_prompt_title() {
    let list = list(&[]);
    let footer = footer();
    let mut view = application(&list, &footer);
    view.overlays.integration_message = Some("Ship it");
    view.overlays.integration_target = Some("release");
    let (rows, _) = draw(&view, 80, 20);
    let (x, y) = locate(&rows, "integration commit message");
    let (target, ty) = locate(&rows, "into release");
    assert_eq!(y, ty);
    assert!(target > x);
    assert!(rows[y].contains("into release ┐"));
}

#[test]
fn file_fixture_opens_at_link_line_and_scrolls_to_end() {
    let list = list(&[]);
    let footer = footer();
    let mut preview = preview();
    preview.file_target = Some(preview::FileTarget {
        location: "long.txt:100".into(),
        line: Some(100),
        content: preview::FileTargetContent::Ready(
            (1..=200).map(|n| format!("row {n:03}\n")).collect(),
        ),
    });
    preview.requested_scroll = 99;
    let mut view = application(&list, &footer);
    view.main = MainView::Preview(&preview);
    let (rows, feedback) = draw(&view, 120, 30);
    assert!(rows.concat().contains("row 100"));
    assert!(!rows.concat().contains("row 001"));
    let measured = &feedback.scroll[0];
    assert!(measured.effective_offset > 0);
    let limit = measured.limit;
    preview.requested_scroll = limit;
    let mut view = application(&list, &footer);
    view.main = MainView::Preview(&preview);
    let (rows, _) = draw(&view, 120, 30);
    assert!(rows.concat().contains("row 200"));
}

#[test]
fn different_entry_identities_never_reuse_the_previous_sessions_prompt() {
    let footer = footer();
    let old = [AgentEvent::UserMessage {
        text: "rename the config loader".into(),
    }];
    let new = [AgentEvent::UserMessage {
        text: "add retries to the uploader".into(),
    }];
    let old = list(&old);
    let new = list(&new);
    assert!(draw(&application(&old, &footer), 80, 20)
        .0
        .concat()
        .contains("rename the config loader"));
    let text = draw(&application(&new, &footer), 80, 20).0.concat();
    assert!(text.contains("add retries to the uploader"));
    assert!(!text.contains("rename the config loader"));
}

#[test]
fn preview_renders_only_the_supplied_answer_section() {
    let list = list(&[]);
    let footer = footer();
    let mut preview = preview();
    preview.answer_target = Some(styra_protocol::Answer {
        contract: styra_protocol::Contract::Lines,
        value: Some(styra_protocol::AnswerValue::Lines(vec!["second answer".into()])),
        error: None,
        source: "Before\n<styra:answer>\nfirst answer\n</styra:answer>\nBetween\n<styra:answer>\nsecond answer\n</styra:answer>".into(),
    });
    let mut view = application(&list, &footer);
    view.main = MainView::Preview(&preview);
    let text = draw(&view, 120, 30).0.concat();
    assert!(text.contains("answer · lines"));
    assert!(text.contains("second answer"));
    assert!(!text.contains("first answer"));
}

#[test]
fn placed_change_fixture_renders_line_numbers_and_workspace_paths() {
    let event = [AgentEvent::FileChanged {
        id: String::new(),
        paths: vec!["/workspace/src/retry.rs".into()],
        diff: None,
        checkpoint: None,
        checkpoint_error: None,
    }];
    let list = list(&[]);
    let footer = footer();
    let mut preview = preview();
    preview.entry = self::list(&event).entries.pop();
    let paths = ["/workspace/src/retry.rs".into()];
    preview.entry_change = Some(preview::ChangeView {
        paths: &paths,
        diff: Some("@@ -3,1 +3,1 @@\n-let delay = base * 3;\n+let delay = base * 2;".into()),
    });
    let mut view = application(&list, &footer);
    view.main = MainView::Preview(&preview);
    let text = draw(&view, 160, 30).0.concat();
    assert!(text.contains("3 +let delay = base * 2;"));
    assert!(text.contains("3 -let delay = base * 3;"));
    assert!(text.contains("ws:src/retry.rs"));
}

#[test]
fn branch_preview_renders_the_supplied_log_and_loading_state() {
    use styra_protocol::event::BranchDirection;
    let marker = [AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "branch".into(),
        name: None,
    }];
    let log = [AgentEvent::AgentMessage {
        text: "done it the other way".into(),
    }];
    let list = list(&[]);
    let footer = footer();
    let mut preview = preview();
    preview.entry = self::list(&marker).entries.pop();
    preview.branch_log = Some(preview::BranchLog::Ready {
        events: &log,
        protocol: Protocol::default(),
    });
    let mut view = application(&list, &footer);
    view.main = MainView::Preview(&preview);
    let text = draw(&view, 140, 30).0.concat();
    assert!(text.contains("done it the other way"));
    assert!(text.contains("branch log"));
    preview.branch_log = Some(preview::BranchLog::Loading);
    let mut view = application(&list, &footer);
    view.main = MainView::Preview(&preview);
    let text = draw(&view, 140, 30).0.concat();
    assert!(text.contains("loading…"));
    assert!(!text.contains("done it the other way"));
}

#[test]
fn external_panel_receives_the_side_pane_area_and_returns_its_feedback() {
    use std::cell::Cell;
    struct FixturePanel(Cell<Rect>);
    impl Panel for FixturePanel {
        fn render(&self, frame: &mut Frame, area: Rect) -> PanelFeedback {
            self.0.set(area);
            frame.render_widget(
                ratatui::widgets::Paragraph::new("Edit report\nfixture.rs"),
                area,
            );
            PanelFeedback {
                diff_layout: Some((false, 3)),
                list_offset: Some(4),
                ..Default::default()
            }
        }
    }
    let panel = FixturePanel(Cell::new(Rect::default()));
    let events = [AgentEvent::UserMessage {
        text: "left conversation".into(),
    }];
    let list = list(&events);
    let footer = footer();
    let mut view = application(&list, &footer);
    let MainView::Events(e) = &mut view.main else {
        unreachable!()
    };
    e.preview = Some(SidePreview::CheckoutDiff(&panel));
    let (rows, feedback) = draw(&view, 160, 30);
    assert_eq!(panel.0.get(), Rect::new(80, 0, 80, 29));
    assert!(locate(&rows, "Edit report").0 >= 80);
    assert!(locate(&rows, "left conversation").0 < 80);
    assert_eq!(feedback.diff_layout, Some((false, 3)));
    assert_eq!(feedback.diff_file_list_offset, Some(4));
}
