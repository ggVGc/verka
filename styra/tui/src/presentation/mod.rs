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
use std::time::Duration;
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
    use crate::launcher::LaunchColumn;
    use styra_ui::launcher::LauncherColumn;

    let provider = launcher.provider();
    styra_ui::launcher::LauncherView {
        selection: launcher.selection().name(),
        provider_locked: launcher.provider_locked,
        providers: styra_protocol::agent::PROVIDERS
            .iter()
            .map(|provider| provider.as_str().to_owned())
            .collect(),
        models: launcher.models(),
        efforts: provider
            .efforts()
            .iter()
            .map(|effort| effort.as_str().to_owned())
            .collect(),
        provider_selected: launcher.provider,
        model_selected: launcher.model,
        effort_selected: launcher.effort,
        focused: match launcher.column {
            LaunchColumn::Provider => LauncherColumn::Provider,
            LaunchColumn::Model => LauncherColumn::Model,
            LaunchColumn::Effort => LauncherColumn::Effort,
        },
    }
}

pub(crate) fn help_rows() -> Vec<styra_ui::help::HelpRow<'static>> {
    crate::keymap::REFERENCE
        .iter()
        .map(|row| match row {
            crate::keymap::ReferenceRow::Section(name) => styra_ui::help::HelpRow::Section(name),
            crate::keymap::ReferenceRow::Binding { keys, action } => {
                styra_ui::help::HelpRow::Binding { keys, action }
            }
            crate::keymap::ReferenceRow::Blank => styra_ui::help::HelpRow::Blank,
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
    styra_ui::modal_input::ModalInput {
        title,
        note: app
            .outbox
            .contract()
            .map(|contract| format!(" asking for {} ", contract.as_str())),
        preceding,
        notice: None,
        text: &app.composer.text,
        placeholder: "type a message, Enter to send",
        cursor: app.focus == Focus::Input,
    }
}

/// Prepare and draw the main application without exposing `App` or Ratatui to
/// the UI trait. Derived strings and file contents live for this draw only;
/// event histories and protocol records remain borrowed.
pub(crate) fn draw_application(ui: &mut dyn Ui, app: &App) -> UiResult<styra_ui::RenderFeedback> {
    use styra_ui::application::{EventView, FilesView as ApplicationFiles, MainView};
    match app.view {
        View::Events => {
            let list = list::view(app);
            let navigator = app.interactions.open.then(|| interactions::view(app));
            let entry_log = app.entry_log_open.then(|| entry_log::view(app));
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
    }
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
        help_key: crate::keymap::HELP,
        working_directory: &working_directory,
        worktrees_enabled: app.workspace.worktrees_enabled,
        idle_interactions: app.interactions.idle_notification_count(),
        quota: &quota_alert,
        auto_retry: app.auto_retry,
    };
    let input = (app.focus == Focus::Input).then(|| modal_input(app));
    let reference_labels = app.references.as_ref().map(|references| {
        references
            .items()
            .iter()
            .map(|reference| reference.label())
            .collect::<Vec<_>>()
    });
    let reference_items =
        app.references
            .as_ref()
            .zip(reference_labels.as_ref())
            .map(|(references, labels)| {
                references
                    .items()
                    .iter()
                    .zip(labels)
                    .map(|(reference, label)| styra_ui::overlays::ReferenceView {
                        label,
                        line: reference.line,
                    })
                    .collect::<Vec<_>>()
            });
    let references =
        app.references
            .as_ref()
            .zip(reference_items.as_ref())
            .map(|(references, items)| styra_ui::overlays::ReferencesView {
                items,
                selected: references.selected_index(),
            });
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
            input: input.as_ref(),
            references,
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
                app.launch.scroll.offset = scroll.effective_offset;
                app.launch.scroll.note_limit(scroll.limit);
            }
            styra_ui::PanelId::EntryLog => {
                app.entry_log.offset = scroll.effective_offset;
                app.entry_log.note_limit(scroll.limit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{self, rendered};
    use super::*;
    use styra_protocol::event::{AgentEvent, TurnOutcome, TurnUsage};

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
