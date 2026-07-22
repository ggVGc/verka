use driva::{ExecutionRequest, Mount, MountAccess, PodmanIsolation};
use std::collections::BTreeMap;
use std::ffi::OsString;

#[test]
fn translates_request_without_implicit_capabilities() {
    let backend = PodmanIsolation {
        executable: "podman".into(),
        image: "example:test".into(),
    };
    let request = ExecutionRequest {
        command: vec!["printf".into(), "hello".into()],
        working_directory: "/work".into(),
        mounts: vec![
            Mount::Temporary {
                destination: "/state".into(),
            },
            Mount::Bind {
                source: "/host".into(),
                destination: "/work".into(),
                access: MountAccess::ReadWrite,
            },
        ],
        environment: BTreeMap::from([(OsString::from("A"), OsString::from("B"))]),
        network: false,
        interactive: true,
    };
    let command = backend.command(&request);
    let args: Vec<_> = command
        .get_args()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "run",
            "--rm",
            "-i",
            "-t",
            "--network",
            "none",
            "--workdir",
            "/work",
            "--tmpfs",
            "/state",
            "--volume",
            "/host:/work",
            "--env",
            "A=B",
            "example:test",
            "printf",
            "hello"
        ]
    );
}
