use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Resolve HEAD through Git so linked worktrees watch their own HEAD.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    let branch = git(&["branch", "--show-current"])
        .map(|branch| {
            if branch.is_empty() {
                "detached".to_owned()
            } else {
                branch
            }
        })
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=STYRA_BUILD_BRANCH={branch}");
}
