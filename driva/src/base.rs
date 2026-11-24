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
use clap::ValueEnum;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The capabilities included when configuration says nothing, in the order
/// they are laid down. `core` comes first because everything else resolves
/// through it.
pub const DEFAULT_CAPABILITIES: [Capability; 5] = [
    Capability::Core,
    Capability::Identity,
    Capability::Certificates,
    Capability::Dns,
    Capability::Timezone,
];

/// A capability built into Driva.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Core,
    Identity,
    Certificates,
    Dns,
    Timezone,
}

impl Capability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Identity => "identity",
            Self::Certificates => "certificates",
            Self::Dns => "dns",
            Self::Timezone => "timezone",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which of Driva's static capabilities a private root is built from.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaseConfig {
    /// Capabilities, in the order they are laid down.
    #[serde(default = "default_capabilities")]
    pub include: Vec<Capability>,
}

fn default_capabilities() -> Vec<Capability> {
    DEFAULT_CAPABILITIES.to_vec()
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
    pub fn include(&mut self, capability: Capability) {
        if !self.include.contains(&capability) {
            self.include.push(capability);
        }
    }

    /// Drop a capability the configuration or a template asked for.
    pub fn exclude(&mut self, capability: Capability) {
        self.include.retain(|existing| *existing != capability);
    }
}

/// The fixed contents of one capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityInfo {
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
pub fn capabilities() -> impl Iterator<Item = Capability> {
    DEFAULT_CAPABILITIES.into_iter()
}

/// The paths, environment, and probe belonging to a capability.
pub fn capability_info(capability: Capability) -> CapabilityInfo {
    match capability {
        Capability::Core => CapabilityInfo {
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
        Capability::Identity => CapabilityInfo {
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
        Capability::Certificates => CapabilityInfo {
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
        Capability::Dns => CapabilityInfo {
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
        Capability::Timezone => CapabilityInfo {
            description: "Report local time as the host does",
            paths: vec![entry(
                "/etc/localtime",
                true,
                "The host's time zone as the C library reads it",
            )],
            environment: &["TZ"],
            probe: None,
        },
    }
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
    pub name: Capability,
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
        let capability = capability_info(*name);
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
            name: *name,
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
mod tests;
