use super::*;
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

fn chrome(workspace: Option<&str>) -> PanelChrome {
    PanelChrome {
        focused: true,
        workspace: workspace.map(str::to_owned),
        session: None,
        worktree: None,
        into: None,
        source_branch: None,
        parent: None,
        agent: "codex".into(),
        model: "default model".into(),
        model_reported: false,
        effort: None,
        effort_reported: false,
        status: "idle".into(),
        status_tone: StatusTone::Idle,
        elapsed: None,
        suffix: None,
    }
}

/// A panel with nothing but its own name starts the title with it.
#[test]
fn a_title_of_only_a_suffix_has_no_leading_separator() {
    let mut chrome = chrome(None);
    chrome.suffix = Some("diff".into());
    let area = Rect::new(0, 0, 40, 3);
    let mut buffer = Buffer::empty(area);
    focused_interaction_block(&chrome).render(area, &mut buffer);
    let top: String = (0..area.width)
        .map(|x| buffer[(x, 0)].symbol().to_owned())
        .collect();
    assert!(top.starts_with("┏ diff ━"), "{top}");
}

#[test]
fn workspace_name_uses_the_navigator_heading_color() {
    let area = Rect::new(0, 0, 80, 3);
    let mut buffer = Buffer::empty(area);
    panel_block(&chrome(Some("payments"))).render(area, &mut buffer);

    for x in 2..10 {
        assert_eq!(buffer[(x, 0)].fg, theme::WORKSPACE_NAME);
    }
}

#[test]
fn a_branched_interaction_names_its_parent_after_its_title() {
    let area = Rect::new(0, 0, 80, 3);
    let mut buffer = Buffer::empty(area);
    let mut chrome = chrome(Some("payments"));
    chrome.session = Some("Fix retries".into());
    chrome.parent = Some("Retry backoff".into());
    focused_interaction_block(&chrome).render(area, &mut buffer);
    let top: String = (0..area.width).map(|x| buffer[(x, 0)].symbol()).collect();
    assert!(
        top.starts_with("┏ payments · Fix retries · ↱ Retry backoff ━"),
        "{top}"
    );
}

/// A linked worktree is named as the interactions list names it, with
/// the branch its work goes into.
#[test]
fn a_worktree_is_named_with_its_target() {
    let area = Rect::new(0, 0, 80, 3);
    let mut buffer = Buffer::empty(area);
    let mut chrome = chrome(Some("payments"));
    chrome.worktree = Some("fix-retries".into());
    chrome.into = Some("main".into());
    focused_interaction_block(&chrome).render(area, &mut buffer);
    let top: String = (0..area.width).map(|x| buffer[(x, 0)].symbol()).collect();
    assert!(
        top.starts_with("┏ payments · [fix-retries] → main ━"),
        "{top}"
    );
    assert_eq!(buffer[(13, 0)].fg, theme::DIRECTORY_NAME);
}

/// The point of the tones is telling one ending from another, so no two
/// reasons may share a color.
#[test]
fn every_stop_reason_has_its_own_color() {
    let tones = [
        StopTone::Paused,
        StopTone::Completed,
        StopTone::Abandoned,
        StopTone::Sealed,
        StopTone::Integrated,
        StopTone::IntegrationPending,
        StopTone::RateLimited,
        StopTone::Failed,
        StopTone::Exited,
        StopTone::ServerRestarted,
        StopTone::ServerRestartedMidTurn,
        StopTone::Unknown,
    ];
    for (i, a) in tones.iter().enumerate() {
        for b in &tones[i + 1..] {
            assert_ne!(a.color(), b.color(), "{a:?} and {b:?}");
        }
    }
}
