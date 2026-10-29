use ratatui::{backend::TestBackend, buffer::Buffer, Frame, Terminal};

pub(crate) fn draw(width: u16, height: u16, render: impl FnOnce(&mut Frame)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(render).unwrap().buffer.clone()
}

pub(crate) fn rows(buffer: &Buffer) -> Vec<String> {
    (buffer.area.top()..buffer.area.bottom())
        .map(|y| {
            (buffer.area.left()..buffer.area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

pub(crate) fn git(directory: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub(crate) fn repository(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::Builder::new().prefix(name).tempdir().unwrap();
    let root = directory.path().to_owned();
    git(&root, &["init", "-q", "-b", "main"]);
    (directory, root)
}
