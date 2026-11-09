//! Noticing that a checkout's diff may have changed, so a view of it can be
//! read again: the files it shows are watched for writes, and `git status`
//! is asked now and then for files it does not show yet and commits made
//! since.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// How often `git status` is asked whether the working tree has changed in a
/// way the watched files would not say: a file newly changed, a commit made.
const STATUS_INTERVAL: Duration = Duration::from_secs(2);

/// Changes to a checkout's working tree since [`Watch::changed`] last asked.
pub struct Watch {
    worktree: PathBuf,
    /// Says, from a thread of its own, each time `git status` answers
    /// differently. It stops once this is dropped.
    status: Receiver<()>,
    #[cfg(target_os = "linux")]
    files: Option<files::Files>,
}

impl Watch {
    /// Start watching the working tree at `worktree`, its status as it is
    /// now being what later answers are compared with — so the diff should
    /// be read after this, not before.
    pub fn new(worktree: &Path) -> Self {
        let (sender, status) = mpsc::channel();
        let directory = worktree.to_owned();
        let mut last = status_of(&directory);
        std::thread::spawn(move || loop {
            std::thread::sleep(STATUS_INTERVAL);
            let now = status_of(&directory);
            if now != last {
                last = now;
                if sender.send(()).is_err() {
                    return;
                }
            }
        });
        Self {
            worktree: worktree.to_owned(),
            status,
            // Without inotify the status is still asked; only a further edit
            // to a file already changed goes unnoticed.
            #[cfg(target_os = "linux")]
            files: files::Files::new().ok(),
        }
    }

    /// Watch the files at `paths`, relative to the working tree, in place of
    /// those watched before: the files the diff shows.
    pub fn follow<'a>(&mut self, paths: impl IntoIterator<Item = &'a str>) {
        #[cfg(target_os = "linux")]
        if let Some(files) = &mut self.files {
            files.follow(&self.worktree, paths);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = paths;
    }

    /// Whether a watched file was written, moved or removed, or the status
    /// changed, since this was last asked. Never waits.
    pub fn changed(&mut self) -> bool {
        let mut changed = false;
        while let Ok(()) = self.status.try_recv() {
            changed = true;
        }
        #[cfg(target_os = "linux")]
        if let Some(files) = &mut self.files {
            changed |= files.changed();
        }
        changed
    }
}

/// The working tree's status, untracked files listed one by one, with the
/// commit `HEAD` is at. Asked without the optional locks, so the real index
/// is not rewritten just to answer.
fn status_of(worktree: &Path) -> Option<Vec<u8>> {
    std::process::Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(worktree)
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
            "-z",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .map(|output| output.stdout)
}

#[cfg(target_os = "linux")]
mod files {
    use std::collections::{HashMap, HashSet};
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};

    use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

    /// The directories the watched files are in, each watched for its
    /// entries being written, made, moved or removed. A directory rather than
    /// the file itself, since an editor that saves by writing a new file over
    /// the old leaves a watch on the old one hearing nothing more.
    pub(super) struct Files {
        inotify: Inotify,
        /// Each watched directory's watch and the names in it that count.
        directories: HashMap<PathBuf, (WatchDescriptor, HashSet<OsString>)>,
    }

    impl Files {
        pub(super) fn new() -> io::Result<Self> {
            Ok(Self {
                inotify: Inotify::init()?,
                directories: HashMap::new(),
            })
        }

        pub(super) fn follow<'a>(
            &mut self,
            worktree: &Path,
            paths: impl IntoIterator<Item = &'a str>,
        ) {
            let mut wanted: HashMap<PathBuf, HashSet<OsString>> = HashMap::new();
            for path in paths {
                let path = worktree.join(path);
                if let (Some(directory), Some(name)) = (path.parent(), path.file_name()) {
                    wanted
                        .entry(directory.to_owned())
                        .or_default()
                        .insert(name.to_owned());
                }
            }
            self.directories
                .retain(|directory, (watch, names)| match wanted.remove(directory) {
                    Some(now) => {
                        *names = now;
                        true
                    }
                    None => {
                        let _ = self.inotify.rm_watch(watch.clone());
                        false
                    }
                });
            for (directory, names) in wanted {
                // A directory that is not there — a removed file's — has
                // nothing to hear; the status says if it comes back.
                if let Ok(watch) = self.inotify.add_watch(
                    &directory,
                    WatchMask::CLOSE_WRITE
                        | WatchMask::CREATE
                        | WatchMask::DELETE
                        | WatchMask::MOVED_FROM
                        | WatchMask::MOVED_TO,
                ) {
                    self.directories.insert(directory, (watch, names));
                }
            }
        }

        pub(super) fn changed(&mut self) -> bool {
            let mut changed = false;
            let mut buffer = [0u8; 4096];
            loop {
                let Ok(events) = self.inotify.read_events(&mut buffer) else {
                    return changed;
                };
                let mut any = false;
                for event in events {
                    any = true;
                    // Events were lost, any of them perhaps a watched file's.
                    if event.mask.contains(EventMask::Q_OVERFLOW) {
                        changed = true;
                        continue;
                    }
                    let watched = self
                        .directories
                        .iter()
                        .find(|(_, (watch, _))| *watch == event.wd);
                    let Some((directory, (_, names))) = watched else {
                        continue;
                    };
                    // The directory itself is gone; the next follow watches
                    // it again if it is back.
                    if event.mask.contains(EventMask::IGNORED) {
                        let directory = directory.clone();
                        self.directories.remove(&directory);
                        continue;
                    }
                    if event.name.is_some_and(|name| names.contains(name)) {
                        changed = true;
                    }
                }
                if !any {
                    return changed;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{git, repository};

    /// Waits a while for the watch to notice a change, as the status is
    /// asked only now and then.
    fn notices(watch: &mut Watch) -> bool {
        let deadline = std::time::Instant::now() + STATUS_INTERVAL * 3;
        while std::time::Instant::now() < deadline {
            if watch.changed() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_shown_file_written_again_is_noticed_at_once() {
        let (_temp, root) = repository("styra-diff-watch-files");
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/shown.txt"), "one\n").unwrap();
        std::fs::write(root.join("src/other.txt"), "one\n").unwrap();
        let mut watch = Watch::new(&root);
        watch.follow(["src/shown.txt"]);
        assert!(!watch.changed());

        std::fs::write(root.join("src/other.txt"), "two\n").unwrap();
        assert!(!watch.files.as_mut().unwrap().changed());
        std::fs::write(root.join("src/shown.txt"), "two\n").unwrap();
        assert!(watch.files.as_mut().unwrap().changed());

        // Saved as an editor that writes a new file over the old one does.
        std::fs::write(root.join("src/.shown.txt.swp"), "three\n").unwrap();
        std::fs::rename(root.join("src/.shown.txt.swp"), root.join("src/shown.txt")).unwrap();
        assert!(watch.files.as_mut().unwrap().changed());

        watch.follow(["src/other.txt"]);
        std::fs::write(root.join("src/shown.txt"), "four\n").unwrap();
        assert!(!watch.files.as_mut().unwrap().changed());
    }

    #[test]
    fn a_file_newly_changed_or_a_commit_is_noticed_by_the_status() {
        let (_temp, root) = repository("styra-diff-watch-status");
        let mut watch = Watch::new(&root);
        assert!(!watch.changed());

        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        assert!(notices(&mut watch));
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "new"]);
        assert!(notices(&mut watch));
    }
}
