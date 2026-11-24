use super::*;

fn press(picker: &mut BranchPicker, code: KeyCode) -> BranchChoice {
    picker.handle_key(KeyEvent::from(code))
}

#[test]
fn typing_narrows_without_reordering_and_enter_picks_the_branch() {
    let mut picker = BranchPicker::base(
        Some("main (Workspace)"),
        vec!["z-new-topic".into(), "a-old-topic".into(), "other".into()],
        None,
    );
    assert_eq!(picker.rows[0], "Default (main (Workspace))");
    for c in "topic".chars() {
        press(&mut picker, KeyCode::Char(c));
    }
    let shown: Vec<_> = picker
        .list
        .matches(&picker.rows)
        .iter()
        .map(|m| m.index)
        .collect();
    assert_eq!(shown, vec![1, 2], "newest commit first, as listed");
    press(&mut picker, KeyCode::Down);
    assert!(matches!(
        press(&mut picker, KeyCode::Enter),
        BranchChoice::Base(Some(branch)) if branch == "a-old-topic"
    ));
    picker.list.query = "nothing-matches".into();
    assert!(matches!(
        press(&mut picker, KeyCode::Enter),
        BranchChoice::Stay
    ));
    assert!(matches!(
        press(&mut picker, KeyCode::Esc),
        BranchChoice::Cancel
    ));
}

#[test]
fn the_first_row_clears_the_choice_and_reopening_starts_on_it() {
    let branches = vec!["main".to_owned(), "topic".to_owned()];
    let mut picker = BranchPicker::base(None, branches.clone(), Some("topic"));
    assert_eq!(picker.list.selected_row(&picker.rows), Some(2));
    press(&mut picker, KeyCode::Up);
    press(&mut picker, KeyCode::Up);
    assert!(matches!(
        press(&mut picker, KeyCode::Enter),
        BranchChoice::Base(None)
    ));
    let unlisted = BranchPicker::base(None, branches, Some("gone"));
    assert_eq!(unlisted.list.selected_row(&unlisted.rows), Some(0));
}

#[test]
fn retargeting_lists_previous_branches_first_and_leaves_out_the_current() {
    let previous = vec!["styra/first".to_owned(), "styra/second".to_owned()];
    let branches = vec![
        "main".to_owned(),
        "styra/now".to_owned(),
        "styra/first".to_owned(),
        "release".to_owned(),
    ];
    let mut picker = BranchPicker::retarget(&previous, branches, Some("styra/now"));
    assert_eq!(
        picker.rows,
        vec![
            "styra/second · previous branch",
            "styra/first · previous branch",
            "main",
            "release",
        ]
    );
    assert!(matches!(
        press(&mut picker, KeyCode::Enter),
        BranchChoice::Restore(branch) if branch == "styra/second"
    ));
    for c in "rel".chars() {
        press(&mut picker, KeyCode::Char(c));
    }
    assert!(matches!(
        press(&mut picker, KeyCode::Enter),
        BranchChoice::Retarget(branch) if branch == "release"
    ));
}

#[test]
fn branches_are_listed_newest_commit_first() {
    let root = std::env::temp_dir().join(format!("styra-base-branch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str], date: &str| {
        let status = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_DATE", date)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"], "2026-01-01T00:00:00");
    git(
        &["commit", "-q", "--allow-empty", "-m", "a"],
        "2026-01-01T00:00:00",
    );
    git(&["switch", "-q", "-c", "newer"], "2026-01-01T00:00:00");
    git(
        &["commit", "-q", "--allow-empty", "-m", "b"],
        "2026-03-01T00:00:00",
    );
    git(
        &["switch", "-q", "-c", "middle", "main"],
        "2026-01-01T00:00:00",
    );
    git(
        &["commit", "-q", "--allow-empty", "-m", "c"],
        "2026-02-01T00:00:00",
    );

    assert_eq!(
        local_branches(&root).unwrap(),
        vec!["newer", "middle", "main"]
    );
    assert!(local_branches(&root.join("no-such-directory")).is_err());
    std::fs::remove_dir_all(&root).ok();
}
