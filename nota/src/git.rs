use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Output, Stdio};

pub(crate) fn output(repository: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "running `git {}` in {}",
                args.join(" "),
                repository.display()
            )
        })
}

pub(crate) fn checked(repository: &Path, args: &[&str]) -> Result<String> {
    let result = output(repository, args)?;
    if !result.status.success() {
        bail!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            repository.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

pub(crate) fn checked_with_input(repository: &Path, args: &[&str], input: &str) -> Result<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "running `git {}` in {}",
                args.join(" "),
                repository.display()
            )
        })?;
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("piped git stdin")
        .write_all(input.as_bytes())?;
    let result = child.wait_with_output()?;
    if !result.status.success() {
        bail!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            repository.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

pub(crate) fn repository_root(path: &Path) -> Result<std::path::PathBuf> {
    let root = checked(path, &["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not inside a Git repository", path.display()))?;
    Ok(root.into())
}

pub(crate) fn resolve_commit(repository: &Path, revision: &str) -> Result<String> {
    checked(
        repository,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )
    .with_context(|| format!("resolving Git revision `{revision}`"))
}
