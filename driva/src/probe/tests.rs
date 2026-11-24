use super::*;

/// The isolated side answers the two network checks and rejects anything
/// else, since the sentinel is Driva talking to itself.
#[test]
fn the_isolated_side_answers_only_known_probes() {
    assert!(run_in_isolation("nonsense").is_err());
    assert!(run_in_isolation("resolve:localhost").is_ok());
}

/// A `run` probe needs nothing mounted; a network probe carries Driva's
/// own binary in to answer it.
#[test]
fn a_network_probe_brings_the_binary_that_answers_it() {
    let (command, mounts) = probe_command(&Probe::Run(vec!["/bin/true".into()])).unwrap();
    assert_eq!(command, vec![OsString::from("/bin/true")]);
    assert!(mounts.is_empty());

    let (command, mounts) = probe_command(&Probe::Resolve("example.com".into())).unwrap();
    assert_eq!(command.len(), 1);
    assert!(matches!(
        &mounts[..],
        [Mount::Bind { source, access: MountAccess::ReadOnly, .. }] if source == &std::env::current_exe().unwrap()
    ));
    assert_eq!(
        probe_environment(&Probe::Resolve("example.com".into())).get(&OsString::from(PROBE_ENV)),
        Some(&OsString::from("resolve:example.com"))
    );
}
