//! Provider switching orchestration, independent of the filesystem and agents.
use super::*;
use crate::agent::Provider;
use crate::event::{AgentEvent, Protocol};

/// Host operations needed to switch and restart an existing interaction.
/// Conversion and eligibility decisions stay in the workflow; implementations
/// provide storage, live-process observations, and the final process launch.
trait Environment {
    type Resumed;

    fn summary(&self, id: &str) -> Result<SessionSummary>;
    fn activity(&self, id: &str) -> Option<InteractionActivity>;
    fn finish_shutdown(&self, id: &str) -> bool;
    fn ensure_not_integrating(&self, id: &str) -> Result<()>;
    fn native_transcripts(&self) -> &dyn NativeTranscripts;
    fn native_location(&self, session: &SessionSummary) -> Result<(String, PathBuf)>;
    fn record_switch(
        &self,
        session: &SessionSummary,
        selection: &Selection,
        native_id: &str,
        marker: AgentEvent,
    ) -> Result<()>;
    fn restart(&self, request: ResumeSession) -> Result<Self::Resumed>;
}

fn resume_inner<E: Environment>(environment: &E, request: ResumeSession) -> Result<E::Resumed> {
    if let Some(activity) = environment.activity(&request.id) {
        anyhow::ensure!(
            !activity.accepting(),
            "session {:?} already has a live interaction",
            request.id
        );
        // A stop marks the interaction stopped before its sandbox has exited.
        anyhow::ensure!(
            environment.finish_shutdown(&request.id),
            "session {:?}'s previous agent has not exited yet; try again in a moment",
            request.id
        );
    }
    if let Some(selection) = &request.selection {
        let stored = environment.summary(&request.id)?;
        if selection.provider != stored.selection.provider {
            switch_with(environment, &request.id, selection)?;
        }
    }
    environment.restart(request)
}

fn switch_with(
    environment: &impl Environment,
    id: &str,
    selection: &Selection,
) -> Result<SessionSummary> {
    environment.ensure_not_integrating(id)?;
    crate::agent::validate_selection(selection)?;
    let summary = environment.summary(id)?;
    let from = summary.selection.provider;
    anyhow::ensure!(
        crate::agent::PROVIDERS.contains(&selection.provider),
        "target provider is not interactive"
    );
    anyhow::ensure!(
        summary.completed != CompletionState::Sealed,
        "a sealed session cannot switch provider"
    );
    if let Some(activity) = environment.activity(id) {
        ensure_provider_switchable(activity)?;
        anyhow::ensure!(
            environment.finish_shutdown(id),
            "previous agent has not exited yet; try again in a moment"
        );
    }
    if from == selection.provider {
        return Ok(summary);
    }
    let (source_id, cwd) = environment.native_location(&summary)?;
    let source = environment.native_transcripts().read(from, &source_id)?;
    let native_id = uuid::Uuid::new_v4().to_string();
    super::native_transcripts::write_converted(
        environment.native_transcripts(),
        &source,
        from,
        selection.provider,
        &native_id,
        &cwd,
        BranchPoint {
            at_ms: None,
            history: crate::protocol::BranchHistory::ThroughSelected,
            selected_operator_message: None,
        },
    )?;
    environment.record_switch(
        &summary,
        selection,
        &native_id,
        AgentEvent::ProviderChanged {
            from: from.as_str().to_owned(),
            to: selection.provider.as_str().to_owned(),
            model: selection.model.clone(),
            effort: selection.effort.as_str().to_owned(),
        },
    )?;
    environment.summary(id)
}

pub(super) fn resume(state: &ServerState, request: ResumeSession) -> Result<SessionInfo> {
    resume_inner(state, request)
}

pub(super) fn switch(
    state: &ServerState,
    id: &str,
    selection: &Selection,
) -> Result<SessionSummary> {
    switch_with(state, id, selection)
}

impl Environment for ServerState {
    type Resumed = SessionInfo;

    fn summary(&self, id: &str) -> Result<SessionSummary> {
        self.stored_summary(id)
    }

    fn activity(&self, id: &str) -> Option<InteractionActivity> {
        self.interaction(id)
            .ok()
            .map(|previous| previous.activity.activity())
    }

    fn finish_shutdown(&self, id: &str) -> bool {
        self.interaction(id)
            .map_or(true, |previous| previous.interaction.shut_down())
    }

    fn ensure_not_integrating(&self, id: &str) -> Result<()> {
        ServerState::ensure_not_integrating(self, id)
    }

    fn native_transcripts(&self) -> &dyn NativeTranscripts {
        self.inner.native_transcripts.as_ref()
    }

    fn native_location(&self, summary: &SessionSummary) -> Result<(String, PathBuf)> {
        let native_id = journal::read_provider_session_id(&summary.path)?
            .context("session has no stored provider session id")?;
        let (_, cwd) = self.stored_workspace_mount(summary)?;
        Ok((native_id, cwd))
    }

    fn record_switch(
        &self,
        summary: &SessionSummary,
        selection: &Selection,
        native_id: &str,
        marker: AgentEvent,
    ) -> Result<()> {
        let protocol = match selection.provider {
            Provider::Claude => Protocol::ClaudeJsonl,
            Provider::Codex => Protocol::CodexAppServer,
        };
        let previous_protocol = journal::read_session_meta(&summary.path)?.protocol;
        journal::prepare_provider_change(&summary.path, previous_protocol)?;
        Journal::open(&summary.path)?
            .record_provider_change(summary.selection.provider, selection)?;
        journal::store_session_provider(&summary.path, selection, protocol, native_id)?;
        if let Ok(previous) = self.interaction(&summary.id) {
            *previous
                .selection
                .lock()
                .expect("interaction selection lock poisoned") = selection.clone();
            push_sequenced(
                &mut previous
                    .updates
                    .lock()
                    .expect("interaction updates lock poisoned"),
                InteractionUpdate::Event(marker),
            );
        }
        self.publish_roster();
        Ok(())
    }

    fn restart(&self, request: ResumeSession) -> Result<SessionInfo> {
        self.resume_prepared_session(request)
    }
}

#[cfg(test)]
mod tests {
    use super::super::native_transcripts::MemoryTranscripts;
    use super::*;
    use std::cell::RefCell;

    const SESSION: &str = "interaction-1";
    const TRANSCRIPT: &str = concat!(
        r#"{"type":"user","uuid":"u1","parentUuid":null,"cwd":"/project","sessionId":"claude-source","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"why is the checkout failing?"}}"#,
        "\n",
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","cwd":"/project","sessionId":"claude-source","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"the lockfile is stale"}]}}"#,
        "\n",
    );

    struct Memory {
        sessions: HashMap<String, SessionSummary>,
        events: Vec<AgentEvent>,
        native_id: String,
        calls: Vec<&'static str>,
        restarted: Vec<ResumeSession>,
    }

    /// A strict in-memory substitute for the host. No sockets, environment
    /// changes, files, shell scripts, or agent processes are needed.
    struct MockEnvironment {
        memory: RefCell<Memory>,
        activity: Option<(InteractionActivity, bool)>,
        transcripts: MemoryTranscripts,
    }

    impl MockEnvironment {
        fn stopped() -> Self {
            let summary = SessionSummary {
                id: SESSION.into(),
                name: Some("review".into()),
                first_prompt: Some("why is the checkout failing?".into()),
                tags: vec![],
                workspace_id: "workspace-1".into(),
                path: "/store/workspace-1/interaction-1".into(),
                selection: Selection::new(Provider::Claude),
                age: "1h ago".into(),
                created_at_ms: Some(1),
                last_event_at_ms: Some(2),
                last_event_age: "1h ago".into(),
                origin: None,
                branched_from: None,
                completed: CompletionState::Active,
                integration: None,
            };
            let transcripts = MemoryTranscripts::default();
            transcripts
                .write(
                    Provider::Claude,
                    "claude-source",
                    Path::new("/project"),
                    TRANSCRIPT,
                )
                .unwrap();
            Self {
                memory: RefCell::new(Memory {
                    sessions: HashMap::from([(SESSION.into(), summary)]),
                    events: vec![
                        AgentEvent::UserMessage {
                            text: "why is the checkout failing?".into(),
                        },
                        AgentEvent::AgentMessage {
                            text: "the lockfile is stale".into(),
                        },
                    ],
                    native_id: "claude-source".into(),
                    calls: vec![],
                    restarted: vec![],
                }),
                activity: Some((InteractionActivity::Stopped, true)),
                transcripts,
            }
        }

        fn request(selection: Selection) -> ResumeSession {
            ResumeSession {
                id: SESSION.into(),
                launch: LaunchPolicy {
                    network: Some(true),
                    writable_workspace: Some(false),
                    templates: vec!["review-tools".into()],
                    ..LaunchPolicy::default()
                },
                selection: Some(selection),
            }
        }

        fn assert_untouched(&self, original: &SessionSummary) {
            let memory = self.memory.borrow();
            assert_eq!(memory.sessions.len(), 1);
            assert_eq!(&memory.sessions[SESSION], original);
            assert_eq!(memory.events.len(), 2);
            assert_eq!(self.transcripts.len(), 1);
            assert_eq!(
                self.transcripts
                    .read(Provider::Claude, "claude-source")
                    .unwrap(),
                TRANSCRIPT
            );
            assert!(memory.calls.is_empty());
            assert!(memory.restarted.is_empty());
        }
    }

    impl Environment for MockEnvironment {
        type Resumed = ResumeSession;

        fn summary(&self, id: &str) -> Result<SessionSummary> {
            self.memory
                .borrow()
                .sessions
                .get(id)
                .cloned()
                .context("unknown session")
        }

        fn activity(&self, id: &str) -> Option<InteractionActivity> {
            assert_eq!(id, SESSION);
            self.activity.map(|(activity, _)| activity)
        }

        fn finish_shutdown(&self, id: &str) -> bool {
            assert_eq!(id, SESSION);
            assert_eq!(
                self.activity.unwrap().0,
                InteractionActivity::Stopped,
                "never stop a live agent as a side effect of a rejected switch"
            );
            self.activity.unwrap().1
        }

        fn ensure_not_integrating(&self, id: &str) -> Result<()> {
            assert_eq!(id, SESSION);
            Ok(())
        }

        fn native_transcripts(&self) -> &dyn NativeTranscripts {
            &self.transcripts
        }

        fn native_location(&self, session: &SessionSummary) -> Result<(String, PathBuf)> {
            assert!(self.memory.borrow().sessions.contains_key(&session.id));
            Ok((self.memory.borrow().native_id.clone(), "/project".into()))
        }

        fn record_switch(
            &self,
            session: &SessionSummary,
            selection: &Selection,
            native_id: &str,
            marker: AgentEvent,
        ) -> Result<()> {
            let mut memory = self.memory.borrow_mut();
            // Recording may only point at a transcript that was successfully stored.
            self.transcripts.read(selection.provider, native_id)?;
            memory.native_id = native_id.into();
            // Update the existing record, never create a new session.
            memory.sessions.get_mut(&session.id).unwrap().selection = selection.clone();
            memory.events.push(marker);
            memory.calls.push("record provider switch");
            Ok(())
        }

        fn restart(&self, request: ResumeSession) -> Result<ResumeSession> {
            let mut memory = self.memory.borrow_mut();
            assert!(memory.sessions.contains_key(&request.id));
            memory.calls.push("restart interaction");
            memory.restarted.push(request.clone());
            Ok(request)
        }
    }

    #[test]
    fn switching_provider_logs_the_change_then_restarts_the_same_interaction_with_the_chosen_model()
    {
        let environment = MockEnvironment::stopped();
        let original = environment.summary(SESSION).unwrap();
        let original_events = environment.memory.borrow().events.clone();
        let chosen = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
        let request = MockEnvironment::request(chosen.clone());

        let restarted = resume_inner(&environment, request.clone()).unwrap();

        assert_eq!(
            restarted, request,
            "restart the same id with the requested model, effort and launch policy"
        );
        let memory = environment.memory.borrow();
        assert_eq!(memory.restarted, vec![request]);
        assert_eq!(
            memory.calls,
            vec!["record provider switch", "restart interaction"]
        );
        assert_eq!(memory.sessions.len(), 1, "no sibling session is created");
        let mut expected = original;
        expected.selection = chosen.clone();
        assert_eq!(
            memory.sessions[SESSION], expected,
            "identity, checkout record, branch origin and completion stay unchanged"
        );
        assert_eq!(environment.transcripts.len(), 2);
        assert_ne!(memory.native_id, "claude-source");
        let converted = genta::session::parse(
            &environment
                .transcripts
                .read(Provider::Codex, &memory.native_id)
                .unwrap(),
            native_session_format(Provider::Codex),
        )
        .unwrap();
        let source =
            genta::session::parse(TRANSCRIPT, native_session_format(Provider::Claude)).unwrap();
        assert_eq!(converted.id, memory.native_id);
        assert_eq!(converted.cwd, "/project");
        assert_eq!(
            converted.messages, source.messages,
            "retain the full conversation"
        );
        assert_eq!(
            environment
                .transcripts
                .read(Provider::Claude, "claude-source")
                .unwrap(),
            TRANSCRIPT
        );
        let mut expected_events = original_events;
        expected_events.push(AgentEvent::ProviderChanged {
            from: "claude".into(),
            to: "codex".into(),
            model: chosen.model,
            effort: "high".into(),
        });
        assert_eq!(
            memory.events, expected_events,
            "preserve the log and append exactly one provider marker, with no branch marker"
        );
    }

    #[test]
    fn an_interaction_must_finish_stopping_before_switching_or_restarting() {
        use InteractionActivity::{Background, Pending, Running, Stopped};
        for (activity, resume_error, switch_error) in [
            (Running, "live interaction", "stop the interaction"),
            (Background, "live interaction", "stop the interaction"),
            (Pending, "live interaction", "stop the interaction"),
            (Stopped, "not exited", "not exited"),
        ] {
            let mut environment = MockEnvironment::stopped();
            environment.activity = Some((activity, false));
            let original = environment.summary(SESSION).unwrap();
            let chosen = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
            let error = resume_inner(&environment, MockEnvironment::request(chosen.clone()))
                .unwrap_err()
                .to_string();
            assert!(error.contains(resume_error), "{activity:?}: {error}");
            let error = switch_with(&environment, SESSION, &chosen)
                .unwrap_err()
                .to_string();
            assert!(error.contains(switch_error), "{activity:?}: {error}");
            environment.assert_untouched(&original);
        }
    }

    #[test]
    fn a_failed_conversion_write_does_not_change_the_session_or_restart_it() {
        let environment = MockEnvironment::stopped();
        environment.transcripts.fail_writes(true);
        let original = environment.summary(SESSION).unwrap();
        let chosen = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
        assert!(resume_inner(&environment, MockEnvironment::request(chosen))
            .unwrap_err()
            .to_string()
            .contains("native storage unavailable"));
        environment.assert_untouched(&original);
    }

    #[test]
    fn a_sealed_session_cannot_switch_or_restart() {
        let environment = MockEnvironment::stopped();
        environment
            .memory
            .borrow_mut()
            .sessions
            .get_mut(SESSION)
            .unwrap()
            .completed = CompletionState::Sealed;
        let original = environment.summary(SESSION).unwrap();
        let chosen = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
        assert!(resume_inner(&environment, MockEnvironment::request(chosen))
            .unwrap_err()
            .to_string()
            .contains("sealed"));
        environment.assert_untouched(&original);
    }

    #[test]
    fn a_model_change_within_the_same_provider_only_restarts() {
        let environment = MockEnvironment::stopped();
        let mut chosen = Selection::new(Provider::Claude);
        chosen.model = "opus".into();
        let request = MockEnvironment::request(chosen);
        assert_eq!(
            resume_inner(&environment, request.clone()).unwrap(),
            request
        );
        let memory = environment.memory.borrow();
        assert_eq!(memory.calls, vec!["restart interaction"]);
        assert_eq!(
            memory.events.len(),
            2,
            "no provider marker for a model-only change"
        );
        assert_eq!(
            environment
                .transcripts
                .read(Provider::Claude, "claude-source")
                .unwrap(),
            TRANSCRIPT
        );
    }
}
