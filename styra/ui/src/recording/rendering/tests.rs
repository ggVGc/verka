use super::*;

fn view(level: f32, loudest: f32) -> RecordingView {
    RecordingView {
        level,
        loudest,
        gain: 1.0,
        captured: Some("3s".into()),
    }
}

#[test]
fn the_bar_grows_with_the_level_and_fills_at_full_scale() {
    assert_eq!(cells(0.0, 40), 0);
    assert!(cells(0.01, 40) < cells(0.1, 40));
    assert!(cells(0.1, 40) < cells(1.0, 40));
    assert_eq!(cells(1.0, 40), 40);
}

/// Clipping cannot overflow the bar it is drawn in.
#[test]
fn a_clipped_level_stops_at_the_end_of_the_bar() {
    assert_eq!(cells(8.0, 40), 40);
    assert_eq!(bar(&view(8.0, 8.0), 40).spans.len(), 40);
}

#[test]
fn the_loudest_so_far_is_held_as_a_mark_ahead_of_the_level() {
    let held = bar(&view(0.01, 0.9), 40);
    let marks = held.spans.iter().filter(|span| span.content == "│").count();
    assert_eq!(marks, 1);
}

/// The advice line is what turns a meter into a control: an input nobody
/// can hear has to name the key that fixes it.
#[test]
fn a_quiet_input_is_told_to_boost_and_a_clipped_one_to_stop() {
    assert!(advice(&view(0.0, 0.01)).to_string().contains("↑ to boost"));
    assert!(advice(&view(1.2, 1.2)).to_string().contains("clipped"));
    assert!(advice(&view(0.4, 0.4)).to_string().starts_with("peak"));
}

/// A device that has not answered yet says exactly that, rather than
/// letting the operator speak into a microphone that is not open.
#[test]
fn an_input_that_has_not_answered_yet_says_nothing_is_being_recorded() {
    let opening = RecordingView {
        captured: None,
        ..view(0.0, 0.0)
    };
    let said = advice(&opening).to_string();
    assert!(said.contains("nothing is being recorded"), "{said}");
}

#[test]
fn a_whole_boost_is_shown_without_a_decimal() {
    assert_eq!(gain(2.0), "2");
    assert_eq!(gain(1.5), "1.5");
}
