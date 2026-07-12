use driva::{validate_request, ExecutionRequest, Mount, MountAccess};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

fn request() -> ExecutionRequest {
    ExecutionRequest {
        command: vec![OsString::from("true")],
        working_directory: PathBuf::from("/workspace"),
        mounts: vec![],
        environment: BTreeMap::new(),
        network: false,
        interactive: false,
    }
}

#[test]
fn defaults_are_deny_by_default() {
    let validated = validate_request(&request()).unwrap();
    assert!(validated.mounts.is_empty());
    assert!(!validated.network);
}

#[test]
fn rejects_relative_and_conflicting_destinations() {
    let source = std::env::current_dir().unwrap();
    let mut value = request();
    value.mounts.push(Mount {
        source: source.clone(),
        destination: "relative".into(),
        access: MountAccess::ReadOnly,
    });
    assert!(validate_request(&value).is_err());

    value.mounts = vec![
        Mount {
            source: source.clone(),
            destination: "/same".into(),
            access: MountAccess::ReadOnly,
        },
        Mount {
            source,
            destination: "/same".into(),
            access: MountAccess::ReadWrite,
        },
    ];
    assert!(validate_request(&value).is_err());
}
