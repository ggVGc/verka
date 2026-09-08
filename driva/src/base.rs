//! The base system: what a private root carries before any mount.
//!
//! A private root starts as an empty tmpfs, so a command in it cannot run at
//! all until something puts the host's loader, libraries, and system files
//! there. That floor is the *base*, and it is a different kind of thing from a
//! mount: a mount grants access to the operator's own data and is a choice,
//! while the base is what any program needs to run on this host and is not.
//!
//! Driva owns the small, fixed set of capabilities and their host paths.
//! Configuration only chooses which of them are enabled.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The capabilities included when configuration says nothing, in the order
/// they are laid down. `core` comes first because everything else resolves
/// through it.
pub const DEFAULT_CAPABILITIES: [&str; 5] = ["core", "identity", "certificates", "dns", "timezone"];

/// Which of Driva's static capabilities a private root is built from.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaseConfig {
    /// Capability names, in the order they are laid down.
    #[serde(default = "default_capabilities")]
    pub include: Vec<String>,
}

fn default_capabilities() -> Vec<String> {
    DEFAULT_CAPABILITIES.map(str::to_owned).to_vec()
}

impl Default for BaseConfig {
    fn default() -> Self {
        Self {
            include: default_capabilities(),
        }
    }
}

impl BaseConfig {
    /// A base carrying nothing, for a prepared rootfs (which brings its own
    /// system) or for an operator who wants to state every path themselves.
    pub fn empty() -> Self {
        Self {
            include: Vec::new(),
        }
    }

    /// Add a capability, keeping the existing order when it is already there.
    pub fn include(&mut self, name: &str) {
        if !self.include.iter().any(|existing| existing == name) {
            self.include.push(name.to_owned());
        }
    }

    /// Drop a capability the configuration or a template asked for.
    pub fn exclude(&mut self, name: &str) {
        self.include.retain(|existing| existing != name);
    }
}

/// One of Driva's fixed capabilities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    pub name: &'static str,
    pub description: &'static str,
    pub paths: Vec<Entry>,
    pub environment: &'static [&'static str],
    pub probe: Option<Probe>,
}

/// One host path a capability needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Where it is on the host, and where it lands in the isolation.
    pub at: PathBuf,
    /// A path this host may not have. An entry that is not optional and not
    /// there fails the launch, naming the capability, rather than producing a
    /// sandbox that is quietly missing part of its floor.
    pub optional: bool,
    /// What needs this path, in one line: which program reads it, and what
    /// stops working when it is not there.
    ///
    /// A path list is unreadable without this. `/etc/ld.so.cache` and
    /// `/etc/alternatives` look alike as strings and are in the base for
    /// completely different reasons, and an operator deciding whether to trim
    /// a capability, or where their own host keeps the same thing, is asking
    /// exactly this question. `driva capabilities NAME` prints it, so the
    /// answer lives with the declaration rather than in a comment only a
    /// reader of Driva's source would find.
    pub doc: &'static str,
}

fn entry(at: &str, optional: bool, doc: &'static str) -> Entry {
    Entry {
        at: at.into(),
        optional,
        doc,
    }
}

/// All capabilities known to this Driva build.
pub fn capabilities() -> Vec<Capability> {
    vec![
        Capability {
            name: "core",
            description: "Run a program: the host's executables, libraries, and loader",
            paths: vec![
                entry(
                    "/usr",
                    false,
                    "Binaries, shared libraries, and their read-only data",
                ),
                entry(
                    "/bin",
                    false,
                    "The shell and core tools, or the symlink standing in for them",
                ),
                entry(
                    "/sbin",
                    true,
                    "Administrative tools, where the host still separates them",
                ),
                entry(
                    "/lib",
                    true,
                    "The dynamic linker and shared libraries, or the link to them",
                ),
                entry(
                    "/lib64",
                    true,
                    "The 64-bit dynamic linker a binary names in its own headers",
                ),
                entry(
                    "/etc/ld.so.cache",
                    true,
                    "The loader's index of installed libraries",
                ),
                entry(
                    "/etc/ld.so.conf",
                    true,
                    "Extra library directories the loader searches",
                ),
                entry(
                    "/etc/ld.so.conf.d",
                    true,
                    "Per-package additions to the loader's search path",
                ),
                entry(
                    "/etc/alternatives",
                    true,
                    "Where a Debian host's generic command names point",
                ),
            ],
            environment: &[],
            probe: Some(Probe::Run(vec![
                "/bin/sh".into(),
                "-c".into(),
                "exit 0".into(),
            ])),
        },
        Capability {
            name: "identity",
            description: "Name the user and group a program runs as",
            paths: vec![
                entry(
                    "/etc/passwd",
                    false,
                    "The user database a program's own identity comes from",
                ),
                entry(
                    "/etc/group",
                    false,
                    "The group database, for group names and membership",
                ),
                entry(
                    "/etc/nsswitch.conf",
                    true,
                    "Which sources answer the user, group, and host databases",
                ),
            ],
            environment: &[],
            probe: Some(Probe::Run(vec!["/usr/bin/id".into(), "-u".into()])),
        },
        Capability {
            name: "certificates",
            description: "Verify TLS certificates and reach a network through a proxy",
            paths: vec![
                entry("/etc/ssl", true, "The usual certificate authority store"),
                entry(
                    "/etc/pki",
                    true,
                    "The Red Hat family's certificate authority store",
                ),
                entry(
                    "/etc/ca-certificates",
                    true,
                    "Certificate store configuration and sources",
                ),
                entry(
                    "/usr/local/share/ca-certificates",
                    true,
                    "Locally added certificate authorities",
                ),
            ],
            environment: &[
                "SSL_CERT_FILE",
                "SSL_CERT_DIR",
                "CURL_CA_BUNDLE",
                "REQUESTS_CA_BUNDLE",
                "NODE_EXTRA_CA_CERTS",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "NO_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "no_proxy",
            ],
            probe: Some(Probe::Run(vec![
                "/usr/bin/curl".into(),
                "-sS".into(),
                "-o".into(),
                "/dev/null".into(),
                "https://example.com".into(),
            ])),
        },
        Capability {
            name: "dns",
            description: "Turn a host name into an address when networking is permitted",
            paths: vec![
                entry(
                    "/etc/resolv.conf",
                    false,
                    "The nameservers and search domains a DNS lookup uses",
                ),
                entry(
                    "/etc/nsswitch.conf",
                    true,
                    "Which sources answer a host name, and in what order",
                ),
                entry("/etc/hosts", true, "Names answered locally before DNS"),
                entry("/etc/services", true, "Port numbers by service name"),
                entry("/etc/protocols", true, "Protocol numbers by name"),
                entry(
                    "/run/systemd/resolve",
                    true,
                    "The systemd-resolved socket and resolver file",
                ),
            ],
            environment: &[],
            probe: Some(Probe::Resolve("one.one.one.one".into())),
        },
        Capability {
            name: "timezone",
            description: "Report local time as the host does",
            paths: vec![entry(
                "/etc/localtime",
                true,
                "The host's time zone as the C library reads it",
            )],
            environment: &["TZ"],
            probe: None,
        },
    ]
}

/// Find a capability known to this Driva build.
pub fn capability(name: &str) -> Result<Capability> {
    capabilities()
        .into_iter()
        .find(|capability| capability.name == name)
        .with_context(|| {
            format!(
                "unknown capability {name:?}; available capabilities: {}",
                DEFAULT_CAPABILITIES.join(", ")
            )
        })
}

/// How to tell whether a capability works in a real sandbox built from it.
///
/// A path list is a guess about a host; a probe checks it from a real sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Resolve a host name from inside the isolation.
    Resolve(String),
    /// Open a TCP connection to `HOST:PORT` from inside the isolation.
    Connect(String),
    /// Run a command in the isolation and require it to succeed. A command the
    /// isolation does not have reports as unprobed rather than as a failure.
    Run(Vec<String>),
}

/// A base resolved against this host: what will be laid down, grouped by the
/// capability that asked for it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Base {
    pub capabilities: Vec<ResolvedCapability>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCapability {
    pub name: String,
    pub description: String,
    pub entries: Vec<RuntimeEntry>,
    /// The forwarded variables that are actually set on this host.
    pub environment: Vec<OsString>,
    pub probe: Option<Probe>,
}

impl Base {
    /// Every entry, in the order it is laid down.
    pub fn entries(&self) -> impl Iterator<Item = &RuntimeEntry> {
        self.capabilities
            .iter()
            .flat_map(|capability| capability.entries.iter())
    }

    /// The forwarded host environment: the variables the included
    /// capabilities name, with the values this host has for them. Anything a
    /// later layer sets wins, so this is the floor for the environment in the
    /// same way the entries are the floor for the filesystem.
    pub fn environment(&self) -> BTreeMap<OsString, OsString> {
        self.capabilities
            .iter()
            .flat_map(|capability| capability.environment.iter())
            .filter_map(|name| std::env::var_os(name).map(|value| (name.clone(), value)))
            .collect()
    }
}

/// One element of the base filesystem, resolved to how it is laid down.
///
/// A caller that wants to *state* what its sandboxes hold reads these rather
/// than re-deriving them, so what is shown and what is bound cannot drift.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeEntry {
    /// Host content exposed read-only inside the isolation. `source` and
    /// `path` are the same host path, except where the host keeps a symlink
    /// at `path` aimed outside the base.
    ReadOnly { source: PathBuf, path: PathBuf },
    /// A symlink the host keeps at that path, recreated rather than followed,
    /// so a distro that points `/bin` at `usr/bin` keeps working.
    Symlink { target: PathBuf, path: PathBuf },
}

impl RuntimeEntry {
    /// Where this entry lands inside the isolation.
    pub fn path(&self) -> &Path {
        match self {
            Self::ReadOnly { path, .. } | Self::Symlink { path, .. } => path,
        }
    }
}

/// Resolve a declared base against this host.
///
/// Every capability named by `config` is looked up, and each of its paths
/// inspected: what exists is recorded as the entry it will become, what does
/// not is skipped when the entry allows it and fails the resolution when it
/// does not. The error names the capability, so a host missing part of its
/// floor says so here rather than inside whatever was launched.
pub fn resolve_base(config: &BaseConfig) -> Result<Base> {
    let mut base = Base::default();
    for name in &config.include {
        let capability = capability(name)?;
        let mut entries = Vec::new();
        for entry in &capability.paths {
            match resolve_entry(entry, &base, &entries)
                .with_context(|| format!("resolving capability {name:?} for the sandbox base"))?
            {
                Some(resolved) => entries.push(resolved),
                None => continue,
            }
        }
        base.capabilities.push(ResolvedCapability {
            name: capability.name.to_owned(),
            description: capability.description.to_owned(),
            entries,
            environment: capability
                .environment
                .iter()
                .filter(|name| std::env::var_os(name).is_some())
                .map(OsString::from)
                .collect(),
            probe: capability.probe.clone(),
        });
    }
    Ok(base)
}

fn resolve_entry(
    entry: &Entry,
    base: &Base,
    pending: &[RuntimeEntry],
) -> Result<Option<RuntimeEntry>> {
    let path = &entry.at;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if entry.optional {
                return Ok(None);
            }
            bail!("{} is not on this host", path.display());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()))
        }
    };
    if !metadata.file_type().is_symlink() {
        return Ok(Some(RuntimeEntry::ReadOnly {
            source: path.clone(),
            path: path.clone(),
        }));
    }
    let target = std::fs::read_link(path)
        .with_context(|| format!("failed to read the link at {}", path.display()))?;
    let symlink = RuntimeEntry::Symlink {
        target,
        path: path.clone(),
    };
    let followed = |resolved: PathBuf| RuntimeEntry::ReadOnly {
        source: resolved,
        path: path.clone(),
    };
    // Reproduce a link while it still leads somewhere the base carries. A
    // link aimed outside it would land on nothing, so bind its content where
    // the link stands instead.
    Ok(Some(match path.canonicalize() {
        Ok(resolved) if !carried(base, pending, &resolved) => followed(resolved),
        _ => symlink,
    }))
}

/// Whether `path` is inside something already laid down, and so reachable
/// through an entry that comes before this one.
fn carried(base: &Base, pending: &[RuntimeEntry], path: &Path) -> bool {
    base.entries()
        .chain(pending.iter())
        .any(|entry| match entry {
            RuntimeEntry::ReadOnly { source, .. } => path.starts_with(source),
            RuntimeEntry::Symlink { .. } => false,
        })
}

#[cfg(test)]
mod tests {
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
            let capability = capability(name).unwrap();
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
            for entry in &capability(name).unwrap().paths {
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
        assert_eq!(config.include, vec!["core".to_owned()]);
        let base = resolve_base(&config).unwrap();
        assert_eq!(base.capabilities.len(), 1);
        assert!(base.entries().count() > 1);
    }

    /// A capability nobody defined is a typo in policy. Failing names what is
    /// available, since the fix is almost always one of those.
    #[test]
    fn an_unknown_capability_is_an_error_naming_the_known_ones() {
        let mut config = BaseConfig::default();
        config.include("speling");
        let message = format!("{:#}", resolve_base(&config).unwrap_err());
        assert!(message.contains("speling"), "{message}");
        assert!(message.contains("certificates"), "{message}");
    }
}
