//! Host mounts an operator asks a launch for: how one is written and how one
//! is read back. Where the git history of a checkout actually lives is
//! [`styra_server::git`]'s question, not this module's.
//!
//! Nothing here touches [`crate::app::App`] — these are the pure parts of the
//! launch policy, kept apart from the state machine in [`crate::launch`] so
//! that parsing a path and deciding which layer it lands in are not the same
//! body of code.

use std::path::{Path, PathBuf};
use styra_server::{LaunchMount, Mount, MountAccess};

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
    let source = expand_home(source);
    if !source.is_absolute() {
        return Err(format!("{} must be an absolute path", source.display()));
    }
    let destination = match destination {
        Some("") => return Err("the destination cannot be empty".into()),
        Some(destination) => {
            let destination = expand_home(destination);
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
    fn a_typed_mount_parses_its_paths_and_access() {
        assert_eq!(
            parse("/srv/data").unwrap(),
            LaunchMount {
                source: PathBuf::from("/srv/data"),
                destination: None,
                writable: false,
            }
        );
        assert_eq!(
            parse("  /srv/data:rw  ").unwrap(),
            LaunchMount {
                source: PathBuf::from("/srv/data"),
                destination: None,
                writable: true,
            }
        );
        assert_eq!(
            parse("/srv/data:/mnt/data").unwrap(),
            LaunchMount {
                source: PathBuf::from("/srv/data"),
                destination: Some(PathBuf::from("/mnt/data")),
                writable: false,
            }
        );
        assert_eq!(
            parse("/srv/data:/mnt/data:rw").unwrap(),
            LaunchMount {
                source: PathBuf::from("/srv/data"),
                destination: Some(PathBuf::from("/mnt/data")),
                writable: true,
            }
        );

        // A directory that happens to be named `rw` is still a destination:
        // only a trailing bare `ro`/`rw` after a path is an access mode.
        assert_eq!(
            parse("/srv/rw:/mnt/rw").unwrap(),
            LaunchMount {
                source: PathBuf::from("/srv/rw"),
                destination: Some(PathBuf::from("/mnt/rw")),
                writable: false,
            }
        );

        assert!(parse("").is_err());
        assert!(parse("data").is_err());
        assert!(parse("/srv/data:relative").is_err());
        assert!(parse("/a:/b:/c:rw").is_err());
        assert!(parse("/srv/data:").is_err());
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
