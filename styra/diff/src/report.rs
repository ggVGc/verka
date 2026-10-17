//! An edit report: what each file of the diff was edited for, as the host had
//! it described. The view only shows one; writing it takes an agent, which
//! only Styra has to ask, so the host fills it in (see
//! [`crate::Outcome::GenerateReport`]).

use std::collections::HashMap;

/// The descriptions, by the path the diff names each file by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub files: HashMap<String, FileReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReport {
    /// [`fingerprint`] of the patch the description was written for.
    pub fingerprint: String,
    pub description: String,
}

/// A diff's report, and whether one is on its way or kept out of view.
#[derive(Clone, Debug, Default)]
pub struct ReportState {
    pub report: Option<Report>,
    /// A report is being written; it replaces this one when it comes.
    pub pending: bool,
    /// The operator has put the report away.
    pub hidden: bool,
}

/// What the view says about one file of the diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Described<'a> {
    /// Its description, and whether the patch has changed since it was
    /// written.
    Described { description: &'a str, stale: bool },
    /// The report does not cover it.
    Missing,
}

impl ReportState {
    /// Whether the view has anything of a report to show.
    pub(crate) fn shown(&self) -> bool {
        !self.hidden && (self.report.is_some() || self.pending)
    }

    pub(crate) fn describe(&self, path: &str, patch: &str) -> Described<'_> {
        match self
            .report
            .as_ref()
            .and_then(|report| report.files.get(path))
        {
            Some(file) => Described::Described {
                description: &file.description,
                stale: file.fingerprint != fingerprint(patch),
            },
            None => Described::Missing,
        }
    }
}

/// A short, stable name for a file's patch, to tell whether the patch a
/// description was written for is the one shown now. FNV-1a: the report is
/// kept on disk, so it must hash the same in every build.
pub fn fingerprint(patch: &str) -> String {
    let hash = patch.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
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
        assert_eq!(state.describe("a.rs", "+a\n"), Described::Missing);

        state.report = Some(Report {
            files: HashMap::from([(
                "a.rs".to_owned(),
                FileReport {
                    fingerprint: fingerprint("+a\n"),
                    description: "Adds a.".into(),
                },
            )]),
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
}
