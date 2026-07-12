use crate::{
    effective_policy, validate_request, EffectivePolicy, ExecutionEvidence, ExecutionIo,
    ExecutionOutcome, ExecutionRequest, ProcessExit,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::str::FromStr for SessionId {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        if s.is_empty()
            || !s
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            bail!("invalid session id")
        }
        Ok(Self(s.into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct BackendReference(pub String);

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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub backend: String,
    pub backend_reference: BackendReference,
    pub request: RedactedExecutionRequest,
    pub effective_policy: EffectivePolicy,
    pub created_at_ms: u64,
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
    pub fn save(&self, record: &SessionRecord) -> Result<()> {
        fs::create_dir_all(self.dir(&record.id))?;
        let path = self.record_path(&record.id);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, toml::to_string_pretty(record)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }
    pub fn load(&self, id: &SessionId) -> Result<SessionRecord> {
        let data = fs::read_to_string(self.record_path(id))
            .with_context(|| format!("unknown session {id}"))?;
        Ok(toml::from_str(&data)?)
    }
    pub fn observe(&self, id: &SessionId, observation: &Observation) -> Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir(id).join("observations.toml"))?;
        f.write_all(b"[[observation]]\n")?;
        f.write_all(toml::to_string(observation)?.as_bytes())?;
        f.sync_data()?;
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<SessionRecord>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out: Vec<SessionRecord> = vec![];
        for e in fs::read_dir(&self.root)? {
            let p = e?.path().join("record.toml");
            if p.exists() {
                out.push(toml::from_str(&fs::read_to_string(p)?)?);
            }
        }
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        Ok(out)
    }
    pub fn remove(&self, id: &SessionId) -> Result<()> {
        let p = self.dir(id);
        if p.exists() {
            fs::remove_dir_all(p)?
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
    pub fn start(&self, request: ExecutionRequest) -> Result<StartedSession> {
        let request = validate_request(&request)?;
        let id = new_id();
        let reference = self.backend.start(&id, &request)?;
        let record = SessionRecord {
            id,
            backend: self.backend.backend_name().into(),
            backend_reference: reference,
            request: (&request).into(),
            effective_policy: effective_policy(&request),
            created_at_ms: now_ms(),
        };
        if let Err(e) = self.store.save(&record) {
            return Err(e).context(
                "backend started but session record could not be saved; run driva recover",
            );
        }
        Ok(StartedSession { record })
    }
    pub fn inspect(&self, id: &SessionId) -> Result<SessionSnapshot> {
        let record = self.store.load(id)?;
        let observed = self
            .backend
            .inspect(&record.backend_reference)
            .unwrap_or_else(|e| ObservedProcessState::Unknown {
                error: format!("{e:#}"),
            });
        let at = SystemTime::now();
        self.store.observe(
            id,
            &Observation {
                observed_at_ms: now_ms(),
                backend_reference: record.backend_reference.clone(),
                state: observed.clone(),
            },
        )?;
        Ok(SessionSnapshot {
            record,
            observed,
            observed_at: at,
        })
    }
    pub fn attach(&self, id: &SessionId, io: ExecutionIo) -> Result<ProcessExit> {
        let r = self.store.load(id)?;
        self.backend.attach(&r.backend_reference)?.connect(io)
    }
    pub fn wait(&self, id: &SessionId) -> Result<ExecutionOutcome> {
        let r = self.store.load(id)?;
        let started = UNIX_EPOCH + Duration::from_millis(r.created_at_ms);
        let exit = self.backend.wait(&r.backend_reference)?;
        self.store.observe(
            id,
            &Observation {
                observed_at_ms: now_ms(),
                backend_reference: r.backend_reference.clone(),
                state: ObservedProcessState::Exited(exit.clone()),
            },
        )?;
        Ok(ExecutionOutcome {
            exit,
            evidence: ExecutionEvidence {
                isolation_backend: r.backend,
                backend_reference: Some(r.backend_reference.0),
                effective_policy: r.effective_policy,
                started_at: started,
                finished_at: SystemTime::now(),
            },
        })
    }
    pub fn terminate(&self, id: &SessionId, grace: Duration) -> Result<ExecutionOutcome> {
        let r = self.store.load(id)?;
        self.backend.terminate(&r.backend_reference, grace)?;
        self.wait(id)
    }
    pub fn remove(&self, id: &SessionId) -> Result<CleanupObservation> {
        let r = self.store.load(id)?;
        self.backend.remove(&r.backend_reference)?;
        let state = self
            .backend
            .inspect(&r.backend_reference)
            .unwrap_or(ObservedProcessState::Missing);
        if state == ObservedProcessState::Missing {
            self.store.remove(id)?
        }
        Ok(CleanupObservation {
            id: id.clone(),
            state,
        })
    }
    pub fn recover(&self) -> Result<Vec<SessionSnapshot>> {
        let mut out = vec![];
        for mut r in self.store.list()? {
            if r.backend != self.backend.backend_name() {
                continue;
            }
            if matches!(
                self.backend.inspect(&r.backend_reference),
                Ok(ObservedProcessState::Missing)
            ) {
                if let Some(found) = self.backend.find(&r.id)? {
                    r.backend_reference = found;
                    self.store.save(&r)?
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
fn new_id() -> SessionId {
    static N: AtomicU64 = AtomicU64::new(0);
    SessionId(format!(
        "{:x}-{:x}-{:x}",
        now_ms(),
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}
