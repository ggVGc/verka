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
    fn ensure_no_automatic_action(&self, id: &str) -> Result<()>;
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
    environment.ensure_no_automatic_action(id)?;
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

    fn ensure_no_automatic_action(&self, id: &str) -> Result<()> {
        ServerState::ensure_no_automatic_action(self, id)
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
mod tests;
