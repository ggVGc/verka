//! How long one frame of the event list takes to build.
//!
//! The list rebuilds every row it holds on every frame, and a frame is drawn
//! per keystroke, so this number is what typing latency is made of. The two
//! cases are the ones that matter: a session that has not changed since the
//! last frame (every keystroke while composing a message) and one whose tail
//! has just been rewritten.

use ratatui::layout::Rect;
use ratatui::widgets::{List, ListItem, Widget};
use std::time::Instant;
use styra_protocol::event::{AgentEvent, Protocol};
use styra_ui::chrome::{PanelChrome, StatusTone};
use styra_ui::event_list::{
    entry_item, render, ActivityCounts, EntryRender, EntryVersion, EventEntry, EventListStatus,
    EventListView,
};
use styra_ui::markdown::LinkDisplay;
use styra_ui::search::SearchView;

const WIDTH: usize = 120;
const VIEWPORT: usize = 40;

fn conversation(messages: usize) -> Vec<AgentEvent> {
    (0..messages)
        .map(|n| AgentEvent::AgentMessage {
            text: format!(
                "## Step {n}\n\nLooked at the retry backoff and found the jitter is applied \
                 before the cap rather than after, so a long backoff loses its spread.\n\n\
                 ```rust\nfn backoff(attempt: u32) -> Duration {{\n    \
                 let base = Duration::from_millis(50) * 2u32.pow(attempt);\n    \
                 base.min(CAP) + jitter()\n}}\n```\n\nSee [retry.rs:{n}](/src/retry.rs:{n})."
            ),
        })
        .collect()
}

fn frame(events: &[AgentEvent], versions: &[EntryVersion]) -> usize {
    let render = EntryRender {
        protocol: Protocol::default(),
        links: LinkDisplay::Compact,
        search: None,
    };
    let items: Vec<ListItem<'static>> = events
        .iter()
        .zip(versions)
        .map(|(event, &version)| {
            let entry = EventEntry {
                event,
                version,
                expanded: true,
                has_detail: true,
                contract: None,
                selected: false,
                link_highlight: None,
                branch_name: None,
                inherited: false,
            };
            entry_item(&entry, WIDTH, VIEWPORT, render)
        })
        .collect();
    // Draw them, so the measurement covers what a frame actually does rather
    // than stopping at the point the rows are built.
    let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, WIDTH as u16, 200));
    let total = items.iter().map(ListItem::height).sum();
    List::new(items).render(buffer.area, &mut buffer);
    total
}

fn time(label: &str, mut run: impl FnMut()) {
    run();
    let rounds = 20;
    let started = Instant::now();
    for _ in 0..rounds {
        run();
    }
    let each = started.elapsed() / rounds;
    println!("{label:<44} {each:>10.2?} per frame");
}

/// One whole frame of the real `render`, which is what a keystroke costs.
///
/// This is the path that used to build every row of the session to work out
/// where to scroll it; the window it draws is the same size whether the
/// session holds fifty messages or five thousand.
fn rendered_frame(events: &[AgentEvent], versions: &[EntryVersion], selected: usize) -> usize {
    let entries: Vec<EventEntry<'_>> = events
        .iter()
        .zip(versions)
        .enumerate()
        .map(|(index, (event, &version))| EventEntry {
            event,
            version,
            expanded: true,
            has_detail: true,
            contract: None,
            selected: index == selected,
            link_highlight: None,
            branch_name: None,
            inherited: false,
        })
        .collect();
    let view = EventListView {
        chrome: PanelChrome {
            focused: true,
            workspace: None,
            worktree: None,
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
        // Anchored where the selection is, as it would be after scrolling there.
        requested_offset: selected,
        requested_row_offset: 0,
        scroll_delta: 0,
        anchor_selection: true,
        moved_backward: false,
        max_lines_above_selection: None,
        protocol: Protocol::default(),
        links: LinkDisplay::Compact,
        search: SearchView {
            query: None,
            typing: false,
        },
        status: EventListStatus::Idle { reason: None },
        queued: &[],
    };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
        WIDTH as u16,
        VIEWPORT as u16,
    ))
    .unwrap();
    let mut offset = 0;
    terminal
        .draw(|frame| {
            offset = render(frame, &view, frame.area()).effective_offset;
        })
        .unwrap();
    offset
}

fn main() {
    for messages in [200, 400, 2000] {
        let events = conversation(messages);
        let versions: Vec<EntryVersion> = (0..messages as u64)
            .map(|id| EntryVersion { id, revision: 0 })
            .collect();

        time(&format!("{messages} messages, unchanged"), || {
            frame(&events, &versions);
        });

        // Every frame sees a different version for every row: what the list
        // costs with the cache defeated, which is what it cost before there
        // was one.
        let mut generation = 0u32;
        time(&format!("{messages} messages, every row rewritten"), || {
            generation += 1;
            let bumped: Vec<EntryVersion> = versions
                .iter()
                .map(|v| EntryVersion {
                    revision: generation,
                    ..*v
                })
                .collect();
            frame(&events, &bumped);
        });

        // The real `render`, scrolled to the end of the session — the common
        // case, and the one that used to cost the most.
        time(&format!("{messages} messages, render() one frame"), || {
            rendered_frame(&events, &versions, messages - 1);
        });
    }
}
