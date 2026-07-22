use driva::{DockerIsolation, ExecutionRequest, Mount, MountAccess};
use std::collections::BTreeMap;
use std::ffi::OsString;

#[test]
fn translates_request_without_implicit_capabilities() {
    let backend = DockerIsolation {
        executable: "docker".into(),
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
                access: MountAccess::ReadOnly,
            },
        ],
        environment: BTreeMap::from([(OsString::from("A"), OsString::from("B"))]),
        network: false,
        interactive: false,
    };
    let command = backend.command(&request);
    let args: Vec<_> = command
        .get_args()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "run",
            "--rm",
            "--network",
            "none",
            "--workdir",
            "/work",
            "--tmpfs",
            "/state",
            "--volume",
            "/host:/work:ro",
            "--env",
            "A=B",
            "example:test",
            "printf",
            "hello"
        ]
    );
}
