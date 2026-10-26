//! The terminal side of a round: painting the frame and waiting for a key.

use anyhow::Result;
use crossterm::event::{Event, KeyEvent, KeyEventKind};
use std::time::{Duration, Instant};

use super::keys::message_paste_active;
use super::Loop;
use crate::app::App;
use crate::audio::AudioInput;
use crate::presentation;
use styra_ui::Ui;

/// How long one round of the loop waits for a key before coming back to poll
/// the server for updates, interactions and quota.
pub(super) const KEY_POLL: Duration = Duration::from_millis(100);

/// How long a level-meter frame waits for one instead.
///
/// The loop's own period is the server's, and a meter repainted on it moves in
/// ten steps a second — slower than speech, so the bar lands on syllables
/// rather than following them and reads as a stutter rather than as a level.
/// These frames cost nothing but a repaint: they ask the device for the level
/// it has already measured, and nothing else about the loop's round happens in
/// them.
const METER_FRAME: Duration = Duration::from_millis(25);

/// How often a frame whose text is read off the clock is repainted for that
/// reason alone — see [`presentation::ticking`](crate::presentation::ticking).
///
/// One second, because that is the resolution those figures are written at:
/// an elapsed count in whole seconds gains nothing from being repainted twice
/// within one, and loses a beat if it is repainted less often than that.
const REDRAW_TICK: Duration = Duration::from_secs(1);

impl Loop<'_> {
    /// The parts of a frame that are read off the clock have nothing to
    /// announce them, so they are read on their own second rather than on
    /// every round — and the frame is repainted only where that reading
    /// differs from the one already on screen. A clock that has not moved
    /// since the last frame is not a reason to paint another one, however
    /// often it is consulted: see `presentation::clock_reading`.
    pub(super) fn tick_clock(&mut self) {
        if self.ticked.elapsed() < REDRAW_TICK {
            return;
        }
        self.ticked = Instant::now();
        let reading = presentation::clock_reading(self.app);
        if reading != self.clock {
            // A frame repainted for nothing but the clock is the one kind
            // the trace cannot otherwise account for, and "why is it still
            // rendering" is exactly what this log gets read to answer. The
            // reading itself is logged, so consecutive lines show what
            // moved.
            tracing::debug!(
                target: "styra_tui::render",
                pid = std::process::id(),
                session_id = %self.app.session_id,
                moving = ?reading,
                "repainting for the clock"
            );
            self.clock = reading;
            self.dirty = true;
        }
    }

    /// Paint the frame, if anything on it has changed.
    pub(super) fn draw(&mut self) -> Result<()> {
        // A capture paints its own frames at the meter's rate, so the round's
        // own frame would be one more painting of what is already there.
        if !self.dirty || self.audio.is_recording() {
            return Ok(());
        }
        let app = &mut *self.app;
        if app.help.is_open() {
            // The reference is for the window underneath it, which cannot
            // change while it is open, so it is read from the app rather than
            // remembered when `?` was pressed.
            let window = presentation::current_window(app);
            let rows = presentation::help_rows(window);
            let feedback = self.terminal.render_help(
                window.name(),
                &rows,
                &crate::keybindings::CLOSE_REFERENCE.label(),
                app.help.offset(),
            )?;
            if let Some(scroll) = feedback
                .scroll
                .iter()
                .find(|scroll| scroll.panel == styra_ui::PanelId::Help)
            {
                app.help
                    .apply_feedback(scroll.limit, scroll.effective_offset);
            }
        } else if let Some(picker) = &app.template_picker {
            match &picker.templates {
                Some(templates) => {
                    self.terminal.render_template_picker(
                        templates,
                        &picker.chosen,
                        picker.cursor,
                    )?;
                }
                None => {
                    self.terminal.render_template_picker_loading()?;
                }
            }
        } else {
            let feedback = presentation::draw_application(self.terminal, app)?;
            presentation::apply_feedback(app, &feedback);
        }
        self.dirty = false;
        Ok(())
    }

    /// Wait out the round for the next key press: one the action menu chose,
    /// or one from the terminal. Pastes and resizes are taken care of here;
    /// anything else that is not a press ends the round with nothing to do.
    pub(super) fn next_key(&mut self) -> Result<Option<KeyEvent>> {
        let paste_active = message_paste_active(self.app) && !self.audio.is_recording();
        if self.bracketed_paste != paste_active {
            self.terminal.set_bracketed_paste(paste_active)?;
            self.bracketed_paste = paste_active;
        }
        let waited = if let Some(key) = self.replay.pop_front() {
            Some(Event::Key(key))
        } else if self.audio.is_recording() {
            meter_frames(self.terminal, self.app, &mut self.audio)?
        } else {
            self.terminal.poll_event(KEY_POLL)?
        };
        Ok(match waited {
            // A terminal that changed size has to be repainted, and nothing
            // about the application changed to say so.
            Some(Event::Resize(..)) => {
                self.dirty = true;
                None
            }
            Some(Event::Paste(text)) => {
                if paste_active {
                    self.app.composer.paste(&text);
                    self.dirty = true;
                }
                None
            }
            Some(Event::Key(key)) if key.kind == KeyEventKind::Press => Some(key),
            _ => None,
        })
    }
}

/// Wait out one round's [`KEY_POLL`] in short frames, repainting the level
/// meter in each — or return as soon as a key arrives, which is the loop's
/// real business.
///
/// Only called while a capture is running, and a capture is only ever running
/// with the message box up: no other window can be on screen to repaint the
/// wrong thing over.
fn meter_frames(
    terminal: &mut dyn Ui,
    app: &mut App,
    audio: &mut AudioInput,
) -> Result<Option<Event>> {
    let until = Instant::now() + KEY_POLL;
    loop {
        if let Some(event) = terminal.poll_event(METER_FRAME)? {
            return Ok(Some(event));
        }
        if Instant::now() >= until {
            return Ok(None);
        }
        audio.note_level(app);
        let feedback = presentation::draw_application(terminal, app)?;
        presentation::apply_feedback(app, &feedback);
    }
}

/// Run a synchronous call the loop has to wait out, behind [`App::busy`].
///
/// Nothing reads the keyboard until `work` returns, so the notice takes the
/// message box's place and is painted before the call is made. Keys pressed
/// under it are thrown away afterwards rather than answered late — least of
/// all by the event list, where a stray `q` quits.
pub(super) fn blocked_on<T>(
    terminal: &mut dyn Ui,
    app: &mut App,
    notice: &str,
    work: impl FnOnce(&mut App) -> T,
) -> Result<T> {
    app.busy = Some(notice.to_owned());
    presentation::draw_application(terminal, app)?;
    let done = work(app);
    app.busy = None;
    while terminal.poll_event(Duration::ZERO)?.is_some() {}
    Ok(done)
}
