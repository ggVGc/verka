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

#[test]
fn a_silent_capture_reports_the_device_reason_when_available() {
    for (fault, expected) in [
        (
            Some("Device or resource busy"),
            "the default input produced no audio: Device or resource busy",
        ),
        (None, "the default input produced no audio"),
    ] {
        let (done, finished) = mpsc::channel();
        done.send(Ok(fault.map(String::from))).unwrap();
        let error = finish_within(recorded(&wave(&[])), &finished, STOP_DEADLINE).unwrap_err();
        assert_eq!(format!("{error:#}"), expected, "{fault:?}");
    }
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

    let error = finish_within(recorded(&wave(&[7u8; 160])), &finished, STOP_DEADLINE).unwrap_err();

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
    let notices = app.notices.iter().collect::<Vec<_>>();
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[0].level, styra_protocol::LogLevel::Error);
    assert!(notices[0]
        .text
        .contains("could not report stopped audio recording"));
    assert_eq!(notices[1].text, "recording cancelled");
}
