//! The processes view's state: the last listing the server gave for the
//! current interaction's sandbox, and the process selected in it.
//!
//! The listing is a snapshot of `/proc` on the server, so the view asks again
//! on the event loop's timer while it is open. The selection follows the
//! selected process's pid across those refreshes rather than its row, which a
//! process starting or exiting above it would otherwise move.

use styra_protocol::SandboxProcess;
use styra_ui::processes::{tree, ProcessListing};

#[derive(Debug, Default)]
pub enum Listing {
    #[default]
    Loading,
    NotRunning,
    Failed(String),
    Running(Vec<SandboxProcess>),
}

#[derive(Debug, Default)]
pub struct ProcessView {
    /// The Session the listing describes. A listing for another Session is
    /// shown as loading until the current one's answer arrives.
    session: String,
    listing: Listing,
    selected: usize,
    selected_pid: Option<u32>,
}

impl ProcessView {
    /// Take the server's answer for `session`; reports whether it changed
    /// what the view shows.
    pub fn replace(
        &mut self,
        session: &str,
        answer: Result<Option<Vec<SandboxProcess>>, String>,
    ) -> bool {
        let listing = match answer {
            Ok(Some(processes)) => Listing::Running(processes),
            Ok(None) => Listing::NotRunning,
            Err(error) => Listing::Failed(error),
        };
        if self.session == session && same(&self.listing, &listing) {
            return false;
        }
        if self.session != session {
            self.selected = 0;
            self.selected_pid = None;
        }
        self.session = session.to_owned();
        self.listing = listing;
        let pids = self.pids();
        if let Some(index) = self
            .selected_pid
            .and_then(|pid| pids.iter().position(|&listed| listed == pid))
        {
            self.selected = index;
        }
        self.selected = self.selected.min(pids.len().saturating_sub(1));
        self.selected_pid = pids.get(self.selected).copied();
        true
    }

    pub fn listing(&self, session: &str) -> ProcessListing<'_> {
        if self.session != session {
            return ProcessListing::Loading;
        }
        match &self.listing {
            Listing::Loading => ProcessListing::Loading,
            Listing::NotRunning => ProcessListing::NotRunning,
            Listing::Failed(error) => ProcessListing::Failed(error),
            Listing::Running(processes) => ProcessListing::Running(processes),
        }
    }

    /// Row of the selection, in the tree order the view draws.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The selected process's command line, for `y`.
    pub fn selected_command(&self) -> Option<String> {
        let Listing::Running(processes) = &self.listing else {
            return None;
        };
        tree(processes)
            .get(self.selected)
            .map(|row| row.process.command.clone())
    }

    pub fn select_next(&mut self) {
        self.select(self.selected.saturating_add(1));
    }

    pub fn select_prev(&mut self) {
        self.select(self.selected.saturating_sub(1));
    }

    pub fn select_first(&mut self) {
        self.select(0);
    }

    pub fn select_last(&mut self) {
        self.select(usize::MAX);
    }

    fn select(&mut self, index: usize) {
        let pids = self.pids();
        self.selected = index.min(pids.len().saturating_sub(1));
        self.selected_pid = pids.get(self.selected).copied();
    }

    fn pids(&self) -> Vec<u32> {
        match &self.listing {
            Listing::Running(processes) => {
                tree(processes).iter().map(|row| row.process.pid).collect()
            }
            _ => Vec::new(),
        }
    }
}

fn same(left: &Listing, right: &Listing) -> bool {
    match (left, right) {
        (Listing::Loading, Listing::Loading) | (Listing::NotRunning, Listing::NotRunning) => true,
        (Listing::Failed(left), Listing::Failed(right)) => left == right,
        (Listing::Running(left), Listing::Running(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
