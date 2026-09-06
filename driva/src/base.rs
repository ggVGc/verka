//! The base system: what a private root carries before any mount.
//!
//! A private root starts as an empty tmpfs, so a command in it cannot run at
//! all until something puts the host's loader, libraries, and system files
//! there. That floor is the *base*, and it is a different kind of thing from a
//! mount: a mount grants access to the operator's own data and is a choice,
//! while the base is what any program needs to run on this host and is not.
//!
//! The base is a list of named [`Capability`] declarations, resolved against
//! the machine it will run on. Naming them separates the portable statement
//! ("this sandbox must be able to resolve host names") from the host-specific
//! answer (`/run/systemd/resolve` here, `/etc/resolv.conf` alone there), which
//! is what lets one set of built-ins work across distributions and lets an
//! operator state the difference where it does not.
//!
//! Nothing here is discovered or implied: a capability is a declaration in
//! configuration, built-in or the operator's own, and [`resolve_base`] reports
//! exactly what it laid down so a host can show it.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The capabilities included when configuration says nothing, in the order
/// they are laid down. `core` comes first because everything else resolves
/// through it.
pub const DEFAULT_CAPABILITIES: [&str; 5] = ["core", "identity", "certificates", "dns", "timezone"];

/// Built-in capability definitions, embedded like the execution templates so a
/// distributed binary needs no support files. A project definition of the same
/// name replaces the built-in.
fn builtin_capabilities() -> BTreeMap<String, CapabilityConfig> {
    [
        ("core", include_str!("../capabilities/core.toml")),
        ("identity", include_str!("../capabilities/identity.toml")),
        (
            "certificates",
            include_str!("../capabilities/certificates.toml"),
        ),
        ("dns", include_str!("../capabilities/dns.toml")),
        ("timezone", include_str!("../capabilities/timezone.toml")),
    ]
    .into_iter()
    .map(|(name, source)| {
        let capability = toml::from_str(source)
            .unwrap_or_else(|error| panic!("invalid built-in capability {name:?}: {error}"));
        (name.to_owned(), capability)
    })
    .collect()
}

/// Which capabilities a private root is built from, and how any that are not
/// built in are defined. This is the declarative form — what an operator
/// writes — and stays free of anything about the machine it will run on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseConfig {
    /// Capability names, in the order they are laid down.
    pub include: Vec<String>,
    /// Definitions available to `include`: the built-ins, with any project
    /// definition of the same name replacing one.
    pub definitions: BTreeMap<String, CapabilityConfig>,
}

impl Default for BaseConfig {
    fn default() -> Self {
        Self {
            include: DEFAULT_CAPABILITIES.map(str::to_owned).to_vec(),
            definitions: builtin_capabilities(),
        }
    }
}

impl BaseConfig {
    /// A base carrying nothing, for a prepared rootfs (which brings its own
    /// system) or for an operator who wants to state every path themselves.
    pub fn empty() -> Self {
        Self {
            include: Vec::new(),
            definitions: builtin_capabilities(),
        }
    }

    /// Apply the configured section over the built-ins: project definitions
    /// replace built-ins of the same name, and a stated `include` replaces the
    /// default list entirely, since the order is the point.
    pub fn with_section(mut self, section: &BaseSection) -> Self {
        self.definitions.extend(section.definitions.clone());
        if let Some(include) = &section.include {
            self.include = include.clone();
        }
        self
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

    /// The named definition, or an error listing what is available — an
    /// unknown capability is a typo in policy, not something to skip.
    pub fn definition(&self, name: &str) -> Result<&CapabilityConfig> {
        self.definitions.get(name).with_context(|| {
            format!(
                "unknown capability {name:?}; defined capabilities: {}",
                self.definitions
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }
}

/// The `[base]` section of a configuration file, plus the `[capability.NAME]`
/// tables that go with it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaseSection {
    /// Replaces the default capability list when present.
    pub include: Option<Vec<String>>,
    #[serde(skip)]
    pub definitions: BTreeMap<String, CapabilityConfig>,
}

/// One named part of the base: the host paths it needs, the host environment
/// variables it forwards, and how to tell whether it actually works here.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapabilityConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "path")]
    pub paths: Vec<EntryConfig>,
    /// Host environment variables forwarded into the isolation when they are
    /// set. Values a program cannot work without and cannot find on disk — a
    /// proxy, an overridden certificate bundle — are configuration of the same
    /// capability, and naming them is the only way they cross the boundary.
    #[serde(default)]
    pub environment: Vec<String>,
    pub probe: Option<Probe>,
    /// Host paths worth reporting when the probe fails: places this capability
    /// is known to live on other systems, offered as configuration to write
    /// rather than anything Driva adds by itself.
    #[serde(default)]
    pub suggest: Vec<PathBuf>,
}

/// One host path a capability needs.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EntryConfig {
    /// Where it is on the host, and where it lands in the isolation.
    pub at: PathBuf,
    #[serde(default)]
    pub mode: EntryMode,
    /// A path this host may not have. An entry that is not optional and not
    /// there fails the launch, naming the capability, rather than producing a
    /// sandbox that is quietly missing part of its floor.
    #[serde(default)]
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
    #[serde(default)]
    pub doc: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryMode {
    /// Recreate a host symlink when it still resolves inside the base, and
    /// follow it when it would land on nothing. Right for nearly everything.
    #[default]
    Auto,
    /// Bind the path itself, read-only.
    Bind,
    /// Recreate the host's symlink, even if it leads outside the base.
    Symlink,
    /// Resolve the link chain and bind the content where the link stands.
    Follow,
}

/// How to tell whether a capability works in a real sandbox built from it.
///
/// A path list is a guess about a host; a probe is an answer. The kinds are a
/// closed set — configuration selects a check, it does not supply host code.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
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
    pub suggest: Vec<PathBuf>,
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
        let capability = config.definition(name)?;
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
            name: name.clone(),
            description: capability.description.clone(),
            entries,
            environment: capability
                .environment
                .iter()
                .filter(|name| std::env::var_os(name).is_some())
                .map(OsString::from)
                .collect(),
            probe: capability.probe.clone(),
            suggest: capability.suggest.clone(),
        });
    }
    Ok(base)
}

fn resolve_entry(
    entry: &EntryConfig,
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
            bail!(
                "{} is not on this host; mark it optional, or point the capability at where this \
                 host keeps it",
                path.display()
            );
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
    match entry.mode {
        EntryMode::Symlink => Ok(Some(symlink)),
        EntryMode::Bind => Ok(Some(RuntimeEntry::ReadOnly {
            source: path.clone(),
            path: path.clone(),
        })),
        EntryMode::Follow => match path.canonicalize() {
            Ok(resolved) => Ok(Some(followed(resolved))),
            Err(_) if entry.optional => Ok(None),
            Err(error) => Err(error)
                .with_context(|| format!("failed to follow the link at {}", path.display())),
        },
        // A link is reproduced while it still leads somewhere the base
        // carries; one aimed outside it — `/etc/resolv.conf` into `/run` on a
        // systemd host — would land on nothing, so its content is bound where
        // the link stands instead. A link the host cannot follow either is
        // left as the host has it: reproducing a broken link is the honest
        // translation, and repairing the host's layout is not Driva's job.
        EntryMode::Auto => Ok(Some(match path.canonicalize() {
            Ok(resolved) if !carried(base, pending, &resolved) => followed(resolved),
            _ => symlink,
        })),
    }
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

    fn capability(paths: Vec<EntryConfig>) -> BaseConfig {
        let mut definitions = BTreeMap::new();
        definitions.insert(
            "test".to_owned(),
            CapabilityConfig {
                paths,
                ..CapabilityConfig::default()
            },
        );
        BaseConfig {
            include: vec!["test".to_owned()],
            definitions,
        }
    }

    fn entry(at: &str) -> EntryConfig {
        EntryConfig {
            at: PathBuf::from(at),
            ..EntryConfig::default()
        }
    }

    /// Every built-in parses and names paths, so a mistake in the embedded
    /// TOML is a test failure rather than a panic on first use.
    #[test]
    fn the_built_in_capabilities_are_well_formed() {
        let config = BaseConfig::default();
        for name in DEFAULT_CAPABILITIES {
            let capability = config.definition(name).unwrap();
            assert!(
                !capability.description.is_empty(),
                "{name} has no description"
            );
            assert!(!capability.paths.is_empty(), "{name} names no paths");
        }
    }

    /// Every built-in path says what needs it. A path list nobody can read is
    /// how the constant this replaced went wrong: `/etc/ld.so.cache` and
    /// `/etc/alternatives` are alike as strings and unrelated in purpose, and
    /// an operator trimming a capability or porting it to their own host is
    /// asking exactly what each one is for. Enforced rather than trusted, so a
    /// path added later cannot arrive unexplained.
    #[test]
    fn every_built_in_path_documents_what_needs_it() {
        let config = BaseConfig::default();
        for name in DEFAULT_CAPABILITIES {
            for entry in &config.definition(name).unwrap().paths {
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
        let error = resolve_base(&capability(vec![entry(missing)])).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("test"), "{message}");
        assert!(message.contains(missing), "{message}");

        let optional = EntryConfig {
            optional: true,
            ..entry(missing)
        };
        let base = resolve_base(&capability(vec![optional])).unwrap();
        assert_eq!(base.entries().count(), 0);
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

        let base = resolve_base(&capability(vec![
            entry(&carried.to_string_lossy()),
            entry(&inward.to_string_lossy()),
            entry(&outward.to_string_lossy()),
        ]))
        .unwrap();
        let entries: Vec<&RuntimeEntry> = base.entries().collect();
        assert!(matches!(entries[1], RuntimeEntry::Symlink { path, .. } if path == &inward));
        assert!(matches!(
            entries[2],
            RuntimeEntry::ReadOnly { source, path }
                if path == &outward && source == &outside.join("file")
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    /// `include` is an ordered list an operator replaces wholesale, and a
    /// project definition takes over the name it defines.
    #[test]
    fn configuration_replaces_the_default_list_and_its_definitions() {
        let section = BaseSection {
            include: Some(vec!["core".to_owned()]),
            definitions: BTreeMap::from([(
                "core".to_owned(),
                CapabilityConfig {
                    description: "just /usr".to_owned(),
                    paths: vec![entry("/usr")],
                    ..CapabilityConfig::default()
                },
            )]),
        };
        let config = BaseConfig::default().with_section(&section);
        assert_eq!(config.include, vec!["core".to_owned()]);
        let base = resolve_base(&config).unwrap();
        assert_eq!(base.capabilities.len(), 1);
        assert_eq!(base.entries().count(), 1);
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
