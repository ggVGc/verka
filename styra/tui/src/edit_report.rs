//! Edit reports for the checkout diff: `E` has the server describe each file
//! of the diff on screen — what was edited and why — and the per-file view
//! shows the selected file's description above its patch.
//!
//! Describing every file takes an agent a while, so the request is made off
//! the terminal thread and the report is put into the diff on whichever round
//! it arrives. The server keeps it with the Session, so a diff opened later,
//! from this client or another, starts with it.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use styra_protocol::{EditReport, EditReportFile, LogEntry};
use styra_server::Client;

use crate::app::App;

/// The most of one file's patch sent. The server shows its agent less than
/// this; the rest would only cross the socket to be dropped.
const PATCH_BYTES: usize = 16 * 1024;
/// The most of all the patches sent together, well inside the server's
/// request limit once escaped as JSON.
const REQUEST_BYTES: usize = 3 * 1024 * 1024;

type Answer = (String, Result<EditReport, String>);

pub struct EditReports {
    send: Sender<Answer>,
    receive: Receiver<Answer>,
    /// The Sessions a report is being written for.
    pending: HashSet<String>,
}

impl EditReports {
    pub fn new() -> Self {
        let (send, receive) = mpsc::channel();
        Self {
            send,
            receive,
            pending: HashSet::new(),
        }
    }

    /// Have each file of the diff on screen described.
    pub fn start(&mut self, app: &mut App, client: &Client) {
        let id = app.session_id.clone();
        let Some(diff) = app.checkout_diff.as_ref() else {
            return;
        };
        if id.is_empty() {
            return app.show_warning("no interaction to describe the edits of");
        }
        if self.pending.contains(&id) {
            return app.show_action_message("already describing this interaction's edits");
        }
        let files = match &diff.diff {
            Ok(text) => request_files(text),
            Err(error) => {
                let message = format!("no diff to describe: {error}");
                return app.show_error(message);
            }
        };
        if files.is_empty() {
            return app.show_warning("no edited files to describe");
        }
        let Some(base) = diff.target().map(|target| target.base.clone()) else {
            return app.show_warning("only a checkout's diff has edits to describe");
        };
        let count = files.len();
        let client = client.clone();
        let send = self.send.clone();
        let session = id.clone();
        let spawned = thread::Builder::new()
            .name("styra-edit-report".into())
            .spawn(move || {
                let report = client
                    .generate_edit_report(&session, &base, files)
                    .map_err(|error| format!("{error:#}"));
                // Gone only when the loop that asked has left; the server
                // has kept the report for the next one regardless.
                let _ = send.send((session, report));
            });
        if let Err(error) = spawned {
            return app.show_error(format!("could not start describing the edits: {error}"));
        }
        if let Some(diff) = app.checkout_diff.as_mut() {
            diff.report.pending = true;
            diff.report.hidden = false;
        }
        self.pending.insert(id);
        let noun = if count == 1 { "file" } else { "files" };
        app.show_action_message(format!(
            "describing the edits of {count} {noun} — the per-file view (f) shows each one"
        ));
    }

    /// Put reports that have come into the diff on screen, if they are its.
    /// Reports whether the screen changed.
    pub fn apply_ready(&mut self, app: &mut App) -> bool {
        let mut applied = false;
        while let Ok((id, report)) = self.receive.try_recv() {
            self.pending.remove(&id);
            if id != app.session_id {
                continue;
            }
            applied = true;
            if let Some(diff) = app.checkout_diff.as_mut() {
                diff.report.pending = false;
                if let Ok(report) = &report {
                    diff.report.report = Some(for_view(report));
                }
            }
            match report {
                Ok(report) => app.show_action_message(format!(
                    "edit report ready: {} of {} files described",
                    report.files.len(),
                    report.requested
                )),
                Err(error) => app.show_error(format!("could not describe the edits: {error}")),
            }
        }
        applied
    }

    /// Start a diff just opened with the report kept for its Session, and
    /// say whether another is on its way.
    pub fn load(&self, app: &mut App, client: &Client) {
        let id = app.session_id.clone();
        let Some(diff) = app.checkout_diff.as_mut() else {
            return;
        };
        diff.report.pending = self.pending.contains(&id);
        if id.is_empty() {
            return;
        }
        match client.edit_report(&id) {
            Ok(report) => diff.report.report = report.as_ref().map(for_view),
            Err(error) => app.push_log(LogEntry::warn(format!(
                "could not read the edit report: {error:#}"
            ))),
        }
    }
}

/// Each file of the diff `text`, as the view lists them, to be described:
/// fingerprinted whole, and sent in part when long.
fn request_files(text: &str) -> Vec<EditReportFile> {
    let files = styra_diff::files::file_diffs(text);
    let budget = (REQUEST_BYTES / files.len().max(1)).min(PATCH_BYTES);
    files
        .into_iter()
        .map(|file| EditReportFile {
            path: file.path.to_owned(),
            patch: head(file.patch, budget).to_owned(),
            fingerprint: styra_diff::report::fingerprint(file.patch),
        })
        .collect()
}

/// At most the first `limit` bytes of `text`, ending on a character.
fn head(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn for_view(report: &EditReport) -> styra_diff::Report {
    styra_diff::Report {
        files: report
            .files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    styra_diff::FileReport {
                        fingerprint: file.fingerprint.clone(),
                        description: file.description.clone(),
                    },
                )
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file goes, named and fingerprinted as the view names and
    /// fingerprints it, with a long patch cut short on a character.
    #[test]
    fn each_file_of_the_diff_is_sent_as_the_view_lists_it() {
        let long = format!(
            "diff --git a/long.txt b/long.txt\n@@ -1 +1 @@\n+{}\n",
            "é".repeat(PATCH_BYTES)
        );
        let text = format!("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n{long}");
        let files = request_files(&text);

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(
            files[0].patch,
            "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n"
        );
        assert_eq!(
            files[0].fingerprint,
            styra_diff::report::fingerprint(&files[0].patch)
        );
        assert_eq!(files[1].path, "long.txt");
        assert!(files[1].patch.len() <= PATCH_BYTES && files[1].patch.len() > PATCH_BYTES - 2);
        assert_eq!(files[1].fingerprint, styra_diff::report::fingerprint(&long));
        assert!(request_files("").is_empty());
    }

    #[test]
    fn a_report_is_shown_by_path() {
        let report = EditReport {
            generated_at_ms: 1,
            base: "4bf5c35d".into(),
            files: vec![styra_protocol::FileEditReport {
                path: "a.rs".into(),
                fingerprint: "f".into(),
                description: "Adds a.".into(),
            }],
            requested: 1,
        };
        let shown = for_view(&report);
        assert_eq!(shown.files["a.rs"].description, "Adds a.");
        assert_eq!(shown.files["a.rs"].fingerprint, "f");
    }
}
