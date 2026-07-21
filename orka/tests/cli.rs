mod common;

use common::workbench;
use orka::config::{CONFIG_FILE, DEFAULT_CONFIG};
use std::process::Command;

#[test]
fn init_creates_the_default_config_and_refuses_to_replace_it() {
    let (_temp, root) = workbench();
    let binary = env!("CARGO_BIN_EXE_orka");

    let first = Command::new(binary)
        .args(["--workbench", root.to_str().unwrap(), "init"])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join(CONFIG_FILE)).unwrap(),
        DEFAULT_CONFIG
    );

    std::fs::write(root.join(CONFIG_FILE), "keep me\n").unwrap();
    let second = Command::new(binary)
        .args(["--workbench", root.to_str().unwrap(), "init"])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
    assert_eq!(
        std::fs::read_to_string(root.join(CONFIG_FILE)).unwrap(),
        "keep me\n"
    );
}

#[test]
fn audit_succeeds_when_every_output_has_evidence() {
    let (_temp, root) = workbench();
    let output = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args(["--workbench", root.to_str().unwrap(), "audit"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("complete evidence"));
}
