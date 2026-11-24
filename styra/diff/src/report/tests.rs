use super::*;

#[test]
fn a_fingerprint_is_fixed_for_a_patch_and_differs_between_patches() {
    assert_eq!(fingerprint(""), "cbf29ce484222325");
    assert_eq!(fingerprint("a"), "af63dc4c8601ec8c");
    assert_ne!(fingerprint("+one\n"), fingerprint("+two\n"));
}

/// A description stays shown once its file has moved on, flagged as
/// written for an earlier patch.
#[test]
fn a_description_is_stale_once_its_patch_changes() {
    let mut state = ReportState::default();
    assert!(!state.shown());
    state.pending = true;
    assert!(state.shown());
    assert_eq!(
        state.describe("a.rs", "+a\n"),
        Described::Missing { failure: None }
    );

    state.report = Some(Report {
        files: HashMap::from([(
            "a.rs".to_owned(),
            FileReport {
                fingerprint: fingerprint("+a\n"),
                description: "Adds a.".into(),
            },
        )]),
        failures: Vec::new(),
    });
    assert_eq!(
        state.describe("a.rs", "+a\n"),
        Described::Described {
            description: "Adds a.",
            stale: false
        }
    );
    assert_eq!(
        state.describe("a.rs", "+a\n+b\n"),
        Described::Described {
            description: "Adds a.",
            stale: true
        }
    );
    state.hidden = true;
    assert!(!state.shown());
}

#[test]
fn a_file_described_alone_is_the_only_one_being_described() {
    let mut state = ReportState::default();
    assert!(!state.describing("a.rs"));
    state.pending = true;
    assert!(state.describing("a.rs") && state.describing("b.rs"));
    state.pending_file = Some("b.rs".into());
    assert!(!state.describing("a.rs") && state.describing("b.rs"));
}
