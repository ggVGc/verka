use driva::{
    BackendReference, DurableIsolation, ExecutionIo, ExecutionRequest, ObservedProcessState,
    ProcessConnection, ProcessExit, SessionId, SessionRunner, SessionStore,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::time::Duration;

struct Fake {
    state: RefCell<ObservedProcessState>,
}

struct FailingStart;
impl DurableIsolation for FailingStart {
    fn backend_name(&self) -> &'static str {
        "fake"
    }
    fn start(&self, _: &SessionId, _: &ExecutionRequest) -> anyhow::Result<BackendReference> {
        anyhow::bail!("creation failed")
    }
    fn find(&self, _: &SessionId) -> anyhow::Result<Option<BackendReference>> {
        Ok(None)
    }
    fn inspect(&self, _: &BackendReference) -> anyhow::Result<ObservedProcessState> {
        Ok(ObservedProcessState::Missing)
    }
    fn attach(&self, _: &BackendReference) -> anyhow::Result<Box<dyn ProcessConnection>> {
        anyhow::bail!("unused")
    }
    fn wait(&self, _: &BackendReference) -> anyhow::Result<ProcessExit> {
        anyhow::bail!("unused")
    }
    fn terminate(&self, _: &BackendReference, _: Duration) -> anyhow::Result<()> {
        anyhow::bail!("unused")
    }
    fn remove(&self, _: &BackendReference) -> anyhow::Result<()> {
        anyhow::bail!("unused")
    }
}
impl DurableIsolation for Fake {
    fn backend_name(&self) -> &'static str {
        "fake"
    }
    fn start(&self, _: &SessionId, _: &ExecutionRequest) -> anyhow::Result<BackendReference> {
        *self.state.borrow_mut() = ObservedProcessState::Running;
        Ok(BackendReference("native-1".into()))
    }
    fn find(&self, _: &SessionId) -> anyhow::Result<Option<BackendReference>> {
        Ok(None)
    }
    fn inspect(&self, _: &BackendReference) -> anyhow::Result<ObservedProcessState> {
        Ok(self.state.borrow().clone())
    }
    fn attach(&self, _: &BackendReference) -> anyhow::Result<Box<dyn ProcessConnection>> {
        anyhow::bail!("unused")
    }
    fn wait(&self, _: &BackendReference) -> anyhow::Result<ProcessExit> {
        *self.state.borrow_mut() = ObservedProcessState::Exited(ProcessExit::Code(7));
        Ok(ProcessExit::Code(7))
    }
    fn terminate(&self, _: &BackendReference, _: Duration) -> anyhow::Result<()> {
        Ok(())
    }
    fn remove(&self, _: &BackendReference) -> anyhow::Result<()> {
        *self.state.borrow_mut() = ObservedProcessState::Missing;
        Ok(())
    }
}

struct ImmediateExit;
impl ProcessConnection for ImmediateExit {
    fn connect(self: Box<Self>, _: ExecutionIo) -> anyhow::Result<ProcessExit> {
        Ok(ProcessExit::Code(0))
    }
}

/// A backend whose observed state is fixed by the test; `terminate` and
/// `resume` record whether they were invoked.
struct Stateful {
    state: ObservedProcessState,
    terminated: RefCell<bool>,
    resumed: RefCell<bool>,
}
impl Stateful {
    fn new(state: ObservedProcessState) -> Self {
        Self {
            state,
            terminated: RefCell::new(false),
            resumed: RefCell::new(false),
        }
    }
}
impl DurableIsolation for Stateful {
    fn backend_name(&self) -> &'static str {
        "fake"
    }
    fn start(&self, _: &SessionId, _: &ExecutionRequest) -> anyhow::Result<BackendReference> {
        Ok(BackendReference("native-1".into()))
    }
    fn find(&self, _: &SessionId) -> anyhow::Result<Option<BackendReference>> {
        Ok(None)
    }
    fn inspect(&self, _: &BackendReference) -> anyhow::Result<ObservedProcessState> {
        Ok(self.state.clone())
    }
    fn attach(&self, _: &BackendReference) -> anyhow::Result<Box<dyn ProcessConnection>> {
        Ok(Box::new(ImmediateExit))
    }
    fn resume(&self, _: &BackendReference) -> anyhow::Result<Box<dyn ProcessConnection>> {
        *self.resumed.borrow_mut() = true;
        Ok(Box::new(ImmediateExit))
    }
    fn wait(&self, _: &BackendReference) -> anyhow::Result<ProcessExit> {
        anyhow::bail!("unused")
    }
    fn terminate(&self, _: &BackendReference, _: Duration) -> anyhow::Result<()> {
        *self.terminated.borrow_mut() = true;
        Ok(())
    }
    fn remove(&self, _: &BackendReference) -> anyhow::Result<()> {
        anyhow::bail!("unused")
    }
}

fn null_io() -> ExecutionIo {
    ExecutionIo {
        stdin: File::open("/dev/null").unwrap(),
        stdout: File::options().write(true).open("/dev/null").unwrap(),
        stderr: File::options().write(true).open("/dev/null").unwrap(),
    }
}

fn stateful_runner<'a>(
    backend: &'a Stateful,
    test: &str,
) -> (SessionRunner<'a>, SessionId, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("driva-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let runner = SessionRunner::new(backend, SessionStore::new(&root));
    let id = runner.start(request()).unwrap().record.id;
    (runner, id, root)
}

#[test]
fn ids_are_canonical_uuid_v4_and_noncanonical_ids_are_rejected() {
    let id = SessionId::new();
    assert_eq!(id.0.len(), 36);
    assert_eq!(&id.0[14..15], "4");
    assert_eq!(id.0.parse::<SessionId>().unwrap(), id);
    assert!(id.0.to_uppercase().parse::<SessionId>().is_err());
    assert!("legacy-id".parse::<SessionId>().is_err());
}

#[test]
fn prepared_record_survives_backend_creation_failure() {
    let root = std::env::temp_dir().join(format!("driva-prepared-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let runner = SessionRunner::new(&FailingStart, SessionStore::new(&root));
    assert!(runner.start(request()).is_err());
    let records = runner.store.list().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].backend_reference.is_none());
    assert_eq!(records[0].schema_version, 1);
    let _ = std::fs::remove_dir_all(root);
}

fn request() -> ExecutionRequest {
    ExecutionRequest {
        command: vec!["sh".into()],
        working_directory: "/tmp".into(),
        mounts: vec![],
        environment: BTreeMap::from([("TOKEN".into(), "secret".into())]),
        network: false,
        interactive: false,
    }
}

#[test]
fn records_redacted_requests_and_observes_backend_state() {
    let root = std::env::temp_dir().join(format!("driva-session-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let fake = Fake {
        state: RefCell::new(ObservedProcessState::Created),
    };
    let runner = SessionRunner::new(&fake, SessionStore::new(&root));
    let started = runner.start(request()).unwrap();
    let record = runner.store.load(&started.record.id).unwrap();
    assert_eq!(record.request.environment_names, ["TOKEN"]);
    assert!(
        !std::fs::read_to_string(root.join(&record.id.0).join("record.toml"))
            .unwrap()
            .contains("secret")
    );
    assert_eq!(
        runner.inspect(&record.id).unwrap().observed,
        ObservedProcessState::Running
    );
    assert_eq!(runner.wait(&record.id).unwrap().exit, ProcessExit::Code(7));
    assert_eq!(
        runner.remove(&record.id).unwrap().state,
        ObservedProcessState::Missing
    );
    assert!(runner.store.list().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn terminate_on_exited_session_reports_exit_without_stopping() {
    let backend = Stateful::new(ObservedProcessState::Exited(ProcessExit::Code(3)));
    let (runner, id, root) = stateful_runner(&backend, "terminate-exited");
    let outcome = runner.terminate(&id, Duration::from_secs(1)).unwrap();
    assert_eq!(outcome.exit, ProcessExit::Code(3));
    assert!(!*backend.terminated.borrow());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn terminate_on_created_session_explains_next_steps() {
    let backend = Stateful::new(ObservedProcessState::Created);
    let (runner, id, root) = stateful_runner(&backend, "terminate-created");
    let error = runner.terminate(&id, Duration::from_secs(1)).unwrap_err();
    assert!(error.to_string().contains("never ran"), "{error}");
    assert!(error.to_string().contains("driva remove"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn attach_on_exited_session_suggests_removal() {
    let backend = Stateful::new(ObservedProcessState::Exited(ProcessExit::Code(0)));
    let (runner, id, root) = stateful_runner(&backend, "attach-exited");
    let error = runner.attach(&id, null_io()).unwrap_err();
    assert!(error.to_string().contains("exited(0)"), "{error}");
    assert!(error.to_string().contains("driva remove"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn attach_on_created_session_resumes_it() {
    let backend = Stateful::new(ObservedProcessState::Created);
    let (runner, id, root) = stateful_runner(&backend, "attach-created");
    assert_eq!(runner.attach(&id, null_io()).unwrap(), ProcessExit::Code(0));
    assert!(*backend.resumed.borrow());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn observed_states_render_for_humans() {
    assert_eq!(
        ObservedProcessState::Exited(ProcessExit::Code(0)).to_string(),
        "exited(0)"
    );
    assert_eq!(ObservedProcessState::Running.to_string(), "running");
    assert_eq!(
        ObservedProcessState::Created.to_string(),
        "created (never started)"
    );
    assert_eq!(
        ObservedProcessState::Unknown { error: "boom".into() }.to_string(),
        "unknown: boom"
    );
}
