use super::*;

fn workspace() -> Placeholders {
    Placeholders::default().with(Some(Path::new("/home/me/proj")), WORKSPACE)
}

#[test]
fn only_whole_paths_are_replaced() {
    let ws = workspace();
    assert_eq!(ws.apply("/home/me/proj"), "<ws>");
    assert_eq!(
        ws.apply("/home/me/proj → /home/me/proj/src (rw)"),
        "<ws> → <ws>/src (rw)"
    );
    assert_eq!(ws.apply("/home/me/project"), "/home/me/project");
    assert_eq!(ws.apply("/mnt/home/me/proj"), "/mnt/home/me/proj");
    assert_eq!(ws.apply("PWD=/home/me/proj:"), "PWD=<ws>:");
}

#[test]
fn a_worktree_is_named_under_its_own_placeholder() {
    let both = workspace().with(Some(Path::new("/store/workspaces/w1/worktrees")), WORKTREES);
    assert_eq!(
        both.apply("/store/workspaces/w1/worktrees/fix-retries/src · linked from /home/me/proj"),
        "<wt>/fix-retries/src · linked from <ws>"
    );
}

#[test]
fn the_more_specific_of_two_nested_prefixes_wins() {
    let nested = workspace().with(Some(Path::new("/home/me/proj/.worktrees")), WORKTREES);
    assert_eq!(nested.apply("/home/me/proj/.worktrees/a"), "<wt>/a");
    assert_eq!(nested.apply("/home/me/proj/src"), "<ws>/src");
}
