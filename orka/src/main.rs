//! The `orka` CLI: orchestrate isolated agent attempts for work in a Linka
//! store.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use linka::{Author, CandidateId, NodeId, Store};
use orka::attempt::{AttemptId, FsAttemptStore, SealedState};
use orka::candidate::Candidates;
use orka::config::{Config, CONFIG_FILE};
use orka::engine::{Engine, RunProgress, RunReport};
use orka::events::follow_codex_events;
use orka::linka_work::LinkaWork;
use orka::review::{AbandonOutcome, FinishOutcome, ReviewVerdict, Reviews};
use orka::review_worktree::{GitReviewWorktrees, ReviewCleanupOutcome};
use orka::workspace::GitWorkspaces;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Parser)]
#[command(
    name = "orka",
    about = "Orchestrate isolated agent attempts for work in a Linka store: snapshot a ready node, run an agent, submit a version-checked result"
)]
struct Cli {
    /// Workbench root (holds .linka/, project/, orka.toml). Defaults to the
    /// nearest ancestor of the current directory containing .linka/.
    #[arg(long, global = true)]
    workbench: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a default orka.toml using Orka's Codex profile.
    Init,
    /// Run one attempt: the given node, or the first ready one.
    Run {
        /// Node id to run; omitted, the first ready machine-workable node.
        node: Option<String>,
    },
    /// List ready work as the orchestrator sees it.
    Ready,
    /// List recorded attempts.
    Attempts,
    /// Show one attempt's durable record.
    Show { attempt: String },
    /// List project candidates and the Linka nodes that produced them.
    Candidates,
    /// Show a Linka candidate (candidate id or producing attempt id).
    Candidate { candidate: String },
    /// Accept an exact Linka candidate for its recorded target branch.
    Accept {
        candidate: String,
        #[arg(long, default_value = "")]
        notes: String,
    },
    /// Reject a Linka candidate and make its source node retryable.
    Reject {
        candidate: String,
        #[arg(long)]
        notes: String,
    },
    /// Publish an accepted candidate by recoverable fast-forward.
    Publish { candidate: String },
    /// Coordinate Git-native Nota reviews with Linka verification nodes.
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    /// Classify unfinished attempts and finish what can be finished.
    Recover,
    /// Verify durable evidence for every Orka-produced project candidate.
    Audit,
}

#[derive(Subcommand)]
enum ReviewCommand {
    /// List active reviews.
    List,
    /// Create a verification node and start a Nota branch at the candidate artifact.
    Start {
        candidate: String,
        #[arg(long, value_enum, default_value = "human")]
        assignee: Author,
        /// Prepare the managed review worktree and print its path.
        #[arg(long)]
        enter: bool,
    },
    /// Finish branch creation after an interrupted review start.
    Resume {
        verification: String,
        /// Prepare the managed review worktree and print its path.
        #[arg(long)]
        enter: bool,
    },
    /// Create or reuse the managed review worktree and print its path.
    Enter { verification: String },
    /// Create or reuse the managed review worktree.
    Worktree {
        verification: String,
        /// Print only the path, suitable for command substitution.
        #[arg(long)]
        print_path: bool,
    },
    /// List managed review worktrees.
    Worktrees,
    /// Safely remove a clean managed review worktree.
    Cleanup { verification: String },
    /// Show the binding and Git-native review entries.
    Show { verification: String },
    /// Submit the Nota review as the verification node's graph-only result.
    Finish {
        verification: String,
        #[arg(long, value_enum)]
        verdict: VerdictArg,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },
    /// Stop a review and record an abandoned verification result.
    #[command(visible_alias = "stop")]
    Abandon {
        verification: String,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum VerdictArg {
    Approved,
    ChangesRequested,
    Commented,
}

impl From<VerdictArg> for ReviewVerdict {
    fn from(value: VerdictArg) -> Self {
        match value {
            VerdictArg::Approved => Self::Approved,
            VerdictArg::ChangesRequested => Self::ChangesRequested,
            VerdictArg::Commented => Self::Commented,
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

struct Workbench {
    root: PathBuf,
}

impl Workbench {
    fn locate(given: Option<PathBuf>) -> Result<Self> {
        if let Some(root) = given {
            if !root.join(".linka").is_dir() {
                bail!("no .linka store under {}", root.display());
            }
            return Ok(Self { root });
        }
        let mut dir = std::env::current_dir()?;
        loop {
            if dir.join(".linka").is_dir() {
                return Ok(Self { root: dir });
            }
            if !dir.pop() {
                bail!("no workbench found: no ancestor directory contains .linka/");
            }
        }
    }

    /// Open the Linka store Orka orchestrates.
    fn linka_store(&self) -> Result<Store> {
        Store::open(self.root.join(".linka"))
    }

    fn attempts(&self) -> FsAttemptStore {
        FsAttemptStore::new(self.root.join(".orka"))
    }

    fn reviews<'a>(&self, store: &'a Store) -> Reviews<'a> {
        Reviews::new(store, self.root.join(".orka"))
    }

    fn workspaces(&self, store: &Store) -> GitWorkspaces {
        GitWorkspaces::new(store.project_root(), self.root.join(".orka/worktrees"))
    }

    fn review_worktrees(&self, store: &Store) -> GitReviewWorktrees {
        GitReviewWorktrees::new(
            store.project_root(),
            self.root.join(".orka/review-worktrees"),
        )
    }

    fn config(&self) -> Result<Config> {
        let path = self.root.join(CONFIG_FILE);
        Config::load(&path).with_context(|| {
            format!(
                "orka needs {} (see orka/DESIGN.md for an example)",
                path.display()
            )
        })
    }
}

/// Parse a CLI node argument as a Linka node id, reporting Linka's validation
/// error at the command boundary.
fn parse_node(arg: String) -> Result<NodeId> {
    arg.parse()
        .map_err(|e| anyhow::anyhow!("invalid node id: {e}"))
}

fn run(cli: Cli) -> Result<()> {
    let workbench = Workbench::locate(cli.workbench)?;
    match cli.command {
        Command::Init => {
            let path = workbench.root.join(CONFIG_FILE);
            Config::init(&path)?;
            println!("created {}", path.display());
        }
        Command::Run { node } => {
            let config = workbench.config()?;
            let store = workbench.linka_store()?;
            let executor = config.executor()?;
            let workspaces = workbench.workspaces(&store);
            let attempts = workbench.attempts();
            let engine = Engine {
                linka: LinkaWork::new(&store),
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy()?,
            };
            let mut printer = ProgressPrinter::new();
            let mut progress = |event: &RunProgress| printer.print(event);
            let report = match node {
                Some(arg) => Some(engine.run_node_with_progress(&parse_node(arg)?, &mut progress)?),
                None => engine.run_next_with_progress(&mut progress)?,
            };
            match report {
                None => println!("nothing ready"),
                Some(report) => print_run(&report),
            }
        }
        Command::Ready => {
            let store = workbench.linka_store()?;
            let linka = LinkaWork::new(&store);
            let ready = linka.ready_for_machine()?;
            if ready.is_empty() {
                println!("nothing ready");
            }
            for item in ready {
                println!("{}  {}", item.node, item.title);
            }
        }
        Command::Attempts => {
            let attempts = workbench.attempts();
            for id in attempts.list()? {
                let snapshot = attempts.load(&id)?;
                println!(
                    "{}  {}  {:?}",
                    id,
                    snapshot.record.input.node(),
                    snapshot.phase()
                );
            }
        }
        Command::Show { attempt } => {
            let attempts = workbench.attempts();
            let id = AttemptId(attempt);
            let snapshot = attempts.load(&id)?;
            let input = &snapshot.record.input;
            println!("attempt   {id}");
            println!("node      {}", input.node());
            println!("phase     {:?}", snapshot.phase());
            println!("input     {}", input.input_commit());
            if let Some(ws) = &snapshot.workspace {
                println!("workspace {} (branch {})", ws.path.display(), ws.branch);
                println!("candidate {}", ws.branch);
            }
            if let Some(evidence) = &snapshot.evidence {
                println!(
                    "exit      {} via {} backend",
                    evidence.exit_code, evidence.backend
                );
            }
            if let Some(seal) = &snapshot.seal {
                println!("sealed    {}", seal_line(&seal.state));
            }
            let transcript = attempts.transcript_path(&id);
            if transcript.exists() {
                println!("transcript {}", transcript.display());
            }
            let events = attempts.events_path(&id);
            if events.exists() {
                println!("events     {}", events.display());
            }
            let diagnostics = attempts.diagnostics_path(&id);
            if diagnostics.exists() {
                println!("diagnostics {}", diagnostics.display());
            }
            let store = workbench.linka_store()?;
            if let Ok(candidate) = Candidates::new(&store, &attempts).get(&id.0) {
                println!("linka     {} ({})", candidate.id, candidate.status());
            }
        }
        Command::Candidates => {
            let store = workbench.linka_store()?;
            let attempts = workbench.attempts();
            let candidates = Candidates::new(&store, &attempts).list()?;
            if candidates.is_empty() {
                println!("no project candidates");
            }
            for candidate in candidates {
                println!(
                    "{}  node {}  {}  {} -> {}{}",
                    candidate.id,
                    candidate.node,
                    candidate.status(),
                    candidate.branch,
                    candidate.target,
                    candidate
                        .attempt
                        .as_ref()
                        .map(|attempt| format!("  attempt {attempt}"))
                        .unwrap_or_default()
                );
            }
        }
        Command::Candidate { candidate } => {
            let store = workbench.linka_store()?;
            let attempts = workbench.attempts();
            let candidates = Candidates::new(&store, &attempts);
            let candidate = candidates.get(&candidate)?;
            println!("candidate {}", candidate.id);
            println!("node      {}", candidate.node);
            println!("status    {}", candidate.status());
            println!("branch    {}", candidate.branch);
            println!("target    {}", candidate.target);
            if let Some(input) = &candidate.input_commit {
                println!("input     {input}");
            }
            println!("head      {}", candidate.head_commit);
            if let Some(attempt) = &candidate.attempt {
                println!("attempt   {attempt}");
            }
            let patch = candidates.patch(&candidate.id.0)?;
            if patch.is_empty() {
                println!("\n(no diff)");
            } else {
                println!("\n{patch}");
            }
        }
        Command::Accept { candidate, notes } => {
            let store = workbench.linka_store()?;
            let attempts = workbench.attempts();
            let accepted = Candidates::new(&store, &attempts).accept(&candidate, notes)?;
            println!("accepted {} for {}", accepted.id, accepted.target);
        }
        Command::Reject { candidate, notes } => {
            let store = workbench.linka_store()?;
            let attempts = workbench.attempts();
            let rejected = Candidates::new(&store, &attempts).reject(&candidate, notes)?;
            println!("rejected {}", rejected.id);
        }
        Command::Publish { candidate } => {
            let store = workbench.linka_store()?;
            let attempts = workbench.attempts();
            let published = Candidates::new(&store, &attempts).publish(&candidate)?;
            println!("published {} at {}", published.id, published.head_commit);
        }
        Command::Review { command } => {
            let store = workbench.linka_store()?;
            let reviews = workbench.reviews(&store);
            match command {
                ReviewCommand::List => {
                    let active = reviews.list()?;
                    if active.is_empty() {
                        println!("no active reviews");
                    }
                    for record in active {
                        println!(
                            "{}  candidate {}  {}  {}",
                            record.verification, record.candidate, record.branch, record.subject
                        );
                    }
                }
                ReviewCommand::Start {
                    candidate,
                    assignee,
                    enter,
                } => {
                    let started = reviews.start(&CandidateId(candidate), assignee)?;
                    print_started_review(&started);
                    if enter {
                        let worktree = workbench
                            .review_worktrees(&store)
                            .prepare(&started.record)?;
                        println!("{}", worktree.path.display());
                    }
                }
                ReviewCommand::Resume {
                    verification,
                    enter,
                } => {
                    let verification = parse_node(verification)?;
                    let started = reviews.resume(&verification)?;
                    print_started_review(&started);
                    if enter {
                        let worktree = workbench
                            .review_worktrees(&store)
                            .prepare(&started.record)?;
                        println!("{}", worktree.path.display());
                    }
                }
                ReviewCommand::Enter { verification } => {
                    let verification = parse_node(verification)?;
                    let started = reviews.resume(&verification)?;
                    let worktree = workbench
                        .review_worktrees(&store)
                        .prepare(&started.record)?;
                    println!("{}", worktree.path.display());
                }
                ReviewCommand::Worktree {
                    verification,
                    print_path,
                } => {
                    let verification = parse_node(verification)?;
                    let started = reviews.resume(&verification)?;
                    let worktree = workbench
                        .review_worktrees(&store)
                        .prepare(&started.record)?;
                    if print_path {
                        println!("{}", worktree.path.display());
                    } else {
                        println!("verification {}", worktree.verification);
                        println!("branch       {}", worktree.branch);
                        println!("worktree     {}", worktree.path.display());
                    }
                }
                ReviewCommand::Worktrees => {
                    let worktrees = workbench.review_worktrees(&store).list()?;
                    if worktrees.is_empty() {
                        println!("no managed review worktrees");
                    }
                    for worktree in worktrees {
                        println!(
                            "{}  {}  {}  {}",
                            worktree.verification,
                            if worktree.dirty { "dirty" } else { "clean" },
                            worktree.branch,
                            worktree.path.display()
                        );
                    }
                }
                ReviewCommand::Cleanup { verification } => {
                    let verification = parse_node(verification)?;
                    let record = reviews.load(&verification)?;
                    match workbench.review_worktrees(&store).cleanup(&record)? {
                        ReviewCleanupOutcome::Removed => println!("removed {verification}"),
                        ReviewCleanupOutcome::RetainedDirty => {
                            println!("retained {verification}: worktree has uncommitted changes")
                        }
                        ReviewCleanupOutcome::AlreadyAbsent => {
                            println!("absent {verification}")
                        }
                    }
                }
                ReviewCommand::Show { verification } => {
                    let verification = parse_node(verification)?;
                    let (record, review) = reviews.review(&verification)?;
                    let vcs = linka::GitVcs::for_store(&store);
                    let state = linka::ops::node_state(&store, &vcs, verification.as_str())?;
                    println!("verification {}", record.verification);
                    println!("candidate    {}", record.candidate);
                    println!("branch       {}", review.branch);
                    println!("subject      {}", review.subject);
                    println!(
                        "state        {:?} {:?} {:?}",
                        state.outcome, state.currency, state.integration
                    );
                    if review.entries.is_empty() {
                        println!("entries      none");
                    }
                    for entry in review.entries {
                        println!(
                            "{}  {:?}  {}",
                            linka::ops::short(&entry.commit),
                            entry.kind,
                            entry.message.lines().next().unwrap_or_default()
                        );
                    }
                }
                ReviewCommand::Finish {
                    verification,
                    verdict,
                    summary,
                    author,
                } => {
                    let verification = parse_node(verification)?;
                    match reviews.finish(
                        &verification,
                        verdict.into(),
                        summary.as_deref(),
                        author,
                    )? {
                        FinishOutcome::Submitted => println!("completed {verification}"),
                        FinishOutcome::AlreadySubmitted => {
                            println!("completed {verification} (already submitted)")
                        }
                        FinishOutcome::Conflict(conflicts) => {
                            println!("stale {verification}: {conflicts:?}")
                        }
                    }
                }
                ReviewCommand::Abandon {
                    verification,
                    notes,
                    author,
                } => {
                    let verification = parse_node(verification)?;
                    match reviews.abandon(&verification, notes.as_deref(), author)? {
                        AbandonOutcome::Abandoned => println!("abandoned {verification}"),
                        AbandonOutcome::AlreadyAbandoned => {
                            println!("abandoned {verification} (already submitted)")
                        }
                        AbandonOutcome::Conflict(conflicts) => {
                            println!("stale {verification}: {conflicts:?}")
                        }
                    }
                }
            }
        }
        Command::Recover => {
            let config = workbench.config()?;
            let store = workbench.linka_store()?;
            let executor = config.executor()?;
            let workspaces = workbench.workspaces(&store);
            let attempts = workbench.attempts();
            let engine = Engine {
                linka: LinkaWork::new(&store),
                executor: &executor,
                workspaces: &workspaces,
                attempts: &attempts,
                policy: config.policy()?,
            };
            let reports = engine.recover()?;
            if reports.is_empty() {
                println!("no attempts recorded");
            }
            for report in reports {
                println!("{}  {}  {}", report.attempt, report.node, report.action);
            }
        }
        Command::Audit => {
            let store = workbench.linka_store()?;
            let problems = LinkaWork::new(&store).audit_output_evidence()?;
            if problems.is_empty() {
                println!("all Orka-produced outputs retain complete evidence");
            } else {
                for problem in &problems {
                    eprintln!("{problem}");
                }
                bail!("{} output evidence problem(s)", problems.len());
            }
        }
    }
    Ok(())
}

fn print_started_review(started: &orka::review::Started) {
    println!("verification {}", started.record.verification);
    println!("candidate    {}", started.record.candidate);
    println!("review       {}", started.review.branch);
    println!("subject      {}", started.review.subject);
    println!(
        "enter        orka review enter {}",
        started.record.verification
    );
    println!(
        "worktree     orka review worktree {}",
        started.record.verification
    );
}

fn print_run(report: &RunReport) {
    println!("attempt {}  node {}", report.attempt, report.node);
    println!("exit    {}", report.exit_code);
    println!("sealed  {}", seal_line(&report.sealed));
    if let Some(candidate) = &report.candidate {
        println!(
            "candidate {}  (view: `orka candidate {}`; accept: `orka accept {}`; publish: `orka publish {}`)",
            candidate, candidate, candidate, candidate
        );
    }
    if report.backend_failed {
        println!("warning: the agent command exited nonzero; its outcome was still handled");
    }
    println!("cleanup {:?}", report.cleanup);
}

struct LiveEventView {
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LiveEventView {
    fn start(path: PathBuf, color: bool) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let follower_done = done.clone();
        let thread = std::thread::spawn(move || {
            if let Err(error) = follow_codex_events(&path, &follower_done, color) {
                eprintln!("[orka] event view failed: {error:#}");
            }
        });
        Self {
            done,
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for LiveEventView {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ProgressPrinter {
    live: Option<LiveEventView>,
    color: bool,
}

impl ProgressPrinter {
    fn new() -> Self {
        Self {
            live: None,
            color: std::io::stderr().is_terminal(),
        }
    }

    fn print(&mut self, progress: &RunProgress) {
        match progress {
            RunProgress::Selected { node } => eprintln!("[orka] selected node {node}"),
            RunProgress::AttemptCreated { attempt } => {
                eprintln!("[orka] created {attempt}")
            }
            RunProgress::WorkspacePrepared { attempt } => {
                eprintln!("[orka] {attempt}: workspace prepared")
            }
            RunProgress::ExecutionStarted { attempt, artifacts } => {
                eprintln!(
                    "[orka] {attempt}: agent running (transcript: {}; diagnostics: {})",
                    artifacts.transcript.display(),
                    artifacts.diagnostics.display()
                );
                if let Some(path) = &artifacts.raw_events {
                    self.live = Some(LiveEventView::start(path.clone(), self.color));
                }
            }
            RunProgress::ExecutionFinished { attempt, exit_code } => {
                if let Some(mut live) = self.live.take() {
                    live.stop();
                }
                eprintln!("[orka] {attempt}: agent exited with code {exit_code}")
            }
            RunProgress::Sealed { attempt, state } => {
                eprintln!("[orka] {attempt}: {}", seal_line(state))
            }
        }
    }
}

impl Drop for ProgressPrinter {
    fn drop(&mut self) {
        if let Some(mut live) = self.live.take() {
            live.stop();
        }
    }
}

fn seal_line(state: &SealedState) -> String {
    match state {
        SealedState::Submitted { output_commit } => match output_commit {
            Some(commit) => format!("submitted (output commit {commit})"),
            None => "submitted (no project output)".into(),
        },
        SealedState::StaleAtSubmit { conflicts } => {
            format!("stale at submit: {conflicts:?}")
        }
        SealedState::FailureRecorded => "failure recorded".into(),
        SealedState::Interrupted { reason } => format!("interrupted: {reason}"),
        SealedState::ContractViolation { reason } => format!("contract violation: {reason}"),
    }
}
