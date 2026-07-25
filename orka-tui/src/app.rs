use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use linka::{Author, CandidateId, NodeId, Store, VerificationOutcome};
use orka::{
    attempt::{AttemptId, FsAttemptStore, SealedState},
    candidate::Candidates,
    config::{Config, CONFIG_FILE},
    engine::{Engine, RunProgress},
    events::{work_log_from_raw, ContentBlock, WorkLogBlock},
    linka_work::LinkaWork,
    review::{AbandonOutcome, FinishOutcome, Reviews},
    review_worktree::{GitReviewWorktrees, ReviewCleanupOutcome},
    workspace::GitWorkspaces,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Ready,
    Attempts,
    Candidates,
    Reviews,
    Worktrees,
    Audit,
    Errors,
}

impl View {
    pub const ALL: [Self; 7] = [
        Self::Ready,
        Self::Attempts,
        Self::Candidates,
        Self::Reviews,
        Self::Worktrees,
        Self::Audit,
        Self::Errors,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Attempts => "Attempts",
            Self::Candidates => "Candidates",
            Self::Reviews => "Reviews",
            Self::Worktrees => "Worktrees",
            Self::Audit => "Audit",
            Self::Errors => "Errors",
        }
    }
}

#[derive(Clone)]
pub struct Row {
    pub id: String,
    pub summary: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    RunSelected,
    RunNext,
    Recover,
    InitConfig,
    ViewAttempt,
    ViewTranscript,
    ViewDiagnostics,
    ViewRawEvents,
    ViewFileChanges,
    ViewAccesses,
    ViewPatch,
    AcceptCandidate,
    RejectCandidate,
    PublishCandidate,
    StartReview,
    ResumeReview,
    ShowReview,
    PrepareWorktree,
    CleanupWorktree,
    FinishAccepted,
    FinishRejected,
    AbandonReview,
    Audit,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::RunSelected => "Run selected node",
            Self::RunNext => "Run next ready node",
            Self::Recover => "Recover unfinished attempts",
            Self::InitConfig => "Create default orka.toml",
            Self::ViewAttempt => "Show durable attempt record",
            Self::ViewTranscript => "View rendered work log / transcript",
            Self::ViewDiagnostics => "View diagnostics",
            Self::ViewRawEvents => "View raw agent events",
            Self::ViewFileChanges => "View file-change checkpoints",
            Self::ViewAccesses => "View observed file accesses",
            Self::ViewPatch => "View candidate patch",
            Self::AcceptCandidate => "Accept with verification",
            Self::RejectCandidate => "Reject with verification",
            Self::PublishCandidate => "Publish candidate",
            Self::StartReview => "Start review",
            Self::ResumeReview => "Resume review",
            Self::ShowReview => "Show review entries",
            Self::PrepareWorktree => "Create/reuse review worktree",
            Self::CleanupWorktree => "Clean up review worktree",
            Self::FinishAccepted => "Finish review: accepted",
            Self::FinishRejected => "Finish review: rejected",
            Self::AbandonReview => "Abandon review",
            Self::Audit => "Audit all output evidence",
        }
    }
}

pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
}

pub struct Form {
    pub action: Action,
    pub target: String,
    pub fields: Vec<Field>,
    pub selected: usize,
    pub error: Option<String>,
}

pub enum Overlay {
    Actions {
        actions: Vec<Action>,
        selected: usize,
    },
    Form(Form),
    Text {
        title: String,
        body: String,
        scroll: u16,
    },
    Confirm {
        action: Action,
        target: String,
    },
    Help,
}

enum WorkerEvent {
    Progress(String),
    Done(std::result::Result<String, String>),
}

pub struct App {
    pub root: PathBuf,
    pub rows: [Vec<Row>; 7],
    pub view: View,
    pub selected: usize,
    pub overlay: Option<Overlay>,
    pub status: String,
    pub busy: bool,
    pub should_quit: bool,
    worker: Option<Receiver<WorkerEvent>>,
}

impl App {
    pub fn open(given: Option<PathBuf>) -> Result<Self> {
        let root = locate_workbench(given)?;
        let mut app = Self {
            root,
            rows: std::array::from_fn(|_| Vec::new()),
            view: View::Ready,
            selected: 0,
            overlay: None,
            status: String::new(),
            busy: false,
            should_quit: false,
            worker: None,
        };
        app.refresh();
        Ok(app)
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows[self.view as usize]
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows().get(self.selected)
    }

    pub fn refresh(&mut self) {
        let old_id = self.selected_row().map(|row| row.id.clone());
        self.rows = std::array::from_fn(|_| Vec::new());
        let mut errors = Vec::new();

        match Store::open(self.root.join(".linka")) {
            Ok(store) => {
                match LinkaWork::new(&store).ready_for_machine() {
                    Ok(items) => {
                        self.rows[View::Ready as usize] = items
                            .into_iter()
                            .map(|item| Row {
                                id: item.node.to_string(),
                                summary: item.title.clone(),
                                detail: format!(
                                    "node      {}\ntitle     {}",
                                    item.node, item.title
                                ),
                            })
                            .collect();
                    }
                    Err(error) => errors.push(format!("ready work: {error:#}")),
                }

                let attempt_store = FsAttemptStore::new(self.root.join(".orka"));
                match attempt_store.list() {
                    Ok(ids) => {
                        for id in ids.into_iter().rev() {
                            match attempt_store.load(&id) {
                                Ok(snapshot) => {
                                    let phase = format!("{:?}", snapshot.phase());
                                    let node = snapshot.record.input.node().to_string();
                                    let seal = snapshot
                                        .seal
                                        .as_ref()
                                        .map(|seal| seal_text(&seal.state))
                                        .unwrap_or_else(|| "not sealed".into());
                                    let mut detail = format!(
                                        "attempt   {id}\nnode      {node}\nphase     {phase}\ncreated   {}\ninput     {}\ntarget    {}\nsealed    {seal}",
                                        snapshot.record.created_at_ms,
                                        snapshot.record.input.input_commit(),
                                        snapshot.record.input.target_branch,
                                    );
                                    if let Some(workspace) = &snapshot.workspace {
                                        detail.push_str(&format!(
                                            "\nbranch    {}\nworkspace {}",
                                            workspace.branch,
                                            workspace.path.display()
                                        ));
                                    }
                                    if let Some(evidence) = &snapshot.evidence {
                                        detail.push_str(&format!(
                                            "\nexit      {} via {}\nduration  {} ms",
                                            evidence.exit_code,
                                            evidence.backend,
                                            evidence.finished_at_ms - evidence.started_at_ms
                                        ));
                                    }
                                    self.rows[View::Attempts as usize].push(Row {
                                        id: id.0,
                                        summary: format!("{node}  {phase}"),
                                        detail,
                                    });
                                }
                                Err(error) => errors.push(format!("{id}: {error:#}")),
                            }
                        }
                    }
                    Err(error) => errors.push(format!("attempts: {error:#}")),
                }

                match Candidates::new(&store).list() {
                    Ok(items) => {
                        self.rows[View::Candidates as usize] = items
                            .into_iter()
                            .map(|item| {
                                let status = item.status();
                                let detail = format!(
                                    "candidate {}\nnode      {}\nstatus    {}\nbranch    {}\ntarget    {}\ninput     {}\nhead      {}\nattempt   {}",
                                    item.id,
                                    item.node,
                                    status,
                                    item.branch,
                                    item.target,
                                    item.input_commit.as_deref().unwrap_or("-"),
                                    item.head_commit,
                                    item.attempt
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_else(|| "-".into())
                                );
                                Row {
                                    id: item.id.0,
                                    summary: format!("{}  {}  {}", item.node, status, item.target),
                                    detail,
                                }
                            })
                            .collect();
                    }
                    Err(error) => errors.push(format!("candidates: {error:#}")),
                }

                let reviews = Reviews::new(&store, self.root.join(".orka"));
                match reviews.list() {
                    Ok(records) => {
                        for record in records {
                            let (state, entries) = match reviews.review(&record.verification) {
                                Ok((_, review)) => (
                                    format!(
                                        "ready ({} entr{})",
                                        review.entries.len(),
                                        if review.entries.len() == 1 {
                                            "y"
                                        } else {
                                            "ies"
                                        }
                                    ),
                                    review
                                        .entries
                                        .iter()
                                        .map(|entry| {
                                            format!(
                                                "{:?} {}  {}\n    paths: {}",
                                                entry.kind,
                                                short(&entry.commit),
                                                entry.message.lines().next().unwrap_or_default(),
                                                if entry.paths.is_empty() {
                                                    "-".into()
                                                } else {
                                                    entry.paths.join(", ")
                                                }
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                ),
                                Err(error) => (format!("needs resume: {error}"), String::new()),
                            };
                            self.rows[View::Reviews as usize].push(Row {
                                id: record.verification.to_string(),
                                summary: format!("{}  {}  {state}", record.candidate, record.branch),
                                detail: format!(
                                    "verification {}\ncandidate    {}\nbranch       {}\nsubject      {}\nstate        {state}{}",
                                    record.verification,
                                    record.candidate,
                                    record.branch,
                                    record.subject,
                                    if entries.is_empty() { String::new() } else { format!("\n\nentries\n{entries}") }
                                ),
                            });
                        }
                    }
                    Err(error) => errors.push(format!("reviews: {error:#}")),
                }

                let worktrees = review_worktrees(&self.root, &store);
                match worktrees.list() {
                    Ok(items) => {
                        self.rows[View::Worktrees as usize] = items
                            .into_iter()
                            .map(|item| Row {
                                id: item.verification.to_string(),
                                summary: format!(
                                    "{}  {}  {}",
                                    if item.dirty { "DIRTY" } else { "clean" },
                                    item.branch,
                                    item.path.display()
                                ),
                                detail: format!(
                                    "verification {}\nstate        {}\nbranch       {}\npath         {}",
                                    item.verification,
                                    if item.dirty { "dirty (cleanup will retain it)" } else { "clean" },
                                    item.branch,
                                    item.path.display()
                                ),
                            })
                            .collect();
                    }
                    Err(error) => errors.push(format!("review worktrees: {error:#}")),
                }
            }
            Err(error) => errors.push(format!("opening Linka store: {error:#}")),
        }

        self.rows[View::Audit as usize] = vec![Row {
            id: "audit".into(),
            summary: "Press a to run the evidence audit".into(),
            detail: "Checks every Orka-produced output for its durable attempt, prompt, request, agent output, harness evidence, and declared outcome.".into(),
        }];
        self.rows[View::Errors as usize] = errors
            .iter()
            .enumerate()
            .map(|(index, error)| Row {
                id: format!("error-{index}"),
                summary: error.lines().next().unwrap_or(error).to_string(),
                detail: error.clone(),
            })
            .collect();
        self.selected = old_id
            .and_then(|id| self.rows().iter().position(|row| row.id == id))
            .unwrap_or(0)
            .min(self.rows().len().saturating_sub(1));
        self.status = format!(
            "{} ready · {} attempts · {} candidates · {} reviews · {} error(s)",
            self.rows[View::Ready as usize].len(),
            self.rows[View::Attempts as usize].len(),
            self.rows[View::Candidates as usize].len(),
            self.rows[View::Reviews as usize].len(),
            self.rows[View::Errors as usize].len()
        );
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.overlay.is_some() {
            self.on_overlay_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') if self.busy => {
                self.status =
                    "An Orka operation is still running; wait for it to finish before quitting"
                        .into()
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('a') => self.open_actions(),
            KeyCode::Enter => {
                if let Some(row) = self.selected_row() {
                    self.overlay = Some(Overlay::Text {
                        title: row.id.clone(),
                        body: row.detail.clone(),
                        scroll: 0,
                    });
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.rows().len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.rows().len().saturating_sub(1)
            }
            KeyCode::Left | KeyCode::BackTab => self.change_view(-1),
            KeyCode::Right | KeyCode::Tab => self.change_view(1),
            KeyCode::Char(c @ '1'..='7') => {
                self.view = View::ALL[(c as u8 - b'1') as usize];
                self.selected = 0;
            }
            _ => {}
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        match &mut overlay {
            Overlay::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => return,
                _ => {}
            },
            Overlay::Text { scroll, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return,
                KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown => *scroll = scroll.saturating_add(20),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(20),
                KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                _ => {}
            },
            Overlay::Actions { actions, selected } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return,
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1).min(actions.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(action) = actions.get(*selected).copied() {
                        self.begin_action(action);
                    }
                    return;
                }
                _ => {}
            },
            Overlay::Confirm { action, target } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.start_worker(*action, target.clone(), vec![]);
                    return;
                }
                KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => return,
                _ => {}
            },
            Overlay::Form(form) => match key.code {
                KeyCode::Esc => return,
                KeyCode::Tab | KeyCode::Down => {
                    form.selected = (form.selected + 1) % form.fields.len()
                }
                KeyCode::BackTab | KeyCode::Up => {
                    form.selected = form
                        .selected
                        .checked_sub(1)
                        .unwrap_or(form.fields.len() - 1)
                }
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let values = form
                        .fields
                        .iter()
                        .map(|field| field.value.clone())
                        .collect();
                    self.start_worker(form.action, form.target.clone(), values);
                    return;
                }
                KeyCode::Enter if form.selected + 1 < form.fields.len() => form.selected += 1,
                KeyCode::Enter => {
                    let values = form
                        .fields
                        .iter()
                        .map(|field| field.value.clone())
                        .collect();
                    self.start_worker(form.action, form.target.clone(), values);
                    return;
                }
                KeyCode::Backspace => {
                    form.fields[form.selected].value.pop();
                    form.error = None;
                }
                KeyCode::Char(c) => {
                    form.fields[form.selected].value.push(c);
                    form.error = None;
                }
                _ => {}
            },
        }
        self.overlay = Some(overlay);
    }

    fn change_view(&mut self, delta: isize) {
        let len = View::ALL.len() as isize;
        let current = self.view as isize;
        self.view = View::ALL[((current + delta).rem_euclid(len)) as usize];
        self.selected = 0;
    }

    fn open_actions(&mut self) {
        if self.busy {
            self.status = "An action is already running".into();
            return;
        }
        let mut actions = match self.view {
            View::Ready => vec![Action::RunSelected, Action::RunNext],
            View::Attempts => vec![
                Action::ViewAttempt,
                Action::ViewTranscript,
                Action::ViewDiagnostics,
                Action::ViewRawEvents,
                Action::ViewFileChanges,
                Action::ViewAccesses,
                Action::Recover,
            ],
            View::Candidates => vec![
                Action::ViewPatch,
                Action::StartReview,
                Action::AcceptCandidate,
                Action::RejectCandidate,
                Action::PublishCandidate,
            ],
            View::Reviews => vec![
                Action::ShowReview,
                Action::ResumeReview,
                Action::PrepareWorktree,
                Action::FinishAccepted,
                Action::FinishRejected,
                Action::AbandonReview,
                Action::CleanupWorktree,
            ],
            View::Worktrees => vec![Action::PrepareWorktree, Action::CleanupWorktree],
            View::Audit => vec![Action::Audit],
            View::Errors => vec![],
        };
        for action in [Action::Recover, Action::InitConfig, Action::Audit] {
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
        self.overlay = Some(Overlay::Actions {
            actions,
            selected: 0,
        });
    }

    fn begin_action(&mut self, action: Action) {
        let target = self
            .selected_row()
            .map(|row| row.id.clone())
            .unwrap_or_default();
        match action {
            Action::ViewAttempt => self.show_attempt(&target),
            Action::ViewTranscript => self.show_attempt_file(&target, AttemptFile::Transcript),
            Action::ViewDiagnostics => self.show_attempt_file(&target, AttemptFile::Diagnostics),
            Action::ViewRawEvents => self.show_attempt_file(&target, AttemptFile::RawEvents),
            Action::ViewFileChanges => self.show_attempt_file(&target, AttemptFile::FileChanges),
            Action::ViewAccesses => self.show_attempt_file(&target, AttemptFile::Accesses),
            Action::ViewPatch => self.show_patch(&target),
            Action::ShowReview => self.show_review(&target),
            Action::AcceptCandidate => self.form(
                action,
                target,
                vec![
                    Field {
                        label: "Verification node",
                        value: String::new(),
                        hint: "node-…",
                    },
                    Field {
                        label: "Notes",
                        value: String::new(),
                        hint: "optional",
                    },
                ],
            ),
            Action::RejectCandidate => self.form(
                action,
                target,
                vec![
                    Field {
                        label: "Verification node",
                        value: String::new(),
                        hint: "node-…",
                    },
                    Field {
                        label: "Notes",
                        value: String::new(),
                        hint: "required",
                    },
                ],
            ),
            Action::StartReview => self.form(
                action,
                target,
                vec![Field {
                    label: "Assignee",
                    value: "human".into(),
                    hint: "human or machine",
                }],
            ),
            Action::FinishAccepted | Action::FinishRejected => self.form(
                action,
                target,
                vec![
                    Field {
                        label: "Summary",
                        value: String::new(),
                        hint: "optional; generated from review when blank",
                    },
                    Field {
                        label: "Author",
                        value: "human".into(),
                        hint: "human or machine",
                    },
                ],
            ),
            Action::AbandonReview => self.form(
                action,
                target,
                vec![
                    Field {
                        label: "Notes",
                        value: String::new(),
                        hint: "optional",
                    },
                    Field {
                        label: "Author",
                        value: "human".into(),
                        hint: "human or machine",
                    },
                ],
            ),
            Action::PublishCandidate | Action::CleanupWorktree => {
                self.overlay = Some(Overlay::Confirm { action, target })
            }
            Action::RunSelected => {
                if target.is_empty() {
                    self.error_overlay("Run node", "No ready node is selected".into());
                } else {
                    self.start_worker(action, target, vec![]);
                }
            }
            _ => self.start_worker(action, target, vec![]),
        }
    }

    fn form(&mut self, action: Action, target: String, fields: Vec<Field>) {
        self.overlay = Some(Overlay::Form(Form {
            action,
            target,
            fields,
            selected: 0,
            error: None,
        }));
    }

    fn start_worker(&mut self, action: Action, target: String, values: Vec<String>) {
        if self.busy {
            return;
        }
        let root = self.root.clone();
        let (tx, rx) = mpsc::channel();
        self.worker = Some(rx);
        self.busy = true;
        self.status = format!("Running: {}", action.label());
        std::thread::spawn(move || {
            let result = execute_action(&root, action, &target, &values, &tx)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerEvent::Done(result));
        });
    }

    pub fn poll_worker(&mut self) {
        let Some(rx) = self.worker.take() else { return };
        let mut done = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                WorkerEvent::Progress(message) => self.status = message,
                WorkerEvent::Done(result) => done = Some(result),
            }
        }
        if let Some(result) = done {
            self.busy = false;
            self.refresh();
            match result {
                Ok(message) => {
                    self.status = message.clone();
                    self.overlay = Some(Overlay::Text {
                        title: "Action completed".into(),
                        body: message,
                        scroll: 0,
                    });
                }
                Err(error) => self.error_overlay("Action failed", error),
            }
        } else {
            self.worker = Some(rx);
        }
    }

    fn show_attempt(&mut self, id: &str) {
        let attempts = FsAttemptStore::new(self.root.join(".orka"));
        match attempts.load(&AttemptId(id.into())) {
            Ok(snapshot) => {
                self.overlay = Some(Overlay::Text {
                    title: format!("Attempt {id}"),
                    body: format!("{:#?}", snapshot),
                    scroll: 0,
                })
            }
            Err(error) => self.error_overlay("Attempt error", format!("{error:#}")),
        }
    }

    fn show_attempt_file(&mut self, id: &str, file: AttemptFile) {
        let attempts = FsAttemptStore::new(self.root.join(".orka"));
        let id = AttemptId(id.into());
        let path = match file {
            AttemptFile::Transcript => attempts.transcript_path(&id),
            AttemptFile::Diagnostics => attempts.diagnostics_path(&id),
            AttemptFile::RawEvents => attempts.raw_events_path(&id),
            AttemptFile::FileChanges => attempts.file_changes_path(&id),
            AttemptFile::Accesses => attempts.accesses_path(&id),
        };
        let loaded = if file == AttemptFile::Transcript {
            render_work_log(&attempts, &id)
                .or_else(|_| fs::read_to_string(&path).map_err(anyhow::Error::from))
        } else {
            fs::read_to_string(&path).map_err(anyhow::Error::from)
        };
        match loaded {
            Ok(body) => {
                self.overlay = Some(Overlay::Text {
                    title: format!("{} — {}", file.label(), path.display()),
                    body: if body.is_empty() {
                        "(empty)".into()
                    } else {
                        body
                    },
                    scroll: 0,
                })
            }
            Err(error) => self.error_overlay(
                file.label(),
                format!("Could not read {}: {error}", path.display()),
            ),
        }
    }

    fn show_patch(&mut self, target: &str) {
        let result = Store::open(self.root.join(".linka"))
            .and_then(|store| Candidates::new(&store).patch(target));
        match result {
            Ok(body) => {
                self.overlay = Some(Overlay::Text {
                    title: format!("Patch — {target}"),
                    body: if body.is_empty() {
                        "(no diff)".into()
                    } else {
                        body
                    },
                    scroll: 0,
                })
            }
            Err(error) => self.error_overlay("Patch error", format!("{error:#}")),
        }
    }

    fn show_review(&mut self, target: &str) {
        let result = (|| -> Result<String> {
            let store = Store::open(self.root.join(".linka"))?;
            let verification = parse_node(target)?;
            let (record, review) =
                Reviews::new(&store, self.root.join(".orka")).review(&verification)?;
            let mut text = format!(
                "verification {}\ncandidate    {}\nbranch       {}\nsubject      {}\nmarker       {}",
                record.verification, record.candidate, review.branch, review.subject, review.marker
            );
            if review.entries.is_empty() {
                text.push_str("\n\n(no review entries)");
            }
            for entry in review.entries {
                text.push_str(&format!(
                    "\n\n{:?} {}\npaths: {}\n{}",
                    entry.kind,
                    entry.commit,
                    if entry.paths.is_empty() {
                        "-".into()
                    } else {
                        entry.paths.join(", ")
                    },
                    entry.message
                ));
            }
            Ok(text)
        })();
        match result {
            Ok(body) => {
                self.overlay = Some(Overlay::Text {
                    title: format!("Review — {target}"),
                    body,
                    scroll: 0,
                })
            }
            Err(error) => self.error_overlay("Review error", format!("{error:#}")),
        }
    }

    fn error_overlay(&mut self, title: &str, body: String) {
        self.status = format!("{title}: {}", body.lines().next().unwrap_or_default());
        self.overlay = Some(Overlay::Text {
            title: format!("ERROR — {title}"),
            body,
            scroll: 0,
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttemptFile {
    Transcript,
    Diagnostics,
    RawEvents,
    FileChanges,
    Accesses,
}

impl AttemptFile {
    fn label(self) -> &'static str {
        match self {
            Self::Transcript => "Work log",
            Self::Diagnostics => "Diagnostics",
            Self::RawEvents => "Raw events",
            Self::FileChanges => "File changes",
            Self::Accesses => "File accesses",
        }
    }
}

fn locate_workbench(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = given {
        if !root.join(".linka").is_dir() {
            bail!("no .linka store under {}", root.display());
        }
        return Ok(root);
    }
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join(".linka").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("no Orka workbench found: no ancestor contains .linka/");
        }
    }
}

fn review_worktrees(root: &Path, store: &Store) -> GitReviewWorktrees {
    GitReviewWorktrees::new(store.project_root(), root.join(".orka/review-worktrees"))
}

fn parse_node(value: &str) -> Result<NodeId> {
    value
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid node id: {error}"))
}

fn parse_author(value: &str) -> Result<Author> {
    match value.trim().to_ascii_lowercase().as_str() {
        "human" => Ok(Author::Human),
        "machine" => Ok(Author::Machine),
        _ => bail!("author/assignee must be `human` or `machine`"),
    }
}

fn engine_parts(root: &Path) -> Result<(Store, Config)> {
    let store = Store::open(root.join(".linka"))?;
    let config_path = root.join(CONFIG_FILE);
    let config = Config::load(&config_path).with_context(|| {
        format!(
            "Orka needs {}; create it with the Init config action",
            config_path.display()
        )
    })?;
    Ok((store, config))
}

fn execute_action(
    root: &Path,
    action: Action,
    target: &str,
    values: &[String],
    tx: &Sender<WorkerEvent>,
) -> Result<String> {
    match action {
        Action::RunSelected | Action::RunNext => {
            let (store, config) = engine_parts(root)?;
            let executor = config.executor()?;
            let attempts = FsAttemptStore::new(root.join(".orka"));
            let workspaces = GitWorkspaces::new(store.project_root(), root.join(".orka/worktrees"));
            let engine = Engine {
                linka: LinkaWork::new(&store),
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy()?,
            };
            let mut progress = |event: &RunProgress| {
                let _ = tx.send(WorkerEvent::Progress(progress_text(event)));
            };
            let report = if action == Action::RunSelected {
                Some(engine.run_node_with_progress(&parse_node(target)?, &mut progress)?)
            } else {
                engine.run_next_with_progress(&mut progress)?
            };
            match report {
                None => Ok("Nothing is ready".into()),
                Some(report) => Ok(format!(
                    "Attempt {} finished for {}\nexit: {}\nsealed: {}\ncandidate: {}\ncleanup: {:?}",
                    report.attempt,
                    report.node,
                    report.exit_code,
                    seal_text(&report.sealed),
                    report.candidate.map(|id| id.0).unwrap_or_else(|| "-".into()),
                    report.cleanup
                )),
            }
        }
        Action::Recover => {
            let (store, config) = engine_parts(root)?;
            let executor = config.executor()?;
            let attempts = FsAttemptStore::new(root.join(".orka"));
            let workspaces = GitWorkspaces::new(store.project_root(), root.join(".orka/worktrees"));
            let engine = Engine {
                linka: LinkaWork::new(&store),
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy()?,
            };
            let reports = engine.recover()?;
            if reports.is_empty() {
                Ok("No attempts recorded".into())
            } else {
                Ok(reports
                    .into_iter()
                    .map(|report| format!("{}  {}  {}", report.attempt, report.node, report.action))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        }
        Action::InitConfig => {
            let path = root.join(CONFIG_FILE);
            Config::init(&path)?;
            Ok(format!("Created {}", path.display()))
        }
        Action::AcceptCandidate | Action::RejectCandidate => {
            let store = Store::open(root.join(".linka"))?;
            let verification = values.first().context("verification node is required")?;
            let verification = parse_node(verification)?;
            let notes = values.get(1).cloned().unwrap_or_default();
            let candidate = if action == Action::AcceptCandidate {
                Candidates::new(&store).accept(target, &verification, notes)?
            } else {
                if notes.trim().is_empty() {
                    bail!("rejection notes are required");
                }
                Candidates::new(&store).reject(target, &verification, notes)?
            };
            Ok(format!("{} is now {}", candidate.id, candidate.status()))
        }
        Action::PublishCandidate => {
            let store = Store::open(root.join(".linka"))?;
            let candidate = Candidates::new(&store).publish(target)?;
            Ok(format!(
                "Published {} at {}",
                candidate.id, candidate.head_commit
            ))
        }
        Action::StartReview => {
            let store = Store::open(root.join(".linka"))?;
            let assignee = parse_author(values.first().map(String::as_str).unwrap_or("human"))?;
            let started = Reviews::new(&store, root.join(".orka"))
                .start(&CandidateId(target.into()), assignee)?;
            Ok(format!(
                "Started verification {}\nbranch: {}\nsubject: {}",
                started.record.verification, started.review.branch, started.review.subject
            ))
        }
        Action::ResumeReview => {
            let store = Store::open(root.join(".linka"))?;
            let started = Reviews::new(&store, root.join(".orka")).resume(&parse_node(target)?)?;
            Ok(format!(
                "Resumed {}\nbranch: {}\nsubject: {}",
                started.record.verification, started.review.branch, started.review.subject
            ))
        }
        Action::PrepareWorktree => {
            let store = Store::open(root.join(".linka"))?;
            let verification = parse_node(target)?;
            let reviews = Reviews::new(&store, root.join(".orka"));
            let started = reviews.resume(&verification)?;
            let worktree = review_worktrees(root, &store).prepare(&started.record)?;
            Ok(format!(
                "Review worktree ready\nverification: {}\nbranch: {}\npath: {}",
                worktree.verification,
                worktree.branch,
                worktree.path.display()
            ))
        }
        Action::CleanupWorktree => {
            let store = Store::open(root.join(".linka"))?;
            let verification = parse_node(target)?;
            let record = Reviews::new(&store, root.join(".orka")).load(&verification)?;
            let outcome = review_worktrees(root, &store).cleanup(&record)?;
            Ok(match outcome {
                ReviewCleanupOutcome::Removed => format!("Removed worktree for {verification}"),
                ReviewCleanupOutcome::RetainedDirty => {
                    format!("Retained {verification}: worktree has uncommitted changes")
                }
                ReviewCleanupOutcome::AlreadyAbsent => {
                    format!("Worktree for {verification} is absent")
                }
            })
        }
        Action::FinishAccepted | Action::FinishRejected => {
            let store = Store::open(root.join(".linka"))?;
            let verification = parse_node(target)?;
            let summary = values
                .first()
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty());
            let author = parse_author(values.get(1).map(String::as_str).unwrap_or("human"))?;
            let outcome = if action == Action::FinishAccepted {
                VerificationOutcome::Accepted
            } else {
                VerificationOutcome::Rejected
            };
            let result = Reviews::new(&store, root.join(".orka")).finish(
                &verification,
                outcome,
                summary,
                author,
            )?;
            Ok(match result {
                FinishOutcome::Submitted => format!("Completed {verification}"),
                FinishOutcome::AlreadySubmitted => {
                    format!("Completed {verification} (already submitted)")
                }
                FinishOutcome::Conflict(conflicts) => {
                    format!("Stale {verification}: {conflicts:?}")
                }
            })
        }
        Action::AbandonReview => {
            let store = Store::open(root.join(".linka"))?;
            let verification = parse_node(target)?;
            let notes = values
                .first()
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty());
            let author = parse_author(values.get(1).map(String::as_str).unwrap_or("human"))?;
            let result =
                Reviews::new(&store, root.join(".orka")).abandon(&verification, notes, author)?;
            Ok(match result {
                AbandonOutcome::Abandoned => format!("Abandoned {verification}"),
                AbandonOutcome::AlreadyAbandoned => {
                    format!("Abandoned {verification} (already submitted)")
                }
                AbandonOutcome::Conflict(conflicts) => {
                    format!("Stale {verification}: {conflicts:?}")
                }
            })
        }
        Action::Audit => {
            let store = Store::open(root.join(".linka"))?;
            let problems = LinkaWork::new(&store).audit_output_evidence()?;
            if problems.is_empty() {
                Ok("All Orka-produced outputs retain complete evidence".into())
            } else {
                bail!(
                    "{} output evidence problem(s):\n{}",
                    problems.len(),
                    problems.join("\n")
                )
            }
        }
        Action::ViewAttempt
        | Action::ViewTranscript
        | Action::ViewDiagnostics
        | Action::ViewRawEvents
        | Action::ViewFileChanges
        | Action::ViewAccesses
        | Action::ViewPatch
        | Action::ShowReview => bail!("view action was sent to worker"),
    }
}

fn progress_text(progress: &RunProgress) -> String {
    match progress {
        RunProgress::Selected { node } => format!("Selected {node}"),
        RunProgress::AttemptCreated { attempt } => format!("Created {attempt}"),
        RunProgress::WorkspacePrepared { attempt } => format!("{attempt}: workspace prepared"),
        RunProgress::ExecutionStarted { attempt, .. } => format!("{attempt}: agent running"),
        RunProgress::ExecutionFinished { attempt, exit_code } => {
            format!("{attempt}: agent exited with {exit_code}")
        }
        RunProgress::Sealed { attempt, state } => format!("{attempt}: {}", seal_text(state)),
    }
}

fn seal_text(state: &SealedState) -> String {
    match state {
        SealedState::Submitted {
            output_commit: Some(commit),
        } => {
            format!("submitted ({})", short(commit))
        }
        SealedState::Submitted {
            output_commit: None,
        } => "submitted (no project output)".into(),
        SealedState::StaleAtSubmit { conflicts } => format!("stale: {conflicts:?}"),
        SealedState::FailureRecorded => "failure recorded".into(),
        SealedState::Interrupted { reason } => format!("interrupted: {reason}"),
        SealedState::ContractViolation { reason } => format!("contract violation: {reason}"),
    }
}

fn short(value: &str) -> &str {
    value.get(..value.len().min(10)).unwrap_or(value)
}

fn render_work_log(attempts: &FsAttemptStore, id: &AttemptId) -> Result<String> {
    let snapshot = attempts.load(id)?;
    let request = snapshot
        .request
        .context("attempt has no execution request")?;
    let output_path = if request.protocol.is_agent() {
        attempts.raw_events_path(id)
    } else {
        attempts.transcript_path(id)
    };
    let output = fs::read(&output_path)?;
    let changes = fs::read(attempts.file_changes_path(id)).ok();
    let blocks = work_log_from_raw(request.protocol, &output, changes.as_deref())?;
    Ok(blocks
        .iter()
        .map(render_block)
        .collect::<Vec<_>>()
        .join("\n\n"))
}

fn content(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::Code { language, text } => format!(
                "```{}\n{}\n```",
                language.as_deref().unwrap_or_default(),
                text
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_block(block: &WorkLogBlock) -> String {
    match block {
        WorkLogBlock::Session { id } => format!("SESSION {id}"),
        WorkLogBlock::TurnStarted => "TURN STARTED".into(),
        WorkLogBlock::CommandStarted { command } => format!("$ {command}"),
        WorkLogBlock::CommandCompleted {
            command,
            status,
            exit_code,
            output,
        } => format!(
            "$ {command}\n[{status}; exit {}]\n{}",
            exit_code
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into()),
            content(output)
        ),
        WorkLogBlock::FilesChanged {
            paths,
            checkpoint,
            checkpoint_error,
        } => format!(
            "FILES CHANGED\n{}\ncheckpoint: {}{}",
            paths.join("\n"),
            checkpoint.as_deref().unwrap_or("-"),
            checkpoint_error
                .as_ref()
                .map(|e| format!("\ncheckpoint error: {e}"))
                .unwrap_or_default()
        ),
        WorkLogBlock::ToolStarted { name, detail } => format!("TOOL {name}\n{detail}"),
        WorkLogBlock::ToolCompleted { name, status } => format!("TOOL {name}: {status}"),
        WorkLogBlock::Plan { content: blocks } => format!("PLAN\n{}", content(blocks)),
        WorkLogBlock::AgentMessage { content: blocks } => content(blocks),
        WorkLogBlock::Usage { usage } => format!("USAGE\n{usage:?}"),
        WorkLogBlock::Error { message } => format!("ERROR\n{message}"),
        WorkLogBlock::Transcript { content: blocks } => content(blocks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_labels_are_stable_and_complete() {
        assert_eq!(
            View::ALL.map(View::label),
            [
                "Ready",
                "Attempts",
                "Candidates",
                "Reviews",
                "Worktrees",
                "Audit",
                "Errors"
            ]
        );
    }

    #[test]
    fn author_input_is_explicit() {
        assert_eq!(parse_author(" HUMAN ").unwrap(), Author::Human);
        assert!(parse_author("robot").is_err());
    }
}
