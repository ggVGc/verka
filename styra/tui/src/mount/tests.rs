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
