use crate::{
    effective_policy, validate_request, EffectivePolicy, ExecutionEvidence, ExecutionIo,
    ExecutionOutcome, ExecutionRequest, ProcessExit,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        let mut bytes = [0u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .expect("operating system random source unavailable");
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(format_uuid(bytes))
    }
}
impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::str::FromStr for SessionId {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let valid = s.len() == 36
            && s.bytes().enumerate().all(|(i, b)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    b == b'-'
                } else {
                    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                }
            });
        if !valid {
            bail!("session id must be a canonical lowercase UUID")
        }
        Ok(Self(s.into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct BackendReference(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct DiscoveredResource {
    pub session_id: SessionId,
    pub reference: BackendReference,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RedactedExecutionRequest {
    pub command: Vec<String>,
    pub working_directory: PathBuf,
    pub mounts: Vec<crate::Mount>,
    pub environment_names: Vec<String>,
    pub network: bool,
    pub interactive: bool,
}
impl From<&ExecutionRequest> for RedactedExecutionRequest {
    fn from(r: &ExecutionRequest) -> Self {
        Self {
            command: r
                .command
                .iter()
                .map(|v| v.to_string_lossy().into())
                .collect(),
            working_directory: r.working_directory.clone(),
            mounts: r.mounts.clone(),
            environment_names: r
                .environment
                .keys()
                .map(|v| v.to_string_lossy().into())
                .collect(),
            network: r.network,
            interactive: r.interactive,
        }
    }
}
impl RedactedExecutionRequest {
    fn incomplete() -> Self {
        Self {
            command: vec![],
            working_directory: "/".into(),
            mounts: vec![],
            environment_names: vec![],
            network: false,
            interactive: false,
        }
    }
}

fn schema_version() -> u32 {
    1
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionRecord {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub id: SessionId,
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_reference: Option<BackendReference>,
    pub request: RedactedExecutionRequest,
    pub effective_policy: EffectivePolicy,
    pub created_at_ms: u64,
    #[serde(default)]
    pub cleanup_requested: bool,
    #[serde(default)]
    pub metadata_incomplete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "lowercase")]
pub enum ObservedProcessState {
    Created,
    Running,
    Exited(ProcessExit),
    Missing,
    Unknown { error: String },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Observation {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub sequence: u64,
    pub observed_at_ms: u64,
    pub backend_reference: BackendReference,
    pub state: ObservedProcessState,
}
#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub record: SessionRecord,
    pub observed: ObservedProcessState,
    pub observed_at: SystemTime,
}
#[derive(Clone, Debug)]
pub struct StartedSession {
    pub record: SessionRecord,
}
#[derive(Clone, Debug)]
pub struct CleanupObservation {
    pub id: SessionId,
    pub state: ObservedProcessState,
}

pub trait ProcessConnection {
    fn connect(self: Box<Self>, io: ExecutionIo) -> Result<ProcessExit>;
}
pub trait DurableIsolation {
    fn backend_name(&self) -> &'static str;
    fn start(&self, id: &SessionId, request: &ExecutionRequest) -> Result<BackendReference>;
    fn find(&self, id: &SessionId) -> Result<Option<BackendReference>>;
    fn enumerate_managed(&self) -> Result<Vec<DiscoveredResource>> {
        Ok(vec![])
    }
    fn inspect(&self, reference: &BackendReference) -> Result<ObservedProcessState>;
    fn attach(&self, reference: &BackendReference) -> Result<Box<dyn ProcessConnection>>;
    fn wait(&self, reference: &BackendReference) -> Result<ProcessExit>;
    fn terminate(&self, reference: &BackendReference, grace: Duration) -> Result<()>;
    fn remove(&self, reference: &BackendReference) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct SessionStore {
    root: PathBuf,
}
impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn default_path() -> PathBuf {
        std::env::var_os("DRIVA_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_STATE_HOME").map(|p| PathBuf::from(p).join("driva")))
            .or_else(|| {
                std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state/driva"))
            })
            .unwrap_or_else(|| PathBuf::from(".driva-state"))
    }
    fn dir(&self, id: &SessionId) -> PathBuf {
        self.root.join(&id.0)
    }
    fn record_path(&self, id: &SessionId) -> PathBuf {
        self.dir(id).join("record.toml")
    }
    fn sync_dir(path: &Path) -> Result<()> {
        File::open(path)?.sync_all()?;
        Ok(())
    }
    fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path.parent().context("path has no parent")?;
        fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(".tmp-{}", SessionId::new()));
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Self::sync_dir(parent)
    }
    pub fn save(&self, record: &SessionRecord) -> Result<()> {
        Self::atomic_write(
            &self.record_path(&record.id),
            toml::to_string_pretty(record)?.as_bytes(),
        )
    }
    pub fn load(&self, id: &SessionId) -> Result<SessionRecord> {
        let data = fs::read_to_string(self.record_path(id))
            .with_context(|| format!("unknown session {id}"))?;
        let record: SessionRecord =
            toml::from_str(&data).with_context(|| format!("corrupt session record {id}"))?;
        if record.id != *id {
            bail!("session record id mismatch for {id}")
        }
        if record.schema_version != 1 {
            bail!(
                "unsupported session schema version {}",
                record.schema_version
            )
        }
        Ok(record)
    }
    pub fn observe(&self, id: &SessionId, observation: &Observation) -> Result<()> {
        let dir = self.dir(id).join("observations");
        fs::create_dir_all(&dir)?;
        // create_new makes concurrent allocation safe without a process-global counter.
        for sequence in 1..=u64::MAX {
            let path = dir.join(format!("{sequence:016}.toml"));
            if path.exists() {
                continue;
            }
            let mut value = observation.clone();
            value.sequence = sequence;
            let bytes = toml::to_string_pretty(&value)?;
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    f.write_all(bytes.as_bytes())?;
                    f.sync_all()?;
                    Self::sync_dir(&dir)?;
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("observation sequence exhausted")
    }
    pub fn list(&self) -> Result<Vec<SessionRecord>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path().join("record.toml");
            if path.exists() {
                out.push(
                    toml::from_str(&fs::read_to_string(&path)?)
                        .with_context(|| format!("corrupt record {}", path.display()))?,
                )
            }
        }
        out.sort_by(|a: &SessionRecord, b| a.id.0.cmp(&b.id.0));
        Ok(out)
    }
    pub fn remove(&self, id: &SessionId) -> Result<()> {
        let path = self.dir(id);
        if path.exists() {
            fs::remove_dir_all(path)?;
            if self.root.exists() {
                Self::sync_dir(&self.root)?
            }
        }
        Ok(())
    }
}

pub struct SessionRunner<'a> {
    pub backend: &'a dyn DurableIsolation,
    pub store: SessionStore,
}
impl<'a> SessionRunner<'a> {
    pub fn new(backend: &'a dyn DurableIsolation, store: SessionStore) -> Self {
        Self { backend, store }
    }
    fn reference(record: &SessionRecord) -> Result<&BackendReference> {
        record
            .backend_reference
            .as_ref()
            .context("session has no backend reference; run driva recover")
    }
    fn ensure_backend(&self, record: &SessionRecord) -> Result<()> {
        if record.backend != self.backend.backend_name() {
            bail!(
                "session uses backend {:?}, not configured backend {:?}",
                record.backend,
                self.backend.backend_name()
            )
        }
        Ok(())
    }
    pub fn start(&self, request: ExecutionRequest) -> Result<StartedSession> {
        let request = validate_request(&request)?;
        let id = SessionId::new();
        let mut record = SessionRecord {
            schema_version: 1,
            id: id.clone(),
            backend: self.backend.backend_name().into(),
            backend_reference: None,
            request: (&request).into(),
            effective_policy: effective_policy(&request),
            created_at_ms: now_ms(),
            cleanup_requested: false,
            metadata_incomplete: false,
        };
        self.store.save(&record)?;
        let reference = self
            .backend
            .start(&id, &request)
            .context("backend creation failed; prepared session retained for recovery")?;
        record.backend_reference = Some(reference.clone());
        self.store
            .save(&record)
            .context("backend started but reference could not be saved; run driva recover")?;
        self.observe(&record, ObservedProcessState::Created)?;
        Ok(StartedSession { record })
    }
    fn observe(&self, record: &SessionRecord, state: ObservedProcessState) -> Result<()> {
        let reference = record
            .backend_reference
            .clone()
            .unwrap_or_else(|| BackendReference(String::new()));
        self.store.observe(
            &record.id,
            &Observation {
                schema_version: 1,
                sequence: 0,
                observed_at_ms: now_ms(),
                backend_reference: reference,
                state,
            },
        )
    }
    pub fn inspect(&self, id: &SessionId) -> Result<SessionSnapshot> {
        let record = self.store.load(id)?;
        self.ensure_backend(&record)?;
        let observed = match Self::reference(&record) {
            Ok(r) => self
                .backend
                .inspect(r)
                .unwrap_or_else(|e| ObservedProcessState::Unknown {
                    error: format!("{e:#}"),
                }),
            Err(e) => ObservedProcessState::Unknown {
                error: e.to_string(),
            },
        };
        let at = SystemTime::now();
        self.observe(&record, observed.clone())?;
        Ok(SessionSnapshot {
            record,
            observed,
            observed_at: at,
        })
    }
    pub fn attach(&self, id: &SessionId, io: ExecutionIo) -> Result<ProcessExit> {
        let r = self.store.load(id)?;
        self.ensure_backend(&r)?;
        self.backend.attach(Self::reference(&r)?)?.connect(io)
    }
    pub fn wait(&self, id: &SessionId) -> Result<ExecutionOutcome> {
        let r = self.store.load(id)?;
        self.ensure_backend(&r)?;
        let reference = Self::reference(&r)?;
        let exit = self.backend.wait(reference)?;
        self.observe(&r, ObservedProcessState::Exited(exit.clone()))?;
        Ok(ExecutionOutcome {
            exit,
            evidence: ExecutionEvidence {
                isolation_backend: r.backend.clone(),
                backend_reference: Some(reference.0.clone()),
                effective_policy: r.effective_policy,
                started_at: UNIX_EPOCH + Duration::from_millis(r.created_at_ms),
                finished_at: SystemTime::now(),
            },
        })
    }
    pub fn terminate(&self, id: &SessionId, grace: Duration) -> Result<ExecutionOutcome> {
        let r = self.store.load(id)?;
        self.ensure_backend(&r)?;
        self.backend.terminate(Self::reference(&r)?, grace)?;
        self.wait(id)
    }
    pub fn remove(&self, id: &SessionId) -> Result<CleanupObservation> {
        let mut r = self.store.load(id)?;
        self.ensure_backend(&r)?;
        r.cleanup_requested = true;
        self.store.save(&r)?;
        let reference = Self::reference(&r)?;
        self.backend.remove(reference)?;
        let state =
            self.backend
                .inspect(reference)
                .unwrap_or_else(|e| ObservedProcessState::Unknown {
                    error: format!("{e:#}"),
                });
        self.observe(&r, state.clone())?;
        if state == ObservedProcessState::Missing {
            self.store.remove(id)?
        }
        Ok(CleanupObservation {
            id: id.clone(),
            state,
        })
    }
    pub fn recover(&self) -> Result<Vec<SessionSnapshot>> {
        let discovered = self.backend.enumerate_managed()?;
        let mut grouped = std::collections::BTreeMap::<String, Vec<BackendReference>>::new();
        for d in discovered {
            grouped.entry(d.session_id.0).or_default().push(d.reference)
        }
        for (id, refs) in &grouped {
            if refs.len() != 1 {
                continue;
            }
            let id: SessionId = id.parse()?;
            if self.store.load(&id).is_err() {
                let record = SessionRecord {
                    schema_version: 1,
                    id: id.clone(),
                    backend: self.backend.backend_name().into(),
                    backend_reference: Some(refs[0].clone()),
                    request: RedactedExecutionRequest::incomplete(),
                    effective_policy: EffectivePolicy {
                        working_directory: "/".into(),
                        mounts: vec![],
                        network: false,
                        interactive: false,
                    },
                    created_at_ms: now_ms(),
                    cleanup_requested: false,
                    metadata_incomplete: true,
                };
                self.store.save(&record)?;
            }
        }
        let mut out = vec![];
        for mut r in self.store.list()? {
            if r.backend != self.backend.backend_name() {
                continue;
            }
            if let Some(refs) = grouped.get(&r.id.0) {
                if refs.len() > 1 {
                    let state = ObservedProcessState::Unknown {
                        error: "multiple managed backend resources claim this session".into(),
                    };
                    self.observe(&r, state.clone())?;
                    out.push(SessionSnapshot {
                        record: r,
                        observed: state,
                        observed_at: SystemTime::now(),
                    });
                    continue;
                }
                if r.backend_reference.as_ref() != refs.first() {
                    r.backend_reference = refs.first().cloned();
                    self.store.save(&r)?
                }
            } else if let Some(found) = self.backend.find(&r.id)? {
                r.backend_reference = Some(found);
                self.store.save(&r)?
            }
            if r.cleanup_requested {
                if let Some(reference) = &r.backend_reference {
                    self.backend.remove(reference)?;
                    if self.backend.inspect(reference)? == ObservedProcessState::Missing {
                        self.store.remove(&r.id)?;
                        continue;
                    }
                }
            }
            out.push(self.inspect(&r.id)?)
        }
        Ok(out)
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn format_uuid(b: [u8; 16]) -> String {
    format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7],b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15])
}
