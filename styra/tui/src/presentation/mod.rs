//! Application-to-presentation mapping for the terminal UI boundary.
//!
//! This is deliberately not a renderer. The TUI owns [`App`] and the behavior
//! encoded in it, so this adapter prepares `styra_ui` presentation models,
//! performs application-owned I/O before a draw, and applies measurements
//! returned by the renderer. Widgets, layout, styling, terminal access, and
//! rendering caches live in the separate `styra_ui` crate.

mod driva;
mod entry_log;
mod files;
#[cfg(test)]
#[path = "tests/footer.rs"]
mod footer_tests;
mod interactions;
mod list;
mod preview;
pub(crate) mod quota;
mod raw;
#[cfg(test)]
mod test_support;

pub use styra_ui::picker::{Preview, SessionsPreview};

use crate::activity::Status;
use crate::app::{App, Focus, View};
use crate::insert::Insert;
use std::time::{Duration, Instant};
use styra_ui::{Ui, UiResult};
/// A duration in the compact form the status line and tail use: `12s`,
/// `2m14s`, `1h04m`. Seconds are dropped past an hour, where they no longer
/// tell the operator anything they are waiting on.
pub(crate) fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// How long the current status has held, for the views' shared title — or
/// `None` where the figure would say nothing: before a launch, once the
/// process has ended, and while idle — none of those is a state the operator
/// is waiting out, so how long it has held is not worth counting.
fn status_elapsed(app: &App) -> Option<String> {
    match app.activity.status {
        Status::Running | Status::Background | Status::Stopped(_) => {
            Some(format_duration(app.activity.progress().in_status))
        }
        Status::Pending | Status::Idle(_) | Status::Ended { .. } => None,
    }
}

/// What the clock currently reads on this frame: everything on screen derived
/// from the time rather than from state, as the text it is actually drawn as.
/// `None` when the frame holds no such thing at all.
///
/// The loop draws when something changed, and this is the exception it has to
/// allow for — these go stale with nothing happening to mark it. But asking
/// only whether a clock is present is the wrong question, because a clock is
/// not the same as a clock that has moved. `format_duration` writes an hour or
/// more as `1h04m`, so a session stopped this morning shows the same six
/// characters for a minute at a time; asked the presence question, the loop
/// repaints that sixty times and paints identical pixels fifty-nine of them.
///
/// So this returns the reading rather than a verdict, and the loop repaints
/// when the reading differs from the one already on screen. What is on the
/// frame and what is repainted for then cannot drift apart: a figure that is
/// still is left alone however often it is consulted.
///
/// Idle is not here at all. Nothing an idle session shows is timed: the title
/// drops its elapsed figure, the list's idle line carries none by design, and
/// the spinner steps with the events that arrive rather than with the clock.
pub(crate) fn clock_reading(app: &App) -> Option<String> {
    // The elapsed figure in the title, and for a running turn the list tail's
    // matching one. Compared as the string, so it counts as having moved
    // exactly when the operator would see it move.
    let elapsed = status_elapsed(app);
    // A boolean rather than a reading: the quota footer's figures never move
    // on their own — a utilization figure stands until a new reading replaces
    // it, and a reset is quoted as the moment it falls at rather than counted
    // down to. The one thing the clock changes is whether a refusal is still
    // waiting on its moment, and that flips once, when the moment lands.
    let reset_pending = quota::ticking(app);
    if elapsed.is_none() && !reset_pending {
        return None;
    }
    Some(format!(
        "{}{}",
        elapsed.unwrap_or_default(),
        if reset_pending { " quota-reset-due" } else { "" }
    ))
}

/// The chrome every full-region view wears: a border that brightens when the
/// list has focus, the session's status title (opening with the Workspace
/// name), and the Session name at the top right. `suffix` names the view in
/// the title; `None` is the event list, which is the default view and so
/// needs no name.
fn panel_chrome(app: &App, suffix: Option<&str>) -> styra_ui::chrome::PanelChrome {
    let label = app.launch_label();
    use styra_ui::chrome::StatusTone;
    let tone = match app.activity.status {
        Status::Pending => StatusTone::Pending,
        Status::Running => StatusTone::Running,
        Status::Idle(_) => StatusTone::Idle,
        Status::Background => StatusTone::Background,
        Status::Stopped(_) => StatusTone::Stopped,
        Status::Ended { error: Some(_), .. } => StatusTone::Error,
        Status::Ended { .. } => StatusTone::Ended,
    };
    styra_ui::chrome::PanelChrome {
        focused: app.focus == Focus::List,
        workspace: app.workspace.name.clone(),
        agent: label.agent,
        model: label.model.unwrap_or_else(|| "default model".into()),
        model_reported: label.model_reported,
        effort: label.effort,
        effort_reported: label.effort_reported,
        status: app.activity.status.label().to_owned(),
        status_tone: tone,
        elapsed: status_elapsed(app),
        suffix: suffix.map(str::to_owned),
        session: app.session_name.clone(),
    }
}

pub(crate) fn launcher_view(
    launcher: &crate::launcher::Launcher,
) -> styra_ui::launcher::LauncherView {
    styra_ui::launcher::LauncherView {
        selection: crate::launcher::label(&launcher.selection()),
        provider_locked: launcher.provider_locked,
        rows: launcher.labels(),
        list: launcher.list.clone(),
    }
}

/// Which window `?` should describe: the topmost thing on screen. The modal
/// overlays and the standalone chooser screens cover the session view, so they
/// answer before it does.
pub(crate) fn current_window(app: &App) -> crate::keybindings::Window {
    use crate::keybindings::Window;
    if app.launcher.is_some() {
        return Window::Launcher;
    }
    if app.template_picker.is_some() {
        return Window::TemplatePicker;
    }
    if app.branch_prompt.is_some() {
        return Window::Branch;
    }
    if app.tag_picker.is_some() {
        return Window::Tags;
    }
    if app.interactions.open {
        return Window::Interactions;
    }
    match app.view {
        View::Events => Window::Events,
        View::Raw => Window::Raw,
        View::Log => Window::Log,
        View::Quota => Window::Quota,
        View::Transcript => Window::Transcript,
        View::Driva => Window::Driva,
        View::Files => Window::Files,
        View::Answer => Window::Answer,
        View::Preview => Window::Preview,
    }
}

/// The reference rows for one window, in the UI's vocabulary.
pub(crate) fn help_rows(
    window: crate::keybindings::Window,
) -> Vec<styra_ui::help::HelpRow<'static>> {
    window
        .reference()
        .into_iter()
        .map(|row| match row {
            crate::keybindings::ReferenceRow::Section(name) => {
                styra_ui::help::HelpRow::Section(name)
            }
            crate::keybindings::ReferenceRow::Binding(binding) => {
                styra_ui::help::HelpRow::Binding {
                    keys: binding.label().into(),
                    action: binding.action().description(),
                }
            }
            crate::keybindings::ReferenceRow::Blank => styra_ui::help::HelpRow::Blank,
        })
        .collect()
}

/// Map the TUI-owned composer and queued-message state to the input model.
/// Rendering, wrapping, dimming, and cursor placement all remain in `ui`.
fn modal_input(app: &App) -> styra_ui::modal_input::ModalInput<'_> {
    let title = if app.can_send() {
        if app.outbox.queued_count() == 0 {
            " message ".to_owned()
        } else {
            format!(" message · {} queued ", app.outbox.queued_count())
        }
    } else {
        " message (resumes on send) ".to_owned()
    };
    let preceding = app
        .outbox
        .queued()
        .map(|message: &styra_protocol::QueuedMessage| {
            let prefix = match message.contract {
                Some(contract) => format!("queued ({}): ", contract.as_str()),
                None => "queued: ".to_owned(),
            };
            format!("{prefix}{}", message.text)
        })
        .collect();
    let label = app.launch_label();
    styra_ui::modal_input::ModalInput {
        title,
        model: Some(label.model.unwrap_or_else(|| "default model".into())),
        model_reported: label.model_reported,
        effort: label.effort,
        effort_reported: label.effort_reported,
        note: app
            .outbox
            .contract()
            .map(|contract| format!(" asking for {} ", contract.as_str())),
        preceding,
        notice: None,
        text: &app.composer.text,
        placeholder: if app.session_id.is_empty() {
            "Enter to send · Ctrl+Enter to send in a new Git workspace"
        } else {
            "type a message, Enter to send"
        },
        cursor: app.focus == Focus::Input,
    }
}

/// Map a running microphone capture to the meter that replaces the message
/// box while it runs.
fn recording(recorded: &crate::audio::Recorded) -> styra_ui::recording::RecordingView {
    styra_ui::recording::RecordingView {
        level: recorded.level,
        loudest: recorded.loudest,
        gain: recorded.gain,
        captured: recorded
            .capturing_since
            .map(|since| format_duration(since.elapsed())),
    }
}

/// Prepare and draw the main application without exposing `App` or Ratatui to
/// the UI trait. Derived strings and file contents live for this draw only;
/// event histories and protocol records remain borrowed.
pub(crate) fn draw_application(ui: &mut dyn Ui, app: &App) -> UiResult<styra_ui::RenderFeedback> {
    let started = Instant::now();
    tracing::debug!(
        target: "styra_tui::render",
        pid = std::process::id(),
        session_id = %app.session_id,
        view = ?app.view,
        timeline_entries = app.timeline.entries.len(),
        "rendering application"
    );
    use styra_ui::application::{EventView, FilesView as ApplicationFiles, MainView};
    let result = match app.view {
        View::Events => {
            let list = list::view(app);
            let navigator = app.interactions.open.then(|| interactions::view(app));
            let entry_log = app.entry_log.open.then(|| entry_log::view(app));
            let preview = app.preview.open.then(|| preview::view(app, false));
            draw_main(
                ui,
                app,
                MainView::Events(EventView {
                    list: &list,
                    navigator: navigator.as_ref(),
                    entry_log: entry_log.as_ref(),
                    preview: preview.as_ref(),
                }),
            )
        }
        View::Raw => {
            let raw = raw::view(app);
            draw_main(ui, app, MainView::Raw(&raw))
        }
        View::Log => {
            let chrome = panel_chrome(app, Some("log"));
            let entries = app
                .log
                .iter()
                .map(|entry| styra_ui::log::LogEntryView {
                    level: match entry.level {
                        styra_protocol::LogLevel::Info => styra_ui::log::LogLevel::Info,
                        styra_protocol::LogLevel::Warn => styra_ui::log::LogLevel::Warn,
                        styra_protocol::LogLevel::Error => styra_ui::log::LogLevel::Error,
                    },
                    message: &entry.message,
                })
                .collect::<Vec<_>>();
            draw_main(
                ui,
                app,
                MainView::Log {
                    chrome: &chrome,
                    entries: &entries,
                    scroll_back: app.log.scroll_back() as usize,
                },
            )
        }
        View::Quota => {
            let readings = quota::readings(app);
            let quota = styra_ui::quota::QuotaView {
                chrome: panel_chrome(app, Some("quota")),
                readings: &readings,
                auto_retry: app.auto_retry,
                scroll_back: app.quota.scroll_back(),
                now_ms: quota::now_ms(),
            };
            draw_main(ui, app, MainView::Quota(&quota))
        }
        View::Transcript => {
            let text = app.transcript_text();
            let transcript = styra_ui::transcript::TranscriptView {
                chrome: panel_chrome(app, Some("transcript")),
                text: &text,
                has_entries: !app.timeline.entries.is_empty(),
                conversation_only: app.timeline.conversation_only,
                uncommitted_changes: uncommitted_changes(app),
                requested_scroll: app.transcript.offset,
            };
            draw_main(ui, app, MainView::Transcript(&transcript))
        }
        View::Driva => {
            let driva = driva::view(app);
            draw_main(ui, app, MainView::Driva(&driva))
        }
        View::Answer => {
            let answer = styra_ui::answer::AnswerView {
                chrome: panel_chrome(app, None),
                answer: app.answer.answer(),
                error: app.answer.error(),
                selected: app.answer.selected_index(),
            };
            draw_main(ui, app, MainView::Answer(&answer))
        }
        View::Preview => {
            let preview = preview::view(app, true);
            draw_main(ui, app, MainView::Preview(&preview))
        }
        View::Files => {
            let list = list::view(app);
            let preview = app.preview.open.then(|| preview::view(app, false));
            let items = files::items(app);
            let selected = app
                .files
                .selected_index()
                .min(items.len().saturating_sub(1));
            let file_views = items
                .iter()
                .map(|file| styra_ui::files::FileView {
                    reported: file.reported.clone(),
                    root: file.root.display().to_string(),
                    relative: file.relative.display().to_string(),
                    components: file
                        .relative
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy().into_owned())
                        .collect(),
                })
                .collect::<Vec<_>>();
            // Content acquisition belongs to the application adapter and is
            // completed before rendering begins.
            let loaded = items.get(selected).map(|file| {
                std::fs::read_to_string(&file.resolved).map_err(|error| error.to_string())
            });
            let content = match loaded.as_ref() {
                None => styra_ui::files::FileContent::None,
                Some(Ok(text)) if text.is_empty() => styra_ui::files::FileContent::Empty,
                Some(Ok(text)) => styra_ui::files::FileContent::Ready(text),
                Some(Err(error)) => styra_ui::files::FileContent::Failed(error),
            };
            let scope = if app.files.shows_all() {
                "files · all session · a: focused"
            } else {
                "files · focused entry · a: all"
            };
            draw_main(
                ui,
                app,
                MainView::Files(ApplicationFiles {
                    list: &list,
                    preview: preview.as_ref(),
                    files: &file_views,
                    selected,
                    scope,
                    selected_name: items.get(selected).map(|file| file.reported.as_str()),
                    content,
                }),
            )
        }
    };
    match &result {
        Ok(_) => tracing::debug!(
            target: "styra_tui::render",
            pid = std::process::id(),
            session_id = %app.session_id,
            view = ?app.view,
            elapsed_ms = started.elapsed().as_millis(),
            "rendered application"
        ),
        Err(error) => tracing::warn!(
            target: "styra_tui::render",
            pid = std::process::id(),
            session_id = %app.session_id,
            view = ?app.view,
            elapsed_ms = started.elapsed().as_millis(),
            error = %error,
            "application render failed"
        ),
    }
    result
}

/// Whether the interaction being shown — not the cursor's — stopped working
/// with changes left in its checkout. The panes report what they are showing,
/// and that is this session's interaction.
pub(crate) fn uncommitted_changes(app: &App) -> bool {
    app.interactions
        .current(&app.session_id)
        .is_some_and(|interaction| interaction.uncommitted_changes)
}

fn draw_main(
    ui: &mut dyn Ui,
    app: &App,
    main: styra_ui::application::MainView<'_>,
) -> UiResult<styra_ui::RenderFeedback> {
    let notices = app
        .notices
        .iter()
        .map(|notice| notice.text.clone())
        .collect::<Vec<_>>();
    let working_directory = app
        .workspace
        .working_directory_or_current()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let quota_alert = quota::alert(app);
    let footer = styra_ui::footer::FooterView {
        working_directory: &working_directory,
        idle_interactions: app.interactions.idle_notification_count(),
        quota: &quota_alert,
        auto_retry: app.auto_retry,
    };
    let launcher = app.launcher.as_ref().map(launcher_view);
    let capture = app.recording.as_ref().map(recording);
    let input = (app.focus == Focus::Input && capture.is_none()).then(|| modal_input(app));
    let insert = app.insert.as_ref().map(|prompt| match prompt.state() {
        Insert::Typing(text) => styra_ui::overlays::InsertPromptView::Typing(text),
        Insert::Grant(host) => {
            styra_ui::overlays::InsertPromptView::Grant(host.display().to_string())
        }
    });
    let branch = app
        .branch_prompt
        .as_ref()
        .map(|prompt| styra_ui::overlays::BranchPromptView {
            selected: prompt.selected_index(),
        });
    let tags = app
        .tag_picker
        .as_ref()
        .map(|picker| styra_ui::overlays::TagPickerView {
            available: &picker.available,
            selected: &picker.selected,
            cursor: picker.cursor,
            new_tag: picker.new_tag.as_deref(),
        });
    let application = styra_ui::application::ApplicationView {
        session_id: &app.session_id,
        main,
        notices: &notices,
        footer: &footer,
        overlays: styra_ui::application::ApplicationOverlays {
            launcher: launcher.as_ref(),
            input: input.as_ref(),
            recording: capture.as_ref(),
            references: None,
            insert,
            branch,
            tags,
        },
    };
    ui.render_application(&application)
}

pub(crate) fn apply_feedback(app: &mut App, feedback: &styra_ui::RenderFeedback) {
    if let Some(offset) = feedback.list_offset {
        app.timeline.list_offset = offset;
        app.timeline.rendered_selection = Some(app.timeline.selected);
    }
    for scroll in &feedback.scroll {
        match &scroll.panel {
            styra_ui::PanelId::Help => app.help.note_limit(scroll.limit),
            styra_ui::PanelId::Transcript { .. } => {
                app.transcript.offset = scroll.effective_offset;
                app.transcript.note_limit(scroll.limit);
            }
            styra_ui::PanelId::Preview {
                target: styra_ui::PreviewPanel::StyraWire,
                ..
            } => {
                app.raw.preview.offset = scroll.effective_offset;
                app.raw.preview.note_limit(scroll.limit);
            }
            styra_ui::PanelId::Preview {
                target: styra_ui::PreviewPanel::ProviderRaw,
                ..
            } => {
                if let Some(raw) = &mut app.provider_raw {
                    raw.preview.offset = scroll.effective_offset;
                    raw.preview.note_limit(scroll.limit);
                }
            }
            styra_ui::PanelId::Preview { .. } => {
                app.preview.scroll.offset = scroll.effective_offset;
                app.preview.scroll.note_limit(scroll.limit);
            }
            styra_ui::PanelId::Driva { .. } => {
                if app.details_tab == crate::app::DetailsTab::Details {
                    app.details_scroll.offset = scroll.effective_offset;
                    app.details_scroll.note_limit(scroll.limit);
                } else {
                    app.launch.scroll.offset = scroll.effective_offset;
                    app.launch.scroll.note_limit(scroll.limit);
                }
            }
            styra_ui::PanelId::EntryLog => {
                app.entry_log.scroll.offset = scroll.effective_offset;
                app.entry_log.scroll.note_limit(scroll.limit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{self, rendered};
    use super::*;
    use styra_protocol::event::{AgentEvent, TurnOutcome, TurnUsage};

    /// What lets the event loop stop drawing an idle Styra: an idle frame has
    /// nothing on it read off the clock, so leaving it up is leaving it right.
    /// A running one counts seconds and has to keep being repainted.
    #[test]
    fn only_a_frame_with_a_clock_on_it_needs_repainting_on_its_own() {
        let mut app = test_support::app("s1");

        app.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);
        assert_eq!(clock_reading(&app), None, "an idle screen is a still one");

        app.activity.status = Status::Running;
        assert!(
            clock_reading(&app).is_some(),
            "a running turn counts seconds"
        );

        app.activity.status = Status::Background;
        assert!(clock_reading(&app).is_some(), "so does background work");
    }

    /// The reading is the text, so a figure that is drawn the same is the same
    /// reading, and the loop leaves the frame alone. `format_duration` writes
    /// an hour or more as `1h04m`: a session stopped this morning holds those
    /// six characters for a minute at a time, and repainting it each second
    /// paints identical pixels fifty-nine times out of sixty.
    #[test]
    fn an_elapsed_figure_too_coarse_to_have_moved_reads_the_same() {
        // Dated from the server's moment, which is how a client attaching to
        // an interaction that stopped a while ago gets the age in the first
        // place — rather than by reaching into the clock behind it.
        let stopped_for = |seconds: u64| {
            let mut app = test_support::app("s1");
            app.activity.adopt_server_status(
                crate::activity::Status::Stopped(crate::activity::StopReason::Completed),
                quota::now_ms() - seconds * 1_000,
            );
            clock_reading(&app)
        };

        let hour = stopped_for(3_600 + 4 * 60);
        assert_eq!(
            hour,
            stopped_for(3_600 + 4 * 60 + 30),
            "half a minute later is the same `1h04m` on screen"
        );
        assert_ne!(hour, stopped_for(3_600 + 5 * 60), "a minute later is not");

        // Under the hour the figure carries seconds, so it does move each one.
        assert_ne!(stopped_for(74), stopped_for(75));
    }

    fn quota(
        status: styra_protocol::QuotaStatus,
        resets_at_ms: Option<u64>,
    ) -> styra_protocol::QuotaEvent {
        styra_protocol::QuotaEvent {
            at_ms: 1_000,
            session_id: "s1".into(),
            provider: styra_protocol::agent::Provider::Codex,
            window: "5h".into(),
            status,
            utilization: Some(0.91),
            resets_at_ms,
            detail: None,
        }
    }

    /// The whole point of the narrow reading of "ticking". A utilization figure
    /// in the footer is a figure taken at a moment: it stands until the next
    /// reading replaces it, and repainting it changes nothing. Counting it as
    /// a clock left an otherwise idle Styra repainting once a second forever,
    /// which is what this exists to stop.
    #[test]
    fn a_standing_quota_figure_does_not_keep_an_idle_screen_repainting() {
        use styra_protocol::QuotaStatus;
        let mut app = test_support::app("s1");
        app.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);

        app.note_quota(quota(QuotaStatus::Warning, None));

        assert!(
            !quota::alert(&app).is_empty(),
            "the footer is showing the figure",
        );
        assert_eq!(
            clock_reading(&app),
            None,
            "but the figure is not going to move"
        );
    }

    /// A refusal quotes the moment work will be taken again, and stops quoting
    /// it once that moment lands — with no new reading to mark the change. So
    /// it ticks until it falls due, and is still again afterwards.
    #[test]
    fn a_refusal_ticks_only_until_the_moment_it_names() {
        use styra_protocol::QuotaStatus;
        let mut ahead = test_support::app("s1");
        ahead.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);
        ahead.note_quota(quota(QuotaStatus::Exhausted, Some(u64::MAX)));

        assert!(
            clock_reading(&ahead).is_some(),
            "the moment is still ahead of us"
        );

        let mut passed = test_support::app("s1");
        passed.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);
        passed.note_quota(quota(QuotaStatus::Exhausted, Some(1_000)));

        assert_eq!(
            clock_reading(&passed),
            None,
            "and that one has already landed"
        );
    }

    #[test]
    fn header_shows_selection_and_status() {
        let title = test_support::screen(&test_support::app("s1")).title();
        // Scoped to the title row, and stated positively. The old form was
        // `!rendered(..).contains("styra")` over the flattened buffer, meant
        // to say the title no longer opens with the program's name — but the
        // footer renders the host's working directory, so it also failed in
        // any checkout whose path happened to contain the word.
        assert!(title.starts_with("┌ codex · "), "{title}");
        assert!(title.contains("running"), "{title}");
    }

    #[test]
    fn header_opens_with_the_workspace_name_at_the_top_left() {
        let mut app = test_support::app("s1");
        app.workspace.name = Some("payments".into());
        let screen = test_support::screen(&app);

        // Spelled as the row it produces rather than as a column number: what
        // the test means is that the workspace name comes first, ahead of the
        // agent, and a bare `(x, y)` says that only by arithmetic over the
        // border and the title's leading pad.
        assert!(
            screen.title().starts_with("┌ payments · codex · "),
            "{}",
            screen.title()
        );
    }

    #[test]
    fn header_shows_the_workspace_name_alongside_the_session_name() {
        let mut app = test_support::app("s1");
        app.workspace.name = Some("payments".into());
        app.session_name = Some("Fix retries".into());
        let screen = rendered(&app);
        assert!(screen.contains("Fix retries"));
        assert!(screen.contains("payments"));
    }

    #[test]
    fn event_list_header_indicates_conversation_only_filter() {
        let mut app = test_support::app("s1");
        app.timeline.conversation_only = true;
        assert!(rendered(&app).contains("conversation only"));
    }

    #[test]
    fn header_shows_a_dot_indicating_running_vs_idle() {
        let mut app = test_support::app("s1");
        assert!(rendered(&app).contains('●'));

        app.push_event(AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage::default(),
        });
        assert!(rendered(&app).contains("idle"));
    }

    /// Completion is a property of the Session, not a fact the update stream
    /// reports, so nothing else would put it in the header — the local status
    /// set when the operator marks it done is what the operator reads back.
    #[test]
    fn header_names_completion_in_the_status() {
        let mut app = test_support::app("s1");
        app.activity.status =
            crate::activity::Status::Stopped(crate::activity::StopReason::Completed);
        let title = rendered(&app);
        assert!(title.contains("completed"), "{title}");
    }

    #[test]
    fn message_box_floats_in_the_center_of_the_primary_view() {
        let mut app = test_support::app("s1");
        app.enter_input();

        let screen = test_support::screen(&app);
        let (_, input_y) = screen.find("type a message, Enter to send");
        let (_, view_y) = screen.find("codex");
        assert_eq!(input_y, 9);
        assert!(
            input_y > view_y,
            "the message box should float over the primary view"
        );
    }

    /// A recording takes the message box's place rather than sitting beside
    /// it: the box's keys belong to the capture while it runs, so leaving the
    /// buffer on screen would offer typing that does not happen.
    #[test]
    fn a_running_recording_replaces_the_message_box() {
        let mut app = test_support::app("s1");
        app.enter_input();
        app.composer.insert("half a sentence");
        app.recording = Some(crate::audio::Recorded::for_test(0.4, 0.6, 2.0));

        let screen = test_support::screen(&app);

        assert!(screen.all().contains("recording"), "{}", screen.all());
        assert!(screen.all().contains("boost ×2"), "{}", screen.all());
        assert!(
            screen.locate("half a sentence").is_none(),
            "the buffer is not being typed into while the microphone is open"
        );
    }

    /// Every view's status line must name the model and effort in use, since
    /// the agent name alone does not say it and the session may be spending a
    /// model nobody typed.
    #[test]
    fn the_status_line_names_the_model_and_effort_in_use() {
        let mut app = test_support::app("s-1");
        let expected = format!(
            " codex · {} · {}",
            test_support::MODEL,
            test_support::EFFORT
        );
        assert!(rendered(&app).contains(&expected), "{}", rendered(&app));

        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some(test_support::MODEL.into()),
            effort: Some(test_support::EFFORT.into()),
        });
        let screen = rendered(&app);
        assert!(
            screen.contains(&format!("{expected} · ● running")),
            "{screen}"
        );

        // Every other view carries the same status line, so switching away from
        // the event list does not lose it.
        let toggles: [fn(&mut App); 4] = [
            App::toggle_raw,
            |app| app.toggle_view(View::Log),
            |app| app.toggle_view(View::Transcript),
            |app| app.toggle_view(View::Driva),
        ];
        for toggle in toggles {
            toggle(&mut app);
            let named = format!("{} · {}", test_support::MODEL, test_support::EFFORT);
            assert!(rendered(&app).contains(&named), "{}", rendered(&app));
            toggle(&mut app);
        }
    }
}
