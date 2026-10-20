//! The diff taken apart into the steps that made it: each commit from the
//! base to `HEAD`, then what is not committed yet. Each step is a layer laid
//! over the ones before it, and every changed line of the diff knows which
//! layers touched it — the one that wrote it and those whose changes it was
//! written over — so the view can tint it with all of their colours.

use std::collections::HashMap;
use std::path::Path;

use ratatui::style::Color;
use styra_ui::theme;

use crate::files::file_diffs;
use crate::git::Commit;

/// One step from the base to the working tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layer {
    /// The commit's short id, or `uncommitted` for the working tree's step.
    pub name: String,
    /// The commit's summary line; empty for the working tree's step.
    pub summary: String,
    /// Who made the commit; empty for the working tree's step.
    pub author: String,
    /// When the commit was made, relative to when the layers were read;
    /// empty for the working tree's step.
    pub date: String,
    /// The commit message after its summary line; empty for the working
    /// tree's step.
    pub body: String,
}

/// The layers a diff was made in, and which of them each of its changed
/// lines was touched by.
#[derive(Debug, Default)]
pub struct Layers {
    /// Oldest first; a line's layers are indices into this.
    pub layers: Vec<Layer>,
    /// What the layers were read from — the base, each commit, and the
    /// working tree's tree — so they need not be read again while these stay
    /// the same.
    states: Vec<String>,
    /// For each file in the working tree that some layer changed, the layers
    /// that touched each of its lines, in order, by line.
    added: HashMap<String, Vec<Vec<u16>>>,
    /// For each file at the base, the layer that removed each of its lines
    /// that some layer did, by line number.
    removed: HashMap<String, HashMap<u32, u16>>,
}

/// How far, in tenths, a removed line's tint is darkened from an added
/// line's, so the two still read apart where they meet.
const REMOVED_DARKEN: u16 = 4;

impl Layers {
    /// Read the layers between `base` and the working tree at `worktree`.
    pub fn read(worktree: &Path, base: &str) -> Result<Self, String> {
        let (commits, tree) = states(worktree, base)?;
        Self::from_states(worktree, base, commits, tree)
    }

    /// Read the layers again, unless what they were read from is unchanged:
    /// `None` when `previous` still holds.
    pub fn reread(
        previous: Option<&Result<Self, String>>,
        worktree: &Path,
        base: &str,
    ) -> Option<Result<Self, String>> {
        let (commits, tree) = match states(worktree, base) {
            Ok(states) => states,
            Err(error) => {
                let unchanged = matches!(previous, Some(Err(previous)) if *previous == error);
                return (!unchanged).then_some(Err(error));
            }
        };
        if let Some(Ok(previous)) = previous {
            let same = previous.states.len() == commits.len() + 2
                && previous.states[0] == base
                && previous.states[1..]
                    .iter()
                    .zip(commits.iter().map(|commit| &commit.id).chain([&tree]))
                    .all(|(was, is)| was == is);
            if same {
                return None;
            }
        }
        Some(Self::from_states(worktree, base, commits, tree))
    }

    fn from_states(
        worktree: &Path,
        base: &str,
        commits: Vec<Commit>,
        tree: String,
    ) -> Result<Self, String> {
        let mut states = vec![base.to_owned()];
        let mut steps = Vec::new();
        for commit in commits {
            let diff = crate::git::step_diff(worktree, states.last().unwrap(), &commit.id)?;
            let layer = Layer {
                name: commit.id.get(..7).unwrap_or(&commit.id).to_owned(),
                summary: commit.summary,
                author: commit.author,
                date: commit.date,
                body: commit.body,
            };
            states.push(commit.id);
            steps.push((layer, diff));
        }
        let uncommitted = crate::git::step_diff(worktree, states.last().unwrap(), &tree)?;
        states.push(tree);
        // Nothing uncommitted is no layer at all.
        if !uncommitted.trim().is_empty() {
            let layer = Layer {
                name: "uncommitted".to_owned(),
                ..Layer::default()
            };
            steps.push((layer, uncommitted));
        }
        let mut layers = Self::from_steps(&steps);
        layers.states = states;
        Ok(layers)
    }

    /// Lay each step's full-context diff over the ones before it.
    pub(crate) fn from_steps(steps: &[(Layer, String)]) -> Self {
        let mut tracker = Tracker::default();
        for (index, (_, diff)) in steps.iter().enumerate() {
            tracker.step(u16::try_from(index).unwrap_or(u16::MAX), diff);
        }
        Self {
            layers: steps.iter().map(|(layer, _)| layer.clone()).collect(),
            states: Vec::new(),
            added: tracker
                .files
                .into_iter()
                .filter_map(|(path, file)| {
                    let lines = file.lines?;
                    Some((path, lines.into_iter().map(|line| line.layers).collect()))
                })
                .collect(),
            removed: tracker.removed,
        }
    }

    /// For each line of `text`, a diff of the base against the working tree
    /// or one file's part of it, the layers that touched it, oldest first:
    /// none for a line no layer is known to have changed.
    pub(crate) fn marks(&self, text: &str) -> Vec<Vec<u16>> {
        let mut marks: Vec<Vec<u16>> = text
            .lines()
            .take_while(|line| !line.starts_with("diff --git "))
            .map(|_| Vec::new())
            .collect();
        for file in file_diffs(text) {
            let (old, new) = sides(file.patch);
            let added = new.and_then(|new| self.added.get(new));
            let removed = old.and_then(|old| self.removed.get(old));
            // The next old and new line numbers, once a hunk has said.
            let mut next: Option<(u32, u32)> = None;
            for line in file.patch.lines() {
                let mut mark = Vec::new();
                if line.starts_with("@@") {
                    next = hunk_starts(line);
                } else if let Some((old, new)) = next.as_mut() {
                    match line.as_bytes().first() {
                        Some(b'+') => {
                            if let Some(layers) =
                                added.and_then(|added| added.get((*new as usize).wrapping_sub(1)))
                            {
                                mark = layers.clone();
                            }
                            *new += 1;
                        }
                        Some(b'-') => {
                            if let Some(&layer) = removed.and_then(|removed| removed.get(old)) {
                                mark = vec![layer];
                            }
                            *old += 1;
                        }
                        Some(b'\\') => {}
                        _ => {
                            *old += 1;
                            *new += 1;
                        }
                    }
                }
                marks.push(mark);
            }
        }
        marks
    }
}

/// The commits since `base` and the working tree's tree: what the layers
/// are read from.
fn states(worktree: &Path, base: &str) -> Result<(Vec<Commit>, String), String> {
    let commits = crate::git::commits_since(worktree, base);
    let tree = crate::git::worktree_tree(worktree)?;
    Ok((commits, tree))
}

/// The background of a line touched by `layers`, oldest first: the first
/// layer's colour, with each later one laid half over what is below it, so
/// the newest shows strongest and the older ones still show through. A
/// removed line is darker. `None` for a line no layer touched.
pub(crate) fn background(layers: &[u16], removed: bool) -> Option<Color> {
    let (first, rest) = layers.split_first()?;
    let mut colour = rest.iter().fold(layer_colour(*first), |below, &layer| {
        mix(below, layer_colour(layer), 5)
    });
    if removed {
        colour = mix(colour, rgb(theme::CODE_BACKGROUND), REMOVED_DARKEN);
    }
    Some(Color::Rgb(colour.0, colour.1, colour.2))
}

/// Layer `layer`'s own colour: hues a golden angle apart, so neighbouring
/// layers differ the most, dark enough for code to read over.
pub(crate) fn layer_colour(layer: u16) -> (u8, u8, u8) {
    let hue = (210.0 + f32::from(layer) * 137.508) % 360.0;
    hsv(hue, 0.5, 0.38)
}

fn hsv(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let chroma = value * saturation;
    let sector = hue / 60.0;
    let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let lift = value - chroma;
    let channel = |c: f32| ((c + lift) * 255.0).round() as u8;
    (channel(r), channel(g), channel(b))
}

/// `from` moved `tenths` tenths of the way toward `to`.
fn mix(from: (u8, u8, u8), to: (u8, u8, u8), tenths: u16) -> (u8, u8, u8) {
    let channel =
        |from: u8, to: u8| ((u16::from(from) * (10 - tenths) + u16::from(to) * tenths) / 10) as u8;
    (
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

fn rgb(colour: Color) -> (u8, u8, u8) {
    match colour {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// A line of a file as the layers so far have left it.
#[derive(Clone, Debug)]
struct Line {
    /// Its number at the base, while no layer has changed it.
    origin: Option<u32>,
    /// The layers that touched it, oldest first.
    layers: Vec<u16>,
}

/// A file as the layers so far have left it.
#[derive(Debug)]
struct File {
    /// Its path at the base, if it was there and has not been made anew.
    base: Option<String>,
    /// Its lines, once a layer has changed them; until then they are all
    /// still the base's, however many there are.
    lines: Option<Vec<Line>>,
}

/// Every file the layers laid so far have touched.
#[derive(Default)]
struct Tracker {
    /// By current path.
    files: HashMap<String, File>,
    /// See [`Layers::removed`].
    removed: HashMap<String, HashMap<u32, u16>>,
}

impl Tracker {
    /// Lay layer `layer`'s diff, with full context, over the files.
    fn step(&mut self, layer: u16, diff: &str) {
        let mut moved = Vec::new();
        for file in file_diffs(diff) {
            let (old, new) = sides(file.patch);
            let mut tracked = match old {
                // Not touched before, so as it was at the base.
                Some(old) => self.files.remove(old).unwrap_or_else(|| File {
                    base: Some(old.to_owned()),
                    lines: None,
                }),
                None => File {
                    base: None,
                    lines: Some(Vec::new()),
                },
            };
            self.apply(layer, &mut tracked, file.patch);
            // Put back once every file has been taken, so a file moved onto
            // the path of another that moved on is not taken for it.
            if let Some(new) = new {
                moved.push((new.to_owned(), tracked));
            }
        }
        self.files.extend(moved);
    }

    fn apply(&mut self, layer: u16, file: &mut File, patch: &str) {
        let hunks = hunks(patch);
        if hunks.is_empty() {
            return;
        }
        let old = file.lines.take().unwrap_or_else(|| {
            // With full context the hunks cover the whole old file.
            let length = hunks
                .iter()
                .map(|hunk| {
                    hunk.old_start
                        + hunk.lines.iter().filter(|l| !l.starts_with('+')).count() as u32
                })
                .max()
                .unwrap_or(1)
                .saturating_sub(1);
            (1..=length)
                .map(|number| Line {
                    origin: Some(number),
                    layers: Vec::new(),
                })
                .collect()
        });
        let mut lines = Vec::with_capacity(old.len());
        let mut cursor = 0;
        for hunk in hunks {
            // A hunk adding to an empty stretch names the line before it.
            let before = match hunk.old_empty {
                true => hunk.old_start,
                false => hunk.old_start.saturating_sub(1),
            };
            let start = cursor.min(old.len());
            let before = (before as usize).clamp(start, old.len());
            lines.extend_from_slice(&old[start..before]);
            cursor = before;
            let mut code = hunk.lines.iter().peekable();
            while let Some(line) = code.next() {
                if line.starts_with(' ') || line.is_empty() {
                    if let Some(kept) = old.get(cursor) {
                        lines.push(kept.clone());
                    }
                    cursor += 1;
                    continue;
                }
                // A block of removals and additions between unchanged lines.
                let mut removed = usize::from(line.starts_with('-'));
                let mut added = usize::from(line.starts_with('+'));
                while let Some(next) = code.next_if(|next| next.starts_with(['-', '+'])) {
                    if next.starts_with('-') {
                        removed += 1;
                    } else {
                        added += 1;
                    }
                }
                let gone = &old[cursor.min(old.len())..(cursor + removed).min(old.len())];
                cursor += removed;
                for line in gone {
                    if let (Some(origin), Some(base)) = (line.origin, &file.base) {
                        self.removed
                            .entry(base.clone())
                            .or_default()
                            .insert(origin, layer);
                    }
                }
                // What the block is written over: the lines it replaces, or
                // for lines only inserted, what is on both sides of them.
                let mut under: Vec<u16> = if gone.is_empty() {
                    match (lines.last(), old.get(cursor)) {
                        (Some(above), Some(below)) => above
                            .layers
                            .iter()
                            .copied()
                            .filter(|layer| below.layers.contains(layer))
                            .collect(),
                        _ => Vec::new(),
                    }
                } else {
                    gone.iter()
                        .flat_map(|line| line.layers.iter().copied())
                        .collect()
                };
                under.sort_unstable();
                under.dedup();
                under.retain(|&under| under != layer);
                under.push(layer);
                lines.extend((0..added).map(|_| Line {
                    origin: None,
                    layers: under.clone(),
                }));
            }
        }
        lines.extend_from_slice(&old[cursor.min(old.len())..]);
        file.lines = Some(lines);
    }
}

/// One hunk of a patch: where it starts in the old file and its lines,
/// `\ No newline at end of file` left out.
struct Hunk<'a> {
    old_start: u32,
    /// Whether it covers no line of the old file, as when adding to an
    /// empty one: its start is then the line before it.
    old_empty: bool,
    lines: Vec<&'a str>,
}

fn hunks(patch: &str) -> Vec<Hunk<'_>> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("@@") {
            let (old_start, old_empty) = old_range(line).unwrap_or((0, true));
            hunks.push(Hunk {
                old_start,
                old_empty,
                lines: Vec::new(),
            });
        } else if let Some(hunk) = hunks.last_mut() {
            if !line.starts_with('\\') {
                hunk.lines.push(line);
            }
        }
    }
    hunks
}

/// A hunk header's old start, and whether its old side is empty.
fn old_range(header: &str) -> Option<(u32, bool)> {
    let old = header.strip_prefix("@@ -")?.split_whitespace().next()?;
    Some(match old.split_once(',') {
        Some((start, length)) => (start.parse().ok()?, length == "0"),
        None => (old.parse().ok()?, false),
    })
}

/// A hunk header's old and new starts.
fn hunk_starts(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.strip_prefix("@@ ")?.split_whitespace();
    let start = |part: Option<&str>, sign: char| -> Option<u32> {
        part?.strip_prefix(sign)?.split(',').next()?.parse().ok()
    };
    Some((start(parts.next(), '-')?, start(parts.next(), '+')?))
}

/// A file patch's path before and after: `None` on the side it is missing
/// from, as for a file added or deleted.
fn sides(patch: &str) -> (Option<&str>, Option<&str>) {
    let headers: Vec<&str> = patch
        .lines()
        .take_while(|line| !line.starts_with("@@"))
        .collect();
    let has = |prefix: &str| headers.iter().any(|line| line.starts_with(prefix));
    let found = |prefix: &str| {
        headers
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
            .map(|path| path.trim_matches('"'))
    };
    let path = file_diffs(patch).first().map(|file| file.path);
    let old = if has("new file mode ") || has("--- /dev/null") {
        None
    } else {
        found("--- a/").or_else(|| found("rename from ")).or(path)
    };
    let new = if has("deleted file mode ") || has("+++ /dev/null") {
        None
    } else {
        found("+++ b/").or_else(|| found("rename to ")).or(path)
    };
    (old, new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(name: &str) -> Layer {
        Layer {
            name: name.into(),
            ..Layer::default()
        }
    }

    /// A line written over another layer's carries both; one written fresh
    /// carries its own; a removed base line carries the layer removing it.
    #[test]
    fn lines_carry_the_layers_they_were_laid_over() {
        let first =
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,5 @@\n a\n-b\n+B1\n+B2\n c\n d\n+e\n";
        // B2 rewritten, a line inserted inside the first layer's block, and
        // one before the base's line c.
        let second = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,6 +1,7 @@\n a\n B1\n-B2\n+B3\n c\n+x\n d\n e\n";
        let layers =
            Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
        let added = &layers.added["f"];
        let marks: Vec<_> = added.iter().map(Vec::as_slice).collect();
        assert_eq!(
            marks,
            [&[][..], &[0], &[0, 1], &[], &[1], &[], &[0]],
            "a, B1, B3, c, x, d, e"
        );
        assert_eq!(layers.removed["f"], HashMap::from([(2, 0)]));

        let final_diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,7 @@\n a\n-b\n+B1\n+B3\n c\n+x\n d\n+e\n";
        assert_eq!(
            layers.marks(final_diff),
            [
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![0],
                vec![0],
                vec![0, 1],
                vec![],
                vec![1],
                vec![],
                vec![0],
            ]
        );
    }

    /// A line inserted inside a block another layer wrote is laid over it.
    #[test]
    fn an_insertion_inside_an_earlier_block_is_laid_over_it() {
        let first = "diff --git a/f b/f\nnew file mode 100644\n--- /dev/null\n+++ b/f\n@@ -0,0 +1,2 @@\n+a\n+b\n";
        let second = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,4 @@\n a\n+x\n b\n+y\n";
        let layers =
            Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
        let marks: Vec<_> = layers.added["f"].iter().map(Vec::as_slice).collect();
        assert_eq!(marks, [&[0][..], &[0, 1], &[0], &[1]]);
    }

    /// A file moved keeps its layers under its new name; one deleted leaves
    /// its base lines marked removed by the layer that deleted it.
    #[test]
    fn moves_and_deletions_keep_track_of_the_file() {
        let first = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n-a\n+A\n b\n\
                     diff --git a/gone b/gone\ndeleted file mode 100644\n--- a/gone\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n";
        let second = "diff --git a/f b/g\nsimilarity index 100%\nrename from f\nrename to g\n";
        let layers =
            Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
        assert!(!layers.added.contains_key("f"));
        let marks: Vec<_> = layers.added["g"].iter().map(Vec::as_slice).collect();
        assert_eq!(marks, [&[0][..], &[]]);
        assert_eq!(layers.removed["gone"], HashMap::from([(1, 0)]));
        assert_eq!(layers.removed["f"], HashMap::from([(1, 0)]));
    }

    #[test]
    fn later_layers_show_strongest_and_removals_darker() {
        assert_eq!(background(&[], false), None);
        let (one, two) = (layer_colour(0), layer_colour(1));
        assert_ne!(one, two);
        assert_eq!(
            background(&[0], false),
            Some(Color::Rgb(one.0, one.1, one.2))
        );
        let both = mix(one, two, 5);
        assert_eq!(
            background(&[0, 1], false),
            Some(Color::Rgb(both.0, both.1, both.2))
        );
        let Some(Color::Rgb(r, g, b)) = background(&[0], true) else {
            panic!("an RGB colour");
        };
        assert!(
            u16::from(r) + u16::from(g) + u16::from(b)
                < u16::from(one.0) + u16::from(one.1) + u16::from(one.2)
        );
    }

    /// Read from a real repository: two commits and uncommitted work, each
    /// over the one before.
    #[test]
    fn layers_are_read_from_each_commit_and_the_working_tree() {
        fn git(directory: &Path, args: &[&str]) -> String {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(directory)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }
        let root = std::env::temp_dir().join("styra-diff-layers");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("f"), "a\nb\nc\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "start"]);
        let base = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("f"), "a\nB\nc\n").unwrap();
        git(
            &root,
            &[
                "commit",
                "-q",
                "-am",
                "first",
                "-m",
                "why, at length",
                "-m",
                "and more",
            ],
        );
        std::fs::write(root.join("f"), "a\nBB\nc\nd\n").unwrap();
        git(&root, &["commit", "-q", "-am", "second"]);
        std::fs::write(root.join("f"), "a\nBBB\nc\nd\n").unwrap();
        std::fs::write(root.join("new"), "n\n").unwrap();

        let layers = Layers::read(&root, &base).unwrap();
        assert_eq!(
            layers
                .layers
                .iter()
                .map(|layer| layer.summary.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", ""]
        );
        assert_eq!(layers.layers[2].name, "uncommitted");
        assert_eq!(layers.layers[0].author, "t");
        assert_eq!(layers.layers[0].summary, "first");
        assert_eq!(layers.layers[0].body, "why, at length\n\nand more");
        assert_eq!(layers.layers[1].body, "");
        assert!(
            layers.layers[0].date.ends_with("ago"),
            "{:?}",
            layers.layers[0]
        );
        assert!(layers.layers[2].author.is_empty());
        let diff = crate::git::diff(&root, &base).unwrap();
        let marked: Vec<_> = diff
            .lines()
            .zip(layers.marks(&diff))
            .filter(|(_, mark)| !mark.is_empty())
            .collect();
        assert_eq!(
            marked,
            [
                ("-b", vec![0]),
                ("+BBB", vec![0, 1, 2]),
                ("+d", vec![1]),
                ("+n", vec![2]),
            ]
        );
        let layers = Ok(layers);
        assert!(Layers::reread(Some(&layers), &root, &base).is_none());
        std::fs::write(root.join("f"), "a\nBBB\nc\n").unwrap();
        assert!(Layers::reread(Some(&layers), &root, &base).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
