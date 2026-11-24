use super::*;

fn test_entry(at: &str) -> Entry {
    Entry {
        at: PathBuf::from(at),
        optional: false,
        doc: "test path",
    }
}

/// Every static capability has the metadata used by the CLI.
#[test]
fn the_built_in_capabilities_are_well_formed() {
    for name in DEFAULT_CAPABILITIES {
        let capability = capability_info(name);
        assert!(
            !capability.description.is_empty(),
            "{name} has no description"
        );
        assert!(!capability.paths.is_empty(), "{name} names no paths");
    }
}

/// Every static path says what needs it. A path list nobody can read is
/// how the constant this replaced went wrong: `/etc/ld.so.cache` and
/// `/etc/alternatives` are alike as strings and unrelated in purpose, and
/// an operator trimming a capability or porting it to their own host is
/// asking exactly what each one is for. Enforced rather than trusted, so a
/// path added later cannot arrive unexplained.
#[test]
fn every_built_in_path_documents_what_needs_it() {
    for name in DEFAULT_CAPABILITIES {
        for entry in &capability_info(name).paths {
            assert!(
                entry.doc.len() > 20,
                "{name} carries {} without saying what needs it",
                entry.at.display()
            );
        }
    }
}

/// The default base is what a host with no configuration gets, so it has
/// to resolve on the machine running the tests.
#[test]
fn the_default_base_resolves_on_this_host() {
    let base = resolve_base(&BaseConfig::default()).unwrap();
    assert!(base.entries().count() > 5);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    assert!(
        base.entries().all(|entry| !home
            .as_ref()
            .is_some_and(|home| entry.path().starts_with(home))),
        "the base must not carry anything from the operator's home"
    );
}

/// A path the host does not have is only skipped when the capability said
/// it could be: the whole point of stating a floor is that a hole in it is
/// an error someone sees.
#[test]
fn a_required_path_that_is_missing_fails_the_base() {
    let missing = "/nonexistent/driva/base";
    let error = resolve_entry(&test_entry(missing), &Base::default(), &[]).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains(missing), "{message}");

    let optional = Entry {
        optional: true,
        ..test_entry(missing)
    };
    assert!(resolve_entry(&optional, &Base::default(), &[])
        .unwrap()
        .is_none());
}

/// A link into the base keeps its shape; a link out of it is followed, so
/// the sandbox gets content rather than a link to nothing.
#[test]
fn a_link_is_reproduced_only_while_it_lands_inside_the_base() {
    let root = std::env::temp_dir().join(format!("driva-base-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let carried = root.join("carried");
    let outside = root.join("outside");
    std::fs::create_dir_all(&carried).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(carried.join("file"), b"in").unwrap();
    std::fs::write(outside.join("file"), b"out").unwrap();
    let inward = root.join("inward-link");
    let outward = root.join("outward-link");
    std::os::unix::fs::symlink(carried.join("file"), &inward).unwrap();
    std::os::unix::fs::symlink(outside.join("file"), &outward).unwrap();

    let mut entries = Vec::new();
    for entry in [
        test_entry(&carried.to_string_lossy()),
        test_entry(&inward.to_string_lossy()),
        test_entry(&outward.to_string_lossy()),
    ] {
        entries.push(
            resolve_entry(&entry, &Base::default(), &entries)
                .unwrap()
                .unwrap(),
        );
    }
    assert!(matches!(&entries[1], RuntimeEntry::Symlink { path, .. } if path == &inward));
    assert!(matches!(
        &entries[2],
        RuntimeEntry::ReadOnly { source, path }
            if path == &outward && source == &outside.join("file")
    ));
    std::fs::remove_dir_all(&root).ok();
}

/// `include` is an ordered list supplied by configuration.
#[test]
fn configuration_selects_static_capabilities() {
    let config: BaseConfig = toml::from_str("include = [\"core\"]").unwrap();
    assert_eq!(config.include, vec![Capability::Core]);
    let base = resolve_base(&config).unwrap();
    assert_eq!(base.capabilities.len(), 1);
    assert!(base.entries().count() > 1);
}

/// Unknown names are rejected while configuration is parsed.
#[test]
fn an_unknown_capability_is_rejected() {
    let message = toml::from_str::<BaseConfig>("include = [\"speling\"]")
        .unwrap_err()
        .to_string();
    assert!(message.contains("speling"), "{message}");
}
