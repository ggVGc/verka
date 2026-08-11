use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use linka::{
    ops::{self, NewNode},
    Author, CandidateId, CandidateRecord, CandidateState, CandidateStore, Currency, DepKind,
    GitVcs, IntegrationStatus, NewCandidate, NewNodeAttachment, NodeId, NodeMeta, NodeState,
    RecordedOutcome, Store, VerificationOutcome, VerificationSubmission,
};
use std::{fs, path::PathBuf, process::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Nodes,
    Candidates,
    Verifications,
    Ready,
    Stale,
    Blocked,
    Errors,
}

impl View {
    pub const ALL: [Self; 7] = [
        Self::Nodes,
        Self::Candidates,
        Self::Verifications,
        Self::Ready,
        Self::Stale,
        Self::Blocked,
        Self::Errors,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nodes => "Nodes",
            Self::Candidates => "Candidates",
            Self::Verifications => "Verifications",
            Self::Ready => "Ready",
            Self::Stale => "Stale",
            Self::Blocked => "Blocked",
            Self::Errors => "Errors",
        }
    }
}

#[derive(Clone)]
pub struct NodeRow {
    pub id: String,
    pub title: String,
    pub meta: NodeMeta,
    pub state: NodeState,
    pub notes: String,
    pub output: Option<String>,
    pub attachments: Vec<linka::NodeAttachment>,
    pub candidates: Vec<String>,
    pub dependents: Vec<String>,
}

impl NodeRow {
    pub fn kind(&self) -> NodeKind {
        if self.meta.verifies.is_some() {
            NodeKind::Verification
        } else {
            NodeKind::Work
        }
    }
}

/// What a node is for: work nodes produce their own output, verification nodes
/// review an exact candidate of another node's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Work,
    Verification,
}

impl NodeKind {
    /// A single-width sigil, so the list stays aligned.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Work => "■",
            Self::Verification => "◆",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Verification => "verification",
        }
    }
}

#[derive(Clone)]
pub struct CandidateRow {
    pub record: CandidateRecord,
    pub integration: IntegrationStatus,
    pub verifications: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Node(String),
    Candidate(String),
}

#[derive(Clone)]
pub struct Association {
    pub label: String,
    pub target: Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Items,
    Associations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    AddNode,
    AddVerification,
    EditNode,
    LinkNodes,
    Complete,
    Respond,
    Fail,
    Verify,
    RegisterCandidate,
    AcceptCandidate,
    RejectCandidate,
    PublishCandidate,
    Attach,
    ReadAttachment,
    ObserveContext,
    Origin,
    History,
    Settled,
    Check,
    CheckArtifacts,
    VerifyPairing,
    Pair,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::AddNode => "Add node",
            Self::AddVerification => "Add verification",
            Self::EditNode => "Edit node description",
            Self::LinkNodes => "Link nodes",
            Self::Complete => "Complete node",
            Self::Respond => "Respond to node",
            Self::Fail => "Fail node",
            Self::Verify => "Submit verification",
            Self::RegisterCandidate => "Register candidate",
            Self::AcceptCandidate => "Accept candidate",
            Self::RejectCandidate => "Reject candidate",
            Self::PublishCandidate => "Publish candidate",
            Self::Attach => "Attach file",
            Self::ReadAttachment => "View attachment",
            Self::ObserveContext => "Record context observation",
            Self::Origin => "Find output origin",
            Self::History => "Show node history",
            Self::Settled => "Check node settlement",
            Self::Check => "Check store",
            Self::CheckArtifacts => "Check store + artifacts",
            Self::VerifyPairing => "Verify project pairing",
            Self::Pair => "Record project pairing",
        }
    }
}

pub const ACTIONS: [Action; 22] = [
    Action::AddNode,
    Action::AddVerification,
    Action::EditNode,
    Action::LinkNodes,
    Action::Complete,
    Action::Respond,
    Action::Fail,
    Action::Verify,
    Action::RegisterCandidate,
    Action::AcceptCandidate,
    Action::RejectCandidate,
    Action::PublishCandidate,
    Action::Attach,
    Action::ReadAttachment,
    Action::ObserveContext,
    Action::Origin,
    Action::History,
    Action::Settled,
    Action::Check,
    Action::CheckArtifacts,
    Action::VerifyPairing,
    Action::Pair,
];

#[derive(Clone)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
}

pub struct Form {
    pub action: Action,
    pub fields: Vec<Field>,
    pub selected: usize,
    pub error: Option<String>,
}

/// The attachments of one node, with the payload of the selected one rendered
/// for reading. Payloads are read on selection so the browser always shows the
/// bytes Linka currently stores.
pub struct AttachmentBrowser {
    pub node: String,
    pub items: Vec<linka::NodeAttachment>,
    pub selected: usize,
    pub body: String,
    pub scroll: u16,
}

pub enum Overlay {
    Actions {
        selected: usize,
    },
    Form(Form),
    Text {
        title: String,
        body: String,
        scroll: u16,
    },
    Attachments(AttachmentBrowser),
    Help,
}

pub struct App {
    pub store: Store,
    pub vcs: GitVcs,
    pub nodes: Vec<NodeRow>,
    pub candidates: Vec<CandidateRow>,
    pub errors: Vec<String>,
    pub view: View,
    pub selected: usize,
    pub association_selected: usize,
    pub focus: Focus,
    pub overlay: Option<Overlay>,
    pub status: String,
    pub should_quit: bool,
    history: Vec<(View, String)>,
}

impl App {
    pub fn open(path: PathBuf) -> Result<Self> {
        let store = Store::open(path)?;
        let vcs = GitVcs::for_store(&store);
        let mut app = Self {
            store,
            vcs,
            nodes: Vec::new(),
            candidates: Vec::new(),
            errors: Vec::new(),
            view: View::Nodes,
            selected: 0,
            association_selected: 0,
            focus: Focus::Items,
            overlay: None,
            status: String::new(),
            should_quit: false,
            history: Vec::new(),
        };
        app.refresh();
        Ok(app)
    }

    pub fn refresh(&mut self) {
        let previous = self.selected_identity();
        self.nodes.clear();
        self.candidates.clear();
        self.errors.clear();

        match self.store.list_ids() {
            Ok(ids) => {
                for id in ids {
                    match self.load_node(&id) {
                        Ok(row) => self.nodes.push(row),
                        Err(error) => self.errors.push(format!("{id}: {error:#}")),
                    }
                }
            }
            Err(error) => self.errors.push(format!("listing nodes: {error:#}")),
        }

        let candidates = CandidateStore::new(&self.store);
        match candidates.list() {
            Ok(ids) => {
                for id in ids {
                    let loaded = (|| -> Result<CandidateRow> {
                        let record = candidates.load(&id)?;
                        let integration = record.integration(&self.vcs)?;
                        let verifications = ops::verifications_for(&self.store, &record.id)?;
                        Ok(CandidateRow {
                            record,
                            integration,
                            verifications,
                        })
                    })();
                    match loaded {
                        Ok(row) => self.candidates.push(row),
                        Err(error) => self.errors.push(format!("{id}: {error:#}")),
                    }
                }
            }
            Err(error) => self.errors.push(format!("listing candidates: {error:#}")),
        }
        self.restore_selection(previous.as_deref());
        self.status = format!(
            "Loaded {} nodes and {} candidates{}",
            self.nodes.len(),
            self.candidates.len(),
            if self.errors.is_empty() {
                String::new()
            } else {
                format!("; {} error(s)", self.errors.len())
            }
        );
    }

    fn load_node(&self, id: &str) -> Result<NodeRow> {
        let (meta, description) = self.store.read_node(id)?;
        let state = ops::node_state(&self.store, &self.vcs, id)?;
        let result = self.store.read_result(id)?;
        let (notes, output) = result
            .map(|(result, notes)| (notes, result.output.map(|output| output.id)))
            .unwrap_or_default();
        let attachments = self.store.list_node_attachments(id)?;
        let node_id: NodeId = id.parse().map_err(anyhow::Error::msg)?;
        let candidates = CandidateStore::new(&self.store)
            .for_node(&node_id)?
            .into_iter()
            .map(|candidate| candidate.id.into())
            .collect();
        Ok(NodeRow {
            id: id.into(),
            title: linka::title_of(&description).into(),
            meta,
            state,
            notes,
            output,
            attachments,
            candidates,
            dependents: ops::dependents(&self.store, id)?,
        })
    }

    pub fn visible_node_indices(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| match self.view {
                View::Nodes => true,
                View::Verifications => node.meta.verifies.is_some(),
                View::Ready => node.state.is_ready(),
                View::Stale => node.state.currency == Currency::Stale,
                View::Blocked => !node.state.blockers.is_empty(),
                _ => false,
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub fn selected_node(&self) -> Option<&NodeRow> {
        let indices = self.visible_node_indices();
        indices
            .get(self.selected)
            .and_then(|index| self.nodes.get(*index))
    }

    pub fn selected_candidate(&self) -> Option<&CandidateRow> {
        (self.view == View::Candidates)
            .then(|| self.candidates.get(self.selected))
            .flatten()
    }

    pub fn associations(&self) -> Vec<Association> {
        if let Some(node) = self.selected_node() {
            let mut links = Vec::new();
            for id in &node.meta.depends_on {
                links.push(Association {
                    label: format!("depends on  {id}"),
                    target: Target::Node(id.to_string()),
                });
            }
            for id in &node.meta.derived_from {
                links.push(Association {
                    label: format!("derived from {id}"),
                    target: Target::Node(id.to_string()),
                });
            }
            for id in &node.dependents {
                links.push(Association {
                    label: format!("dependent   {id}"),
                    target: Target::Node(id.clone()),
                });
            }
            if let Some(candidate) = &node.meta.verifies {
                links.push(Association {
                    label: format!("verifies    {candidate}"),
                    target: Target::Candidate(candidate.to_string()),
                });
            }
            for id in &node.candidates {
                links.push(Association {
                    label: format!("candidate   {id}"),
                    target: Target::Candidate(id.clone()),
                });
            }
            return links;
        }
        if let Some(candidate) = self.selected_candidate() {
            let mut links = vec![Association {
                label: format!("source node  {}", candidate.record.node),
                target: Target::Node(candidate.record.node.to_string()),
            }];
            for id in &candidate.verifications {
                links.push(Association {
                    label: format!("verification {id}"),
                    target: Target::Node(id.clone()),
                });
            }
            match &candidate.record.state {
                CandidateState::Accepted { verification, .. }
                | CandidateState::Rejected { verification, .. } => links.push(Association {
                    label: format!("decided by  {verification}"),
                    target: Target::Node(verification.to_string()),
                }),
                CandidateState::Pending => {}
            }
            return links;
        }
        Vec::new()
    }

    pub fn item_count(&self) -> usize {
        match self.view {
            View::Candidates => self.candidates.len(),
            View::Errors => self.errors.len(),
            _ => self.visible_node_indices().len(),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.overlay.is_some() {
            self.overlay_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('a') | KeyCode::Char(':') => {
                self.overlay = Some(Overlay::Actions { selected: 0 })
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('A') => self.open_attachments(),
            KeyCode::Char('b') | KeyCode::Backspace => self.go_back(),
            KeyCode::Tab => {
                if !self.associations().is_empty() {
                    self.focus = match self.focus {
                        Focus::Items => Focus::Associations,
                        Focus::Associations => Focus::Items,
                    };
                }
            }
            KeyCode::Enter => {
                if self.focus == Focus::Associations {
                    self.follow_selected_association();
                } else if !self.associations().is_empty() {
                    self.focus = Focus::Associations;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.change_view(-1),
            KeyCode::Right | KeyCode::Char('l') => self.change_view(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Home | KeyCode::Char('g') => self.set_active_selection(0),
            KeyCode::End | KeyCode::Char('G') => {
                let count = if self.focus == Focus::Associations {
                    self.associations().len()
                } else {
                    self.item_count()
                };
                self.set_active_selection(count.saturating_sub(1));
            }
            _ => {}
        }
    }

    fn overlay_key(&mut self, key: KeyEvent) {
        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        match &mut overlay {
            Overlay::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter) {
                    return;
                }
            }
            Overlay::Text { scroll, .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => return,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll = scroll.saturating_add(10),
                _ => {}
            },
            Overlay::Attachments(browser) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('A') => return,
                KeyCode::Up | KeyCode::Char('k') => {
                    let next = browser.selected.saturating_sub(1);
                    if next != browser.selected {
                        browser.selected = next;
                        browser.scroll = 0;
                        browser.body = self.attachment_body(&browser.node, &browser.items[next]);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let next = (browser.selected + 1).min(browser.items.len().saturating_sub(1));
                    if next != browser.selected {
                        browser.selected = next;
                        browser.scroll = 0;
                        browser.body = self.attachment_body(&browser.node, &browser.items[next]);
                    }
                }
                KeyCode::Char('K') | KeyCode::PageUp => {
                    browser.scroll = browser.scroll.saturating_sub(10)
                }
                KeyCode::Char('J') | KeyCode::PageDown => {
                    browser.scroll = browser.scroll.saturating_add(10)
                }
                KeyCode::Home | KeyCode::Char('g') => browser.scroll = 0,
                _ => {}
            },
            Overlay::Actions { selected } => match key.code {
                KeyCode::Esc => return,
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1).min(ACTIONS.len() - 1)
                }
                KeyCode::Enter => {
                    let action = ACTIONS[*selected];
                    self.overlay = Some(Overlay::Form(self.form_for(action)));
                    return;
                }
                _ => {}
            },
            Overlay::Form(form) => match key.code {
                KeyCode::Esc => return,
                KeyCode::Tab | KeyCode::Down => {
                    form.selected = (form.selected + 1) % form.fields.len().max(1)
                }
                KeyCode::BackTab | KeyCode::Up => {
                    form.selected = form
                        .selected
                        .checked_sub(1)
                        .unwrap_or_else(|| form.fields.len().saturating_sub(1))
                }
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let action = form.action;
                    let values = form
                        .fields
                        .iter()
                        .map(|field| field.value.clone())
                        .collect();
                    match self.execute(action, values) {
                        Ok(message) => {
                            self.refresh();
                            self.status = message;
                            return;
                        }
                        Err(error) => form.error = Some(format!("{error:#}")),
                    }
                }
                KeyCode::Enter => {
                    if form.selected + 1 < form.fields.len() {
                        form.selected += 1;
                    } else {
                        let action = form.action;
                        let values = form
                            .fields
                            .iter()
                            .map(|field| field.value.clone())
                            .collect();
                        match self.execute(action, values) {
                            Ok(message) => {
                                self.refresh();
                                self.status = message;
                                return;
                            }
                            Err(error) => form.error = Some(format!("{error:#}")),
                        }
                    }
                }
                KeyCode::Backspace if !form.fields.is_empty() => {
                    form.fields[form.selected].value.pop();
                    form.error = None;
                }
                KeyCode::Char(character)
                    if !form.fields.is_empty()
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    form.fields[form.selected].value.push(character);
                    form.error = None;
                }
                _ => {}
            },
        }
        self.overlay = Some(overlay);
    }

    fn form_for(&self, action: Action) -> Form {
        let node = self
            .selected_node()
            .map(|node| node.id.clone())
            .unwrap_or_default();
        let candidate = self
            .selected_candidate()
            .map(|candidate| candidate.record.id.to_string())
            .or_else(|| {
                self.selected_node()
                    .and_then(|node| node.meta.verifies.as_ref())
                    .map(|id| id.to_string())
            })
            .unwrap_or_default();
        let first_attachment = self
            .selected_node()
            .and_then(|node| node.attachments.first())
            .map(|item| (item.namespace.clone(), item.key.clone()))
            .unwrap_or_default();
        let fields = match action {
            Action::AddNode => vec![
                field("Description", "", "required"),
                field("Author", "human", "human | machine"),
                field("Assignee", "", "blank | human | machine"),
                field("Depends on", "", "comma-separated node ids"),
                field("Derived from", "", "comma-separated node ids"),
            ],
            Action::AddVerification => vec![
                field("Candidate", &candidate, "candidate id"),
                field("Description", "", "blank generates a title"),
                field("Author", "human", "human | machine"),
                field("Assignee", "", "blank | human | machine"),
            ],
            Action::EditNode => vec![
                field("Node", &node, "node id"),
                field("Description", "", "replaces the entire description"),
            ],
            Action::LinkNodes => vec![
                field("From", &node, "node gaining the relation"),
                field("To", "", "related node id"),
                field("Relation", "depends_on", "depends_on | derived_from"),
            ],
            Action::Complete => vec![
                field("Node", &node, "node id"),
                field("Outputs", "", "comma-separated project paths"),
                field("Context", "", "comma-separated project paths"),
                field("Message", "", "optional commit message"),
                field("Notes", "", "optional notes"),
                field("Author", "human", "human | machine"),
            ],
            Action::Respond => vec![
                field("Node", &node, "node id"),
                field("Response", "", "required"),
                field("Author", "human", "human | machine"),
            ],
            Action::Fail => vec![
                field("Node", &node, "node id"),
                field("Notes", "", "what went wrong"),
                field("Author", "human", "human | machine"),
            ],
            Action::Verify => vec![
                field("Node", &node, "verification node id"),
                field("Outcome", "accepted", "accepted | rejected | abandoned"),
                field("Notes", "", "required for rejection"),
                field("Author", "human", "human | machine"),
            ],
            Action::RegisterCandidate => vec![
                field("Node", &node, "source node id"),
                field("Branch", "", "candidate branch"),
                field("Target", "main", "target branch"),
                field(
                    "External namespace",
                    "",
                    "optional; use both external fields",
                ),
                field("External id", "", "optional; use both external fields"),
            ],
            Action::AcceptCandidate | Action::RejectCandidate => vec![
                field("Candidate", &candidate, "candidate id"),
                field("Verification", "", "deciding verification node"),
                field("Notes", "", "required for rejection"),
                field("Author", "human", "human | machine"),
            ],
            Action::PublishCandidate => {
                vec![field("Candidate", &candidate, "accepted candidate id")]
            }
            Action::Attach => vec![
                field("Node", &node, "node id"),
                field("Namespace", "", "attachment namespace"),
                field("Key", "", "attachment key"),
                field("File", "", "path to payload"),
                field("Media type", "", "optional, e.g. text/plain"),
            ],
            Action::ReadAttachment => vec![
                field("Node", &node, "node id"),
                field("Namespace", &first_attachment.0, "attachment namespace"),
                field("Key", &first_attachment.1, "attachment key"),
            ],
            Action::ObserveContext => vec![
                field("Node", &node, "node id with a result"),
                field("Paths", "", "comma-separated project paths"),
            ],
            Action::Origin => vec![field("Commit", "", "output commit")],
            Action::History | Action::Settled => vec![field("Node", &node, "node id")],
            Action::Check | Action::CheckArtifacts | Action::VerifyPairing => Vec::new(),
            Action::Pair => vec![
                field("Name", "", "optional project name"),
                field("Force", "false", "true | false"),
            ],
        };
        Form {
            action,
            fields,
            selected: 0,
            error: None,
        }
    }

    fn execute(&mut self, action: Action, values: Vec<String>) -> Result<String> {
        let value = |index: usize| values.get(index).map(String::as_str).unwrap_or("");
        let message = match action {
            Action::AddNode => {
                let id = ops::add(
                    &self.store,
                    &self.vcs,
                    NewNode {
                        description: value(0).into(),
                        author: author(value(1))?,
                        assignee: optional_author(value(2))?,
                        depends_on: csv(value(3)),
                        derived_from: csv(value(4)),
                    },
                )?;
                format!("Added {id}")
            }
            Action::AddVerification => {
                let candidate: CandidateId = value(0).parse().map_err(anyhow::Error::msg)?;
                let description = if value(1).trim().is_empty() {
                    format!("Verify candidate {candidate}")
                } else {
                    value(1).into()
                };
                let id = ops::add_verification(
                    &self.store,
                    &self.vcs,
                    &candidate,
                    NewNode {
                        description,
                        author: author(value(2))?,
                        assignee: optional_author(value(3))?,
                        depends_on: vec![],
                        derived_from: vec![],
                    },
                )?;
                format!("Added verification {id}")
            }
            Action::EditNode => {
                ops::edit(&self.store, &self.vcs, value(0), value(1).into())?;
                format!("Updated {}", value(0))
            }
            Action::LinkNodes => {
                ops::link(
                    &self.store,
                    &self.vcs,
                    value(0),
                    value(1),
                    dep_kind(value(2))?,
                )?;
                format!("Linked {} to {}", value(0), value(1))
            }
            Action::Complete => {
                let output = ops::complete(
                    &self.store,
                    &self.vcs,
                    value(0),
                    &csv(value(1)),
                    &csv(value(2)),
                    optional(value(3)),
                    value(4),
                    author(value(5))?,
                )?;
                match output {
                    Some(commit) => format!("Completed {} at {}", value(0), ops::short(&commit)),
                    None => format!("Completed {} without output", value(0)),
                }
            }
            Action::Respond => {
                ops::respond(
                    &self.store,
                    &self.vcs,
                    value(0),
                    value(1),
                    author(value(2))?,
                )?;
                format!("Responded to {}", value(0))
            }
            Action::Fail => {
                ops::fail(
                    &self.store,
                    &self.vcs,
                    value(0),
                    value(1),
                    author(value(2))?,
                )?;
                format!("Failed {}", value(0))
            }
            Action::Verify => {
                let snapshot = ops::snapshot_work(&self.store, &self.vcs, value(0), &[])?;
                let outcome = verification_outcome(value(1))?;
                ops::submit_verification(
                    &self.store,
                    &self.vcs,
                    VerificationSubmission {
                        snapshot,
                        outcome,
                        notes: value(2).into(),
                        author: author(value(3))?,
                        producer: None,
                    },
                )
                .map_err(anyhow::Error::msg)?;
                format!("Recorded {} for {}", outcome.as_str(), value(0))
            }
            Action::RegisterCandidate => {
                let external = match (value(3).trim(), value(4).trim()) {
                    ("", "") => None,
                    (namespace, id) if !namespace.is_empty() && !id.is_empty() => {
                        Some(linka::ExternalIdentity {
                            namespace: namespace.into(),
                            id: id.into(),
                        })
                    }
                    _ => bail!("external namespace and id must be supplied together"),
                };
                let record = CandidateStore::new(&self.store).register(
                    &self.vcs,
                    NewCandidate {
                        node: value(0).parse().map_err(anyhow::Error::msg)?,
                        branch: value(1).into(),
                        target: value(2).into(),
                        external,
                    },
                )?;
                format!("Registered {}", record.id)
            }
            Action::AcceptCandidate => {
                CandidateStore::new(&self.store).accept(
                    &self.vcs,
                    &value(0).parse().map_err(anyhow::Error::msg)?,
                    &value(1).parse().map_err(anyhow::Error::msg)?,
                    author(value(3))?,
                    value(2).into(),
                )?;
                format!("Accepted {}", value(0))
            }
            Action::RejectCandidate => {
                CandidateStore::new(&self.store).reject(
                    &self.vcs,
                    &value(0).parse().map_err(anyhow::Error::msg)?,
                    &value(1).parse().map_err(anyhow::Error::msg)?,
                    author(value(3))?,
                    value(2).into(),
                )?;
                format!("Rejected {}", value(0))
            }
            Action::PublishCandidate => {
                CandidateStore::new(&self.store)
                    .publish(&self.vcs, &value(0).parse().map_err(anyhow::Error::msg)?)?;
                format!("Published {}", value(0))
            }
            Action::Attach => {
                let path = PathBuf::from(value(3));
                let attachment = ops::record_node_attachment(
                    &self.store,
                    &self.vcs,
                    value(0),
                    NewNodeAttachment {
                        namespace: value(1).into(),
                        key: value(2).into(),
                        media_type: optional(value(4)),
                        data: fs::read(&path)
                            .with_context(|| format!("reading {}", path.display()))?,
                    },
                )?;
                format!(
                    "Attached {}/{} ({} bytes)",
                    attachment.namespace, attachment.key, attachment.size
                )
            }
            Action::ReadAttachment => {
                let (meta, data) = self
                    .store
                    .read_node_attachment(value(0), value(1), value(2))?
                    .with_context(|| {
                        format!("no attachment `{}/{}` on {}", value(1), value(2), value(0))
                    })?;
                let body = match String::from_utf8(data) {
                    Ok(text) => text,
                    Err(_error) => format!(
                        "Binary attachment: {} bytes\ncontent: {}\nmedia type: {}",
                        meta.size,
                        meta.content,
                        meta.media_type.as_deref().unwrap_or("unknown")
                    ),
                };
                self.overlay = Some(Overlay::Text {
                    title: format!("{}/{}", meta.namespace, meta.key),
                    body,
                    scroll: 0,
                });
                return Ok(String::new());
            }
            Action::ObserveContext => {
                let version = self.store.result_version(value(0))?;
                let count = ops::record_context_observation(
                    &self.store,
                    &self.vcs,
                    value(0),
                    &version,
                    &csv(value(1)),
                )?;
                format!("Recorded {count} context observation(s)")
            }
            Action::Origin => {
                let body = ops::origin(&self.store, value(0))?
                    .unwrap_or_else(|| "No node produced this commit".into());
                self.overlay = Some(Overlay::Text {
                    title: "Output origin".into(),
                    body,
                    scroll: 0,
                });
                return Ok(String::new());
            }
            Action::History => {
                if !self.store.exists(value(0)) {
                    bail!("unknown node `{}`", value(0));
                }
                let pathspec = format!("{}/nodes/{}", self.store.store_name(), value(0));
                let output = Command::new("git")
                    .arg("-C")
                    .arg(self.store.workbench_root())
                    .args(["log", "--oneline", "--stat", "--", &pathspec])
                    .output()
                    .context("running git log")?;
                if !output.status.success() {
                    bail!("{}", String::from_utf8_lossy(&output.stderr));
                }
                self.overlay = Some(Overlay::Text {
                    title: format!("History · {}", value(0)),
                    body: String::from_utf8_lossy(&output.stdout).into_owned(),
                    scroll: 0,
                });
                return Ok(String::new());
            }
            Action::Settled => {
                let reasons = ops::unsettled(&self.store, &self.vcs, value(0))?;
                let body = if reasons.is_empty() {
                    format!("{} is settled.", value(0))
                } else {
                    format!("{} is not settled:\n\n{}", value(0), reasons.join("\n"))
                };
                self.overlay = Some(Overlay::Text {
                    title: "Settlement".into(),
                    body,
                    scroll: 0,
                });
                return Ok(String::new());
            }
            Action::Check | Action::CheckArtifacts => {
                let problems = if action == Action::CheckArtifacts {
                    ops::check_artifacts(&self.store, &self.vcs)?
                } else {
                    ops::check_workbench(&self.store, &self.vcs)?
                };
                let body = if problems.is_empty() {
                    "Store is consistent.".into()
                } else {
                    format!("{} problem(s):\n\n{}", problems.len(), problems.join("\n"))
                };
                self.overlay = Some(Overlay::Text {
                    title: action.label().into(),
                    body,
                    scroll: 0,
                });
                return Ok(String::new());
            }
            Action::VerifyPairing => {
                let (pairing, problems) = ops::verify_pairing(&self.store, &self.vcs, true)?;
                let mut body = match pairing {
                    Some(pairing) => format!(
                        "Recorded root: {}\nName: {}\nRemote: {}",
                        pairing.root_commit,
                        pairing.name.as_deref().unwrap_or("—"),
                        pairing.remote.as_deref().unwrap_or("—")
                    ),
                    None => "Store is not paired.".into(),
                };
                if problems.is_empty() {
                    body.push_str("\n\nPairing is valid.");
                } else {
                    body.push_str(&format!("\n\nProblems:\n{}", problems.join("\n")));
                }
                self.overlay = Some(Overlay::Text {
                    title: "Project pairing".into(),
                    body,
                    scroll: 0,
                });
                return Ok(String::new());
            }
            Action::Pair => {
                let pairing = ops::pair(
                    &self.store,
                    &self.vcs,
                    optional(value(0)),
                    boolean(value(1))?,
                )?;
                format!(
                    "Paired to project root {}",
                    ops::short(&pairing.root_commit)
                )
            }
        };
        Ok(message)
    }

    /// Open the attachment browser for the selected node, or for the source
    /// node of the selected candidate.
    fn open_attachments(&mut self) {
        let node_id = self
            .selected_node()
            .map(|node| node.id.clone())
            .or_else(|| {
                self.selected_candidate()
                    .map(|candidate| candidate.record.node.to_string())
            });
        let Some(node_id) = node_id else {
            self.status = "No node selected".into();
            return;
        };
        let items = self
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .map(|node| node.attachments.clone())
            .unwrap_or_default();
        if items.is_empty() {
            self.status = format!("{node_id} has no attachments");
            return;
        }
        let body = self.attachment_body(&node_id, &items[0]);
        self.overlay = Some(Overlay::Attachments(AttachmentBrowser {
            node: node_id,
            items,
            selected: 0,
            body,
            scroll: 0,
        }));
    }

    /// Render one attachment for reading: its metadata, then the payload as
    /// text when it is valid UTF-8 and as a hex dump when it is not.
    fn attachment_body(&self, node: &str, item: &linka::NodeAttachment) -> String {
        let mut body = format!(
            "namespace   {}\nkey         {}\nsize        {} bytes\nmedia type  {}\ncontent     {}\n\n",
            item.namespace,
            item.key,
            item.size,
            item.media_type.as_deref().unwrap_or("—"),
            item.content
        );
        match self
            .store
            .read_node_attachment(node, &item.namespace, &item.key)
        {
            Ok(Some((_, data))) => match String::from_utf8(data) {
                Ok(text) => body.push_str(&text),
                Err(error) => body.push_str(&hex_dump(error.as_bytes())),
            },
            Ok(None) => body.push_str("Attachment is no longer present in the store."),
            Err(error) => body.push_str(&format!("Cannot read payload: {error:#}")),
        }
        body
    }

    fn change_view(&mut self, delta: isize) {
        let index = View::ALL
            .iter()
            .position(|view| *view == self.view)
            .unwrap_or_default();
        let next = (index as isize + delta).rem_euclid(View::ALL.len() as isize) as usize;
        self.view = View::ALL[next];
        self.selected = 0;
        self.association_selected = 0;
        self.focus = Focus::Items;
    }

    fn move_selection(&mut self, delta: isize) {
        let (current, count) = if self.focus == Focus::Associations {
            (self.association_selected, self.associations().len())
        } else {
            (self.selected, self.item_count())
        };
        if count == 0 {
            return;
        }
        let next = (current as isize + delta).clamp(0, count.saturating_sub(1) as isize) as usize;
        self.set_active_selection(next);
    }

    fn set_active_selection(&mut self, selection: usize) {
        if self.focus == Focus::Associations {
            self.association_selected = selection;
        } else {
            self.selected = selection;
            self.association_selected = 0;
        }
    }

    fn follow_selected_association(&mut self) {
        let Some(link) = self.associations().get(self.association_selected).cloned() else {
            return;
        };
        if let Some(id) = self.selected_identity() {
            self.history.push((self.view, id));
        }
        self.navigate(link.target);
    }

    fn navigate(&mut self, target: Target) {
        match target {
            Target::Node(id) => {
                self.view = View::Nodes;
                self.selected = self
                    .nodes
                    .iter()
                    .position(|node| node.id == id)
                    .unwrap_or(0);
            }
            Target::Candidate(id) => {
                self.view = View::Candidates;
                self.selected = self
                    .candidates
                    .iter()
                    .position(|candidate| candidate.record.id.as_str() == id)
                    .unwrap_or(0);
            }
        }
        self.association_selected = 0;
        self.focus = Focus::Items;
    }

    fn go_back(&mut self) {
        let Some((view, id)) = self.history.pop() else {
            return;
        };
        self.view = view;
        self.restore_selection(Some(&id));
        self.focus = Focus::Items;
    }

    fn selected_identity(&self) -> Option<String> {
        if self.view == View::Candidates {
            self.selected_candidate()
                .map(|candidate| candidate.record.id.to_string())
        } else {
            self.selected_node().map(|node| node.id.clone())
        }
    }

    fn restore_selection(&mut self, id: Option<&str>) {
        self.selected = match self.view {
            View::Candidates => id
                .and_then(|id| {
                    self.candidates
                        .iter()
                        .position(|candidate| candidate.record.id.as_str() == id)
                })
                .unwrap_or(0),
            View::Errors => self.selected.min(self.errors.len().saturating_sub(1)),
            _ => {
                let visible = self.visible_node_indices();
                id.and_then(|id| visible.iter().position(|index| self.nodes[*index].id == id))
                    .unwrap_or(0)
            }
        };
        self.association_selected = 0;
    }
}

pub fn state_label(state: &NodeState) -> String {
    if state.currency == Currency::Current {
        match state.outcome {
            RecordedOutcome::Accepted => return "accepted".into(),
            RecordedOutcome::Rejected => return "rejected".into(),
            RecordedOutcome::Abandoned => return "abandoned".into(),
            _ => {}
        }
    }
    if state.is_complete() {
        "complete".into()
    } else if state.is_awaiting_integration() {
        match state.integration {
            IntegrationStatus::Pending => "awaiting decision".into(),
            IntegrationStatus::Accepted => "awaiting publish".into(),
            _ => "awaiting integration".into(),
        }
    } else if state.is_ready() {
        match (state.currency, state.outcome) {
            (Currency::Stale, _) => "ready · stale".into(),
            (_, RecordedOutcome::Failed) => "ready · retry".into(),
            _ => "ready".into(),
        }
    } else {
        "blocked".into()
    }
}

pub fn candidate_state_label(candidate: &CandidateRow) -> &'static str {
    match candidate.integration {
        IntegrationStatus::Pending => "pending",
        IntegrationStatus::Accepted => "accepted",
        IntegrationStatus::Published => "published",
        IntegrationStatus::Rejected => "rejected",
        IntegrationStatus::NotRequired => "direct",
    }
}

fn field(label: &'static str, value: &str, hint: &'static str) -> Field {
    Field {
        label,
        value: value.into(),
        hint,
    }
}

/// Sixteen bytes per line: offset, hex, then printable ASCII. Long payloads are
/// truncated so the browser stays responsive on large binary attachments.
fn hex_dump(data: &[u8]) -> String {
    const LIMIT: usize = 8 * 1024;
    let shown = data.len().min(LIMIT);
    let mut text = format!("Binary payload · {} bytes\n\n", data.len());
    for (index, chunk) in data[..shown].chunks(16).enumerate() {
        let hex = chunk
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii = chunk
            .iter()
            .map(|byte| match byte {
                0x20..=0x7e => *byte as char,
                _ => '.',
            })
            .collect::<String>();
        text.push_str(&format!("{:08x}  {hex:<47}  {ascii}\n", index * 16));
    }
    if shown < data.len() {
        text.push_str(&format!("\n… {} more byte(s)\n", data.len() - shown));
    }
    text
}

fn csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(Into::into)
        .collect()
}

fn optional(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.into())
}

fn author(value: &str) -> Result<Author> {
    match value.trim().to_ascii_lowercase().as_str() {
        "human" => Ok(Author::Human),
        "machine" => Ok(Author::Machine),
        _ => bail!("author must be `human` or `machine`"),
    }
}

fn optional_author(value: &str) -> Result<Option<Author>> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        author(value).map(Some)
    }
}

fn dep_kind(value: &str) -> Result<DepKind> {
    match value.trim().replace('-', "_").as_str() {
        "depends_on" => Ok(DepKind::DependsOn),
        "derived_from" => Ok(DepKind::DerivedFrom),
        _ => bail!("relation must be `depends_on` or `derived_from`"),
    }
}

fn verification_outcome(value: &str) -> Result<VerificationOutcome> {
    match value.trim().to_ascii_lowercase().as_str() {
        "accepted" => Ok(VerificationOutcome::Accepted),
        "rejected" => Ok(VerificationOutcome::Rejected),
        "abandoned" => Ok(VerificationOutcome::Abandoned),
        _ => bail!("outcome must be `accepted`, `rejected`, or `abandoned`"),
    }
}

fn boolean(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        _ => bail!("expected true or false"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_ignores_empty_values_and_whitespace() {
        assert_eq!(csv(" a, ,b "), vec!["a", "b"]);
    }

    #[test]
    fn hex_dump_lays_out_sixteen_bytes_per_line() {
        let dump = hex_dump(b"hi\0there, attachment!");
        let mut lines = dump.lines().skip(2);
        assert_eq!(
            lines.next().unwrap(),
            "00000000  68 69 00 74 68 65 72 65 2c 20 61 74 74 61 63 68  hi.there, attach"
        );
        assert_eq!(
            lines.next().unwrap(),
            "00000010  6d 65 6e 74 21                                   ment!"
        );
    }

    #[test]
    fn hex_dump_reports_truncated_bytes() {
        let dump = hex_dump(&vec![0u8; 8 * 1024 + 3]);
        assert!(dump.contains("Binary payload · 8195 bytes"));
        assert!(dump.contains("… 3 more byte(s)"));
    }

    #[test]
    fn parsers_accept_documented_values() {
        assert_eq!(author("machine").unwrap(), Author::Machine);
        assert_eq!(dep_kind("depends-on").unwrap(), DepKind::DependsOn);
        assert_eq!(
            verification_outcome("abandoned").unwrap(),
            VerificationOutcome::Abandoned
        );
    }
}
