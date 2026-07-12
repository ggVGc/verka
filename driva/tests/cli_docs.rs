//! Keeps docs/cli.md in sync with the compiled command-line interface.
//!
//! Every ```console block in the document holds a `$ driva ...` command
//! followed by its verbatim stdout. This test re-runs each command and fails
//! on any difference. Regenerate the blocks with:
//!
//! ```sh
//! DRIVA_UPDATE_DOCS=1 cargo test --test cli_docs
//! ```

use std::path::PathBuf;
use std::process::Command;

fn document_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/cli.md")
}

fn driva_stdout(arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .args(arguments)
        .output()
        .expect("failed to execute the driva binary");
    assert!(
        output.status.success(),
        "`driva {}` failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("driva emitted non-UTF-8 help output")
}

/// Rebuild the document, replacing the body of every console block with the
/// current output of the command on its `$ driva ...` line.
fn regenerate(document: &str) -> String {
    let mut result = String::new();
    let mut lines = document.lines().peekable();
    let mut found_blocks = 0;
    while let Some(line) = lines.next() {
        result.push_str(line);
        result.push('\n');
        if line.trim() != "```console" {
            continue;
        }
        let command_line = lines
            .next()
            .expect("console block is missing its command line");
        let arguments: Vec<&str> = command_line
            .strip_prefix("$ driva")
            .unwrap_or_else(|| panic!("console block must start with `$ driva`: {command_line}"))
            .split_whitespace()
            .collect();
        result.push_str(command_line);
        result.push('\n');
        result.push_str(&driva_stdout(&arguments));
        for stale in lines.by_ref() {
            if stale.trim() == "```" {
                result.push_str(stale);
                result.push('\n');
                break;
            }
        }
        found_blocks += 1;
    }
    assert!(found_blocks > 0, "docs/cli.md contains no console blocks");
    result
}

#[test]
fn cli_reference_matches_binary() {
    let path = document_path();
    let document = std::fs::read_to_string(&path).expect("failed to read docs/cli.md");
    let expected = regenerate(&document);
    if std::env::var_os("DRIVA_UPDATE_DOCS").is_some() {
        if expected != document {
            std::fs::write(&path, &expected).expect("failed to update docs/cli.md");
        }
        return;
    }
    assert!(
        expected == document,
        "docs/cli.md is out of date with the compiled CLI; \
         run `DRIVA_UPDATE_DOCS=1 cargo test --test cli_docs` and review the result"
    );
}
