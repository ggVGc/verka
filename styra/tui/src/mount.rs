//! Host mounts an operator asks a launch for: how one is written and how one
//! is read back. Where the git history of a checkout actually lives is
//! [`styra_protocol::git`]'s question, not this module's.
//!
//! Nothing here touches [`crate::app::App`] — these are the pure parts of the
//! launch policy, kept apart from the state machine in [`crate::launch`] so
//! that parsing a path and deciding which layer it lands in are not the same
//! body of code.

use std::path::{Path, PathBuf};
use styra_protocol::{LaunchMount, Mount, MountAccess};
use styra_ui::fuzzy_list::FuzzyList;

/// How an extra mount reads in the view and in the prompt that adds one.
pub fn label(mount: &LaunchMount) -> String {
    let access = if mount.writable { "rw" } else { "ro" };
    match &mount.destination {
        Some(destination) => format!(
            "{} → {} ({access})",
            mount.source.display(),
            destination.display()
        ),
        None => format!("{} ({access})", mount.source.display()),
    }
}

/// Parse the `source[:destination][:ro|rw]` an operator types into a mount
/// request.
///
/// The access suffix is recognized only as a trailing `ro`/`rw`, so a path
/// component is never mistaken for one. Access defaults to read-only: this
/// grants an agent reach outside its workspace, so the quiet default is the
/// one that grants least. Paths are checked for being absolute here rather
/// than only on the server, because a relative path would otherwise resolve
/// against the *server's* directory rather than the operator's.
pub fn parse(text: &str) -> Result<LaunchMount, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("give a path to mount, e.g. /srv/data:ro".into());
    }
    let mut parts: Vec<&str> = text.split(':').map(str::trim).collect();
    let writable = match parts.last() {
        Some(&"rw") => {
            parts.pop();
            true
        }
        Some(&"ro") => {
            parts.pop();
            false
        }
        _ => false,
    };
    let (source, destination) = match parts.as_slice() {
        [source] => (*source, None),
        [source, destination] => (*source, Some(*destination)),
        _ => return Err("expected source[:destination][:ro|rw]".into()),
    };
    if source.is_empty() {
        return Err("give a path to mount, e.g. /srv/data:ro".into());
    }
    // Rebuilt from its components, which drops the trailing `/` a directory
    // taken from the prompt's list arrives with.
    let source: PathBuf = expand_home(source).components().collect();
    if !source.is_absolute() {
        return Err(format!("{} must be an absolute path", source.display()));
    }
    let destination = match destination {
        Some("") => return Err("the destination cannot be empty".into()),
        Some(destination) => {
            let destination: PathBuf = expand_home(destination).components().collect();
            if !destination.is_absolute() {
                return Err(format!(
                    "the destination {} must be an absolute path",
                    destination.display()
                ));
            }
            Some(destination)
        }
        None => None,
    };
    Ok(LaunchMount {
        source,
        destination,
        writable,
    })
}

/// Expand a leading `~`, so a typed path behaves the way it does in a shell.
/// A `~` with no `HOME` to expand is left alone and rejected as non-absolute.
pub fn expand_home(text: &str) -> PathBuf {
    let path = PathBuf::from(text);
    if path != Path::new("~") && !path.starts_with("~/") {
        return path;
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(path.strip_prefix("~").expect("prefix checked")),
        None => path,
    }
}

/// The list that stands under the mount prompt while it is open: the entries
/// of the directory being typed into, narrowed as the name after its last `/`
/// is typed.
///
/// Only the part after the last `:` is listed — the source, or the
/// destination once one is being typed. Which directory is listed is read off
/// the text alone, up to its last `/`, so the list never jumps into a
/// directory just because a name typed so far happens to be one: `Tab` is what
/// goes into it. Narrowing is [`FuzzyList`]'s subsequence matching, best match
/// first, so `dcs` finds `docs`. Hidden entries are listed only, and alone,
/// once a `.` has been typed for them, as in a shell. A part with no `/` is not
/// a path the prompt would accept, so nothing is listed for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathList {
    /// The prompt text the list was last brought up to date with.
    text: String,
    /// What stands before the listed directory: up to and including the last
    /// `:`, kept in front of a chosen entry.
    head: String,
    /// The directory listed, as typed and ending in `/`; empty when the part
    /// being typed is not a path.
    directory: String,
    /// Whether the directory's hidden entries are the ones listed.
    hidden: bool,
    /// Each entry as the list shows it and matches it: its name, with a
    /// trailing `/` on a directory.
    pub rows: Vec<String>,
    /// The name typed after the last `/`, and the cursor among what it leaves.
    pub list: FuzzyList,
}

impl PathList {
    /// Bring the list up to date with the prompt holding `text`. Nothing is
    /// listed for the empty text a default list stands for, so the first
    /// update reads whatever directory `text` names. After that the directory
    /// is read again only when a different one is named; a different name in
    /// the same one only narrows, and puts the cursor back on the best match.
    pub fn update(&mut self, text: &str) {
        if self.text == text {
            return;
        }
        self.text = text.to_owned();
        let start = text.rfind(':').map_or(0, |colon| colon + 1);
        let (head, part) = text.split_at(start);
        let (directory, stem) = match part.rfind('/') {
            Some(slash) => (&part[..=slash], &part[slash + 1..]),
            None => ("", ""),
        };
        let hidden = stem.starts_with('.');
        if (head, directory, hidden) != (&self.head[..], &self.directory[..], self.hidden) {
            self.head = head.to_owned();
            self.directory = directory.to_owned();
            self.hidden = hidden;
            self.rows = entries(directory, hidden);
        }
        self.list.query = stem.to_owned();
        self.list.selected = 0;
    }

    /// The row under the cursor, as the whole prompt text it would leave
    /// behind: a directory with its trailing `/`, so the list goes on to what
    /// is under it.
    pub fn highlighted(&self) -> Option<String> {
        let row = &self.rows[self.list.selected_row(&self.rows)?];
        Some(format!("{}{}{row}", self.head, self.directory))
    }

    /// Whether a name is being typed, rather than the list standing open on a
    /// whole directory. Only then is the highlighted row what `Enter` means
    /// when the typed path itself does not exist.
    pub fn narrowing(&self) -> bool {
        self.list.is_filtering()
    }
}

/// The entries of `directory`, sorted, with a `/` after each directory: the
/// hidden ones when `hidden`, and otherwise the rest.
fn entries(directory: &str, hidden: bool) -> Vec<String> {
    if directory.is_empty() {
        return Vec::new();
    }
    let mut entries: Vec<String> = std::fs::read_dir(expand_home(directory))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            // A leading `.` asks for hidden entries, and only those: matched
            // fuzzily, it would also find every name with a dot inside.
            if name.starts_with('.') != hidden {
                return None;
            }
            Some(match entry.path().is_dir() {
                true => format!("{name}/"),
                false => name,
            })
        })
        .collect();
    entries.sort();
    entries
}

/// Where a host path turns out to be reachable from inside the sandbox, and on
/// what terms. See [`visibility`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visible {
    /// The path the *agent* would use, which is the host path rewritten through
    /// the mount that carries it. Only equal to the host path when the mount
    /// binds a source at its own name, which is Driva's default but not its
    /// only shape.
    pub path: PathBuf,
    /// How the mount grants it, in the same words the driva view uses.
    pub access: &'static str,
}

/// Whether `host` is reachable inside a sandbox holding `mounts`, and as what.
///
/// A path is reachable when some bind or overlay mount carries it — the mount's
/// source is the path itself or an ancestor of it. The longest such source wins:
/// mounts nest, and the innermost one is the one whose destination and access
/// actually apply to this path.
///
/// A temporary mount grants no host path, so it is not consulted: it makes a
/// destination writable inside the sandbox, but nothing on the host appears
/// there.
pub fn visibility(mounts: &[Mount], host: &Path) -> Option<Visible> {
    mounts
        .iter()
        .filter_map(|mount| {
            let (source, destination, access) = match mount {
                Mount::Bind {
                    source,
                    destination,
                    access,
                } => (
                    source,
                    destination,
                    match access {
                        MountAccess::ReadOnly => "ro",
                        MountAccess::ReadWrite => "rw",
                    },
                ),
                // Writable, but the writes never reach the host. Said as its own
                // word rather than as `rw`, because an operator who reads "rw"
                // will expect to find the agent's edits afterwards.
                Mount::Overlay {
                    source,
                    destination,
                } => (source, destination, "overlay"),
                Mount::Temporary { .. } => return None,
            };
            let relative = host.strip_prefix(source).ok()?;
            Some((
                source.components().count(),
                Visible {
                    path: destination.join(relative),
                    access,
                },
            ))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, visible)| visible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_mounts_parse_paths_access_and_trailing_slashes() {
        for (input, source, destination, writable) in [
            ("/srv/data", "/srv/data", None, false),
            ("  /srv/data:rw  ", "/srv/data", None, true),
            ("/srv/data:/mnt/data", "/srv/data", Some("/mnt/data"), false),
            (
                "/srv/data:/mnt/data:rw",
                "/srv/data",
                Some("/mnt/data"),
                true,
            ),
            // A directory named rw is still a destination.
            ("/srv/rw:/mnt/rw", "/srv/rw", Some("/mnt/rw"), false),
            // A directory selected from the list arrives with its slash.
            (
                "/srv/data/:/mnt/data/:rw",
                "/srv/data",
                Some("/mnt/data"),
                true,
            ),
        ] {
            assert_eq!(
                parse(input).unwrap(),
                LaunchMount {
                    source: source.into(),
                    destination: destination.map(PathBuf::from),
                    writable
                },
                "{input}"
            );
        }
        for input in [
            "",
            "data",
            "/srv/data:relative",
            "/a:/b:/c:rw",
            "/srv/data:",
        ] {
            assert!(parse(input).is_err(), "{input:?}");
        }
    }

    /// How a mount reads back is what the two settings panes list, so the
    /// destination and the access are both part of it.
    #[test]
    fn a_mount_reads_back_as_it_was_written() {
        assert_eq!(
            label(&parse("/srv/data:/mnt/data:rw").unwrap()),
            "/srv/data → /mnt/data (rw)"
        );
        assert_eq!(label(&parse("/srv/data").unwrap()), "/srv/data (ro)");
        assert_eq!(label(&parse("/srv/data/").unwrap()), "/srv/data (ro)");
    }

    /// What the agent can reach, and under which of several overlapping mounts:
    /// the innermost one, because that is the one whose destination and access
    /// the path actually lands under.
    #[test]
    fn a_path_is_read_through_the_innermost_mount_that_carries_it() {
        let mounts = vec![
            Mount::Bind {
                source: PathBuf::from("/srv"),
                destination: PathBuf::from("/mnt"),
                access: MountAccess::ReadOnly,
            },
            Mount::Bind {
                source: PathBuf::from("/srv/data"),
                destination: PathBuf::from("/workspace"),
                access: MountAccess::ReadWrite,
            },
            Mount::Temporary {
                destination: PathBuf::from("/tmp"),
            },
        ];

        assert_eq!(
            visibility(&mounts, Path::new("/srv/data/notes.txt")),
            Some(Visible {
                path: PathBuf::from("/workspace/notes.txt"),
                access: "rw",
            })
        );
        assert_eq!(
            visibility(&mounts, Path::new("/srv/corpus/a.txt")),
            Some(Visible {
                path: PathBuf::from("/mnt/corpus/a.txt"),
                access: "ro",
            })
        );
        // The mount's own source, with nothing below it, is reachable too.
        assert_eq!(
            visibility(&mounts, Path::new("/srv/data")).unwrap().path,
            PathBuf::from("/workspace")
        );
        // A temporary mount grants no host path, so `/tmp` on the host is not
        // what appears at `/tmp` inside.
        assert_eq!(visibility(&mounts, Path::new("/tmp/scratch")), None);
        assert_eq!(visibility(&mounts, Path::new("/etc/passwd")), None);
        // A prefix that is not a path prefix does not count: `/srv-old` merely
        // starts with the same characters as `/srv`.
        assert_eq!(visibility(&mounts, Path::new("/srv-old/data")), None);
    }

    fn completion_tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "styra-mount-complete-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["data", "docs", ".hidden"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("notes.txt"), "").unwrap();
        root
    }

    /// The list for the prompt holding `text`.
    fn listed(text: &str) -> PathList {
        let mut list = PathList::default();
        list.update(text);
        list
    }

    /// The rows `list` shows, best match first.
    fn shown(list: &PathList) -> Vec<String> {
        list.list
            .matches(&list.rows)
            .into_iter()
            .map(|found| list.rows[found.index].clone())
            .collect()
    }

    /// A directory up to its last `/` is what is listed: all of it, or with a
    /// name typed after the `/`, the entries that name fuzzily matches, in any
    /// case and closest first. A name that happens to be a directory is not
    /// listed into until its `/` is typed.
    #[test]
    fn the_list_is_the_typed_directory_narrowed_by_the_typed_name() {
        let root = completion_tree("narrow");
        let base = root.display().to_string();

        let under = listed(&format!("{base}/"));
        assert_eq!(shown(&under), ["data/", "docs/", "notes.txt"]);
        assert_eq!(under.highlighted(), Some(format!("{base}/data/")));
        assert!(!under.narrowing());

        let skipped = listed(&format!("{base}/dcs"));
        assert_eq!(shown(&skipped), ["docs/"]);
        assert_eq!(skipped.highlighted(), Some(format!("{base}/docs/")));
        assert!(skipped.narrowing());

        assert_eq!(shown(&listed(&format!("{base}/NTS"))), ["notes.txt"]);
        // Both `docs` and `notes.txt` hold an `s`; the shorter one, whose `s`
        // sits nearer the front, ranks first.
        assert_eq!(shown(&listed(&format!("{base}/s"))), ["docs/", "notes.txt"]);
        assert_eq!(shown(&listed(&format!("{base}/data"))), ["data/"]);
        assert!(shown(&listed(&format!("{base}/zz"))).is_empty());
        assert!(listed("relative").rows.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Hidden entries are listed once a `.` is typed, and then alone: a dot
    /// matched fuzzily would otherwise find every name with one inside.
    #[test]
    fn a_leading_dot_lists_hidden_entries_alone() {
        let root = completion_tree("hidden");
        let base = root.display().to_string();
        assert_eq!(shown(&listed(&format!("{base}/."))), [".hidden/"]);
        assert!(shown(&listed(&format!("{base}/hid"))).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Typing on in the same directory narrows without reading it again, and
    /// puts the cursor back on the best match.
    #[test]
    fn typing_on_narrows_and_resets_the_cursor() {
        let root = completion_tree("update");
        let base = root.display().to_string();
        let mut list = listed(&format!("{base}/"));
        list.list.next(&list.rows);
        assert_eq!(list.highlighted(), Some(format!("{base}/docs/")));

        list.update(&format!("{base}/n"));
        assert_eq!(list.list.selected, 0);
        assert_eq!(list.highlighted(), Some(format!("{base}/notes.txt")));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The destination after a `:` is listed on its own, keeping the source.
    #[test]
    fn the_list_keeps_what_comes_before_the_last_colon() {
        let root = completion_tree("colon");
        let base = root.display().to_string();
        let list = listed(&format!("/srv:{base}/no"));
        assert_eq!(list.highlighted(), Some(format!("/srv:{base}/notes.txt")));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The client resolves `~` itself: a path left for the server to expand
    /// would expand against whoever is running the server.
    #[test]
    fn a_leading_tilde_expands_against_this_operators_home() {
        let Some(home) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
        else {
            return;
        };
        assert_eq!(parse("~/data").unwrap().source, home.join("data"));
        assert_eq!(parse("~").unwrap().source, home);
    }
}
