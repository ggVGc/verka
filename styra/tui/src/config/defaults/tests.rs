use super::*;
use std::ffi::OsStr;

fn argv(command: &Command) -> Vec<&OsStr> {
    command.get_args().collect()
}

#[test]
fn default_commands_use_the_expected_program_and_arguments() {
    for (name, command, program, args) in [
        (
            "file at line",
            Defaults.open_file_at(Path::new("/work/src/monitor.c"), 42),
            "urxvt",
            vec!["-e", "nvim", "+42", "/work/src/monitor.c"],
        ),
        (
            "file",
            Defaults.open_file(Path::new("/work/src/monitor.c")),
            "urxvt",
            vec!["-e", "nvim", "/work/src/monitor.c"],
        ),
        (
            "URL",
            Defaults.open_url("https://example.com/a"),
            "firefox",
            vec!["https://example.com/a"],
        ),
        (
            "shell",
            Defaults.open_terminal(&[
                OsString::from("/usr/bin/tmux"),
                OsString::from("attach-session"),
            ]),
            "urxvt",
            vec!["-e", "/usr/bin/tmux", "attach-session"],
        ),
    ] {
        assert_eq!(command.get_program(), OsStr::new(program), "{name}");
        assert_eq!(
            argv(&command),
            args.iter().map(OsStr::new).collect::<Vec<_>>(),
            "{name}"
        );
    }
}

#[test]
fn the_interaction_directory_opens_in_fish() {
    assert_eq!(Defaults.shell(), [OsString::from("fish")]);
}
