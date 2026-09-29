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
use styra_ui::event_list::{entry_item, EntryRender, EntryVersion, EventEntry};
use styra_ui::markdown::LinkDisplay;

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

fn main() {
    for messages in [200, 400] {
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
    }
}
