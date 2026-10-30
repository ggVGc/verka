//! Application-to-presentation mapping for the terminal UI boundary.
//!
//! This is deliberately not a renderer. The TUI owns [`App`] and the behavior
//! encoded in it, so this adapter prepares `styra_ui` presentation models,
//! performs application-owned I/O before a draw, and applies measurements
//! returned by the renderer. Widgets, layout, styling, terminal access, and
//! rendering caches live in the separate `styra_ui` crate.
//!
//! Tests here assert the data passed to `Ui` and the application of explicit
//! feedback. Screen, layout, and styling regressions belong in `styra_ui`,
//! using view fixtures independent of `App`.

mod driva;
mod entry_log;
mod files;
#[cfg(test)]
#[path = "tests/footer.rs"]
mod footer_tests;
mod interactions;
mod list;
mod overview;
pub(crate) mod preview;
pub(crate) mod quota;
mod raw;
#[cfg(test)]
pub(crate) mod test_support;

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
    // exactly when the operator would see it move. The overview has no title
    // of its own session's, but times each working tile.
    let elapsed = if app.view == View::Overview {
        Some(overview::clock_reading(app)).filter(|reading| !reading.is_empty())
    } else {
        status_elapsed(app)
    };
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
        if reset_pending {
            " quota-reset-due"
        } else {
            ""
        }
    ))
}

/// The color a stopped interaction is drawn in, by why it stopped.
pub(crate) fn stop_tone(reason: &crate::activity::StopReason) -> styra_ui::chrome::StopTone {
    use crate::activity::StopReason;
    use styra_ui::chrome::StopTone;
    match reason {
        // Stopped at the operator's word, as a pause is: nothing has started
        // it yet.
        StopReason::Paused | StopReason::Branched => StopTone::Paused,
        StopReason::Completed => StopTone::Completed,
        StopReason::Abandoned => StopTone::Abandoned,
        StopReason::Sealed => StopTone::Sealed,
        StopReason::Integrated { sealed: true, .. } => StopTone::Integrated,
        StopReason::Integrated { sealed: false, .. } => StopTone::IntegrationPending,
        StopReason::RateLimited(_) => StopTone::RateLimited,
        StopReason::Failed { .. } => StopTone::Failed,
        StopReason::Exited { .. } => StopTone::Exited,
        StopReason::ServerRestarted => StopTone::ServerRestarted,
        StopReason::NotAccepting => StopTone::Unknown,
    }
}

/// The chrome every full-region view wears: a border that brightens when the
/// list has focus, and the session's status title, opening with the
/// Workspace name followed by the interaction's own title. `suffix` names
/// the view in the title; `None` is the event list, which is the default
/// view and so needs no name.
/// The diff on screen, in the side panel or fullscreen: the checkout's, or —
/// while `u` has it so — the selected turn's, with the message that started
/// the turn above it. A turn diff has no edit report to write: that is the
/// checkout's.
fn shown_diff(app: &App) -> Option<styra_diff::CheckoutDiffView<'_>> {
    let turn = app.turn_diff_shown();
    let title = if turn {
        "turn diff · u: full diff · b: branch"
    } else if app.diff_turn_highlight {
        "diff · H: turn highlight on · u: turn diff · b: branch"
    } else {
        "diff · H: highlight turn · u: turn diff · b: branch"
    };
    let mut chrome = panel_chrome(app, Some("diff"));
    chrome.suffix = Some(title.to_owned());
    let mut view = match app.shown_diff() {
        Some(diff) => diff.view_model(chrome),
        None if turn || app.view == View::CheckoutDiff => {
            styra_diff::CheckoutDiffView::from_chrome(chrome)
        }
        None => return None,
    };
    view.host_shortcuts = &[
        ("u", "toggle selected turn / full checkout diff"),
        (
            "H",
            "toggle highlighting the selected turn in the full diff",
        ),
    ];
    // The side panel is too narrow to give the files a column; fullscreen
    // puts them back beside the patch.
    view.files_below = app.view != View::CheckoutDiff;
    if !turn && app.diff_turn_highlight {
        view.highlight_diff = app
            .turn_diff
            .as_ref()
            .filter(|panel| panel.turn == app.selected_turn())
            .and_then(|panel| panel.diff.diff.as_deref().ok());
    }
    view.prompts = app.diff_prompts();
    if turn {
        view.report = None;
        view.intro_message = app
            .turn_message()
            .map(|event| styra_ui::preview::MessageView {
                event,
                protocol: app.selection.provider.protocol(),
                links: list::ui_link_display(app.link_display),
            });
    }
    Some(view)
}

fn panel_chrome(app: &App, suffix: Option<&str>) -> styra_ui::chrome::PanelChrome {
    let label = app.launch_label();
    use styra_ui::chrome::StatusTone;
    let tone = match app.activity.status {
        Status::Pending => StatusTone::Pending,
        Status::Running => StatusTone::Running,
        // The agent survived the failure, but the title must not read as an
        // ordinary idle: the turn it was given did not get done.
        Status::Idle(crate::activity::IdleReason::Failed { .. }) => StatusTone::Error,
        Status::Idle(_) => StatusTone::Idle,
        Status::Background => StatusTone::Background,
        Status::Stopped(ref why) => StatusTone::Stopped(stop_tone(why)),
        Status::Ended { error: Some(_), .. } => StatusTone::Error,
        Status::Ended { .. } => StatusTone::Ended,
    };
    let worktree = app
        .interactions
        .current(&app.session_id)
        .and_then(|interaction| interaction.checkout.as_ref())
        .filter(|checkout| checkout.linked())
        .and_then(|checkout| checkout.worktree.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    styra_ui::chrome::PanelChrome {
        focused: if app.interactions.open {
            false
        } else if suffix == Some("diff") {
            // Fullscreen, the diff is the only panel: nothing to tell it from.
            app.diff_focused() && app.view != View::CheckoutDiff
        } else {
            app.focus == Focus::List && !app.diff_focused() && shares_screen(app)
        },
        workspace: app.workspace.name.clone(),
        worktree,
        source_branch: app.session_id.is_empty().then(|| {
            app.checkout_from
                .as_deref()
                .and_then(|id| app.interactions.current(id))
                .and_then(|interaction| interaction.checkout.as_ref())
                .and_then(|checkout| checkout.branch.clone())
                .unwrap_or_else(|| "Workspace branch".into())
        }),
        agent: label.agent,
        model: label.model.unwrap_or_else(|| "default model".into()),
        model_reported: label.model_reported,
        effort: label.effort,
        effort_reported: label.effort_reported,
        status: app.activity.status.label().to_owned(),
        status_tone: tone,
        elapsed: status_elapsed(app),
        suffix: suffix.map(str::to_owned),
        session: app.interactions.name(&app.session_id).map(str::to_owned),
        parent: app.interactions.parent(&app.session_id).map(|parent| {
            app.interactions
                .name(parent)
                .unwrap_or("untitled")
                .to_owned()
        }),
    }
}

/// Whether the main panel has a neighbour that could own the keys instead.
/// Alone on the event view — the composer is a modal, drawn only while
/// typing — there is nothing to tell it apart from, so the thick selection
/// border would only linger.
fn shares_screen(app: &App) -> bool {
    app.view != View::Events
        || app.entry_log.open
        || app.processes_pane
        || app.preview.open
        || app.diff_visible()
}

pub(crate) fn launcher_view(
    launcher: &crate::launcher::Launcher,
) -> styra_ui::launcher::LauncherView {
    styra_ui::launcher::LauncherView {
        title: launcher.choice.title().to_owned(),
        hint: launcher.choice.hint().to_owned(),
        selection: crate::launcher::label(&launcher.selection()),
        provider_locked: launcher.provider_locked,
        rows: launcher.labels(),
        list: launcher.list.clone(),
        effort: (launcher.choice == crate::launcher::Choice::ModelEffort).then(|| {
            styra_ui::launcher::EffortView {
                rows: launcher.effort_labels(),
                list: launcher.effort_list.clone(),
                focused: launcher.active == crate::launcher::Field::Effort,
            }
        }),
    }
}

/// Which window `?` should describe: the topmost thing on screen. The modal
/// overlays and the standalone chooser screens cover the session view, so they
/// answer before it does.
pub(crate) fn current_window(app: &App) -> crate::keybindings::Window {
    use crate::keybindings::Window;
    if app.action_menu.is_some() {
        return Window::ActionMenu;
    }
    if app.workspace_jump.is_some() {
        return Window::WorkspaceJump;
    }
    if app.launcher.is_some() {
        return Window::Launcher;
    }
    if app.template_picker.is_some() {
        return Window::TemplatePicker;
    }
    if app.branch_prompt.is_some() {
        return Window::Branch;
    }
    if app.link_menu.is_some() {
        return Window::LinkMenu;
    }
    if app.tag_picker.is_some() {
        return Window::Tags;
    }
    if app.interactions.open {
        return Window::Interactions;
    }
    view_window(app.view)
}

/// The window a view is, with no modal over it: whose keys the list
/// dispatcher reads.
pub(crate) fn view_window(view: View) -> crate::keybindings::Window {
    use crate::keybindings::Window;
    match view {
        View::Events => Window::Events,
        View::Raw => Window::Raw,
        View::Messages => Window::Messages,
        View::Log => Window::Log,
        View::Quota => Window::Quota,
        View::Processes => Window::Processes,
        View::Transcript => Window::Transcript,
        View::CheckoutDiff => Window::CheckoutDiff,
        View::Driva => Window::Driva,
        View::Files => Window::Files,
        View::Answer => Window::Answer,
        View::Preview => Window::Preview,
        View::Overview => Window::Overview,
    }
}

/// How many of the conversation's messages the fullscreen diff shows in place
/// of its header.
const DIFF_MESSAGES: usize = 3;

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
            crate::keybindings::ReferenceRow::Diff(binding) => styra_ui::help::HelpRow::Binding {
                keys: binding.label.into(),
                action: binding.description,
            },
            crate::keybindings::ReferenceRow::Blank => styra_ui::help::HelpRow::Blank,
        })
        .collect()
}

/// Map the TUI-owned composer and queued-message state to the input model.
/// Rendering, wrapping, dimming, and cursor placement all remain in `ui`.
fn modal_input(app: &App) -> styra_ui::modal_input::ModalInput<'_> {
    let mut title = if app.can_send() {
        if app.outbox.queued_count() == 0 {
            " message ".to_owned()
        } else {
            format!(" message · {} queued ", app.outbox.queued_count())
        }
    } else {
        " message (resumes on send) ".to_owned()
    };
    let composer = &app.composer;
    if composer.choosing() {
        title.push_str("· j/k choose · Enter edit · d delete ");
    } else if composer.parts().len() > 1 {
        title.push_str(&format!(
            "· box {}/{} ",
            composer.focused() + 1,
            composer.parts().len()
        ));
    }
    let preceding = app
        .outbox
        .queued()
        .iter()
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
        parts: composer.parts(),
        focused: composer.focused(),
        choosing: composer.choosing(),
        source: app.message_source.clone(),
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
        "started rendering interaction"
    );
    use styra_ui::application::{EventView, FilesView as ApplicationFiles, MainView};
    let result = match app.view {
        View::Events => {
            let list = list::view(app);
            let navigator = app.interactions.open.then(|| interactions::view(app));
            let entry_log = app.entry_log.open.then(|| entry_log::view(app));
            let processes = app
                .processes_pane
                .then(|| styra_ui::processes::ProcessesView {
                    chrome: styra_ui::chrome::PanelChrome {
                        focused: false,
                        ..panel_chrome(app, None)
                    },
                    listing: app.processes.listing(&app.session_id),
                    selected: None,
                });
            let preview = app.preview.open.then(|| preview::view(app, false));
            let diff = shown_diff(app);
            let side_preview = if app.diff_visible() {
                diff.as_ref()
                    .map(|diff| styra_ui::application::SidePreview::CheckoutDiff(diff))
            } else {
                preview
                    .as_ref()
                    .map(styra_ui::application::SidePreview::Entry)
            };
            draw_main(
                ui,
                app,
                MainView::Events(EventView {
                    list: &list,
                    navigator: navigator.as_ref(),
                    entry_log: entry_log.as_ref(),
                    processes: processes.as_ref(),
                    preview: side_preview,
                    link_mode: app.link_highlight.is_some(),
                }),
            )
        }
        View::Raw => {
            let raw = raw::view(app);
            draw_main(ui, app, MainView::Raw(&raw))
        }
        View::Messages => {
            let messages = styra_ui::raw::RawView {
                chrome: panel_chrome(app, None),
                source: styra_ui::raw::RawSource::Received(app.messages.as_slice()),
                selected: app.messages.selected_index(),
                requested_preview_scroll: app.messages.preview.offset,
            };
            draw_main(ui, app, MainView::Raw(&messages))
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
        View::Processes => {
            let processes = styra_ui::processes::ProcessesView {
                chrome: panel_chrome(app, None),
                listing: app.processes.listing(&app.session_id),
                selected: Some(app.processes.selected_index()),
            };
            draw_main(ui, app, MainView::Processes(&processes))
        }
        View::Transcript => {
            let text = app.transcript_text();
            let transcript = styra_ui::transcript::TranscriptView {
                chrome: panel_chrome(app, Some("transcript")),
                text: &text,
                has_entries: !app.timeline.entries.is_empty(),
                all_events: app.timeline.all_events,
                show_minor: app.timeline.show_minor,
                uncommitted_changes: uncommitted_changes(app),
                requested_scroll: app.transcript.offset,
            };
            draw_main(ui, app, MainView::Transcript(&transcript))
        }
        View::CheckoutDiff => {
            let mut diff = shown_diff(app).expect("fullscreen always draws a diff");
            diff.thumbnail = true;
            if app.diff_conversation {
                diff.conversation_data = Some(styra_ui::picker::conversation(
                    app.timeline.entries.iter().map(|entry| entry.event()),
                    app.selection.provider.protocol(),
                    DIFF_MESSAGES,
                ));
            }
            draw_main(ui, app, MainView::CheckoutDiff(&diff))
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
        View::Overview => {
            let overview = overview::view(app);
            draw_main(ui, app, MainView::Overview(&overview))
        }
        View::Files => {
            let list = list::view(app);
            let preview = app.preview.open.then(|| preview::view(app, false));
            let diff = shown_diff(app);
            let side_preview = if app.diff_visible() {
                diff.as_ref()
                    .map(|diff| styra_ui::application::SidePreview::CheckoutDiff(diff))
            } else {
                preview
                    .as_ref()
                    .map(styra_ui::application::SidePreview::Entry)
            };
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
                    preview: side_preview,
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
        .map(|notice| styra_ui::messages::MessageView {
            text: &notice.text,
            level: match notice.level {
                styra_protocol::LogLevel::Info => styra_ui::log::LogLevel::Info,
                styra_protocol::LogLevel::Warn => styra_ui::log::LogLevel::Warn,
                styra_protocol::LogLevel::Error => styra_ui::log::LogLevel::Error,
            },
        })
        .collect::<Vec<_>>();
    // A linked worktree's path is long and the same up to its name, which is
    // what tells one interaction's checkout from another's.
    let worktrees = app.workspace.worktrees_path();
    let working_directory = app
        .workspace
        .working_directory_or_current()
        .map(|path| {
            styra_ui::placeholders::Placeholders::default()
                .with(worktrees.as_deref(), styra_ui::placeholders::WORKTREES)
                .apply(&path.display().to_string())
        })
        .unwrap_or_default();
    let quota_alert = quota::alert(app);
    let background = match app.describing_edits {
        0 => None,
        1 => Some("describing edits".to_owned()),
        count => Some(format!("describing edits of {count} interactions")),
    };
    let footer = styra_ui::footer::FooterView {
        working_directory: &working_directory,
        quota: &quota_alert,
        auto_retry: app.auto_retry,
        auto_commit: app.auto_commit && !app.session_id.is_empty(),
        // Shown on the blank screen too: there it is the choice the first
        // message's interaction will be created with.
        background: background.as_deref(),
    };
    let launcher = app.launcher.as_ref().map(launcher_view);
    let capture = app.recording.as_ref().map(recording);
    let input = (app.focus == Focus::Input && capture.is_none()).then(|| modal_input(app));
    let insert = app.insert.as_ref().map(|prompt| match prompt.state() {
        Insert::Typing(text) => styra_ui::overlays::InsertPromptView::Typing(text),
        Insert::Grant(host) => styra_ui::overlays::InsertPromptView::Grant {
            host: host.display().to_string(),
            restarts: app.mount_change_restarts(),
        },
    });
    let branch = app
        .branch_prompt
        .as_ref()
        .map(|prompt| styra_ui::overlays::BranchPromptView {
            selected: prompt.selected_index(),
            history: prompt.history(),
            checkout: prompt.checkout(),
        });
    let link_labels = app.link_menu.as_ref().map(|menu| menu.labels());
    let link_menu = app
        .link_menu
        .as_ref()
        .zip(link_labels.as_deref())
        .map(|(menu, actions)| styra_ui::overlays::LinkMenuView {
            destination: menu.destination(),
            actions,
            selected: menu.selected_index(),
        });
    let tags = app
        .tag_picker
        .as_ref()
        .map(|picker| styra_ui::overlays::TagPickerView {
            available: &picker.available,
            selected: &picker.selected,
            list: &picker.list,
            new_tag: picker.new_tag.as_deref(),
            filter: picker.purpose == crate::tag_picker::TagPurpose::Filter,
        });
    let application = styra_ui::application::ApplicationView {
        session_id: &app.session_id,
        main,
        notices: &notices,
        footer: &footer,
        overlays: styra_ui::application::ApplicationOverlays {
            rebase_status: app.rebase_status.as_ref(),
            retarget_prompt: app.retarget_prompt.as_ref().map(|(text, history)| {
                (
                    text.as_str(),
                    history.iter().any(|saved| saved.branch == text.trim()),
                )
            }),
            rename_prompt: app.rename_prompt.as_deref(),
            integration_message: app.integration_prompt.as_deref(),
            integration_target: app
                .interactions
                .current(&app.session_id)
                .and_then(|interaction| interaction.branched_from.as_ref())
                .and_then(|point| point.branch.as_deref()),
            launcher: launcher.as_ref(),
            input: input.as_ref(),
            recording: capture.as_ref(),
            references: None,
            insert,
            branch,
            link_menu,
            diff_branches: app.diff_branch_picker.as_ref().map(|picker| {
                styra_ui::fuzzy_list::FuzzyListView {
                    title: " Comparison branch · newest commits first ",
                    rows: &picker.rows,
                    list: &picker.list,
                    focused: true,
                    empty_note: "no matching branches",
                    hint: " type to narrow · ↑/↓ choose · Enter selects · Esc cancels ",
                }
            }),
            action_menu: app
                .action_menu
                .as_ref()
                .map(|menu| styra_ui::overlays::ActionMenuView {
                    window: menu.window().name(),
                    rows: menu.rows(),
                    list: &menu.list,
                }),
            workspace_jump: app.workspace_jump.as_ref().map(|jump| {
                styra_ui::overlays::WorkspaceJumpView {
                    rows: jump.rows(),
                    list: &jump.list,
                }
            }),
            tags,
            busy: app
                .busy
                .as_deref()
                .map(|message| styra_ui::busy::BusyView { message }),
        },
    };
    ui.render_application(&application)
}

pub(crate) fn apply_feedback(app: &mut App, feedback: &styra_ui::RenderFeedback) {
    if let (Some((side_by_side, horizontal_scroll)), Some(diff)) =
        (feedback.diff_layout, app.shown_diff_mut())
    {
        diff.rendered_side = side_by_side;
        diff.horizontal_scroll = horizontal_scroll;
    }
    if let (Some((selected, scroll, preview)), Some(page)) = (
        feedback.diff_report_page,
        app.shown_diff_mut()
            .and_then(|diff| diff.report_page.as_mut()),
    ) {
        *page = styra_diff::ReportPage {
            selected,
            scroll,
            reveal: false,
            preview,
        };
    }
    if let Some(offset) = feedback.diff_file_list_offset {
        if let Some(diff) = app.shown_diff_mut() {
            diff.file_list_offset = offset;
        }
    }
    if let Some(rows) = &feedback.overview_rows {
        app.overview.note_rows(rows);
    }
    if let Some(offset) = feedback.list_offset {
        app.timeline.list_offset = offset;
        app.timeline.list_row_offset = feedback.list_row_offset.unwrap_or_default();
        app.timeline.list_scroll_delta = 0;
        app.timeline.rendered_selection = Some(app.timeline.selected);
    }
    if let Some(offset) = feedback.navigator_offset {
        app.interactions.scroll_offset = Some(offset);
    }
    for scroll in &feedback.scroll {
        match &scroll.panel {
            styra_ui::PanelId::Help => app.help.note_limit(scroll.limit),
            styra_ui::PanelId::Transcript { .. } => {
                app.transcript.offset = scroll.effective_offset;
                app.transcript.note_limit(scroll.limit);
            }
            styra_ui::PanelId::Preview {
                target: styra_ui::PreviewPanel::Messages,
                ..
            } => {
                app.messages.preview.offset = scroll.effective_offset;
                app.messages.preview.note_limit(scroll.limit);
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
                app.preview.scroll.note_viewport(scroll.viewport);
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
            styra_ui::PanelId::CheckoutDiff { .. } => {
                if let Some(diff) = app.shown_diff_mut() {
                    diff.scroll.offset = scroll.effective_offset;
                    diff.scroll.note_limit(scroll.limit);
                    diff.scroll.note_viewport(scroll.viewport);
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
    use super::test_support::{self, inspect};
    use super::*;
    use styra_protocol::event::{AgentEvent, TurnOutcome, TurnUsage};
    use styra_protocol::{CheckoutState, InteractionSummary};
    use styra_ui::application::MainView;

    #[test]
    fn turn_diff_mapping_passes_semantic_chrome_and_the_original_message() {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "fix the retry backoff".into(),
        });
        app.push_event(AgentEvent::FileChanged {
            id: String::new(),
            paths: vec!["a.rs".into()],
            diff: Some("@@ -1 +1 @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.diff_preview = true;
        app.diff_turn = true;
        app.sync_turn_diff();
        let view = shown_diff(&app).unwrap();
        let message = view.intro_message.unwrap();
        assert!(
            matches!(message.event, AgentEvent::UserMessage { text } if text == "fix the retry backoff")
        );
        assert_eq!(message.protocol, app.selection.provider.protocol());
        let chrome = view.chrome.unwrap();
        assert!(chrome.focused);
        assert!(chrome.suffix.unwrap().starts_with("turn diff"));
    }

    #[test]
    fn ui_receives_requested_anchor_and_feedback_is_applied_explicitly() {
        let mut app = test_support::app("s1");
        app.timeline.list_offset = 19;
        let mut ui = crate::test_ui::TestUi::default();
        ui.application = Box::new(|view| {
            assert_eq!(view.session_id, "s1");
            let MainView::Events(events) = &view.main else {
                panic!("events expected")
            };
            assert_eq!(events.list.requested_offset, 19);
            Ok(styra_ui::RenderFeedback {
                list_offset: Some(7),
                list_row_offset: Some(2),
                ..Default::default()
            })
        });
        let feedback = draw_application(&mut ui, &app).unwrap();
        assert_eq!(ui.applications, 1);
        assert_eq!(app.timeline.list_offset, 19);
        apply_feedback(&mut app, &feedback);
        assert_eq!(
            (app.timeline.list_offset, app.timeline.list_row_offset),
            (7, 2)
        );
    }

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
    fn header_data_contains_selection_and_status() {
        inspect(&test_support::app("s1"), |v| {
            let MainView::Events(events) = &v.main else {
                panic!("events expected")
            };
            assert_eq!(events.list.chrome.agent, "codex");
            assert_eq!(events.list.chrome.status, "running");
        });
    }

    #[test]
    fn header_data_resolves_workspace_name() {
        let mut app = test_support::app("s1");
        app.workspace.name = Some("payments".into());
        inspect(&app, |v| {
            let MainView::Events(events) = &v.main else {
                panic!("events expected")
            };
            assert_eq!(events.list.chrome.workspace.as_deref(), Some("payments"));
        });
    }

    #[test]
    fn header_data_resolves_interaction_title() {
        let mut app = test_support::app("s1");
        app.workspace.name = Some("payments".into());
        app.interactions.note_name("s1", Some("Fix retries".into()));
        inspect(&app, |v| {
            let MainView::Events(events) = &v.main else {
                panic!("events expected")
            };
            assert_eq!(events.list.chrome.session.as_deref(), Some("Fix retries"));
        });
    }

    /// An interaction branched off another names that parent in its header;
    /// one that was not names none.
    #[test]
    fn header_data_names_the_parent_it_was_branched_off() {
        use styra_protocol::agent::Provider;
        let mut app = test_support::app("s1");
        let parent = |app: &App| {
            let mut parent = None;
            inspect(app, |v| {
                let MainView::Events(events) = &v.main else {
                    panic!("events expected")
                };
                parent = events.list.chrome.parent.clone();
            });
            parent
        };
        assert_eq!(parent(&app), None);
        let origin = styra_protocol::SessionOrigin {
            session_id: "source".into(),
            provider: Provider::Claude,
            at_ms: None,
            history: styra_protocol::BranchHistory::ThroughSelected,
        };
        app.interactions
            .note_origin("s1", Provider::Claude, Some(&origin));
        assert_eq!(parent(&app).as_deref(), Some("untitled"));
        app.interactions
            .note_name("source", Some("Retry backoff".into()));
        assert_eq!(parent(&app).as_deref(), Some("Retry backoff"));
    }

    #[test]
    fn lone_event_list_only_requests_focus_when_another_pane_opens() {
        let mut app = test_support::app("s1");
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(!e.list.chrome.focused);
        });
        app.preview.open = true;
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(e.list.chrome.focused);
            assert!(e.preview.is_some());
        });
    }

    fn interaction_with_checkout(id: &str, checkout: Option<CheckoutState>) -> InteractionSummary {
        InteractionSummary {
            checkout,
            ..crate::interactions::tests::interaction(
                id,
                styra_protocol::InteractionActivity::Running,
            )
        }
    }

    #[test]
    fn header_names_only_linked_worktrees() {
        for (worktree, branch, expected) in [
            (
                "/workspace/.worktrees/fix-retries",
                "fix-retries",
                Some("fix-retries"),
            ),
            ("/workspace", "main", None),
        ] {
            let mut app = test_support::app("s1");
            app.interactions.items = vec![interaction_with_checkout(
                "s1",
                Some(CheckoutState {
                    worktree: worktree.into(),
                    repository: "/workspace".into(),
                    branch: Some(branch.into()),
                }),
            )];
            inspect(&app, |v| {
                let MainView::Events(events) = &v.main else {
                    panic!("events expected")
                };
                assert_eq!(
                    events.list.chrome.worktree.as_deref(),
                    expected,
                    "{worktree}"
                );
            });
        }
    }

    #[test]
    fn event_list_data_carries_both_filter_flags() {
        let mut app = test_support::app("s1");
        for (all, minor) in [(false, false), (true, false), (true, true)] {
            app.timeline.all_events = all;
            app.timeline.show_minor = minor;
            inspect(&app, |v| {
                let MainView::Events(e) = &v.main else {
                    panic!()
                };
                assert_eq!((e.list.all_events, e.list.show_minor), (all, minor));
            });
        }
    }

    #[test]
    fn turn_completion_updates_header_status_tone() {
        let mut app = test_support::app("s1");
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!(
                e.list.chrome.status_tone,
                styra_ui::chrome::StatusTone::Running
            );
        });
        app.push_event(AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage::default(),
        });
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!(
                e.list.chrome.status_tone,
                styra_ui::chrome::StatusTone::Idle
            );
        });
    }

    /// Completion is a property of the Session, not a fact the update stream
    /// reports, so nothing else would put it in the header — the local status
    /// set when the operator marks it done is what the operator reads back.
    #[test]
    fn header_names_completion_in_the_status() {
        let mut app = test_support::app("s1");
        for (reason, expected) in [
            (
                crate::activity::StopReason::Completed,
                "stopped · completed",
            ),
            (
                crate::activity::StopReason::Abandoned,
                "stopped · abandoned",
            ),
        ] {
            app.activity.status = crate::activity::Status::Stopped(reason);
            inspect(&app, |v| {
                let MainView::Events(e) = &v.main else {
                    panic!()
                };
                assert_eq!(e.list.chrome.status, expected);
            });
        }
    }

    /// Integration stops the agent itself, so the update stream only says the
    /// process ended. The listing is what says the work landed, and the header
    /// reads it from there.
    #[test]
    fn header_names_integration_once_the_agent_is_gone() {
        let mut app = test_support::app("s1");
        let mut interaction = interaction_with_checkout("s1", None);
        interaction.integration = Some(styra_protocol::Integration {
            branch: Some("main".into()),
            commit: "0123456789abcdef".into(),
        });
        interaction.completed = styra_protocol::CompletionState::Sealed;
        app.interactions.items.push(interaction);
        app.activity.status = crate::activity::Status::ended(Some(0), None);

        assert!(app.adopt_integration());
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!(e.list.chrome.status, "integrated into main · 0123456");
        });
        assert!(!app.adopt_integration());
    }

    #[test]
    fn integration_prompt_data_resolves_target_branch() {
        let mut app = test_support::app("s1");
        let mut interaction = interaction_with_checkout("s1", None);
        interaction.branched_from = Some(styra_protocol::BranchPoint {
            branch: Some("release".into()),
            commit: "0123456".into(),
        });
        app.interactions.items.push(interaction);
        app.integration_prompt = Some("Ship it".into());

        inspect(&app, |v| {
            assert_eq!(v.overlays.integration_message, Some("Ship it"));
            assert_eq!(v.overlays.integration_target, Some("release"));
        });
    }

    #[test]
    fn a_new_interaction_header_names_the_source_branch() {
        let mut app = test_support::app("");
        app.checkout_from = Some("source-session".into());
        app.interactions.items.push(interaction_with_checkout(
            "source-session",
            Some(CheckoutState {
                branch: Some("styra/source-work".into()),
                worktree: "/state/worktrees/source-work".into(),
                repository: "/workspace".into(),
            }),
        ));
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!(
                e.list.chrome.source_branch.as_deref(),
                Some("styra/source-work")
            );
        });
        app.checkout_from = None;
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!(
                e.list.chrome.source_branch.as_deref(),
                Some("Workspace branch")
            );
        });
        app.session_id = "source-session".into();
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(e.list.chrome.source_branch.is_none());
        });
    }

    #[test]
    fn entering_input_supplies_message_box_data() {
        let mut app = test_support::app("s1");
        app.enter_input();

        inspect(&app, |v| {
            assert!(v.overlays.input.is_some());
        });
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

        inspect(&app, |v| {
            assert!(v.overlays.input.is_none());
            let recording = v.overlays.recording.expect("capture data");
            assert_eq!(
                (recording.level, recording.loudest, recording.gain),
                (0.4, 0.6, 2.0)
            );
        });
    }

    /// Creating a Git workspace blocks the loop, so while it runs the message
    /// box is replaced by a notice saying so: a box left on screen would offer
    /// typing that nothing reads.
    #[test]
    fn a_blocking_call_replaces_the_message_box_with_its_notice() {
        let mut app = test_support::app("");
        app.enter_input();
        app.composer.insert("start on the picker");
        app.busy = Some("creating a new Git workspace…".into());

        inspect(&app, |v| {
            assert_eq!(
                v.overlays.busy.as_ref().unwrap().message,
                "creating a new Git workspace…"
            );
        });
    }

    /// Every view's status line must name the model and effort in use, since
    /// the agent name alone does not say it and the session may be spending a
    /// model nobody typed.
    #[test]
    fn the_status_line_names_the_model_and_effort_in_use() {
        let mut app = test_support::app("s-1");
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some(test_support::MODEL.into()),
            effort: Some(test_support::EFFORT.into()),
        });
        for view in [
            View::Events,
            View::Raw,
            View::Log,
            View::Transcript,
            View::Driva,
        ] {
            app.view = view;
            inspect(&app, |v| {
                let chrome = match &v.main {
                    MainView::Events(e) => &e.list.chrome,
                    MainView::Raw(r) => &r.chrome,
                    MainView::Log { chrome, .. } => chrome,
                    MainView::Transcript(t) => &t.chrome,
                    MainView::Driva(d) => &d.chrome,
                    _ => panic!("unexpected view"),
                };
                assert_eq!(chrome.model, test_support::MODEL);
                assert_eq!(chrome.effort.as_deref(), Some(test_support::EFFORT));
            });
        }
    }

    #[test]
    fn checkout_diff_layout_feedback_reaches_the_host() {
        let mut app = test_support::app("s1");
        let mut diff = styra_diff::CheckoutDiff::read(styra_diff::DiffTarget {
            worktree: "/no-such-styra-diff-checkout".into(),
            branch: None,
            base_branch: None,
            base: "HEAD".into(),
            branch_point: false,
        });
        diff.diff = Ok("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into());
        diff.side_by_side = true;
        app.checkout_diff = Some(diff);
        app.view = View::CheckoutDiff;
        inspect(&app, |v| {
            assert!(matches!(v.main, MainView::CheckoutDiff(_)));
        });
        for split in [true, false, true] {
            let feedback = styra_ui::RenderFeedback {
                diff_layout: Some((split, 7)),
                ..Default::default()
            };
            apply_feedback(&mut app, &feedback);
            let diff = app.checkout_diff.as_ref().unwrap();
            assert_eq!((diff.rendered_side, diff.horizontal_scroll), (split, 7));
            assert!(diff.side_by_side);
        }
    }

    #[test]
    fn opening_a_side_diff_report_keeps_the_events_view() {
        let mut app = test_support::app("s1");
        let mut diff = styra_diff::CheckoutDiff::read(styra_diff::DiffTarget {
            worktree: "/no-such-styra-diff-checkout".into(),
            branch: None,
            base_branch: None,
            base: "HEAD".into(),
            branch_point: false,
        });
        diff.diff = Ok("diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old\n+new\n".into());
        diff.report.report = Some(styra_diff::Report::default());
        app.checkout_diff = Some(diff);
        app.diff_preview = true;
        crate::input::handle_diff_key(
            &mut app,
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('o')),
        );
        assert_eq!(app.view, View::Events, "the diff stays in its pane");
        assert!(app.checkout_diff.as_ref().unwrap().report_page.is_some());
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(matches!(
                e.preview,
                Some(styra_ui::application::SidePreview::CheckoutDiff(_))
            ));
        });
    }

    #[test]
    fn checkout_diff_is_dispatched_to_fullscreen_and_side_views() {
        let mut app = test_support::app("s1");
        app.checkout_diff = Some(styra_diff::CheckoutDiff {
            commits: Default::default(),
            source: styra_diff::Source::Checkout(styra_diff::DiffTarget {
                worktree: "/state/worktrees/styra-7".into(),
                branch: Some("styra/rename".into()),
                base_branch: Some("main".into()),
                base: "4bf5c35d".into(),
                branch_point: true,
            }),
            diff: Ok(
                "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n"
                    .into(),
            ),
            base_names: Vec::new(),
            scroll: Default::default(),
            combined_scroll: Default::default(),
            per_file: false,
            side_by_side: false,
            rendered_side: false,
            combined_side: false,
            help: None,
            horizontal_scroll: 0,
            hide_removed: false,
            compact_dirs: true,
            tint: Default::default(),
            word_highlights: true,
            layers: None,
            involved_layers: None,
            thumbnail_hidden: false,
            selected_file: 0,
            file_list_offset: 0,
            search: Default::default(),
            report: Default::default(),
            report_page: None,
            report_preview: false,
            line_cursor: None,
            find: Default::default(),
            find_from: None,
        });
        app.view = View::CheckoutDiff;

        inspect(&app, |v| {
            assert!(matches!(v.main, MainView::CheckoutDiff(_)));
        });
        for view in [View::Events, View::Files] {
            app.view = view;
            app.diff_preview = true;
            assert!(panel_chrome(&app, Some("diff")).focused);
            inspect(&app, |v| {
                let preview = match &v.main {
                    MainView::Events(e) => e.preview,
                    MainView::Files(f) => f.preview,
                    _ => panic!(),
                };
                assert!(matches!(
                    preview,
                    Some(styra_ui::application::SidePreview::CheckoutDiff(_))
                ));
            });
            app.diff_interaction_focus = true;
            assert!(!panel_chrome(&app, Some("diff")).focused);
            assert!(panel_chrome(&app, None).focused);
            app.diff_interaction_focus = false;
            app.enter_input();
            assert!(!panel_chrome(&app, Some("diff")).focused);
            app.enter_list();
        }
    }
}
