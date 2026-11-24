use super::{format_source, parse_source, split_trailers};
use crate::review::NoteSource;

#[test]
fn sources_round_trip_through_their_trailer() {
    let source = NoteSource {
        revision: "a".repeat(40),
        path: "dir/odd:name.rs".into(),
        first: 3,
        last: 7,
    };
    assert_eq!(parse_source(&format_source(&source)).unwrap(), source);
    for invalid in ["abc:file:1-2", &format!("{}:file:2-1", "a".repeat(40))] {
        assert!(parse_source(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn only_a_final_paragraph_of_trailers_is_split_off() {
    assert_eq!(
        split_trailers("Text\n\nMore: prose\n\nNota-Note: true\n"),
        ("Text\n\nMore: prose", vec![("Nota-Note", "true")])
    );
    assert_eq!(
        split_trailers("Nota-Note: true"),
        ("Nota-Note: true", vec![])
    );
    assert_eq!(
        split_trailers("Text\n\nnot a trailer\nNota-Note: true"),
        ("Text\n\nnot a trailer\nNota-Note: true", vec![])
    );
}
