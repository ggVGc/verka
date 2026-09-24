//! Recording and transcription for the message editor.
//!
//! Audio capture is an ordinary host-side input operation, not part of an
//! interaction's sandbox. The default input device is opened in this process
//! and its samples are written to a temporary WAV file; once stopped, the
//! server transcribes that file with its own local Whisper model and returns
//! only the text.
//!
//! The device is opened when the message box is, and not when recording
//! starts. Opening an input is the slow part — and on a Bluetooth headset it
//! is slower still, because the profile switch that gives the machine a
//! microphone happens then: seconds of it, during which the operator is
//! already speaking. So the box taking focus arms the device, recording only
//! begins writing what an already-open device is handing over, and the device
//! is let go of when the box closes.
//!
//! The device is owned by a thread of its own. A capture stream cannot be
//! moved between threads, and the terminal thread must never be the one
//! waiting on an input that has stopped answering — so the thread that opened
//! the device is also the one that closes it and finishes the file, and the
//! thread asking for a transcript waits on that with a deadline.
//!
//! An operator who pressed record twice must end up with either a transcript
//! or a reason, never with a notice that simply never turns into anything.

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream};
use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempPath;

use crate::app::{App, Focus};
use styra_protocol::LogEntry;
use styra_server::Client;

/// How long a stopped capture is given to close the device and finish its file.
const STOP_DEADLINE: Duration = Duration::from_secs(3);

/// What the device is asked for before it is asked what it has: the mono
/// 16 kHz the transcriber works in, at the backend's own buffer size.
const PREFERRED: cpal::StreamConfig = cpal::StreamConfig {
    channels: 1,
    sample_rate: cpal::SampleRate(16_000),
    buffer_size: cpal::BufferSize::Default,
};

/// A WAV header this size or smaller carries no audio at all.
const EMPTY_WAVE: u64 = 44;

/// Whether a recording outlives the transcription it was made for.
///
/// Temporarily true while the capture path is being tuned: a transcript that
/// comes back empty or wrong is only diagnosable against the audio it was made
/// from, and until now that audio was deleted before anyone could listen to
/// it. Each recording's path is logged as it is kept. Setting this back to
/// `false` restores the old behavior, where a recording lives exactly as long
/// as the request that transcribes it.
const KEEP_RECORDINGS: bool = true;

/// The boosts the meter's control steps through. A ladder rather than a
/// multiplier so the figure on screen is one an operator can aim at and come
/// back to: a microphone that needs ×4 needs it again tomorrow.
const GAINS: [f32; 10] = [1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0];

/// How much of the level survives a tenth of a second with nothing arriving.
/// Without a fall the bar would flicker between syllables instead of dropping
/// through them.
///
/// Counted in time rather than in frames because the meter is repainted far
/// more often than the event loop comes round: a fall of so much per frame
/// would empty the bar at one rate while the microphone is open and at quite
/// another the moment the loop had server work to do.
const FALL_PER_100MS: f32 = 0.25;

type Writer = WavWriter<BufWriter<File>>;

/// What the device or the file said went wrong, if either did.
type Fault = Arc<Mutex<Option<String>>>;

/// The device thread's side of the microphone: an open input, handing its
/// buffers to a file whenever there is one to write to.
struct Capture {
    /// Held so the device stays open; dropping it is what closes the input.
    _stream: Stream,
    /// What the device is actually giving, which is what a recording's file
    /// has to be written as — settled when the device opened, and so known
    /// before any recording asks for it.
    spec: WavSpec,
    /// `Some` only while a recording is running: the callback writes into it
    /// when it is there and returns when it is not, so arming the device costs
    /// nothing but the device.
    writer: Arc<Mutex<Option<Writer>>>,
    /// The first complaint from the device or from writing, kept rather than
    /// discarded: when a capture produces nothing, the reason ("Device or
    /// resource busy", "No such device") is only ever said here.
    fault: Fault,
}

/// What the editor asks the open device for.
enum Command {
    /// Begin writing what the device is handing over into this file.
    Start(PathBuf),
    /// Stop writing and finish the file, answering with whatever the device
    /// complained of along the way.
    Stop(Sender<Result<Option<String>>>),
}

/// The editor's side of the open device: somewhere to send commands, the
/// levels coming back, and whatever killed the device thread if something did.
struct Mic {
    command: Sender<Command>,
    meter: Arc<Meter>,
    /// One message, and the thread is gone with the device: opening failed, or
    /// a recording could not be started or finished. Read by
    /// [`AudioInput::check_capture`].
    trouble: Receiver<String>,
}

/// The editor's side of a running recording: the file being written, and the
/// meter of the device writing it.
struct Recording {
    path: TempPath,
    meter: Arc<Meter>,
}

/// What the capture thread and the interface say to each other while the
/// device is open: how loud the input is, and how much it is being boosted.
///
/// Atomics rather than a lock, because one end of this is an audio callback.
/// A callback that blocks on a mutex the terminal thread happens to hold drops
/// the buffer it was handed, and a dropped buffer is a missing word.
pub struct Meter {
    /// The loudest post-gain sample since the interface last looked, as the
    /// bits of an `f32`. Taken rather than read, so falling quiet shows as
    /// quiet rather than as the last loud thing said.
    peak: AtomicU32,
    /// What every sample is multiplied by on its way to the file.
    gain: AtomicU32,
    /// Whether a buffer has ever arrived from the device.
    ///
    /// Not the same question as whether the level is above zero: a device that
    /// is open and hearing a silent room reports silence, which is a recording,
    /// while a device still being opened reports nothing at all, which is not.
    /// Only the first is worth telling the operator to speak into.
    capturing: AtomicBool,
}

impl Meter {
    fn new() -> Self {
        Self {
            peak: AtomicU32::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            capturing: AtomicBool::new(false),
        }
    }

    /// Forget what an earlier recording heard, keeping the boost: the device
    /// outlives any one recording now, and the level and the "has it answered
    /// yet" of the last one are not this one's.
    fn restart(&self) {
        self.peak.store(0, Ordering::Relaxed);
        self.capturing.store(false, Ordering::Relaxed);
    }

    fn capturing(&self) -> bool {
        self.capturing.load(Ordering::Relaxed)
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Keep `peak` if it is the loudest the interface has not yet seen, and
    /// record that the device has begun answering at all.
    fn report(&self, peak: f32) {
        self.capturing.store(true, Ordering::Relaxed);
        let mut seen = self.peak.load(Ordering::Relaxed);
        while peak > f32::from_bits(seen) {
            match self.peak.compare_exchange_weak(
                seen,
                peak.to_bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(current) => seen = current,
            }
        }
    }

    /// The loudest sample since this was last called, and start again from
    /// silence.
    fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }
}

/// What the message box shows in place of itself while the microphone is open.
///
/// Lives on [`App`] rather than beside the device so the renderer can be given
/// it like any other view state; [`AudioInput::note_level`] is what keeps it
/// true.
pub struct Recorded {
    /// The bar's height now: the loudest recent sample, eased downwards.
    pub level: f32,
    /// The loudest sample of the whole recording so far — which is what says
    /// whether the input is too quiet to be worth transcribing.
    pub loudest: f32,
    /// The boost being applied, as a multiplier.
    pub gain: f32,
    /// When the device began handing this recording's buffers over, or `None`
    /// until it has.
    ///
    /// This, rather than when the key was pressed, is when the recording
    /// started. With the device already open it is the very next buffer, which
    /// is the point of opening it with the message box; with a device still
    /// opening it is however long that takes, and an operator who starts
    /// speaking into that gap loses the beginning of what they said. The
    /// elapsed figure is counted from here for the same reason — it is the
    /// length of the audio, not of the wait.
    pub capturing_since: Option<Instant>,
    /// When the level was last taken, which is what the fall is measured from.
    updated: Instant,
}

impl Recorded {
    /// A capture in a stated condition, for a rendering test that has no
    /// microphone to put into one.
    #[cfg(test)]
    pub fn for_test(level: f32, loudest: f32, gain: f32) -> Self {
        Self {
            level,
            loudest,
            gain,
            capturing_since: Some(Instant::now()),
            updated: Instant::now(),
        }
    }

    fn new() -> Self {
        Self {
            level: 0.0,
            loudest: 0.0,
            gain: 1.0,
            capturing_since: None,
            updated: Instant::now(),
        }
    }

    /// How much of the current level survives `since` having passed with
    /// nothing louder arriving.
    fn fall(&self, now: Instant) -> f32 {
        let tenths = (now - self.updated).as_secs_f32() * 10.0;
        FALL_PER_100MS.powf(tenths)
    }
}

/// Let go of a finished recording's file: kept on disk, or deleted with the
/// temporary path, according to [`KEEP_RECORDINGS`].
///
/// The error is returned rather than logged because this runs on a worker,
/// which has no interface to say anything to.
fn release(path: TempPath) -> std::result::Result<(), String> {
    if !KEEP_RECORDINGS {
        // Dropping the temporary path is what deletes the file.
        return Ok(());
    }
    path.keep()
        .map(|_| ())
        .map_err(|error| format!("could not keep the recording: {error}"))
}

/// Say where a recording is being left, for an operator who wants to listen to
/// what the transcriber was given. Silent unless [`KEEP_RECORDINGS`] holds,
/// since otherwise the file is gone by the time the line could be read.
fn note_kept(app: &mut App, recording: &Recording) {
    if !KEEP_RECORDINGS {
        return;
    }
    app.push_log(LogEntry::info(format!(
        "keeping the recording at {}",
        recording.path.display()
    )));
}

/// The boost one step above `gain`, or `gain` at the top of the ladder.
fn louder(gain: f32) -> f32 {
    *GAINS
        .iter()
        .find(|step| **step > gain + f32::EPSILON)
        .unwrap_or(&GAINS[GAINS.len() - 1])
}

/// The boost one step below `gain`, or unboosted at the bottom of it.
fn quieter(gain: f32) -> f32 {
    *GAINS
        .iter()
        .rev()
        .find(|step| **step < gain - f32::EPSILON)
        .unwrap_or(&GAINS[0])
}

enum Completed {
    Transcript(String),
    Failed(String),
    /// Something the interaction log should carry but the operator need not be
    /// interrupted with — a recording that could not be kept.
    Noted(String),
}

/// The one active microphone capture and any transcriptions completing in the
/// background. Whisper takes seconds on a long recording — and longer still
/// on the first request, which loads the model — so it must not own the
/// terminal thread while the rest of the interface is live.
pub struct AudioInput {
    /// The open device, held for as long as the message box is.
    mic: Option<Mic>,
    recording: Option<Recording>,
    /// Whether the device refused the last time it was asked for, which stops
    /// the box from asking again every round. Cleared when the box closes, and
    /// when the operator presses record — which is them asking again.
    refused: bool,
    completed: std::sync::mpsc::Sender<Completed>,
    receive: Receiver<Completed>,
}

impl AudioInput {
    pub fn new() -> Self {
        let (completed, receive) = mpsc::channel();
        Self {
            mic: None,
            recording: None,
            refused: false,
            completed,
            receive,
        }
    }

    /// Open the device because the message box is open, or let go of it
    /// because the box has closed.
    ///
    /// This, rather than the record key, is what pays for opening an input:
    /// by the time the operator has typed what they were going to type and
    /// decided to speak the rest, the device — Bluetooth profile switch and
    /// all — has long since answered.
    pub(crate) fn follow_focus(&mut self, app: &mut App) {
        match app.focus {
            Focus::Input => self.arm(app),
            // A recording keeps the box, so this is not a case of dropping the
            // device out from under one; the guard is for a focus change no
            // one has written yet.
            Focus::List if self.recording.is_none() => self.release(),
            Focus::List => {}
        }
    }

    /// Have the device open and answering, if it is not already.
    ///
    /// Failing to arm is not worth interrupting anybody over: nobody has asked
    /// to record yet, and the operator who does is told then. It goes in the
    /// log, and the box stops asking until it is opened again.
    fn arm(&mut self, app: &mut App) {
        if self.mic.is_some() || self.refused {
            return;
        }
        match open_mic() {
            Ok(mic) => self.mic = Some(mic),
            Err(error) => {
                self.refused = true;
                app.push_log(LogEntry::warn(format!(
                    "could not open the audio input: {error:#}"
                )));
            }
        }
    }

    /// Close the device. Dropping the command channel is what tells the device
    /// thread to; nothing waits for it to have happened.
    fn release(&mut self) {
        self.mic = None;
        self.refused = false;
    }

    /// Whether a recording is running — not merely whether the device is open,
    /// which it is for as long as the message box is. This is what puts the
    /// meter on screen in the box's place and gives it the keyboard.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Start a capture, or finish the current one and hand it to the server.
    pub fn toggle(&mut self, app: &mut App, client: Client) {
        if self.recording.is_some() {
            self.finish(app, client);
            return;
        }
        // The operator asking to record is the operator asking for the device
        // again, whatever it said last time the box opened.
        self.refused = false;
        self.arm(app);
        match self.start() {
            Ok(recording) => {
                self.recording = Some(recording);
                app.recording = Some(Recorded::new());
                if let Err(error) = client.audio_recording_started() {
                    app.push_log(LogEntry::error(format!(
                        "could not report started audio recording: {error:#}"
                    )));
                }
            }
            Err(error) => {
                let message = format!("could not start audio recording: {error:#}");
                let _ = client.audio_transcription_error(&message);
                app.push_log(LogEntry::error(message.clone()));
                app.show_action_message(message);
            }
        }
    }

    /// Stop the capture and transcribe what it recorded.
    pub fn finish(&mut self, app: &mut App, client: Client) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        app.recording = None;
        // Only ask here: waiting for the file to be finished, and giving up
        // when it is not, is the worker's job — this is the terminal thread.
        let finished = self.stop();
        self.report_stopped(app, &client);
        note_kept(app, &recording);
        self.transcribe(recording.path, finished, client);
        app.show_action_message("recording stopped — transcribing…");
    }

    /// Stop the capture and transcribe nothing.
    ///
    /// The file still has to be finished before it can be thrown away, and the
    /// device could take its time about it — so the ending is handed to a
    /// worker rather than waited for here, for the same reason stopping is.
    pub fn cancel(&mut self, app: &mut App, client: Client) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        app.recording = None;
        let finished = self.stop();
        self.report_stopped(app, &client);
        note_kept(app, &recording);
        let completed = self.completed.clone();
        let path = recording.path;
        std::thread::Builder::new()
            .name("styra-audio-discard".into())
            .spawn(move || {
                let _ = finished.recv_timeout(STOP_DEADLINE);
                // A cancelled recording is released the same way a transcribed
                // one is — kept or deleted by the flag, not by which key ended
                // it — and released here rather than on the terminal thread so
                // a device that will not let go cannot hold the interface.
                if let Err(error) = release(path) {
                    let _ = completed.send(Completed::Noted(error));
                }
            })
            .expect("spawning audio discard worker");
        app.show_action_message("recording cancelled");
    }

    /// Raise the boost applied to the samples on their way to the file.
    ///
    /// The gain is the capture's, not the meter's: a quiet microphone produces
    /// a quiet file, and a file the transcriber can barely hear is the thing
    /// being fixed. Live, because the operator is already speaking — the
    /// callback picks the new figure up on its next buffer.
    pub fn boost(&self, app: &mut App) {
        self.set_gain(app, louder(self.gain()));
    }

    /// Lower the boost, for an input that is loud enough to clip.
    pub fn quieten(&self, app: &mut App) {
        self.set_gain(app, quieter(self.gain()));
    }

    /// Point the open device at a file of its own and let it start writing.
    ///
    /// Nothing here waits for the device: if it is still opening, the command
    /// is read the moment it has opened, and the operator sees the recording
    /// begin then. A device that never opens is taken back by
    /// [`AudioInput::check_capture`].
    fn start(&mut self) -> Result<Recording> {
        let mic = self.mic.as_ref().context("the audio input is not open")?;
        let path = tempfile::Builder::new()
            .prefix("styra-recording-")
            .suffix(".wav")
            .tempfile()
            .context("creating a temporary WAV file")?
            .into_temp_path();
        mic.meter.restart();
        mic.command
            .send(Command::Start(path.to_path_buf()))
            .map_err(|_| anyhow!("the audio input closed itself"))?;
        Ok(Recording {
            path,
            meter: Arc::clone(&mic.meter),
        })
    }

    /// Ask the device to finish the file it is writing. The answer — what the
    /// device complained of, if anything — comes back on the returned channel,
    /// which a worker waits on with a deadline.
    ///
    /// A device that has already gone leaves the channel's other end dropped,
    /// which is read as the capture having ended without finishing.
    fn stop(&self) -> Receiver<Result<Option<String>>> {
        let (done, finished) = mpsc::channel();
        if let Some(mic) = self.mic.as_ref() {
            let _ = mic.command.send(Command::Stop(done));
        }
        finished
    }

    fn gain(&self) -> f32 {
        self.recording
            .as_ref()
            .map(|recording| recording.meter.gain())
            .unwrap_or(1.0)
    }

    fn set_gain(&self, app: &mut App, gain: f32) {
        let Some(recording) = self.recording.as_ref() else {
            return;
        };
        recording.meter.set_gain(gain);
        if let Some(recorded) = app.recording.as_mut() {
            recorded.gain = gain;
        }
    }

    fn report_stopped(&self, app: &mut App, client: &Client) {
        if let Err(error) = client.audio_recording_stopped() {
            app.push_log(LogEntry::error(format!(
                "could not report stopped audio recording: {error:#}"
            )));
        }
    }

    fn transcribe(
        &mut self,
        path: TempPath,
        finished: Receiver<Result<Option<String>>>,
        client: Client,
    ) {
        let completed = self.completed.clone();
        std::thread::Builder::new()
            .name("styra-audio-transcript".into())
            .spawn(move || {
                let result = match finish_recording(path, &finished) {
                    Ok(path) => {
                        let transcript = client
                            .transcribe_audio(&path)
                            .map(Completed::Transcript)
                            // A request that reached the server was logged
                            // there by its handler before the error came back.
                            .unwrap_or_else(|error| Completed::Failed(format!("{error:#}")));
                        // After the transcription, not before it: the file has
                        // to be there to be read, whichever of the two fates
                        // the flag gives it afterwards.
                        if let Err(error) = release(path) {
                            let _ = completed.send(Completed::Noted(error));
                        }
                        transcript
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        // Capture errors happen wholly on the TUI side, so
                        // report them explicitly for the server stdout/log.
                        let _ = client.audio_transcription_error(&message);
                        Completed::Failed(message)
                    }
                };
                let _ = completed.send(result);
            })
            .expect("spawning audio transcription worker");
    }

    /// Insert every finished transcript at the message cursor (the composer is
    /// append-only today) and surface failures in the normal interaction log.
    pub fn apply_ready(&mut self, app: &mut App, client: &Client) {
        self.check_capture(app, client);
        self.note_level(app);
        while let Ok(result) = self.receive.try_recv() {
            match result {
                Completed::Transcript(transcript) => {
                    app.composer.insert(transcript.trim());
                    app.show_action_message("audio transcript inserted");
                }
                Completed::Failed(error) => {
                    app.push_log(LogEntry::error(format!(
                        "audio transcription failed: {error}"
                    )));
                    // The log view is not the one an operator is looking at
                    // while they wait for their words to appear, and a failure
                    // seen only there is indistinguishable from a transcript
                    // that never arrives. Say it where the notice was.
                    app.show_action_message(format!(
                        "audio transcription failed: {}",
                        first_line(&error)
                    ));
                }
                Completed::Noted(message) => app.push_log(LogEntry::warn(message)),
            }
        }
    }

    /// Move the meter on: take what the device has been hearing since the last
    /// frame and let the bar fall towards it.
    ///
    /// Called both on the loop's own round and between its rounds, while the
    /// meter is being repainted; see `event_loop`.
    pub(crate) fn note_level(&mut self, app: &mut App) {
        let Some(recording) = self.recording.as_ref() else {
            return;
        };
        let Some(recorded) = app.recording.as_mut() else {
            return;
        };
        let now = Instant::now();
        let peak = recording.meter.take_peak();
        recorded.level = peak.max(recorded.level * recorded.fall(now));
        recorded.updated = now;
        recorded.loudest = recorded.loudest.max(peak);
        recorded.gain = recording.meter.gain();
        if recorded.capturing_since.is_none() && recording.meter.capturing() {
            recorded.capturing_since = Some(now);
        }
    }

    /// Take back a capture whose device went away.
    ///
    /// The notice says recording started the moment the key was pressed,
    /// because waiting for the device to answer is exactly the delay this
    /// avoids. The other side of that promise is this: a device that turns out
    /// not to open has to be said so here, rather than leaving the interface
    /// claiming to record until the operator presses stop and finds out.
    ///
    /// The device thread says nothing until something goes wrong, and it says
    /// it once before going: anything on `trouble` is the microphone gone.
    fn check_capture(&mut self, app: &mut App, client: &Client) {
        let Some(mic) = self.mic.as_ref() else {
            return;
        };
        let failure = match mic.trouble.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return,
            Ok(failure) => failure,
            Err(mpsc::TryRecvError::Disconnected) => {
                "the audio capture ended on its own".to_owned()
            }
        };
        self.mic = None;
        // Asked for and refused: stop asking until the box is opened again, or
        // until the operator presses record.
        self.refused = true;
        let Some(recording) = self.recording.take() else {
            // Nobody was recording, so nobody is owed an interruption — but
            // the next press of the record key will fail too, and this is the
            // reason it will.
            app.push_log(LogEntry::warn(format!("the audio input closed: {failure}")));
            return;
        };
        drop(recording);
        app.recording = None;
        let message = format!("could not start audio recording: {failure}");
        let _ = client.audio_transcription_error(&message);
        let _ = client.audio_recording_stopped();
        app.push_log(LogEntry::error(message.clone()));
        app.show_action_message(first_line(&message).to_owned());
    }
}

impl Drop for AudioInput {
    fn drop(&mut self) {
        if let Some(recording) = self.recording.take() {
            // Nothing will read this recording, but the device is the
            // machine's and is owed a close before the process goes.
            let _ = self.stop().recv_timeout(STOP_DEADLINE);
            // Released by the same rule as every other ending, so a recording
            // interrupted by the interface going down is kept if the rest are.
            let _ = release(recording.path);
        }
    }
}

/// Open the device, without waiting for it to have opened.
///
/// Opening an input costs anything from a few milliseconds to the better part
/// of a second — and several seconds on a headset the machine has to take from
/// playback to a profile with a microphone in it. None of that may be spent on
/// the terminal thread: the operator pressed a key to open the message box and
/// the interface has to answer. A device that then refuses to open is picked
/// up by [`AudioInput::apply_ready`].
fn open_mic() -> Result<Mic> {
    let (command, commands) = mpsc::channel();
    let (said, trouble) = mpsc::channel();
    let meter = Arc::new(Meter::new());
    let heard = Arc::clone(&meter);
    std::thread::Builder::new()
        .name("styra-audio-capture".into())
        .spawn(move || {
            if let Err(error) = hold_device(&commands, &heard) {
                let _ = said.send(format!("{error:#}"));
            }
        })
        .context("starting the audio capture thread")?;
    Ok(Mic {
        command,
        meter,
        trouble,
    })
}

/// Keep the device open, writing whatever it hands over into whichever file
/// the editor has asked for, until the editor lets go of the microphone.
///
/// The device is opened, written and closed all on this one thread because a
/// capture stream cannot be moved off the thread that made it.
fn hold_device(commands: &Receiver<Command>, meter: &Arc<Meter>) -> Result<()> {
    let capture = open(meter)?;
    // Runs until the editor drops the command channel, which is the message
    // box closing — or the interface going down around it.
    while let Ok(command) = commands.recv() {
        match command {
            Command::Start(path) => capture.begin(&path)?,
            Command::Stop(done) => {
                let _ = done.send(capture.end());
            }
        }
    }
    // Dropping the capture is what closes the device.
    Ok(())
}

/// Open the default input device and start it running.
fn open(meter: &Arc<Meter>) -> Result<Capture> {
    let device = cpal::default_host()
        .default_input_device()
        .context("no default audio input device")?;
    // Asking a device what it supports means opening it and testing every
    // format, channel count and sample rate in turn, and only then opening it
    // again for the stream itself — on a PipeWire or PulseAudio bridge that
    // doubles the wait before the first sample, which an operator hears as the
    // beginning of what they said going missing. Mono 16 kHz is what the
    // transcriber wants anyway and what a default device converts to, so ask
    // for it outright and only enumerate when it is refused.
    if let Ok(capture) = capture::<i16>(&device, &PREFERRED, meter) {
        return Ok(capture);
    }
    let config = device
        .default_input_config()
        .context("reading the default audio input's configuration")?;
    // Whatever the device does have is recorded as it is: the transcriber
    // resamples, and asking for a format a device lacks is a way to fail at
    // opening it at all.
    let format = config.sample_format();
    let config = config.into();
    match format {
        SampleFormat::I8 => capture::<i8>(&device, &config, meter),
        SampleFormat::I16 => capture::<i16>(&device, &config, meter),
        SampleFormat::I32 => capture::<i32>(&device, &config, meter),
        SampleFormat::I64 => capture::<i64>(&device, &config, meter),
        SampleFormat::U8 => capture::<u8>(&device, &config, meter),
        SampleFormat::U16 => capture::<u16>(&device, &config, meter),
        SampleFormat::U32 => capture::<u32>(&device, &config, meter),
        SampleFormat::U64 => capture::<u64>(&device, &config, meter),
        SampleFormat::F32 => capture::<f32>(&device, &config, meter),
        SampleFormat::F64 => capture::<f64>(&device, &config, meter),
        other => bail!("the default audio input uses an unsupported sample format ({other})"),
    }
}

/// Run `device`, ready to write its `T` samples as 16-bit PCM into whichever
/// file a recording later asks for.
fn capture<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    meter: &Arc<Meter>,
) -> Result<Capture>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let spec = WavSpec {
        channels: config.channels,
        sample_rate: config.sample_rate.0,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    // Empty: the device runs from here on, but until a recording starts there
    // is nothing for its buffers to go into, and the callback drops them.
    let writer = Arc::new(Mutex::new(None));
    let fault: Fault = Arc::new(Mutex::new(None));
    let stream = record::<T>(device, config, &writer, &fault, meter)?;
    stream.play().context("starting the default audio input")?;
    Ok(Capture {
        _stream: stream,
        spec,
        writer,
        fault,
    })
}

/// Build the stream that writes `T` samples into the file as 16-bit PCM,
/// boosted by whatever the meter's gain currently is and measured on the way
/// past.
fn record<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    writer: &Arc<Mutex<Option<Writer>>>,
    fault: &Fault,
    meter: &Arc<Meter>,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let sink = Arc::clone(writer);
    let written = Arc::clone(fault);
    let reported = Arc::clone(fault);
    let heard = Arc::clone(meter);
    device
        .build_input_stream(
            config,
            move |samples: &[T], _: &cpal::InputCallbackInfo| {
                // Read once per buffer rather than per sample: a boost applied
                // halfway through a buffer would be a step in the waveform.
                let gain = heard.gain();
                let mut peak = 0.0f32;
                let Ok(mut sink) = sink.lock() else { return };
                let Some(writer) = sink.as_mut() else { return };
                for sample in samples {
                    let value = f32::from_sample(*sample) * gain;
                    // Measured before the clamp, so an input loud enough to
                    // clip reads as over the top of the bar rather than as
                    // merely full.
                    peak = peak.max(value.abs());
                    if let Err(error) = writer.write_sample(pcm(value)) {
                        // The file is no longer being written to; say why once
                        // and stop, rather than repeat it every buffer.
                        remember(&written, format!("writing the recording failed: {error}"));
                        sink.take();
                        return;
                    }
                }
                heard.report(peak);
            },
            move |error| remember(&reported, error.to_string()),
            None,
        )
        .context("opening the default audio input")
}

/// A boosted sample as 16-bit PCM. Boosting is what makes a clamp necessary:
/// past full scale there is nothing to record but the loudest value there is.
fn pcm(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

/// Keep the first thing that went wrong; later ones are usually its echo.
fn remember(fault: &Fault, said: String) {
    if let Ok(mut fault) = fault.lock() {
        fault.get_or_insert(said);
    }
}

impl Capture {
    /// Give the running device a file to write into, which is what makes it a
    /// recording. Anything the device complained of before now belonged to the
    /// last one and is forgotten here.
    fn begin(&self, path: &Path) -> Result<()> {
        let writer = WavWriter::create(path, self.spec)
            .with_context(|| format!("creating the recording {}", path.display()))?;
        self.take_fault()?;
        *self
            .writer
            .lock()
            .map_err(|_| anyhow!("the audio capture was interrupted"))? = Some(writer);
        Ok(())
    }

    /// Finish the WAV file and report what the device said while it was being
    /// written. The device itself stays open: the message box is still up, and
    /// the next recording should not pay for opening it again.
    fn end(&self) -> Result<Option<String>> {
        // Taking the writer out under the lock is what stops the callback
        // writing; it can only be holding the lock or not holding it, and
        // either way it is done with the file before `finalize` sees it.
        let writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("the audio capture was interrupted"))?
            .take();
        if let Some(writer) = writer {
            writer
                .finalize()
                .context("finishing the recorded WAV file")?;
        }
        self.take_fault()
    }

    /// What has gone wrong since this was last asked, and a clean slate for
    /// the next recording.
    fn take_fault(&self) -> Result<Option<String>> {
        Ok(self
            .fault
            .lock()
            .map_err(|_| anyhow!("the audio capture was interrupted"))?
            .take())
    }
}

fn finish_recording(
    path: TempPath,
    finished: &Receiver<Result<Option<String>>>,
) -> Result<TempPath> {
    finish_within(path, finished, STOP_DEADLINE)
}

/// [`finish_recording`], with the wait named — so a test can spend
/// milliseconds on a device that will not let go rather than the seconds a
/// live one is owed.
fn finish_within(
    path: TempPath,
    finished: &Receiver<Result<Option<String>>>,
    deadline: Duration,
) -> Result<TempPath> {
    let fault = match finished.recv_timeout(deadline) {
        Ok(finished) => finished?,
        Err(RecvTimeoutError::Timeout) => {
            bail!("the audio input did not stop within {deadline:?}")
        }
        Err(RecvTimeoutError::Disconnected) => {
            bail!("the audio capture ended without finishing the recording")
        }
    };
    let size = std::fs::metadata(&path)
        .context("reading the recorded audio file")?
        .len();
    if size <= EMPTY_WAVE {
        match fault {
            Some(fault) => bail!("the default input produced no audio: {fault}"),
            None => bail!("the default input produced no audio"),
        }
    }
    Ok(path)
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use styra_protocol::agent::Selection;

    /// A recording holding the given file's bytes, so the paths after the
    /// device are testable without one.
    fn recording(bytes: &[u8]) -> Recording {
        Recording {
            path: recorded(bytes),
            meter: Arc::new(Meter::new()),
        }
    }

    /// A finished recording's file.
    fn recorded(bytes: &[u8]) -> TempPath {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        file.into_temp_path()
    }

    /// A microphone the test speaks for: no device behind it, but a command
    /// channel to read what the editor asked the device for and a way to say
    /// that the device went away.
    fn mic() -> (Mic, Sender<String>, Receiver<Command>) {
        let (command, commands) = mpsc::channel();
        let (said, trouble) = mpsc::channel();
        (
            Mic {
                command,
                meter: Arc::new(Meter::new()),
                trouble,
            },
            said,
            commands,
        )
    }

    /// 16-bit mono WAV bytes carrying `samples` bytes of audio.
    fn wave(samples: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&((36 + samples.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&16_000u32.to_le_bytes());
        bytes.extend_from_slice(&32_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        bytes.extend_from_slice(samples);
        bytes
    }

    #[test]
    fn a_finished_capture_is_handed_on_as_the_file_it_wrote() {
        let recorded = recorded(&wave(&[7u8; 160]));
        let path = recorded.to_path_buf();
        let (done, finished) = mpsc::channel();
        done.send(Ok(None)).unwrap();

        assert_eq!(
            *finish_within(recorded, &finished, STOP_DEADLINE).unwrap(),
            path
        );
    }

    /// A capture that never produced a sample is a failure worth the reason
    /// the device gave, not the bare fact that nothing was recorded.
    #[test]
    fn a_silent_capture_is_reported_with_what_the_device_said() {
        let (done, finished) = mpsc::channel();
        done.send(Ok(Some("Device or resource busy".into())))
            .unwrap();

        let error = finish_within(recorded(&wave(&[])), &finished, STOP_DEADLINE).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "the default input produced no audio: Device or resource busy"
        );
    }

    #[test]
    fn a_capture_that_said_nothing_still_reports_the_silence() {
        let (done, finished) = mpsc::channel();
        done.send(Ok(None)).unwrap();

        let error = finish_within(recorded(&wave(&[])), &finished, STOP_DEADLINE).unwrap_err();

        assert_eq!(format!("{error:#}"), "the default input produced no audio");
    }

    /// An input that will not let go is given up on rather than waited on:
    /// this is the wait that used to be unbounded, and with it a recording the
    /// operator had already stopped never reached the transcriber at all.
    #[test]
    fn a_capture_that_never_stops_is_given_up_on() {
        let (done, finished) = mpsc::channel::<Result<Option<String>>>();

        let error = finish_within(
            recorded(&wave(&[7u8; 160])),
            &finished,
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").starts_with("the audio input did not stop"),
            "{error:#}"
        );
        drop(done);
    }

    /// A capture thread that failed while closing the device says so, rather
    /// than leaving the operator with a file nobody could finish.
    #[test]
    fn a_capture_that_failed_to_close_reports_its_own_error() {
        let (done, finished) = mpsc::channel();
        done.send(Err(anyhow!("finishing the recorded WAV file")))
            .unwrap();

        let error =
            finish_within(recorded(&wave(&[7u8; 160])), &finished, STOP_DEADLINE).unwrap_err();

        assert_eq!(format!("{error:#}"), "finishing the recorded WAV file");
    }

    /// The notice says recording began before the device has answered, so the
    /// device saying no has to reach the operator on its own — otherwise the
    /// interface claims to be recording a microphone that never opened.
    #[test]
    fn a_device_that_never_opened_takes_the_recording_back() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let client = Client::new("/nonexistent/styra.sock");
        let mut audio = AudioInput::new();
        let (mic, said, _commands) = mic();
        said.send("Device or resource busy".into()).unwrap();
        audio.mic = Some(mic);
        audio.recording = Some(recording(&wave(&[])));

        audio.apply_ready(&mut app, &client);

        assert!(audio.recording.is_none());
        assert!(audio.mic.is_none());
        assert_eq!(
            app.notices.iter().map(|notice| &notice.text).next(),
            Some(&"could not start audio recording: Device or resource busy".to_owned())
        );
    }

    /// A capture that is still opening is still a capture: nothing is said and
    /// nothing is taken back until the device has had its say.
    #[test]
    fn a_capture_that_has_not_answered_yet_is_left_alone() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let client = Client::new("/nonexistent/styra.sock");
        let mut audio = AudioInput::new();
        let (mic, _said, _commands) = mic();
        audio.mic = Some(mic);
        audio.recording = Some(recording(&wave(&[])));

        audio.apply_ready(&mut app, &client);

        assert!(audio.recording.is_some());
        assert!(app.notices.is_empty());
    }

    /// A device that goes away while nobody is recording is the log's business
    /// and not the operator's: they asked for a message box, not a microphone.
    #[test]
    fn an_armed_device_that_goes_away_says_so_only_in_the_log() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let client = Client::new("/nonexistent/styra.sock");
        let mut audio = AudioInput::new();
        let (mic, said, _commands) = mic();
        said.send("No such device".into()).unwrap();
        audio.mic = Some(mic);

        audio.apply_ready(&mut app, &client);

        assert!(audio.mic.is_none());
        assert!(app.notices.is_empty());
        // And not asked for again round after round while the box stays open.
        app.focus = Focus::Input;
        audio.follow_focus(&mut app);
        assert!(audio.mic.is_none());
    }

    /// The device is the message box's, and goes when it does: an input left
    /// open over the event list is a microphone nobody asked to hold.
    #[test]
    fn closing_the_message_box_lets_the_device_go() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let (mic, _said, _commands) = mic();
        let mut audio = AudioInput::new();
        audio.mic = Some(mic);
        app.focus = Focus::List;

        audio.follow_focus(&mut app);

        assert!(audio.mic.is_none());
    }

    /// Recording is the open device being pointed at a file, and the meter
    /// starts from this recording rather than from whatever the last one left
    /// behind.
    #[test]
    fn starting_a_recording_points_the_open_device_at_a_file() {
        let (mic, _said, commands) = mic();
        let meter = Arc::clone(&mic.meter);
        let mut audio = AudioInput::new();
        audio.mic = Some(mic);
        meter.report(0.7);

        let recording = audio.start().unwrap();

        assert_eq!(meter.take_peak(), 0.0);
        assert!(!meter.capturing());
        match commands.try_recv().unwrap() {
            Command::Start(path) => assert_eq!(path, *recording.path),
            Command::Stop(_) => panic!("recording started by stopping"),
        }
    }

    /// The bar follows the loudest sample up at once and falls towards silence
    /// rather than dropping to it, so speech reads as a moving level and not
    /// as a flicker between syllables.
    #[test]
    fn the_meter_rises_with_the_input_and_falls_between_it() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let client = Client::new("/nonexistent/styra.sock");
        let mut audio = AudioInput::new();
        let recording = recording(&wave(&[]));
        let meter = Arc::clone(&recording.meter);
        audio.recording = Some(recording);
        app.recording = Some(Recorded::new());

        meter.report(0.5);
        audio.apply_ready(&mut app, &client);
        assert_eq!(app.recording.as_ref().unwrap().level, 0.5);

        // A tenth of a second in which nothing was heard: the bar falls by the
        // stated fraction of it, and the loudest of the recording is still
        // what it was. Backdated rather than slept through — the fall is a
        // function of elapsed time, so a test can state the time.
        app.recording.as_mut().unwrap().updated = Instant::now() - Duration::from_millis(100);
        audio.apply_ready(&mut app, &client);
        let recorded = app.recording.as_ref().unwrap();
        assert!(
            (recorded.level - 0.5 * FALL_PER_100MS).abs() < 0.01,
            "{}",
            recorded.level
        );
        assert_eq!(recorded.loudest, 0.5);
    }

    /// The box may not claim to be recording before the device has handed over
    /// a buffer: opening an input takes real time, and an operator who speaks
    /// into that gap loses the beginning of what they said.
    #[test]
    fn the_recording_counts_from_the_first_buffer_not_from_the_key() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let client = Client::new("/nonexistent/styra.sock");
        let mut audio = AudioInput::new();
        let recording = recording(&wave(&[]));
        let meter = Arc::clone(&recording.meter);
        audio.recording = Some(recording);
        app.recording = Some(Recorded::new());

        audio.apply_ready(&mut app, &client);
        assert!(app.recording.as_ref().unwrap().capturing_since.is_none());

        // A silent room is still a recording: it is the buffer arriving that
        // says the device is open, not anything audible in it.
        meter.report(0.0);
        audio.apply_ready(&mut app, &client);
        assert!(app.recording.as_ref().unwrap().capturing_since.is_some());
    }

    /// Boosting is the capture's, not the display's: what changes is what the
    /// callback multiplies the samples by, so the file handed to the
    /// transcriber is the louder one.
    #[test]
    fn boosting_raises_the_gain_the_capture_applies() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let recording = recording(&wave(&[]));
        let meter = Arc::clone(&recording.meter);
        let mut audio = AudioInput::new();
        audio.recording = Some(recording);
        app.recording = Some(Recorded::new());

        audio.boost(&mut app);
        audio.boost(&mut app);

        assert_eq!(meter.gain(), 2.0);
        assert_eq!(app.recording.as_ref().unwrap().gain, 2.0);

        audio.quieten(&mut app);
        assert_eq!(meter.gain(), 1.5);
    }

    /// Neither end of the ladder is a place the control can fall off.
    #[test]
    fn the_gain_ladder_stops_at_its_ends() {
        assert_eq!(quieter(1.0), 1.0);
        assert_eq!(louder(24.0), 24.0);
    }

    /// A boosted sample past full scale is recorded as the loudest value there
    /// is, rather than wrapping into the noise that an overflowing cast makes.
    #[test]
    fn boosted_samples_are_clamped_rather_than_wrapped() {
        assert_eq!(pcm(2.0), i16::MAX);
        assert_eq!(pcm(-2.0), -i16::MAX);
        assert_eq!(pcm(0.0), 0);
    }

    /// Cancelling throws the audio away: nothing is transcribed, and the
    /// meter leaves the message box.
    #[test]
    fn cancelling_a_recording_transcribes_nothing() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        let client = Client::new("/nonexistent/styra.sock");
        let mut audio = AudioInput::new();
        audio.recording = Some(recording(&wave(&[7u8; 160])));
        app.recording = Some(Recorded::new());

        audio.cancel(&mut app, client);

        assert!(audio.recording.is_none());
        assert!(app.recording.is_none());
        assert!(audio.receive.try_recv().is_err());
        assert_eq!(
            app.notices.iter().map(|notice| &notice.text).next(),
            Some(&"recording cancelled".to_owned())
        );
    }
}
