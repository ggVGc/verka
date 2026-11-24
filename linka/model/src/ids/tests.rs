use super::*;

#[test]
fn graph_identifiers_and_project_paths_are_validated_and_normalized() {
    for invalid in [
        "",
        ".",
        "..",
        "../secret",
        "/absolute",
        r"..\secret",
        ".git",
        "C:node",
        "bad\nnode",
    ] {
        assert!(
            invalid.parse::<NodeId>().is_err(),
            "accepted node id {invalid:?}"
        );
    }
    assert_eq!("node-good".parse::<NodeId>().unwrap().as_str(), "node-good");

    for invalid in [
        "",
        "..",
        "../secret",
        "/absolute",
        r"..\secret",
        ".git/config",
        "src/.git/config",
        "C:/windows",
        "bad\npath",
    ] {
        assert!(
            invalid.parse::<ProjectPath>().is_err(),
            "accepted project path {invalid:?}"
        );
    }
    assert_eq!(
        r"src\nested\file.rs"
            .parse::<ProjectPath>()
            .unwrap()
            .as_str(),
        "src/nested/file.rs"
    );
}
