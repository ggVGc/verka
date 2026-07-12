use driva::{
    BackendReference, DurableIsolation, ExecutionRequest, ObservedProcessState, ProcessConnection,
    ProcessExit, SessionId, SessionRunner, SessionStore,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

struct Fake {
    state: RefCell<ObservedProcessState>,
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
