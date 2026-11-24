use super::*;
use crate::test_support::{git, repository};

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
    let layers = Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
    let added = &layers.added["f"];
    let marks: Vec<_> = added.iter().map(Vec::as_slice).collect();
    assert_eq!(
        marks,
        [&[][..], &[0], &[0, 1], &[], &[1], &[], &[0]],
        "a, B1, B3, c, x, d, e"
    );
    assert_eq!(layers.removed["f"], HashMap::from([(2, 0)]));

    let final_diff =
        "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,7 @@\n a\n-b\n+B1\n+B3\n c\n+x\n d\n+e\n";
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
    let layers = Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
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
    let layers = Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
    assert!(!layers.added.contains_key("f"));
    let marks: Vec<_> = layers.added["g"].iter().map(Vec::as_slice).collect();
    assert_eq!(marks, [&[0][..], &[]]);
    assert_eq!(layers.removed["gone"], HashMap::from([(1, 0)]));
    assert_eq!(layers.removed["f"], HashMap::from([(1, 0)]));
}

#[test]
fn file_layers_include_moves_through_later_edits_and_moves() {
    let moved = "diff --git a/old name b/new name\nsimilarity index 100%\nrename from old name\nrename to new name\n";
    let edited = "diff --git a/new name b/new name\n--- a/new name\n+++ b/new name\n@@ -1 +1 @@\n-old\n+new\n";
    let moved_again = "diff --git a/new name b/final name\nsimilarity index 100%\nrename from new name\nrename to final name\n";
    let unrelated = "diff --git a/other b/other\n@@ -1 +1 @@\n-x\n+y\n";
    let mut steps = vec![(layer("move"), moved.into())];
    let layers = Layers::from_steps(&steps);
    assert_eq!(layers.file_layers(moved), [0]);
    assert!(layers.marks(moved).iter().all(Vec::is_empty));

    steps.extend([
        (layer("edit"), edited.into()),
        (layer("move again"), moved_again.into()),
        (layer("other"), unrelated.into()),
    ]);
    let layers = Layers::from_steps(&steps);
    let combined = "diff --git a/old name b/final name\nrename from old name\nrename to final name\n--- a/old name\n+++ b/final name\n@@ -1 +1 @@\n-old\n+new\n";
    assert_eq!(layers.file_layers(combined), [0, 1, 2]);
    assert_eq!(layers.file_layers(unrelated), [3]);
    assert_eq!(layers.marks(combined).last().unwrap(), &[1]);
}

#[cfg(feature = "renderer")]
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
    let (_directory, root) = repository("styra-diff-layers");
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
